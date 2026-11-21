//! Gate 12 of lot L1a-2: `KChunks` against a plain byte vector model, sizing and budget rules.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

fn contents(c: &KChunks) -> Vec<u8> {
    let mut v = vec![];
    for s in c.chunks() {
        v.extend_from_slice(s);
    }
    v
}

fn fill(rng: &mut Rng, n: usize) -> Vec<u8> {
    (0..n).map(|_| rng.next() as u8).collect()
}

/// 20 000 seeded operations; sizes straddle the 64 KiB chunk edge.
fn model() {
    let b = root(30, BIG);
    let mut c = KChunks::new_in(&b);
    let mut r: Vec<u8> = vec![];
    let mut rng = Rng(0xBEEF);
    let size = |rng: &mut Rng| match rng.below(6) {
        0 => 0,
        1..=2 => rng.below(100) as usize,
        3 => rng.below(5000) as usize,
        4 => 65_536 - 30 + rng.below(60) as usize,
        _ => rng.below(200_000) as usize,
    };
    for step in 0..20_000 {
        match rng.below(10) {
            0..=2 => {
                let n = size(&mut rng);
                let d = fill(&mut rng, n);
                c.try_append(&d).unwrap();
                r.extend_from_slice(&d);
            }
            3..=4 => {
                // Inside, across the end, and past the end.
                let off = match rng.below(3) {
                    0 => rng.below(r.len() as u64 + 1),
                    1 => r.len() as u64 - (r.len() as u64).min(rng.below(100)),
                    _ => r.len() as u64 + rng.below(70_000),
                };
                let n = size(&mut rng).min(100_000);
                let d = fill(&mut rng, n);
                c.try_write_at(off, &d).unwrap();
                let end = off as usize + d.len();
                if end > r.len() {
                    r.resize(end, 0);
                }
                r[off as usize..end].copy_from_slice(&d);
            }
            5..=6 => {
                let off = rng.below(r.len() as u64 + 10);
                let mut out = vec![0u8; size(&mut rng).min(150_000)];
                let n = c.read_at(off, &mut out);
                let want = (r.len().saturating_sub(off as usize)).min(out.len());
                assert_eq!(n, want);
                if n > 0 {
                    assert_eq!(&out[..n], &r[off as usize..off as usize + n]);
                }
            }
            7 => {
                let to = rng.below(r.len() as u64 + 20);
                c.truncate(to);
                r.truncate(to as usize);
            }
            8 => {
                let max = size(&mut rng);
                let mut got = vec![];
                let n = c.drain_front(max, |s| got.extend_from_slice(s));
                assert_eq!(n, max.min(r.len()));
                assert_eq!(got, r[..n]);
                r.drain(..n);
            }
            _ => {
                if rng.below(40) == 0 {
                    c.clear();
                    r.clear();
                }
            }
        }
        assert_eq!(c.len(), r.len() as u64, "length at step {step}");
        if step % 100 == 0 {
            assert_eq!(contents(&c), r, "content at step {step}");
            // The memory held is the length rounded up, plus 16 bytes of bookkeeping per chunk (and the
            // drained prefix of the first chunk, at most one chunk).
            let held = b.used();
            assert!(held <= r.len() as u64 + 2 * KChunks::CHUNK as u64 + 16 * c.chunk_count() as u64 + 64, "held {held} for {}", r.len());
        }
    }
    assert_eq!(contents(&c), r);
    let copy = c.try_clone().unwrap();
    assert_eq!(contents(&copy), r);
    drop(copy);
    drop(c);
    assert_eq!(b.used(), 0);
}

/// 100 MiB in 1 MiB steps: no block larger than a chunk, bounded charge; the sizes of small sequences.
fn sizing() {
    let b = root(31, BIG);
    let mut c = KChunks::new_in(&b);
    let step = vec![0xA5u8; 1 << 20];
    for i in 0..100 {
        c.try_append(&step).unwrap();
        let len = c.len();
        assert!(b.used() <= len + 65_536 + 16 * c.chunk_count() as u64, "charge at {i}");
        for k in 0..c.chunk_count() {
            assert!(c.capacity_of_chunk(k) <= KChunks::CHUNK, "no block larger than CHUNK");
        }
    }
    assert_eq!(c.len(), 100 << 20);
    assert_eq!(c.chunks().map(|s| s.len() as u64).sum::<u64>(), c.len());
    assert!(c.chunks().all(|s| s.len() <= KChunks::CHUNK));
    // Drain everything: memory is released along the way and ends at 0.
    let mut seen = 0u64;
    let mut peak_after = u64::MAX;
    while !c.is_empty() {
        seen += c.drain_front(3 << 19, |s| assert!(s.iter().all(|&x| x == 0xA5))) as u64;
        assert!(b.used() <= peak_after, "memory never grows while draining");
        peak_after = b.used();
    }
    assert_eq!((seen, b.used()), (100 << 20, 0), "budget back to 0 after draining everything");
    // Small sequences cost a small block.
    let mut s = KChunks::new_in(&b);
    s.try_append(&[1; 10]).unwrap();
    assert!(b.used() <= 64 + 16, "10 bytes charge {}", b.used());
    s.try_append(&[2; 100]).unwrap();
    assert!(b.used() <= 128 + 16);
    s.truncate(10);
    assert!(b.used() <= 64 + 16, "shrunk after truncate: {}", b.used());
    drop(s);
    assert_eq!(b.used(), 0);
    drop(c);
    assert_eq!(b.used(), 0);
}

