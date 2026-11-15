//! The allocator reserve (spec L1a-2 4.6, 5.1): 64 KiB below the ceiling that ordinary allocations cannot
//! use. Inside [`reserve_scope`] allocations may use it, so that a fiber that hit a limit can still build the
//! message and exit code that report it.
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

static OPEN: AtomicBool = AtomicBool::new(false);

/// True while a `reserve_scope` is running.
#[cfg(target_arch = "wasm32")]
pub(crate) fn is_open() -> bool {
    OPEN.load(Relaxed)
}

struct Close;

impl Drop for Close {
    fn drop(&mut self) {
        OPEN.store(false, Relaxed);
    }
}

/// Runs `f` with access to the allocator's reserve. Not re-entrant: a nested call runs `f` without further
/// effect (in particular it does not close the reserve when it returns). Meant for the short path that
/// reports a limit result. On the native target there is no reserve to open and `f` just runs.
pub fn reserve_scope<R>(f: impl FnOnce() -> R) -> R {
    if OPEN.swap(true, Relaxed) {
        return f();
    }
    let _close = Close;
    f()
}
