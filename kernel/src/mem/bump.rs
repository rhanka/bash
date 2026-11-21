//! Bump allocator of the wasm build (spec 6). The logic is target-independent so that native unit tests can
//! exercise it with a fake page source; only `WasmBump` touches `memory.grow`.
//!
//! Space is never recycled except at the top of the bump region: freeing or reallocating the most recent
//! block reuses its space. Fragmentation behaviour is replaced in L1a-2.
use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub(crate) const PAGE: usize = 65536;

/// Source of linear-memory pages.
pub(crate) trait Pages {
    /// Grows memory by `pages`; returns the previous size in pages, or `None` when the engine refuses.
    fn grow(&self, pages: usize) -> Option<usize>;
}

/// Bump state: the next free address, the end of the grown region and the pages obtained so far.
pub(crate) struct BumpState {
    cur: AtomicUsize,
    end: AtomicUsize,
    pages: AtomicUsize,
}

impl BumpState {
    pub(crate) const fn new() -> Self {
        BumpState { cur: AtomicUsize::new(0), end: AtomicUsize::new(0), pages: AtomicUsize::new(0) }
    }

    /// Returns the address of a block of `size` bytes aligned to `align`, or 0 when the region would pass
    /// `ceiling` bytes (rounded down to whole pages) or the engine refuses to grow.
    pub(crate) fn alloc<P: Pages>(&self, src: &P, size: usize, align: usize, ceiling: u64) -> usize {
        let max_pages = (ceiling / PAGE as u64).min((usize::MAX / PAGE) as u64) as usize;
        loop {
            let cur = self.cur.load(Relaxed);
            let end = self.end.load(Relaxed);
            if end != 0 {
                if let Some(start) = cur.checked_add(align - 1).map(|a| a & !(align - 1)) {
                    if let Some(new_cur) = start.checked_add(size) {
                        if new_cur <= end {
                            self.cur.store(new_cur, Relaxed);
                            return start;
                        }
                    }
                }
            }
            // Not enough room: ask for the missing pages plus the worst-case alignment padding.
            let missing = size.saturating_add(align - 1).saturating_sub(end.saturating_sub(cur));
            let want = missing.div_ceil(PAGE).max(1);
            let total = self.pages.load(Relaxed);
            if want > max_pages.saturating_sub(total) {
                return 0;
            }
            let Some(old_pages) = src.grow(want) else { return 0 };
            self.pages.store(total + want, Relaxed);
            let grown = want.saturating_mul(PAGE);
            if old_pages == 0 {
                // Page 0 starts at address 0, which is the refusal value: keep the other grown pages and
                // skip only page 0 (the region is empty when `want == 1`, and the loop asks again).
                self.cur.store(PAGE, Relaxed);
                self.end.store(grown, Relaxed);
                continue;
            }
            let Some(old_addr) = old_pages.checked_mul(PAGE) else { return 0 };
            if old_addr != end {
                // Someone else grew the memory: the new region is not contiguous with the old one.
                self.cur.store(old_addr, Relaxed);
            }
            self.end.store(old_addr.saturating_add(grown), Relaxed);
        }
    }

    /// Frees a block; only the most recent one gives its space back.
    pub(crate) fn dealloc(&self, addr: usize, size: usize) {
        if addr.checked_add(size) == Some(self.cur.load(Relaxed)) {
            self.cur.store(addr, Relaxed);
        }
    }

