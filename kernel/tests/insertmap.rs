//! Gate 11 of lot L1a-2 (functional part): `KInsertMap` against an insertion-ordered reference model.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;

/// Reference: a vector of pairs in insertion order.
#[derive(Default)]
struct Model(Vec<(u64, u64)>);

impl Model {
    fn insert(&mut self, k: u64, v: u64) -> Option<u64> {
        match self.0.iter_mut().find(|e| e.0 == k) {
            Some(e) => Some(std::mem::replace(&mut e.1, v)),
            None => {
                self.0.push((k, v));
                None
            }
        }
    }
    fn remove(&mut self, k: u64) -> Option<u64> {
        let i = self.0.iter().position(|e| e.0 == k)?;
        Some(self.0.remove(i).1)
    }
}

fn model(_base: u64) {
    let b = root(20, BIG);
    let mut m: KInsertMap<u64, u64> = KInsertMap::new_in(&b);
    let mut r = Model::default();
    let mut rng = Rng(0xABCD);
    for step in 0..200_000u64 {
        let key = rng.below(400);
        match rng.below(5) {
            0..=1 => assert_eq!(m.try_insert(key, step).unwrap(), r.insert(key, step)),
            2 => assert_eq!(m.get(&key).copied(), r.0.iter().find(|e| e.0 == key).map(|e| e.1)),
            3 => assert_eq!(m.contains_key(&key), r.0.iter().any(|e| e.0 == key)),
            _ => assert_eq!(m.remove(&key), r.remove(key)),
        }
        assert_eq!(m.len(), r.0.len());
        if step % 500 == 0 {
            assert!(m.iter().map(|(k, v)| (*k, *v)).eq(r.0.iter().copied()), "insertion order at {step}");
            assert!(m.keys().copied().eq(r.0.iter().map(|e| e.0)));
            assert!(m.values().copied().eq(r.0.iter().map(|e| e.1)));
        }
    }
    if let Some(v) = m.get_mut(&r.0[0].0) {
        *v = 7;
    }
    for (_, v) in m.iter_mut() {
        *v += 1;
    }
    assert_eq!(m.values().next(), Some(&8));
    let copy = m.try_clone().unwrap();
    assert!(copy.iter().map(|(k, v)| (*k, *v)).eq(m.iter().map(|(k, v)| (*k, *v))), "a clone keeps the order");
    drop(copy);
    m.clear();
    assert_eq!((m.len(), b.used()), (0, 0));
    drop(m);
}

/// Overwriting keeps the position; removal closes the gap; a removed key comes back at the end.
fn positions(_base: u64) {
    let b = root(21, BIG);
    let mut m: KInsertMap<KString, u32> = KInsertMap::new_in(&b);
    let k = |s: &str| KString::try_from_str_in(s, &b).unwrap();
    for (i, name) in ["c", "a", "b", "d"].iter().enumerate() {
        assert_eq!(m.try_insert(k(name), i as u32).unwrap(), None);
    }
    assert_eq!(m.try_insert(k("a"), 10).unwrap(), Some(1));
    assert_eq!(m.keys().map(|s| s.as_str()).collect::<Vec<_>>(), ["c", "a", "b", "d"]);
    assert_eq!(m.remove("a"), Some(10));
    assert_eq!(m.keys().map(|s| s.as_str()).collect::<Vec<_>>(), ["c", "b", "d"]);
    m.try_insert(k("a"), 11).unwrap();
    assert_eq!(m.keys().map(|s| s.as_str()).collect::<Vec<_>>(), ["c", "b", "d", "a"]);
    assert_eq!(m.get("b"), Some(&2));
    assert!(format!("{m:?}").starts_with("{\"c\": 0"));
    drop(m);
    assert_eq!(b.used(), 0);
}

/// A refusal leaves the map unchanged, wherever it strikes.
fn atomicity(_base: u64) {
    let probe = root(22, BIG);
    let mut full: KInsertMap<u64, u64> = KInsertMap::new_in(&probe);
    let mut rng = Rng(5);
    let keys: Vec<u64> = (0..1500).map(|_| rng.next()).collect();
    for &k in &keys {
        full.try_insert(k, k).unwrap();
    }
    let total = probe.used();
    drop(full);
    for cap in [total / 4, total / 2, total - 1] {
        let b = root(22, cap);
        let mut m: KInsertMap<u64, u64> = KInsertMap::new_in(&b);
        let mut kept: Vec<u64> = vec![];
        let mut refused = 0;
        for &k in &keys {
            let before = (m.len(), b.used(), m.node_count());
            match m.try_insert(k, k) {
                Ok(_) => kept.push(k),
                Err(e) => {
                    refused += 1;
                    assert_eq!(e, MemError::Limit(LimitId(22)));
                    assert_eq!(before, (m.len(), b.used(), m.node_count()));
                    assert!(!m.contains_key(&k));
                }
            }
        }
        assert!(refused > 0);
        assert!(m.keys().copied().eq(kept.iter().copied()));
    }
}

/// Memory stays within twice what the live entries need, also after removing 90% of them.
fn memory_bound(n: usize) {
    let b = root(23, BIG);
    let mut m: KInsertMap<u64, u64> = KInsertMap::new_in(&b);
    let mut rng = Rng(77);
    let mut keys: Vec<u64> = (0..n).map(|_| rng.next()).collect();
    for &k in &keys {
        m.try_insert(k, k).unwrap();
    }
    assert_eq!(m.len(), n);
    for i in (1..keys.len()).rev() {
        keys.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let cut = keys.len() * 9 / 10;
    for k in &keys[..cut] {
        assert!(m.remove(k).is_some());
    }
    assert_eq!(m.len(), n - cut);
    let fresh_budget = root(23, BIG);
    let mut fresh = KInsertMap::new_in(&fresh_budget);
    for (&k, &v) in m.iter() {
        fresh.try_insert(k, v).unwrap();
    }
    assert_eq!(fresh.len(), m.len());
    assert!(
        b.used() <= 2 * fresh_budget.used(),
        "{} bytes for {} survivors after removing 90%, versus {} bytes in a fresh map",
        b.used(),
        m.len(),
        fresh_budget.used()
    );
}

#[test]
fn insertmap() {
    let base = start();
    model(base);
    positions(base);
    atomicity(base);
    memory_bound(50_000);
    assert_eq!(usage().used, base, "no leak");
}
