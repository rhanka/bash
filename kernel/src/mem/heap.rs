//! The allocator of the wasm build (spec L1a-2 section 4): a boundary-tag heap with segregated free lists.
//!
//! Design:
//! * A block is an 8-byte header followed by its payload; every block size is a multiple of 16 and at least
//!   32, and block addresses are 8 modulo 16, so payloads are 16-aligned. The overhead is the 8-byte header
//!   plus rounding. The header holds the size and two flags: `IN_USE` and `PREV_FREE` (the block before is
//!   free). A free block also holds its list links and, in its last 8 bytes, a footer with its size, which is
//!   how a freed block finds the free block before it.
//! * Free blocks sit in bins by size (two-level segregated fit): sizes below 256 have one bin per 16 bytes,
//!   above that each power-of-two range is cut into 16 equal bins. Two bitmaps find the first non-empty bin
//!   at or above a size, so `alloc` and `dealloc` are constant-time for every size. For sizes of 256 and more
//!   the request's own bin is also scanned for a block that fits before rounding the request up.
//! * `dealloc` merges the block with free neighbours at once. `realloc` shrinks or grows in place when the
//!   next block is free, or when the block ends the heap and the heap can grow; otherwise it moves.
//! * Memory is obtained in steps of whole 64 KiB pages: the pages the request is missing, or one eighth of
//!   the ordinary footprint (at least one page) for amortized allocation growth when that is larger. An
//!   in-place extension at the tail grows only by the missing pages. A contiguous region joins its
//!   predecessor: the old closing sentinel becomes the start of the new free block.
//! * Large reallocations try a rounded payload capacity to leave room for subsequent growth. If it cannot
//!   be served, the exact request is retried.
//! * The first page grown is a separate arena. Only requests inside `reserve_scope` can allocate from it,
//!   even when the ordinary arena is fragmented or the engine refuses further growth. The ceiling covers
//!   both arenas, and memory is never given back.
//!
//! The logic is target-independent so that native unit tests drive it with a fake page source; only
//! `WasmHeap` touches `memory.grow`.
use core::ptr;

pub(crate) const PAGE: usize = 65_536;
/// Size of the dedicated arena that ordinary requests cannot use.
pub(crate) const RESERVE: usize = 65_536;

const HDR: usize = 8;
const MIN_BLOCK: usize = 32;
const ALIGN: usize = 16;
const MAX_BLOCK: usize = 1 << 31;
const IN_USE: u64 = 1;
const PREV_FREE: u64 = 2;
const FLAGS: u64 = 0xF;
const SMALL: usize = 256;
const SL: usize = 16;
const FL_COUNT: usize = 25;
const BINS: usize = FL_COUNT * SL;
const W: usize = core::mem::size_of::<usize>();
/// Blocks examined in the request's own bin before the bitmaps are used.
const OWN_BIN_SCAN: usize = 8;

/// Source of linear-memory pages.
pub(crate) trait Pages {
    /// Grows memory by `pages`; returns the address of the first new byte (a multiple of 64 KiB), or `None`
    /// when the engine refuses.
    fn grow(&mut self, pages: usize) -> Option<usize>;
}

fn rd64(a: usize) -> u64 {
    // SAFETY: `a` is 8-aligned and inside memory this heap manages (a block header, footer or sentinel).
    unsafe { (a as *const u64).read() }
}
fn wr64(a: usize, v: u64) {
    // SAFETY: as for `rd64`; the heap owns that word.
    unsafe { (a as *mut u64).write(v) }
}
fn rdw(a: usize) -> usize {
    // SAFETY: `a` is word-aligned and inside a free block of this heap (its list links).
    unsafe { (a as *const usize).read() }
}
fn wrw(a: usize, v: usize) {
    // SAFETY: as for `rdw`; the heap owns that word.
    unsafe { (a as *mut usize).write(v) }
}

/// Bin of a block of `size` bytes (a multiple of 16, at least 32): every block of the bin is at least the
/// lower bound of the bin.
fn mapping(size: usize) -> (usize, usize) {
    if size < SMALL {
        return (0, size / 16);
    }
    let l = (usize::BITS - 1 - size.leading_zeros()) as usize;
    let fl = l - 7;
    if fl >= FL_COUNT {
        return (FL_COUNT - 1, SL - 1);
    }
    (fl, (size >> (l - 4)) & (SL - 1))
}

