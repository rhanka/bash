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
//! * Memory is obtained from the engine only when no free block fits, in steps of whole 64 KiB pages: the
//!   pages the request is missing, or one eighth of the footprint (at least one page) when that is larger,
//!   never above the ceiling. A region that follows the previous one is merged into it: the closing sentinel
//!   header of the old region becomes the start of the new free block.
//! * The reserve: ordinary requests may not take the last 64 KiB below the ceiling (the sum of in-use block
//!   sizes stays at most the ceiling, rounded down to pages, minus the reserve). Requests inside
//!   `reserve_scope` may. The footprint never exceeds the ceiling, and memory is never given back.
//!
//! The logic is target-independent so that native unit tests drive it with a fake page source; only
//! `WasmHeap` touches `memory.grow`.
use core::ptr;

pub(crate) const PAGE: usize = 65_536;
/// Bytes below the ceiling that ordinary requests cannot use.
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

pub(crate) struct Heap {
    fl_bitmap: u32,
    sl_bitmap: [u32; FL_COUNT],
    heads: [usize; BINS],
    footprint: usize,
    footprint_hw: usize,
    /// Sum of the sizes of the in-use blocks.
    in_use: usize,
    /// Address of the sentinel header that closes the last region (0 before the first growth).
    tail: usize,
    /// Did the last growth extend the previous region? (Always true on wasm: nothing else grows memory.)
    contiguous: bool,
}

impl Heap {
    pub(crate) const fn new() -> Heap {
        Heap { fl_bitmap: 0, sl_bitmap: [0; FL_COUNT], heads: [0; BINS], footprint: 0, footprint_hw: 0, in_use: 0, tail: 0, contiguous: true }
    }

    pub(crate) fn footprint(&self) -> usize {
        self.footprint
    }

    pub(crate) fn footprint_high_water(&self) -> usize {
        self.footprint_hw
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

    /// May a request that raises the in-use bytes by `delta` proceed? Ordinary requests stay out of the reserve.
    fn may_use(&self, delta: usize, ceiling: u64, reserve_open: bool) -> bool {
        if reserve_open {
            return true;
        }
        let cap = (ceiling / PAGE as u64 * PAGE as u64).saturating_sub(RESERVE as u64);
        (self.in_use as u64).saturating_add(delta as u64) <= cap
    }

    // ---- growth ----

    /// Makes a free block of at least `need` bytes appear at the end of the heap, growing memory.
    fn grow<P: Pages>(&mut self, pages: &mut P, need: usize, ceiling: u64) -> bool {
        let tail_free = if self.tail != 0 && self.contiguous && rd64(self.tail) & PREV_FREE != 0 { rd64(self.tail - HDR) as usize } else { 0 };
        let missing = need.saturating_sub(tail_free);
        let min_pages = (missing + 2 * HDR).div_ceil(PAGE);
        let room = (ceiling / PAGE as u64).saturating_sub((self.footprint / PAGE) as u64);
        let room = usize::try_from(room).unwrap_or(usize::MAX);
        if min_pages > room {
            return false;
        }
        let floor = self.footprint / 8 / PAGE;
        let mut want = min_pages.max(floor).max(1).min(room);
        let addr = loop {
            match pages.grow(want) {
                Some(a) => break a,
                None if want > min_pages => want = min_pages,
                None => return false,
            }
        };
        let Some(bytes) = want.checked_mul(PAGE) else { return false };
        self.footprint += bytes;
        self.footprint_hw = self.footprint_hw.max(self.footprint);
        self.add_region(addr, bytes);
        true
    }

    fn add_region(&mut self, addr: usize, bytes: usize) {
        let end = addr + bytes;
        wr64(end - HDR, IN_USE);
        if self.tail != 0 && addr == self.tail + HDR {
            // Contiguous with the previous region: the old closing sentinel becomes the start of the block.
            let mut start = self.tail;
            let mut size = end - HDR - start;
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
            self.insert_free(addr + HDR, bytes - 2 * HDR);
            self.contiguous = false;
        }
        self.tail = end - HDR;
    }

    // ---- interface ----

    /// Allocates `size` bytes aligned to `align`; returns the address, or 0 on refusal.
    pub(crate) fn alloc<P: Pages>(&mut self, pages: &mut P, size: usize, align: usize, ceiling: u64, reserve_open: bool) -> usize {
        if align > ALIGN {
            return 0;
        }
        let Some(need) = block_size(size) else { return 0 };
        if !self.may_use(need, ceiling, reserve_open) {
            return 0;
        }
        // A second growth is needed only when the engine placed the new pages away from the old region.
        for _ in 0..3 {
            if let Some(blk) = self.find(need) {
                return self.take(blk, need);
            }
            if !self.grow(pages, need, ceiling) {
                return 0;
            }
        }
        0
    }

    /// Frees the block at `addr`.
    pub(crate) fn dealloc(&mut self, addr: usize) {
        let blk = addr - HDR;
        let size = (rd64(blk) & !FLAGS) as usize;
        self.in_use -= size;
        self.free_block(blk, size);
    }

    /// Resizes the block at `addr` (`old_size` payload bytes were requested). Returns the new address (0 on
    /// refusal, the block untouched).
    pub(crate) fn realloc<P: Pages>(&mut self, pages: &mut P, addr: usize, old_size: usize, align: usize, new_size: usize, ceiling: u64, reserve_open: bool) -> usize {
        if align > ALIGN {
            return 0;
        }
        let Some(need) = block_size(new_size) else { return 0 };
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
            return addr;
        }
        if !self.may_use(need - size, ceiling, reserve_open) {
            return 0;
        }
        if self.grow_in_place(pages, blk, size, need, ceiling) {
            return addr;
        }
        let moved = self.alloc(pages, new_size, align, ceiling, reserve_open);
        if moved == 0 {
            return 0;
        }
        // SAFETY: both blocks are live and distinct, so they do not overlap; each holds at least
        // `min(old_size, new_size)` payload bytes.
        unsafe { ptr::copy_nonoverlapping(addr as *const u8, moved as *mut u8, old_size.min(new_size)) };
        self.dealloc(addr);
        moved
    }

