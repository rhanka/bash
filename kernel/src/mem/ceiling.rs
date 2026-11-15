//! Instance ceiling (spec 5.2): one wrapper around an inner allocator, installed as the global allocator.
//! The state is a handful of atomics: the kernel is single-threaded and needs no lock, and atomics keep the
//! native test binaries (which have several threads) sound without `unsafe`.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// Bootstrap ceiling used until `configure` is called.
pub(crate) const BOOTSTRAP_CEILING: u64 = 1 << 20;

static CEILING: AtomicU64 = AtomicU64::new(BOOTSTRAP_CEILING);
static USED: AtomicU64 = AtomicU64::new(0);
static HIGH_WATER: AtomicU64 = AtomicU64::new(0);
static CONFIGURED: AtomicBool = AtomicBool::new(false);

/// Why `configure` or `raise_ceiling` refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConfigError {
    /// `configure` was already called.
    AlreadyConfigured,
    /// `raise_ceiling` before `configure`.
    NotConfigured,
    /// `raise_ceiling` with a value below the current ceiling.
    BelowCurrent,
    /// `configure` with a value below the bytes already in use.
    BelowUsage,
}

/// Snapshot of the allocator counters, in bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Usage {
    /// Bytes currently obtained from the allocator.
    pub used: u64,
    /// The current ceiling.
    pub ceiling: u64,
    /// Maximum of `used` since start.
    pub high_water: u64,
}

/// Sets the ceiling. Valid once: a second call returns `AlreadyConfigured`.
/// Returns `BelowUsage` if `ceiling_bytes` is below the bytes already in use. On failure nothing changes.
pub fn configure(ceiling_bytes: u64) -> Result<(), ConfigError> {
    if CONFIGURED.load(Relaxed) {
        return Err(ConfigError::AlreadyConfigured);
    }
    if ceiling_bytes < USED.load(Relaxed) {
        return Err(ConfigError::BelowUsage);
    }
    CEILING.store(ceiling_bytes, Relaxed);
    CONFIGURED.store(true, Relaxed);
    Ok(())
}

/// Raises the ceiling. `NotConfigured` before `configure`; `BelowCurrent` if `ceiling_bytes` is below the
/// current ceiling. Equal to the current ceiling is accepted and changes nothing.
pub fn raise_ceiling(ceiling_bytes: u64) -> Result<(), ConfigError> {
    if !CONFIGURED.load(Relaxed) {
        return Err(ConfigError::NotConfigured);
    }
    if ceiling_bytes < CEILING.load(Relaxed) {
        return Err(ConfigError::BelowCurrent);
    }
    CEILING.store(ceiling_bytes, Relaxed);
    Ok(())
}

/// Returns the current counters.
pub fn usage() -> Usage {
    Usage { used: USED.load(Relaxed), ceiling: CEILING.load(Relaxed), high_water: HIGH_WATER.load(Relaxed) }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_ceiling() -> u64 {
    CEILING.load(Relaxed)
}

/// Test support: forgets the configuration and sets the ceiling, keeping `used`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reset(ceiling_bytes: u64) {
    CEILING.store(ceiling_bytes, Relaxed);
    CONFIGURED.store(false, Relaxed);
    HIGH_WATER.store(USED.load(Relaxed), Relaxed);
}

/// Takes `delta` bytes from the ceiling; false when it would be exceeded.
fn reserve(delta: u64) -> bool {
    let mut cur = USED.load(Relaxed);
    loop {
        let next = match cur.checked_add(delta) {
            Some(n) if n <= CEILING.load(Relaxed) => n,
            _ => return false,
        };
        match USED.compare_exchange_weak(cur, next, Relaxed, Relaxed) {
            Ok(_) => {
                HIGH_WATER.fetch_max(next, Relaxed);
                return true;
            }
            Err(seen) => cur = seen,
        }
    }
}

fn unreserve(delta: u64) {
    USED.fetch_sub(delta, Relaxed);
}

/// The allocator wrapper: counts live bytes and refuses an allocation that would pass the ceiling.
pub struct Ceiling<A>(pub A);

// SAFETY: every method forwards to the inner allocator with unchanged arguments, so the inner allocator's
// contract is the one callers rely on; the wrapper only adds counting and refusing (returning null).
unsafe impl<A: GlobalAlloc> GlobalAlloc for Ceiling<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let n = layout.size() as u64;
        if !reserve(n) {
            return core::ptr::null_mut();
        }
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        let p = unsafe { self.0.alloc(layout) };
        if p.is_null() {
            unreserve(n);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let n = layout.size() as u64;
        if !reserve(n) {
            return core::ptr::null_mut();
        }
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        let p = unsafe { self.0.alloc_zeroed(layout) };
        if p.is_null() {
            unreserve(n);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` describe a live block of this allocator, as the caller guarantees.
        unsafe { self.0.dealloc(ptr, layout) };
        unreserve(layout.size() as u64);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size() as u64;
        let new = new_size as u64;
        if new > old && !reserve(new - old) {
            return core::ptr::null_mut();
        }
        // SAFETY: arguments are the caller's, forwarded unchanged.
        let p = unsafe { self.0.realloc(ptr, layout, new_size) };
        if p.is_null() {
            if new > old {
                unreserve(new - old);
            }
        } else if new < old {
            unreserve(old - new);
        }
        p
    }
}