/// Bin whose blocks are all at least `size`.
fn mapping_search(size: usize) -> (usize, usize) {
    if size < SMALL {
        return (0, size / 16);
    }
    let l = (usize::BITS - 1 - size.leading_zeros()) as usize;
    mapping(size + (1usize << (l - 4)) - 1)
}

/// Block size for a payload request, or `None` when it is out of range.
fn block_size(size: usize) -> Option<usize> {
    let need = size.max(1).checked_add(HDR + ALIGN - 1)? & !(ALIGN - 1);
    let need = need.max(MIN_BLOCK);
    (need <= MAX_BLOCK).then_some(need)
}

/// The closing header fits even when the exclusive end of the region does not fit in an address.
fn region_end_header(addr: usize, bytes: u64) -> Option<usize> {
    let offset = usize::try_from(bytes.checked_sub(HDR as u64)?).ok()?;
    addr.checked_add(offset)
}

struct Arena {
    fl_bitmap: u32,
    sl_bitmap: [u32; FL_COUNT],
    heads: [usize; BINS],
    footprint: u64,
    /// Sum of the sizes of the in-use blocks.
    in_use: usize,
    /// Address of the sentinel header that closes the last region (0 before the first growth).
    tail: usize,
    /// Did the last growth extend the previous region? (Always true on wasm: nothing else grows memory.)
    contiguous: bool,
}

impl Arena {
    const fn new() -> Self {
        Self { fl_bitmap: 0, sl_bitmap: [0; FL_COUNT], heads: [0; BINS], footprint: 0, in_use: 0, tail: 0, contiguous: true }
    }

    // ---- free lists ----

    fn insert_free(&mut self, blk: usize, size: usize) {
        wr64(blk, size as u64);
        wr64(blk + size - HDR, size as u64);
        let next = blk + size;
        wr64(next, rd64(next) | PREV_FREE);
        let (fl, sl) = mapping(size);
        let head = self.heads[fl * SL + sl];
        wrw(blk + HDR, head);
        wrw(blk + HDR + W, 0);
        if head != 0 {
            wrw(head + HDR + W, blk);
        }
        self.heads[fl * SL + sl] = blk;
        self.sl_bitmap[fl] |= 1 << sl;
        self.fl_bitmap |= 1 << fl;
    }

    fn remove_free(&mut self, blk: usize, size: usize) {
        let next = rdw(blk + HDR);
        let prev = rdw(blk + HDR + W);
        let (fl, sl) = mapping(size);
        if prev != 0 {
            wrw(prev + HDR, next);
        } else {
            self.heads[fl * SL + sl] = next;
            if next == 0 {
                self.sl_bitmap[fl] &= !(1 << sl);
                if self.sl_bitmap[fl] == 0 {
                    self.fl_bitmap &= !(1 << fl);
                }
            }
        }
        if next != 0 {
            wrw(next + HDR + W, prev);
        }
    }

    fn clear_prev_free(&self, blk: usize) {
        wr64(blk, rd64(blk) & !PREV_FREE);
    }

    /// Releases the in-use block (`blk`, `size`) into the free lists, merging with free neighbours.
    fn free_block(&mut self, blk: usize, size: usize) {
        let hdr = rd64(blk);
        let (mut start, mut total) = (blk, size);
        let next = blk + size;
        let nh = rd64(next);
        if nh & IN_USE == 0 {
            let nsz = (nh & !FLAGS) as usize;
            self.remove_free(next, nsz);
            total += nsz;
        }
        if hdr & PREV_FREE != 0 {
            let psz = rd64(blk - HDR) as usize;
            start = blk - psz;
            self.remove_free(start, psz);
            total += psz;
        }
        self.insert_free(start, total);
    }