    /// Tries to extend the in-use block (`blk`, `size`) to `need` bytes without moving it.
    fn grow_in_place<P: Pages>(&mut self, pages: &mut P, blk: usize, size: usize, need: usize, ceiling: u64) -> bool {
        let mut grown = false;
        loop {
            let next = blk + size;
            let nh = rd64(next);
            let nsz = if nh & IN_USE == 0 { (nh & !FLAGS) as usize } else { 0 };
            if nsz != 0 && size + nsz >= need {
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
                return true;
            }
            // Only the end of the heap can grow underneath the block.
            if grown || next + nsz != self.tail {
                return false;
            }
            if !self.grow(pages, need - size, ceiling) {
                return false;
            }
            grown = true;
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::WasmHeap;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::{Heap, Pages, PAGE};
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;

    struct Memory;

    impl Pages for Memory {
        fn grow(&mut self, pages: usize) -> Option<usize> {
            let old = core::arch::wasm32::memory_grow(0, pages);
            if old == usize::MAX {
                None
            } else {
                Some(old * PAGE)
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

        fn heap(&self) -> &mut Heap {
            // SAFETY: single-threaded, and no allocator method re-enters another one while it holds this
            // reference (the heap calls no allocation function).
            unsafe { &mut *self.0.get() }
        }

        pub(crate) fn footprint(&self) -> u64 {
            self.heap().footprint() as u64
        }

        pub(crate) fn footprint_high_water(&self) -> u64 {
            self.heap().footprint_high_water() as u64
        }
    }

    // SAFETY: `Heap` hands out disjoint, 16-aligned blocks inside memory it obtained from `memory.grow`, never
    // hands out a live block twice, and returns null (never traps) when it cannot satisfy a request.
    unsafe impl GlobalAlloc for WasmHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let open = super::super::reserve::is_open();
            self.heap().alloc(&mut Memory, layout.size(), layout.align(), super::super::ceiling::current_ceiling(), open) as *mut u8
        }

        unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
            self.heap().dealloc(ptr as usize);
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let open = super::super::reserve::is_open();
            self.heap().realloc(&mut Memory, ptr as usize, layout.size(), layout.align(), new_size, super::super::ceiling::current_ceiling(), open) as *mut u8
        }
    }
}

#[cfg(test)]
#[path = "heap_tests.rs"]
mod tests;
