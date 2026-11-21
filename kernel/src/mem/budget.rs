//! Byte budgets (spec 5.3): a logical cap tied to a named limit, nestable.
use super::error::{LimitId, MemError, MemResult};
use super::alloc::alloc::{alloc, dealloc, Layout};
use core::cell::Cell;
use core::ptr::NonNull;

struct Node {
    limit: LimitId,
    cap: Cell<u64>,
    used: Cell<u64>,
    refs: Cell<usize>,
    parent: Option<Budget>,
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
        unsafe { node.as_ptr().write(Node { limit, cap: Cell::new(cap), used: Cell::new(0), refs: Cell::new(1), parent }) };
        Ok(Budget { node })
    }

    fn node(&self) -> &Node {
        // SAFETY: the node is live while this handle exists (the handle owns one reference count).
        unsafe { self.node.as_ref() }
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
            match n.used.get().checked_add(bytes) {
                Some(total) if total <= n.cap.get() => {}
                _ => return Err(MemError::Limit(n.limit)),
            }
            at = n.parent.as_ref();
        }
        let mut at = Some(self);
        while let Some(b) = at {
            let n = b.node();
            n.used.set(n.used.get() + bytes);
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
            let used = n.used.get();
            debug_assert!(bytes <= used, "budget released more than charged");
            n.used.set(used.saturating_sub(bytes));
            at = n.parent.as_ref();
        }
    }

    /// Bytes currently charged to this budget (including those of its descendants).
    pub fn used(&self) -> u64 {
        self.node().used.get()
    }

    /// The cap, in bytes.
    pub fn cap(&self) -> u64 {
        self.node().cap.get()
    }

    /// The named limit this budget stands for.
    pub fn limit(&self) -> LimitId {
        self.node().limit
    }

    /// Changes the cap. Lowering it below `used` is allowed; later charges fail until usage drops.
    pub fn set_cap(&self, cap: u64) {
        self.node().cap.set(cap);
    }
}

impl Clone for Budget {
    /// A handle copy: never fails, allocates nothing.
    fn clone(&self) -> Budget {
        let n = self.node();
        match n.refs.get().checked_add(1) {
            Some(r) => n.refs.set(r),
            None => panic!("budget handle count overflow"),
        }
        Budget { node: self.node }
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        let n = self.node();
        let left = n.refs.get() - 1;
        n.refs.set(left);
        if left == 0 {
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