    /// A free block of at least `need` bytes, still in its list.
    fn find(&self, need: usize) -> Option<usize> {
        if need >= SMALL {
            let (fl, sl) = mapping(need);
            let mut at = self.heads[fl * SL + sl];
            let mut seen = 0;
            while at != 0 && seen < OWN_BIN_SCAN {
                if (rd64(at) & !FLAGS) as usize >= need {
                    return Some(at);
                }
                at = rdw(at + HDR);
                seen += 1;
            }
        }
        let (fl, sl) = mapping_search(need);
        if fl >= FL_COUNT {
            return None;
        }
        let sl_map = self.sl_bitmap[fl] & (!0u32 << sl);
        let (fl, sl) = if sl_map != 0 {
            (fl, sl_map.trailing_zeros() as usize)
        } else {
            let fl_map = if fl + 1 < 32 { self.fl_bitmap & (!0u32 << (fl + 1)) } else { 0 };
            if fl_map == 0 {
                return None;
            }
            let f = fl_map.trailing_zeros() as usize;
            (f, self.sl_bitmap[f].trailing_zeros() as usize)
        };
        match self.heads[fl * SL + sl] {
            0 => None,
            b => Some(b),
        }
    }

    /// Takes the free block `blk` for an allocation of `need` bytes; returns the payload address.
    fn take(&mut self, blk: usize, need: usize) -> usize {
        let size = (rd64(blk) & !FLAGS) as usize;
        self.remove_free(blk, size);
        let rem = size - need;
        if rem >= MIN_BLOCK {
            wr64(blk, need as u64 | IN_USE);
            self.insert_free(blk + need, rem);
            self.in_use += need;
        } else {
            wr64(blk, size as u64 | IN_USE);
            self.clear_prev_free(blk + size);
            self.in_use += size;
        }
        blk + HDR
    }

    // ---- growth ----

    /// Makes a free block of at least `need` bytes appear at the end of the heap, growing memory.
    fn grow<P: Pages>(&mut self, pages: &mut P, need: usize, ceiling: u64, amortize: bool) -> bool {
        let extends_tail = self.tail != 0 && self.contiguous;
        let tail_free = if extends_tail && rd64(self.tail) & PREV_FREE != 0 { rd64(self.tail - HDR) as usize } else { 0 };
        let missing = need.saturating_sub(tail_free);
        let padding = if extends_tail { 0 } else { 2 * HDR };
        let min_pages = (missing + padding).div_ceil(PAGE).max(1);
        let room = (ceiling / PAGE as u64).saturating_sub(self.footprint / PAGE as u64);
        let room = usize::try_from(room).unwrap_or(usize::MAX);
        if min_pages > room {
            return false;
        }
        let floor = if amortize { (self.footprint / 8 / PAGE as u64) as usize } else { 1 };
        let mut want = min_pages.max(floor).max(1).min(room);
        let addr = loop {
            match pages.grow(want) {
                Some(a) => break a,
                None if want > min_pages => want = min_pages,
                None => return false,
            }
        };
        let bytes = want as u64 * PAGE as u64;
        let Some(end_hdr) = region_end_header(addr, bytes) else { return false };
        self.footprint += bytes;
        self.add_region(addr, end_hdr);
        true
    }

    fn add_region(&mut self, addr: usize, end_hdr: usize) {
        wr64(end_hdr, IN_USE);
        if self.tail != 0 && self.tail.checked_add(HDR) == Some(addr) {
            // Contiguous with the previous region: the old closing sentinel becomes the start of the block.
            let mut start = self.tail;
            let mut size = end_hdr - start;
            if rd64(start) & PREV_FREE != 0 {
                let psz = rd64(start - HDR) as usize;
                let prev = start - psz;
                self.remove_free(prev, psz);
                start = prev;
                size += psz;
            }
            self.insert_free(start, size);
            self.contiguous = true;
        } else {
            self.insert_free(addr + HDR, end_hdr - (addr + HDR));
            self.contiguous = self.tail == 0;
        }
        self.tail = end_hdr;
    }

    // ---- interface ----

    fn alloc<P: Pages>(&mut self, pages: &mut P, need: usize, ceiling: u64) -> usize {
        // A second growth is needed only when the engine placed the new pages away from the old region.
        for _ in 0..3 {
            if let Some(blk) = self.find(need) {
                return self.take(blk, need);
            }
            if !self.grow(pages, need, ceiling, true) {
                return 0;
            }
        }
        0
    }