    /// Resizes a block. Returns the new address (0 on refusal, the old block untouched) and whether the
    /// block moved, in which case the caller copies `min(old, new)` bytes. The most recent block grows or
    /// shrinks in place.
    pub(crate) fn realloc<P: Pages>(&self, src: &P, addr: usize, old: usize, align: usize, new: usize, ceiling: u64) -> (usize, bool) {
        if addr.checked_add(old) == Some(self.cur.load(Relaxed)) {
            let prev_end = self.end.load(Relaxed);
            self.cur.store(addr, Relaxed);
            let got = self.alloc(src, new, align, ceiling);
            if got == 0 {
                // Restore the old top only if no grow happened meanwhile: after a discontiguous grow,
                // `[cur, end)` would otherwise span pages this allocator does not own.
                if self.end.load(Relaxed) == prev_end {
                    self.cur.store(addr + old, Relaxed);
                }
                return (0, false);
            }
            return (got, got != addr);
        }
        let got = self.alloc(src, new, align, ceiling);
        (got, got != 0)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::WasmBump;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{BumpState, Pages};
    use core::alloc::{GlobalAlloc, Layout};

    struct Memory;

    impl Pages for Memory {
        fn grow(&self, pages: usize) -> Option<usize> {
            let old = core::arch::wasm32::memory_grow(0, pages);
            if old == usize::MAX {
                None
            } else {
                Some(old)
            }
        }
    }

    /// The inner allocator of the wasm build.
    pub(crate) struct WasmBump(BumpState);

    impl WasmBump {
        pub(crate) const fn new() -> Self {
            WasmBump(BumpState::new())
        }
    }

    // SAFETY: the bump state hands out disjoint, aligned ranges of linear memory that it obtained from
    // `memory.grow`, and never hands out the same live range twice; `dealloc` only moves the bump pointer
    // back over the most recent block.
    unsafe impl GlobalAlloc for WasmBump {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            self.0.alloc(&Memory, layout.size(), layout.align(), super::super::ceiling::current_ceiling()) as *mut u8
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            self.0.dealloc(ptr as usize, layout.size());
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let (addr, moved) =
                self.0.realloc(&Memory, ptr as usize, layout.size(), layout.align(), new_size, super::super::ceiling::current_ceiling());
            if moved {
                // SAFETY: both blocks are live, disjoint (the new one starts after the old one) and at
                // least `min(old, new)` bytes long.
                unsafe { core::ptr::copy_nonoverlapping(ptr, addr as *mut u8, layout.size().min(new_size)) };
            }
            addr as *mut u8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// A fake memory that grows contiguously from a given page, with an optional page limit.
    struct Fake {
        pages: Cell<usize>,
        limit: usize,
    }

    impl Pages for Fake {
        fn grow(&self, n: usize) -> Option<usize> {
            let old = self.pages.get();
            if old.checked_add(n)? > self.limit {
                return None;
            }
            self.pages.set(old + n);
            Some(old)
        }
    }

    fn fake(start: usize, limit: usize) -> Fake {
        Fake { pages: Cell::new(start), limit }
    }

    const BIG: u64 = 1 << 30;

    #[test]
    fn aligned_disjoint_blocks_and_one_page_steps() {
        let m = fake(3, 100);
        let b = BumpState::new();
        let a = b.alloc(&m, 10, 1, BIG);
        assert_eq!(a, 3 * PAGE);
        let c = b.alloc(&m, 8, 8, BIG);
        assert_eq!(c % 8, 0);
        assert!(c >= a + 10);
        assert_eq!(m.pages.get(), 4);
        let d = b.alloc(&m, PAGE, 16, BIG);
        assert_eq!(d % 16, 0);
        assert!(d >= c + 8);
        assert_eq!(m.pages.get(), 5);
    }

    #[test]
    fn last_block_space_is_reused() {
        let m = fake(1, 100);
        let b = BumpState::new();
        let a = b.alloc(&m, 100, 8, BIG);
        let c = b.alloc(&m, 50, 8, BIG);
        b.dealloc(c, 50);
        assert_eq!(b.alloc(&m, 50, 8, BIG), c);
        // Freeing a block that is not the last changes nothing.
        b.dealloc(a, 100);
        assert!(b.alloc(&m, 8, 8, BIG) > c);
    }

    #[test]
    fn realloc_grows_the_last_block_in_place_and_moves_others() {
        let m = fake(1, 100);
        let b = BumpState::new();
        let a = b.alloc(&m, 16, 8, BIG);
        let (a2, moved) = b.realloc(&m, a, 16, 8, 3 * PAGE, BIG);
        assert_eq!((a2, moved), (a, false));
        let c = b.alloc(&m, 8, 8, BIG);
        let (a3, moved) = b.realloc(&m, a2, 3 * PAGE, 8, 4 * PAGE, BIG);
        assert!(moved);
        assert!(a3 > c);
    }

    #[test]
    fn the_ceiling_bounds_the_pages_and_a_refusal_changes_nothing() {
        let m = fake(1, 100);
        let b = BumpState::new();
        let ceiling = 2 * PAGE as u64 + 5;
        assert_ne!(b.alloc(&m, PAGE, 1, ceiling), 0);
        assert_ne!(b.alloc(&m, PAGE, 1, ceiling), 0);
        assert_eq!(b.alloc(&m, 1, 1, ceiling), 0);
        assert_eq!(m.pages.get(), 3);
        let last = b.alloc(&m, 0, 1, ceiling);
        assert_ne!(last, 0);
    }

    #[test]
    fn an_engine_refusal_returns_zero() {
        let m = fake(1, 2);
        let b = BumpState::new();
        assert_ne!(b.alloc(&m, 10, 1, BIG), 0);
        assert_eq!(b.alloc(&m, PAGE, 1, BIG), 0);
        assert_ne!(b.alloc(&m, 10, 1, BIG), 0);
    }

    /// A memory that is grown by a foreign party between our grows: each grow skips one foreign page.
    struct Gappy {
        pages: Cell<usize>,
        left: Cell<usize>,
    }

    impl Pages for Gappy {
        fn grow(&self, n: usize) -> Option<usize> {
            if self.left.get() == 0 {
                return None;
            }
            self.left.set(self.left.get() - 1);
            let old = self.pages.get() + 1; // one foreign page was added first
            self.pages.set(old + n);
            Some(old)
        }
    }

    #[test]
    fn a_failed_realloc_after_a_discontiguous_grow_does_not_span_foreign_pages() {
        let m = Gappy { pages: Cell::new(1), left: Cell::new(2) };
        let b = BumpState::new();
        let a = b.alloc(&m, 16, 8, BIG);
        assert_ne!(a, 0);
        // Growing the last block needs two grows (the first discontiguous), the second one is refused.
        let (r, moved) = b.realloc(&m, a, 16, 8, 2 * PAGE + 16, BIG);
        assert_eq!((r, moved), (0, false));
        // The foreign page 3 must stay outside the free range [cur, end).
        let (cur, end) = (b.cur.load(Relaxed), b.end.load(Relaxed));
        assert!(cur >= 4 * PAGE && cur < end, "the free range [{cur}, {end}) spans the foreign page");
    }

    #[test]
    fn page_zero_skip_keeps_the_other_grown_pages() {
        let m = fake(0, 100);
        let b = BumpState::new();
        let a = b.alloc(&m, 3 * PAGE, 8, BIG);
        assert_eq!(a, PAGE, "only page 0 is skipped");
        assert_eq!(m.pages.get(), 4, "a single grow of four pages, none discarded");
    }

    #[test]
    fn the_end_of_a_region_at_the_top_of_the_address_space_does_not_overflow() {
        let top = usize::MAX / PAGE;
        let m = fake(top, usize::MAX);
        let b = BumpState::new();
        // Must neither panic (debug overflow) nor wrap the end to a small value.
        let a = b.alloc(&m, 8, 8, u64::MAX);
        assert!(a == 0 || a >= top * PAGE);
        assert_eq!(BumpState::new().alloc(&fake(usize::MAX, usize::MAX), 8, 8, u64::MAX), 0, "address overflow is a refusal");
    }

    #[test]
    fn page_zero_is_never_handed_out() {
        let m = fake(0, 100);
        let b = BumpState::new();
        let a = b.alloc(&m, 8, 8, BIG);
        assert_ne!(a, 0);
        let z = fake(0, 100);
        assert_ne!(BumpState::new().alloc(&z, 0, 1, BIG), 0, "a zero-size request on a fresh state");
    }

    #[test]
    fn a_failed_realloc_leaves_the_block_in_place() {
        let m = fake(1, 2);
        let b = BumpState::new();
        let a = b.alloc(&m, 16, 8, BIG);
        assert_eq!(b.realloc(&m, a, 16, 8, 10 * PAGE, BIG), (0, false));
        assert_eq!(b.alloc(&m, 8, 8, BIG), a + 16);
    }
}
