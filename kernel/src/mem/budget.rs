//! Byte budgets (spec 5.3): a logical cap tied to a named limit, nestable.
use super::error::{LimitId, MemError, MemResult};
use super::alloc::alloc::{alloc, dealloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

// The counters are atomics so that the one process-wide node (the headroom account) can live in a static
// and be touched by the several threads of the native test binaries. On wasm they compile to plain loads
// and stores.
struct Node {
    limit: LimitId,
    cap: AtomicU64,
    used: AtomicU64,
    refs: AtomicUsize,
    parent: Option<Budget>,
}

/// A node that lives in a static: it has no parent and one permanent reference, so it is never freed.
pub(crate) struct StaticNode(Node);

// SAFETY: the node has no parent handle (the only non-`Sync` part of `Node`) and every field is an atomic
// or a plain `Copy` value, so shared references to it from several threads are sound.
unsafe impl Sync for StaticNode {}

impl StaticNode {
    pub(crate) const fn new(limit: LimitId, cap: u64) -> StaticNode {
        StaticNode(Node { limit, cap: AtomicU64::new(cap), used: AtomicU64::new(0), refs: AtomicUsize::new(1), parent: None })
    }
}

/// A byte budget. Cloning yields another handle to the same budget (never fails, allocates nothing).
/// The memory of the node itself comes from the allocator and is charged to no budget; it lives as long
/// as a handle, a child or a container refers to it. Not `Send` and not `Sync`.
pub struct Budget {
    node: NonNull<Node>,
}

impl Budget {
    fn make(limit: LimitId, cap: u64, parent: Option<Budget>) -> MemResult<Budget> {
        let layout = Layout::new::<Node>();
        // SAFETY: `Node` has a non-zero size, so the layout is valid for `alloc`.
        let raw = unsafe { alloc(layout) } as *mut Node;
        let Some(node) = NonNull::new(raw) else { return Err(MemError::Ceiling) };
        // SAFETY: `raw` is a fresh, aligned allocation of `size_of::<Node>()` bytes, not yet initialised.
        unsafe {
            node.as_ptr().write(Node { limit, cap: AtomicU64::new(cap), used: AtomicU64::new(0), refs: AtomicUsize::new(1), parent })
        };
        Ok(Budget { node })
    }

    fn node(&self) -> &Node {
        // SAFETY: the node is live while this handle exists (the handle owns one reference count).
        unsafe { self.node.as_ref() }
    }

    /// A handle to a static node (the headroom account): counts one more reference.
    pub(crate) fn of_static(node: &'static StaticNode) -> Budget {
        let n = &node.0;
        n.refs.fetch_add(1, Relaxed);
        Budget { node: NonNull::from(n) }
    }

    /// A root budget with the given cap, in bytes. Fails only with `Ceiling` (the node is allocated).
    pub fn root(limit: LimitId, cap: u64) -> MemResult<Budget> {
        Budget::make(limit, cap, None)
    }

    /// A child budget: a charge on it is also charged to every ancestor. Fails only with `Ceiling`.
    pub fn child(&self, limit: LimitId, cap: u64) -> MemResult<Budget> {
        Budget::make(limit, cap, Some(self.clone()))
    }

    /// Charges `bytes`. Fails with `Limit(id)` of the first budget, from this one up to the root, whose cap
    /// would be exceeded; on failure nothing is charged anywhere.
    pub fn charge(&self, bytes: u64) -> MemResult<()> {
        let mut at = Some(self);
        while let Some(b) = at {
            let n = b.node();
            match n.used.load(Relaxed).checked_add(bytes) {
                Some(total) if total <= n.cap.load(Relaxed) => {}
                _ => return Err(MemError::Limit(n.limit)),
            }
            at = n.parent.as_ref();
        }
        let mut at = Some(self);
        while let Some(b) = at {
            let n = b.node();
            n.used.store(n.used.load(Relaxed) + bytes, Relaxed);
            at = n.parent.as_ref();
        }
        Ok(())
    }

    /// Releases `bytes` from this budget and every ancestor. Releasing more than charged is a bug: debug
    /// builds panic, release builds saturate at zero.
    pub fn release(&self, bytes: u64) {
        let mut at = Some(self);
        while let Some(b) = at {
            let n = b.node();
            let used = n.used.load(Relaxed);
            debug_assert!(bytes <= used, "budget released more than charged");
            n.used.store(used.saturating_sub(bytes), Relaxed);
            at = n.parent.as_ref();
        }
    }

    /// Bytes currently charged to this budget (including those of its descendants).
    pub fn used(&self) -> u64 {
        self.node().used.load(Relaxed)
    }

    /// The cap, in bytes.
    pub fn cap(&self) -> u64 {
        self.node().cap.load(Relaxed)
    }

    /// The named limit this budget stands for.
    pub fn limit(&self) -> LimitId {
        self.node().limit
    }

    /// Changes the cap. Lowering it below `used` is allowed; later charges fail until usage drops.
    pub fn set_cap(&self, cap: u64) {
        self.node().cap.store(cap, Relaxed);
    }
}

impl Clone for Budget {
    /// A handle copy: never fails, allocates nothing.
    fn clone(&self) -> Budget {
        let n = self.node();
        if n.refs.fetch_add(1, Relaxed) == usize::MAX {
            panic!("budget handle count overflow");
        }
        Budget { node: self.node }
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        let n = self.node();
        if n.refs.fetch_sub(1, Relaxed) == 1 {
            // SAFETY: this was the last handle, so nothing else refers to the node; it was allocated in
            // `make` with `Layout::new::<Node>()` and is initialised. Dropping it drops the parent handle.
            unsafe {
                core::ptr::drop_in_place(self.node.as_ptr());
                dealloc(self.node.as_ptr() as *mut u8, Layout::new::<Node>());
            }
        }
    }
}

impl core::fmt::Debug for Budget {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Budget").field("limit", &self.limit()).field("used", &self.used()).field("cap", &self.cap()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_and_release_walk_the_chain() {
        let root = Budget::root(LimitId(10), 100).unwrap();
        let child = root.child(LimitId(11), 60).unwrap();
        child.charge(40).unwrap();
        assert_eq!((child.used(), root.used()), (40, 40));
        assert_eq!(child.charge(30), Err(MemError::Limit(LimitId(11))));
        root.charge(50).unwrap();
        assert_eq!(child.charge(20), Err(MemError::Limit(LimitId(10))));
        assert_eq!((child.used(), root.used()), (40, 90));
        child.release(40);
        assert_eq!((child.used(), root.used()), (0, 50));
    }

    #[test]
    fn overflowing_charge_is_refused() {
        let b = Budget::root(LimitId(10), u64::MAX).unwrap();
        b.charge(u64::MAX).unwrap();
        assert_eq!(b.charge(1), Err(MemError::Limit(LimitId(10))));
    }
}