    /// Frees the block at `addr`.
    fn dealloc(&mut self, addr: usize) {
        let blk = addr - HDR;
        let size = (rd64(blk) & !FLAGS) as usize;
        self.in_use -= size;
        self.free_block(blk, size);
    }

    /// Resizes a block using either of its neighbours or by growing the tail.
    fn resize<P: Pages>(&mut self, pages: &mut P, addr: usize, old_size: usize, need: usize, ceiling: u64) -> Option<usize> {
        let blk = addr - HDR;
        let hdr = rd64(blk);
        let size = (hdr & !FLAGS) as usize;
        if need <= size {
            let rem = size - need;
            if rem >= MIN_BLOCK {
                wr64(blk, need as u64 | IN_USE | (hdr & PREV_FREE));
                wr64(blk + need, rem as u64 | IN_USE);
                self.in_use -= rem;
                self.free_block(blk + need, rem);
            }
            return Some(addr);
        }
        if self.grow_in_place(blk, size, need) {
            return Some(addr);
        }
        if let Some(moved) = self.grow_backward(blk, size, old_size, need) {
            return Some(moved);
        }
        let next = blk + size;
        let nh = rd64(next);
        let nsz = if nh & IN_USE == 0 { (nh & !FLAGS) as usize } else { 0 };
        if next + nsz == self.tail
            && self.grow(pages, need - size, ceiling, false)
            && self.grow_in_place(blk, size, need)
        {
            return Some(addr);
        }
        None
    }

    /// Slides the in-use block (`blk`, `size`) down over the free block before it (and absorbs a free block
    /// after it) when together they hold `need` bytes: the payload moves, but no new space is taken.
    fn grow_backward(&mut self, blk: usize, size: usize, old_size: usize, need: usize) -> Option<usize> {
        if rd64(blk) & PREV_FREE == 0 {
            return None;
        }
        let psz = rd64(blk - HDR) as usize;
        let next = blk + size;
        let nh = rd64(next);
        let nsz = if nh & IN_USE == 0 { (nh & !FLAGS) as usize } else { 0 };
        let total = psz + size + nsz;
        if total < need {
            return None;
        }
        let prev = blk - psz;
        self.remove_free(prev, psz);
        if nsz != 0 {
            self.remove_free(next, nsz);
        }
        // SAFETY: source and destination lie inside the merged block (all three parts are owned by this
        // heap); `copy` handles the overlap, and this growth keeps all `old_size` live payload bytes.
        unsafe { ptr::copy((blk + HDR) as *const u8, (prev + HDR) as *mut u8, old_size) };
        let rem = total - need;
        if rem >= MIN_BLOCK {
            wr64(prev, need as u64 | IN_USE);
            self.insert_free(prev + need, rem);
            self.in_use += need - size;
        } else {
            wr64(prev, total as u64 | IN_USE);
            self.clear_prev_free(prev + total);
            self.in_use += total - size;
        }
        Some(prev + HDR)
    }

    /// Tries to extend the in-use block (`blk`, `size`) to `need` bytes without moving it.
    fn grow_in_place(&mut self, blk: usize, size: usize, need: usize) -> bool {
        let next = blk + size;
        let nh = rd64(next);
        let nsz = if nh & IN_USE == 0 { (nh & !FLAGS) as usize } else { 0 };
        if nsz == 0 || size + nsz < need {
            return false;
        }
        self.remove_free(next, nsz);
        let total = size + nsz;
        let rem = total - need;
        let keep = rd64(blk) & PREV_FREE;
        if rem >= MIN_BLOCK {
            wr64(blk, need as u64 | IN_USE | keep);
            self.insert_free(blk + need, rem);
            self.in_use += need - size;
        } else {
            wr64(blk, total as u64 | IN_USE | keep);
            self.clear_prev_free(blk + total);
            self.in_use += total - size;
        }
        true
    }
}

pub(crate) struct Heap {
    ordinary: Arena,
    reserve: Arena,
}

impl Heap {
    pub(crate) const fn new() -> Self {
        Self { ordinary: Arena::new(), reserve: Arena::new() }
    }

    pub(crate) fn footprint(&self) -> u64 {
        self.ordinary.footprint + self.reserve.footprint
    }

