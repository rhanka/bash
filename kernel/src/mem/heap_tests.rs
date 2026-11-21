//! Native unit tests of the heap logic, driven through a fake page source over a leaked arena.
use super::*;
use core::alloc::{GlobalAlloc, Layout};

/// A fake memory: pages come from a leaked, 64 KiB-aligned arena. With `gap`, every grow skips one page so
/// that regions are not contiguous.
struct Fake {
    base: usize,
    pages: usize,
    used: usize,
    gap: bool,
    refuse: bool,
    starts: Vec<usize>,
    last_end: usize,
}

impl Fake {
    fn new(pages: usize, gap: bool) -> Fake {
        // The system allocator is used directly: the arena must not count against the module ceiling.
        let layout = Layout::from_size_align(pages * PAGE, PAGE).unwrap();
        // SAFETY: the layout has a non-zero size.
        let base = unsafe { std::alloc::System.alloc(layout) } as usize;
        assert_ne!(base, 0);
        Fake { base, pages, used: 0, gap, refuse: false, starts: vec![], last_end: 0 }
    }
}

impl Pages for Fake {
    fn grow(&mut self, n: usize) -> Option<usize> {
        if self.refuse {
            return None;
        }
        let starts_arena = self.used <= 1;
        let skip = if self.gap && self.used > 0 { 1 } else { 0 };
        if self.used + skip + n > self.pages {
            return None;
        }
        self.used += skip;
        let addr = self.base + self.used * PAGE;
        self.used += n;
        if starts_arena || addr != self.last_end {
            self.starts.push(addr);
        }
        self.last_end = addr + n * PAGE;
        Some(addr)
    }
}

const BIG: u64 = 1 << 40;

/// Walks every region and checks the boundary-tag invariants; returns the sum of the in-use block sizes.
fn validate(h: &Heap, f: &Fake) -> usize {
    let (reserve, ordinary) = f.starts.split_at(f.starts.len().min(1));
    validate_arena(&h.reserve, reserve) + validate_arena(&h.ordinary, ordinary)
}

