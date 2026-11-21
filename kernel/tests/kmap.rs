//! Gate 10 of lot L1a-2 (functional part): the B-tree `KMap` against a reference model, structure stress,
//! failure atomicity and the memory bound. The timing part lives in `perf.rs`.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;
use std::collections::BTreeMap;

fn bound(len: usize, nodes: usize, entry: usize) -> u64 {
    (2 * len * entry + 64 * nodes) as u64
}

/// 200 000 seeded operations against a model; ascending iteration every 1 000th step.
fn model(_base: u64) {
    let b = root(10, BIG);
    let mut m: KMap<u64, u64> = KMap::new_in(&b);
    let mut r: BTreeMap<u64, u64> = BTreeMap::new();
    let mut rng = Rng(0xC0FFEE);
    for step in 0..200_000u64 {
        let key = rng.below(5000);
        match rng.below(4) {
            0 | 1 => assert_eq!(m.try_insert(key, step).unwrap(), r.insert(key, step), "insert/overwrite {key}"),
            2 => assert_eq!(m.get(&key), r.get(&key), "lookup {key}"),
            _ => assert_eq!(m.remove(&key), r.remove(&key), "remove {key}"),
        }
        assert_eq!(m.len(), r.len());
        if step % 1000 == 0 {
            assert!(m.iter().map(|(k, v)| (*k, *v)).eq(r.iter().map(|(k, v)| (*k, *v))), "ascending iteration at {step}");
            assert_eq!(m.first_key_value(), r.iter().next());
            assert_eq!(m.last_key_value(), r.iter().next_back());
            if m.len() >= 16 {
                assert!(b.used() <= bound(m.len(), m.node_count(), 16), "memory bound at {step}: {} nodes", m.node_count());
            }
        }
    }
    for (_, v) in m.iter_mut() {
        *v += 1;
    }
    assert!(m.values().zip(r.values()).all(|(a, b)| *a == *b + 1));
    m.clear();
    assert_eq!((m.len(), b.used(), m.node_count()), (0, 0, 0), "clear releases everything");
    drop(m);
}

/// Sequential, reverse and random insertion followed by removal in several orders: every node kind (split at
/// each position, borrow left and right, merge, root collapse) is crossed; the budget returns to 0.
fn structure(_base: u64) {
    for n in [0usize, 1, 7, 8, 16, 17, 100, 1000, 5000] {
        for order in 0..4 {
            let b = root(11, BIG);
            let mut m: KMap<u32, u32> = KMap::new_in(&b);
            let mut keys: Vec<u32> = (0..n as u32).map(|i| i * 3).collect();
            let mut rng = Rng(n as u64 * 7 + order);
            match order {
                0 => {}
                1 => keys.reverse(),
                _ => {
                    for i in (1..keys.len()).rev() {
                        keys.swap(i, rng.below(i as u64 + 1) as usize);
                    }
                }
            }
            for &k in &keys {
                assert_eq!(m.try_insert(k, k + 1).unwrap(), None);
            }
            assert_eq!(m.len(), n);
            assert!(m.keys().copied().eq((0..n as u32).map(|i| i * 3)));
            let mut gone = keys.clone();
            match order {
                0 => gone.sort(),
                1 => gone.sort_by(|a, b| b.cmp(a)),
                _ => {}
            }
            for (j, &k) in gone.iter().enumerate() {
                assert_eq!(m.remove(&k), Some(k + 1));
                assert_eq!(m.remove(&k), None);
                assert_eq!(m.len(), n - j - 1);
                if j % 97 == 0 {
                    assert!(m.keys().is_sorted());
                    if m.len() >= 16 {
                        assert!(b.used() <= bound(m.len(), m.node_count(), 8), "bound while removing, n={n} order={order}");
                    }
                }
            }
            assert!(m.is_empty());
            assert_eq!((b.used(), m.node_count()), (0, 0), "n={n} order={order}");
        }
    }
}