    pub(crate) fn footprint_high_water(&self) -> u64 {
        // Pages are never returned, so the current footprint is also the peak.
        self.footprint()
    }

    fn in_reserve(&self, addr: usize) -> bool {
        addr < self.reserve.tail && self.reserve.tail - addr < RESERVE
    }

    /// Allocates `size` bytes aligned to `align`; returns the address, or 0 on refusal.
    pub(crate) fn alloc<P: Pages>(&mut self, pages: &mut P, size: usize, align: usize, ceiling: u64, reserve_open: bool) -> usize {
        if align > ALIGN {
            return 0;
        }
        let Some(need) = block_size(size) else { return 0 };
        self.alloc_block(pages, need, ceiling, reserve_open)
    }

    fn alloc_block<P: Pages>(&mut self, pages: &mut P, need: usize, ceiling: u64, reserve_open: bool) -> usize {
        if self.reserve.tail == 0 && !self.reserve.grow(pages, MIN_BLOCK, ceiling.min(RESERVE as u64), false) {
            return 0;
        }
        if reserve_open {
            if let Some(blk) = self.ordinary.find(need) {
                return self.ordinary.take(blk, need);
            }
            if let Some(blk) = self.reserve.find(need) {
                return self.reserve.take(blk, need);
            }
        }
        self.ordinary.alloc(pages, need, ceiling.saturating_sub(RESERVE as u64))
    }

    pub(crate) fn dealloc(&mut self, addr: usize) {
        if self.in_reserve(addr) {
            self.reserve.dealloc(addr);
        } else {
            self.ordinary.dealloc(addr);
        }
    }

    /// Resizes a live block. Refusal returns 0 and leaves that block intact.
    pub(crate) fn realloc<P: Pages>(&mut self, pages: &mut P, addr: usize, old_size: usize, align: usize, new_size: usize, ceiling: u64, reserve_open: bool) -> usize {
        if align > ALIGN {
            return 0;
        }
        let Some(need) = block_size(new_size) else { return 0 };
        let growing = new_size > old_size;
        if growing && new_size >= PAGE {
            // Leave up to one eighth of the size class for later growth. The old payload is untouched
            // on refusal, so tight ceilings can still serve the exact size.
            let log = usize::BITS - 1 - new_size.leading_zeros();
            let step = 1 << (log - 3);
            if let Some(rounded) = block_size(new_size.next_multiple_of(step)).filter(|&n| n > need) {
                let adjacent = Self::adjacent_capacity(addr);
                if need > adjacent || rounded <= adjacent {
                    let p = self.resize(pages, addr, old_size, rounded, ceiling, reserve_open, true);
                    if p != 0 {
                        return p;
                    }
                }
            }
        }
        self.resize(pages, addr, old_size, need, ceiling, reserve_open, growing)
    }

    fn adjacent_capacity(addr: usize) -> usize {
        let blk = addr - HDR;
        let size = (rd64(blk) & !FLAGS) as usize;
        let next = rd64(blk + size);
        size + if next & IN_USE == 0 { (next & !FLAGS) as usize } else { 0 }
    }

