//! Timing parts of gates 10 and 11 of lot L1a-2, native. The limits apply to a release build
//! (`cargo test --release`, run by test/mem-native-perf.test.mjs); a debug build runs a reduced size
//! without timing assertions.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;
use std::time::Instant;

#[cfg(debug_assertions)]
const N: usize = 20_000;
#[cfg(not(debug_assertions))]
const N: usize = 1_000_000;

fn limit(label: &str, started: Instant, seconds: f64) {
    let took = started.elapsed().as_secs_f64();
    println!("perf {label}: {took:.3} s (limit {seconds})");
    #[cfg(not(debug_assertions))]
    assert!(took < seconds, "{label} took {took} s, limit {seconds} s");
}

fn kmap() {
    let b = root(40, BIG);
    let mut m: KMap<u64, u64> = KMap::new_in(&b);
    let mut rng = Rng(0x1234_5678);
    let keys: Vec<u64> = (0..N).map(|_| rng.next()).collect();
    let started = Instant::now();
    for &k in &keys {
        m.try_insert(k, k).unwrap();
    }
    // The memory bound of the spec, at the peak.
    assert!(b.used() <= (2 * m.len() * 16 + 64 * m.node_count()) as u64, "bound at the peak: {} B, {} nodes", b.used(), m.node_count());
    println!("perf KMap peak: {} entries, {} nodes, {} B charged ({:.2} B per entry)", m.len(), m.node_count(), b.used(), b.used() as f64 / m.len() as f64);
    for &k in &keys {
        assert_eq!(m.get(&k), Some(&k));
    }
    for &k in &keys {
        assert_eq!(m.remove(&k), Some(k));
    }
    assert!(m.is_empty());
    assert_eq!(b.used(), 0);
    limit("KMap insert+lookup+remove", started, 2.0);
}

fn insertmap() {
    let b = root(41, BIG);
    let mut m: KInsertMap<u64, u64> = KInsertMap::new_in(&b);
    let mut rng = Rng(0x8765_4321);
    let keys: Vec<u64> = (0..N).map(|_| rng.next()).collect();
    let started = Instant::now();
    for &k in &keys {
        m.try_insert(k, k).unwrap();
    }
    assert!(m.keys().copied().eq(keys.iter().copied()), "insertion order");
    for &k in &keys {
        assert_eq!(m.remove(&k), Some(k));
    }
    assert!(m.is_empty());
    assert_eq!(b.used(), 0);
    limit("KInsertMap insert+remove", started, 3.0);
}

#[test]
fn perf() {
    // Each scenario checks its own budget returns to 0 (the output capture of the harness allocates too).
    start();
    kmap();
    insertmap();
}