/// A refusal in the middle of an insertion (a split that cannot allocate) leaves the map as it was.
fn atomicity(_base: u64) {
    let mut rng = Rng(99);
    let probe = root(12, BIG);
    let mut full: KMap<u64, u64> = KMap::new_in(&probe);
    let keys: Vec<u64> = (0..3000).map(|_| rng.next()).collect();
    for &k in &keys {
        full.try_insert(k, k).unwrap();
    }
    let total = probe.used();
    drop(full);
    for cap in [total / 3, total / 2, total - 1, total] {
        let b = root(12, cap);
        let mut m: KMap<u64, u64> = KMap::new_in(&b);
        let mut refused = 0;
        let mut kept = BTreeMap::new();
        for &k in &keys {
            let before = (m.len(), b.used(), m.node_count());
            match m.try_insert(k, k ^ 1) {
                Ok(old) => assert_eq!(old, kept.insert(k, k ^ 1)),
                Err(e) => {
                    refused += 1;
                    assert_eq!(e, MemError::Limit(LimitId(12)));
                    assert_eq!(before, (m.len(), b.used(), m.node_count()), "unchanged on refusal");
                    assert!(!m.contains_key(&k));
                }
            }
        }
        assert_eq!(refused > 0, cap < total);
        assert!(m.iter().map(|(k, v)| (*k, *v)).eq(kept.iter().map(|(k, v)| (*k, *v))));
        // Replacing and removing still work when the budget is exhausted.
        if let Some((&k, _)) = kept.iter().next() {
            assert!(m.try_insert(k, 5).unwrap().is_some());
            assert_eq!(m.remove(&k), Some(5));
        }
    }
}

/// Keys that own memory are dropped exactly once; `TryClone`, `Debug`, borrowed lookups.
fn ownership(_base: u64) {
    let b = root(13, BIG);
    let mut m: KMap<KString, KBytes> = KMap::new_in(&b);
    for i in 0..500u32 {
        let k = KString::try_from_str_in(&format!("key{i:04}"), &b).unwrap();
        let mut v = KBytes::new_in(&b);
        v.try_extend_from_slice(&i.to_le_bytes()).unwrap();
        m.try_insert(k, v).unwrap();
    }
    assert_eq!(m.get("key0042").map(|v| v.as_slice()), Some(&42u32.to_le_bytes()[..]));
    assert!(m.contains_key("key0499") && !m.contains_key("key0500"));
    let copy = m.try_clone().unwrap();
    assert_eq!(copy.len(), 500);
    assert!(copy.keys().map(|k| k.as_str()).eq(m.keys().map(|k| k.as_str())));
    assert!(format!("{:?}", copy).starts_with("{\"key0000\": [0, 0, 0, 0]"));
    for i in 0..500u32 {
        assert!(m.remove(format!("key{i:04}").as_str()).is_some());
    }
    drop(m);
    drop(copy);
    assert_eq!(b.used(), 0);
    // Zero-sized keys and values.
    let z = root(14, BIG);
    let mut zm: KMap<(), ()> = KMap::new_in(&z);
    assert_eq!(zm.try_insert((), ()).unwrap(), None);
    assert_eq!(zm.try_insert((), ()).unwrap(), Some(()));
    assert_eq!(zm.len(), 1);
}

/// Count keys and values without the padding that a tuple would add to every slot.
fn padded_pair_bound() {
    let b = root(15, BIG);
    let mut m = KMap::<u64, u8>::new_in(&b);
    for k in 0..1_000_000 {
        m.try_insert(k, k as u8).unwrap();
    }
    let limit = bound(m.len(), m.node_count(), size_of::<u64>() + size_of::<u8>());
    assert!(b.used() <= limit, "{} bytes charged, bound {limit}", b.used());
    assert!(m.iter().enumerate().all(|(i, (&k, &v))| k == i as u64 && v == i as u8));
    m.clear();
    assert_eq!(b.used(), 0);
}

fn clear_after_panicking_drop() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    struct Value(bool);

    impl Drop for Value {
        fn drop(&mut self) {
            assert!(!self.0, "entry destructor panic");
        }
    }

    let b = root(16, BIG);
    let mut m = KMap::new_in(&b);
    for k in 0..17 {
        assert!(m.try_insert(k, Value(true)).unwrap().is_none());
    }
    assert!(m.node_count() > 1, "exercise a tree with an internal root");

    assert!(catch_unwind(AssertUnwindSafe(|| m.clear())).is_err());
    assert_eq!((m.len(), m.node_count()), (0, 0));
    assert!(m.try_insert(42, Value(false)).unwrap().is_none());
    assert!(m.keys().copied().eq([42]));
    assert!(m.remove(&42).is_some());
    assert!(m.is_empty());
}

#[test]
fn kmap() {
    let base = start();
    model(base);
    structure(base);
    atomicity(base);
    ownership(base);
    padded_pair_bound();
    assert_eq!(usage().used, base, "no leak");
    // Run last because the interrupted destructor leaves the old tree allocated.
    clear_after_panicking_drop();
}