/// A refusal leaves length and content unchanged and the budget as it was.
fn refusal() {
    let cap = 3 * KChunks::CHUNK as u64;
    let b = root(32, cap);
    let mut c = KChunks::new_in(&b);
    let a = vec![7u8; 100_000];
    c.try_append(&a).unwrap();
    let used = b.used();
    let big = vec![9u8; 500_000];
    assert_eq!(c.try_append(&big), Err(MemError::Limit(LimitId(32))));
    assert_eq!(c.len(), 100_000);
    assert!(b.used() <= used, "the budget is not left higher after a refusal");
    assert_eq!(contents(&c), a);
    assert_eq!(c.try_write_at(1_000_000, b"x"), Err(MemError::Limit(LimitId(32))));
    assert_eq!(c.len(), 100_000);
    assert!(b.used() <= used, "the budget is not left higher after a refusal");
    assert_eq!(c.try_write_at(u64::MAX - 1, b"xyz"), Err(MemError::Overflow));
    assert_eq!(c.try_write_at(u64::MAX / 2, b"x"), Err(MemError::Limit(LimitId(32))));
    // What fits still goes through.
    c.try_append(&[1; 50_000]).unwrap();
    assert_eq!(c.len(), 150_000);
    // Writing a hole zero-fills it.
    c.try_write_at(150_100, b"z").unwrap();
    let mut gap = [9u8; 100];
    assert_eq!(c.read_at(150_000, &mut gap), 100);
    assert!(gap.iter().all(|&x| x == 0));
    // Stale bytes of an earlier truncate never reappear.
    c.truncate(100);
    c.try_write_at(500, b"q").unwrap();
    let mut mid = [9u8; 400];
    assert_eq!(c.read_at(100, &mut mid), 400);
    assert!(mid.iter().all(|&x| x == 0));
    assert_eq!(format!("{c:?}"), "KChunks { len: 501 }");
    drop(c);
    assert_eq!(b.used(), 0);
}

fn shrink_after_large_removal() {
    let b = root(33, BIG);
    let mut c = KChunks::new_in(&b);
    let step = vec![0xA5; 1 << 20];
    for drain in [false, true] {
        for _ in 0..100 {
            c.try_append(&step).unwrap();
        }
        let remaining = b"0123456789";
        if drain {
            let removed = c.len() as usize - remaining.len();
            c.try_write_at(removed as u64, remaining).unwrap();
            assert_eq!(c.drain_front(removed, |_| {}), removed);
        } else {
            c.try_write_at(0, remaining).unwrap();
            c.truncate(remaining.len() as u64);
        }
        assert_eq!(contents(&c), remaining);
        assert!(b.used() <= 80, "{} bytes charged for 10 bytes after drain={drain}", b.used());
        c.clear();
        assert_eq!(b.used(), 0);
    }
    drop(c);
    assert_eq!(b.used(), 0);
}

fn truncate_after_partial_drain() {
    let b = root(34, BIG);
    let mut c = KChunks::new_in(&b);
    let prefix = KChunks::CHUNK - 20;
    c.try_append(&vec![0; prefix]).unwrap();
    c.try_append(b"0123456789abcdefghij").unwrap();
    c.try_append(b"next chunk").unwrap();
    assert_eq!(c.drain_front(prefix, |_| {}), prefix);
    c.truncate(10);
    assert_eq!(contents(&c), b"0123456789");
    assert!(b.used() <= 80, "{} bytes charged for 10 bytes", b.used());
    c.try_append(b"next").unwrap();
    assert_eq!(contents(&c), b"0123456789next");
    drop(c);
    assert_eq!(b.used(), 0);
}

fn short_drain_result_across_chunk_boundary() {
    let b = root(35, BIG);
    let mut c = KChunks::new_in(&b);
    let prefix = KChunks::CHUNK - 4;
    c.try_append(&vec![0; prefix]).unwrap();
    c.try_append(b"0123456789").unwrap();
    assert_eq!(c.drain_front(prefix, |_| {}), prefix);
    assert_eq!(contents(&c), b"0123456789");
    assert_eq!(c.chunk_count(), 1);
    assert!(b.used() <= 80, "{} bytes charged for 10 bytes", b.used());
    drop(c);
    assert_eq!(b.used(), 0);
}

#[test]
fn chunks() {
    start();
    model();
    sizing();
    refusal();
    shrink_after_large_removal();
    truncate_after_partial_drain();
    short_drain_result_across_chunk_boundary();
}