fn validate_arena(h: &Arena, starts: &[usize]) -> usize {
    let mut in_use = 0;
    let mut free_total = 0;
    let mut free_count = 0;
    for &start in starts {
        let mut blk = start + HDR;
        let mut prev_free = false;
        loop {
            let hdr = rd64(blk);
            let size = (hdr & !FLAGS) as usize;
            assert_eq!(hdr & PREV_FREE != 0, prev_free, "prev-free flag at {blk:#x}");
            if size == 0 {
                assert_ne!(hdr & IN_USE, 0, "sentinel");
                break;
            }
            assert_eq!(size % 16, 0);
            assert!(size >= MIN_BLOCK);
            assert_eq!(blk % 16, 8);
            if hdr & IN_USE != 0 {
                in_use += size;
                prev_free = false;
            } else {
                assert!(!prev_free, "two adjacent free blocks at {blk:#x}");
                assert_eq!(rd64(blk + size - HDR) as usize, size, "footer");
                free_total += size;
                free_count += 1;
                prev_free = true;
            }
            blk += size;
        }
    }
    assert_eq!(in_use, h.in_use);
    // Every free block is in exactly the right bin.
    let mut listed = 0;
    let mut listed_total = 0;
    for bin in 0..BINS {
        let (fl, sl) = (bin / SL, bin % SL);
        let mut at = h.heads[bin];
        assert_eq!(at != 0, h.sl_bitmap[fl] & (1 << sl) != 0, "sl bitmap bin {bin}");
        let mut prev = 0;
        while at != 0 {
            let size = (rd64(at) & !FLAGS) as usize;
            assert_eq!(rd64(at) & IN_USE, 0);
            assert_eq!(mapping(size), (fl, sl));
            assert_eq!(rdw(at + HDR + W), prev);
            listed += 1;
            listed_total += size;
            prev = at;
            at = rdw(at + HDR);
        }
    }
    for fl in 0..FL_COUNT {
        assert_eq!(h.sl_bitmap[fl] != 0, h.fl_bitmap & (1 << fl) != 0, "fl bitmap {fl}");
    }
    assert_eq!((listed, listed_total), (free_count, free_total));
    in_use
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn fill(addr: usize, len: usize, tag: u8) {
    // SAFETY: the block at `addr` holds at least `len` writable payload bytes (the test allocated it).
    unsafe { ptr::write_bytes(addr as *mut u8, tag, len) };
}

fn check_fill(addr: usize, len: usize, tag: u8) {
    // SAFETY: the block at `addr` holds at least `len` readable payload bytes (the test allocated it).
    let s = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
    assert!(s.iter().all(|&b| b == tag), "block content damaged at {addr:#x}");
}

#[test]
fn bins_cover_every_size_and_search_rounds_up() {
    let mut last = (0, 0);
    let mut size = MIN_BLOCK;
    while size < 1 << 22 {
        let m = mapping(size);
        assert!(m >= last, "bins are monotonic");
        last = m;
        let (fl, sl) = mapping_search(size);
        assert!(fl * SL + sl >= m.0 * SL + m.1);
        // Every block of the searched bin is at least `size`: the lower bound of that bin is >= size.
        let lower = if fl == 0 { sl * 16 } else { (1usize << (fl + 7)) + sl * (1usize << (fl + 3)) };
        assert!(lower >= size, "size {size} bin ({fl},{sl}) lower bound {lower}");
        size += 16;
    }
    assert_eq!(mapping(1 << 33), (FL_COUNT - 1, SL - 1));
}

#[test]
fn alignment_size_and_reuse() {
    let mut f = Fake::new(64, false);
    let mut h = Heap::new();
    let a = h.alloc(&mut f, 1, 1, BIG, false);
    let b = h.alloc(&mut f, 24, 8, BIG, false);
    let c = h.alloc(&mut f, 100, 16, BIG, false);
    for p in [a, b, c] {
        assert_ne!(p, 0);
        assert_eq!(p % 16, 0);
    }
    assert_eq!(b - a, 32, "a small allocation uses one minimum-size block");
    assert_eq!(c - b, 32, "a 24-byte payload fits in one minimum-size block");
    assert_eq!(h.footprint(), 2 * PAGE as u64);
    validate(&h, &f);
    h.dealloc(b);
    // A freed block of the same class is reused.
    assert_eq!(h.alloc(&mut f, 24, 8, BIG, false), b);
    validate(&h, &f);
    assert_eq!(h.alloc(&mut f, 8, 32, BIG, false), 0, "alignment 32 is refused");
}

#[test]
fn merging_returns_the_whole_heap() {
    let mut f = Fake::new(64, false);
    let mut h = Heap::new();
    let ps: Vec<usize> = (0..200).map(|i| h.alloc(&mut f, 16 + i * 8, 8, BIG, false)).collect();
    assert!(ps.iter().all(|&p| p != 0));
    for (i, &p) in ps.iter().enumerate() {
        if i % 2 == 0 {
            h.dealloc(p);
        }
    }
    validate(&h, &f);
    for (i, &p) in ps.iter().enumerate() {
        if i % 2 == 1 {
            h.dealloc(p);
        }
    }
    assert_eq!(validate(&h, &f), 0);
    // One block spans the region: the largest request that fits does fit without growing.
    let fp = h.footprint();
    let big = h.alloc(&mut f, fp as usize - RESERVE - 3 * HDR, 16, BIG, false);
    assert_ne!(big, 0);
    assert_eq!(h.footprint(), fp);
}

#[test]
fn realloc_grows_and_shrinks_in_place_when_it_can() {
    let mut f = Fake::new(64, false);
    let mut h = Heap::new();
    let a = h.alloc(&mut f, 100, 8, BIG, false);
    fill(a, 100, 7);
    // The block sits at the heap end: growing is in place, across pages.
    let a2 = h.realloc(&mut f, a, 100, 8, 3 * PAGE, BIG, false);
    assert_eq!(a2, a);
    check_fill(a2, 100, 7);
    validate(&h, &f);
    let a3 = h.realloc(&mut f, a2, 3 * PAGE, 8, 50, BIG, false);
    assert_eq!(a3, a);
    check_fill(a3, 50, 7);
    validate(&h, &f);
    // A block followed by an in-use block moves.
    let b = h.alloc(&mut f, 100, 8, BIG, false);
    let a4 = h.realloc(&mut f, a3, 50, 8, 400, BIG, false);
    assert_ne!(a4, a3);
    assert_ne!(b, 0);
    check_fill(a4, 50, 7);
    validate(&h, &f);
    // Shrinking with a free neighbour merges the remainder into it.
    let before = h.ordinary.in_use;
    let a5 = h.realloc(&mut f, a4, 400, 8, 64, BIG, false);
    assert_eq!(a5, a4);
    assert!(h.ordinary.in_use < before);
    validate(&h, &f);
}

#[test]
fn large_realloc_retries_the_exact_size_at_the_ceiling() {
    let mut f = Fake::new(4, false);
    let mut h = Heap::new();
    let ceiling = 4 * PAGE as u64;
    let a = h.alloc(&mut f, 100, 8, ceiling, false);
    fill(a, 100, 17);
    let new_size = 3 * PAGE - 3 * HDR;
    let p = h.realloc(&mut f, a, 100, 8, new_size, ceiling, false);
    assert_eq!(p, a, "the exact size fits when the rounded capacity does not");
    check_fill(p, 100, 17);
    assert_eq!(h.footprint(), ceiling);
    h.dealloc(p);
    assert_eq!(validate(&h, &f), 0);
}

#[test]
fn large_realloc_prefers_an_exact_adjacent_fit_to_rounded_growth() {
    let mut f = Fake::new(8, false);
    let mut h = Heap::new();
    let a = h.alloc(&mut f, 100, 8, BIG, false);
    fill(a, 100, 23);
    let new_size = PAGE + 1;
    let neighbour_size = block_size(new_size).unwrap() - block_size(100).unwrap() - HDR;
    let neighbour = h.alloc(&mut f, neighbour_size, 8, BIG, false);
    let blocker = h.alloc(&mut f, PAGE, 8, BIG, false);
    assert_eq!(neighbour, a + block_size(100).unwrap());
    assert_eq!(blocker, neighbour + block_size(neighbour_size).unwrap());
    h.dealloc(neighbour);
    let before = h.footprint();
    assert_eq!(h.realloc(&mut f, a, 100, 8, new_size, BIG, false), a);
    assert_eq!(h.footprint(), before);
    check_fill(a, 100, 23);
    h.dealloc(a);
    h.dealloc(blocker);
    assert_eq!(validate(&h, &f), 0);
}

#[test]
fn refusals_leave_the_heap_untouched() {
    let mut f = Fake::new(4, false);
    let mut h = Heap::new();
    // Engine refusal.
    let a = h.alloc(&mut f, 100, 8, BIG, false);
    fill(a, 100, 3);
    assert_eq!(h.alloc(&mut f, 10 * PAGE, 8, BIG, false), 0);
    assert_eq!(h.realloc(&mut f, a, 100, 8, 10 * PAGE, BIG, false), 0);
    check_fill(a, 100, 3);
    // Absurd sizes and alignments.
    assert_eq!(h.alloc(&mut f, usize::MAX / 2, 8, BIG, false), 0);
    assert_eq!(h.alloc(&mut f, usize::MAX, 8, BIG, false), 0);
    assert_eq!(h.alloc(&mut f, 1 << 31, 8, BIG, false), 0);
    assert_eq!(h.alloc(&mut f, 8, 64, BIG, false), 0);
    f.refuse = true;
    assert_eq!(h.alloc(&mut f, 2 * PAGE, 8, BIG, false), 0);
    // The module is still usable.
    assert_ne!(h.alloc(&mut f, 100, 8, BIG, false), 0);
    validate(&h, &f);
}

#[test]
fn ceiling_bounds_the_footprint_and_the_reserve_holds_back_ordinary_requests() {
    let ceiling = 8 * PAGE as u64;
    let mut f = Fake::new(64, false);
    let mut h = Heap::new();
    let mut ps = vec![];
    loop {
        let p = h.alloc(&mut f, 1000, 8, ceiling, false);
        if p == 0 {
            break;
        }
        ps.push(p);
    }
    assert!(h.footprint() <= ceiling);
    assert_eq!(h.reserve.in_use, 0, "ordinary requests stay out of the reserve");
    assert_eq!(h.alloc(&mut f, 4096, 8, ceiling, false), 0);
    let r = h.alloc(&mut f, 4096, 8, ceiling, true);
    assert_ne!(r, 0, "the reserve serves a request inside the scope");
    assert!(h.footprint() <= ceiling);
    // Beyond the ceiling the reserve cannot help either.
    assert_eq!(h.alloc(&mut f, 4 * PAGE, 8, ceiling, true), 0);
    for p in ps {
        h.dealloc(p);
    }
    h.dealloc(r);
    assert_eq!(validate(&h, &f), 0);
}

#[test]
fn the_top_page_closes_at_a_representable_header() {
    let top = usize::MAX - (PAGE - 1);
    assert_eq!(region_end_header(top, PAGE as u64), Some(usize::MAX - (HDR - 1)));
    assert_eq!(region_end_header(top, 2 * PAGE as u64), None);
    assert_eq!(region_end_header(top, 0), None);
}

#[test]
fn the_reserve_survives_the_engine_limit_and_closed_scope_reallocation() {
    let mut f = Fake::new(8, false);
    let mut h = Heap::new();
    let small: Vec<_> = (0..4096).map(|_| h.alloc(&mut f, 16, 8, BIG, false)).collect();
    assert!(small.iter().all(|&p| p != 0));
    for &p in small.iter().step_by(2) {
        h.dealloc(p);
    }
    let mut large = Vec::new();
    loop {
        let p = h.alloc(&mut f, 4096, 8, BIG, false);
        if p == 0 {
            break;
        }
        large.push(p);
    }
    assert_eq!(f.used, f.pages, "the engine limit is below the configured ceiling");
    let footprint = h.footprint();
    assert_eq!(h.footprint_high_water(), footprint);
    let r = h.alloc(&mut f, 4096, 8, BIG, true);
    assert_ne!(r, 0);
    assert!(h.in_reserve(r));
    fill(r, 4096, 11);
    assert_eq!(h.realloc(&mut f, r, 4096, 8, 8192, BIG, false), 0);
    check_fill(r, 4096, 11);
    assert_eq!(h.realloc(&mut f, r, 4096, 8, 8192, BIG, true), r);
    check_fill(r, 4096, 11);
    for p in large {
        h.dealloc(p);
    }
    let ordinary = h.realloc(&mut f, r, 8192, 8, 16384, BIG, false);
    assert_ne!(ordinary, 0);
    assert!(!h.in_reserve(ordinary));
    check_fill(ordinary, 4096, 11);
    h.dealloc(ordinary);
    for &p in small.iter().skip(1).step_by(2) {
        h.dealloc(p);
    }
    assert_eq!(h.footprint(), footprint);
    assert_eq!(validate(&h, &f), 0);
}

fn stress(gap: bool, seed: u64, ops: usize) {
    let mut f = Fake::new(1024, gap);
    let mut h = Heap::new();
    let mut rng = Rng(seed);
    // (address, requested size, tag)
    let mut live: Vec<(usize, usize, u8)> = vec![];
    for step in 0..ops {
        match rng.below(10) {
            0..=4 => {
                let size = match rng.below(20) {
                    0 => 20_000 + rng.below(200_000),
                    1..=3 => 256 + rng.below(4096),
                    _ => 1 + rng.below(300),
                };
                let align = [1, 2, 4, 8, 16][rng.below(5)];
                let p = h.alloc(&mut f, size, align, BIG, false);
                assert_ne!(p, 0, "step {step} size {size} footprint {} arena used {}", h.footprint(), f.used);
                assert_eq!(p % align.max(1), 0);
                assert_eq!(p % 16, 0);
                let tag = (step % 251) as u8 + 1;
                fill(p, size, tag);
                live.push((p, size, tag));
            }
            5..=7 if !live.is_empty() => {
                let i = rng.below(live.len());
                let (p, size, tag) = live.swap_remove(i);
                check_fill(p, size, tag);
                h.dealloc(p);
            }
            _ if !live.is_empty() => {
                let i = rng.below(live.len());
                let (p, size, tag) = live[i];
                let new = if rng.below(2) == 0 { size + rng.below(5000) + 1 } else { 1 + rng.below(size) };
                let q = h.realloc(&mut f, p, size, 8, new, BIG, false);
                assert_ne!(q, 0);
                check_fill(q, size.min(new), tag);
                fill(q, new, tag);
                live[i] = (q, new, tag);
            }
            _ => {}
        }
        if step % 500 == 0 {
            validate(&h, &f);
        }
    }
    // Live blocks never overlap.
    let mut spans: Vec<(usize, usize)> = live.iter().map(|&(p, s, _)| (p, p + s)).collect();
    spans.sort();
    for w in spans.windows(2) {
        assert!(w[0].1 <= w[1].0, "overlapping blocks");
    }
    for (p, size, tag) in live {
        check_fill(p, size, tag);
        h.dealloc(p);
    }
    assert_eq!(validate(&h, &f), 0);
}

#[test]
fn stress_contiguous() {
    stress(false, 0x9E37_79B9_7F4A_7C15, 20_000);
}

#[test]
fn stress_with_gaps_between_regions() {
    stress(true, 0x1234_5678_9ABC_DEF1, 20_000);
}