    fn resize<P: Pages>(&mut self, pages: &mut P, addr: usize, old_size: usize, need: usize, ceiling: u64, reserve_open: bool, growing: bool) -> usize {
        let resized = if self.in_reserve(addr) {
            if reserve_open || !growing {
                self.reserve.resize(pages, addr, old_size, need, RESERVE as u64)
            } else {
                // A reserve allocation can outlive the scope, but can only grow in the ordinary arena.
                None
            }
        } else {
            self.ordinary.resize(pages, addr, old_size, need, ceiling.saturating_sub(RESERVE as u64))
        };
        if let Some(addr) = resized {
            return addr;
        }
        let moved = self.alloc_block(pages, need, ceiling, reserve_open);
        if moved == 0 {
            return 0;
        }
        // SAFETY: both allocations are live and disjoint; each holds at least the number of bytes copied.
        unsafe { ptr::copy_nonoverlapping(addr as *const u8, moved as *mut u8, old_size) };
        self.dealloc(addr);
        moved
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::WasmHeap;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{rd64, wr64, Heap, Pages, HDR, PAGE};
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;

    // One of the header bits already masked by FLAGS on every size read.
    const TRANSFER: u64 = 4;

    struct Memory;

    impl Pages for Memory {
        fn grow(&mut self, pages: usize) -> Option<usize> {
            let old = core::arch::wasm32::memory_grow(0, pages);
            if old == usize::MAX {
                None
            } else {
                old.checked_mul(PAGE)
            }
        }
    }

    /// The inner allocator of the wasm build. The only place in the kernel that grows linear memory.
    pub(crate) struct WasmHeap(UnsafeCell<Heap>);

    // SAFETY: the kernel runs on one thread (spec L1a-1 section 4), so the heap state is never accessed
    // concurrently.
    unsafe impl Sync for WasmHeap {}

    impl WasmHeap {
        pub(crate) const fn new() -> Self {
            WasmHeap(UnsafeCell::new(Heap::new()))
        }

        /// Grows linear memory by exactly `pages` for a snapshot restore, without touching the free
        /// lists: the restored image carries the heap metadata itself. Checks the final linear-memory
        /// byte size against the explicit 268,435,456 B default cap with non-wrapping arithmetic.
        /// Returns false (growing nothing) when the cap would be exceeded or the engine refuses.
        pub(crate) fn restore_grow(&self, pages: usize) -> bool {
            const DEFAULT_CAP: u64 = 268_435_456;
            let Some(want) = (pages as u64).checked_mul(PAGE as u64) else { return false };
            let current = (core::arch::wasm32::memory_size(0) as u64).saturating_mul(PAGE as u64);
            let Some(final_bytes) = current.checked_add(want) else { return false };
            if final_bytes > DEFAULT_CAP {
                return false;
            }
            if Memory.grow(pages).is_none() {
                return false;
            }
            // The grown bytes hold restored live data, not free blocks; only the footprint moves,
            // by exactly the grown amount (every page since start was grown, so this rejoins the
            // capture; the restored arenas keep their own split once the image is installed).
            self.heap().ordinary.footprint += want;
            true
        }

        fn heap(&self) -> &mut Heap {
            // SAFETY: single-threaded, and no allocator method re-enters another one while it holds this
            // reference (the heap calls no allocation function).
            unsafe { &mut *self.0.get() }
        }

        pub(crate) fn footprint(&self) -> u64 {
            self.heap().footprint()
        }

        pub(crate) fn footprint_high_water(&self) -> u64 {
            self.heap().footprint_high_water()
        }

        /// Sets whether this live block carries a canonical ABI transfer charge.
        ///
        /// # Safety
        /// `ptr` must designate a live, nonempty block of this allocator.
        pub(crate) unsafe fn mark_transfer(ptr: *mut u8, charged: bool) {
            let header = ptr as usize - HDR;
            let flag = if charged { TRANSFER } else { 0 };
            wr64(header, (rd64(header) & !TRANSFER) | flag);
        }
    }

    // SAFETY: `Heap` hands out disjoint, 16-aligned blocks inside memory it obtained from `memory.grow`, never
    // hands out a live block twice, and returns null (never traps) when it cannot satisfy a request.
    unsafe impl GlobalAlloc for WasmHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let open = super::super::reserve::is_open();
            self.heap().alloc(&mut Memory, layout.size(), layout.align(), super::super::ceiling::current_ceiling(), open) as *mut u8
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if rd64(ptr as usize - HDR) & TRANSFER != 0 {
                super::super::headroom().release(layout.size() as u64);
            }
            self.heap().dealloc(ptr as usize);
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let transfer = rd64(ptr as usize - HDR) & TRANSFER != 0;
            let open = super::super::reserve::is_open();
            let resized = self.heap().realloc(&mut Memory, ptr as usize, layout.size(), layout.align(), new_size, super::super::ceiling::current_ceiling(), open) as *mut u8;
            if transfer && !resized.is_null() {
                // An ordinary Rust resize takes ownership from the transfer account.
                // SAFETY: the allocator just resized this live block and returned its address.
                unsafe { Self::mark_transfer(resized, false) };
                super::super::headroom().release(layout.size() as u64);
            }
            resized
        }
    }
}

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
