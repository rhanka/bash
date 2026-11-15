//! Gates 2, 3, 6 and 7: budget exactness, nested budgets, overflow, value preservation.
mod common;
use common::*;
use sentropic_bash_kernel::mem::*;
use std::any::Any;

const NS: [usize; 3] = [1, 4, 1000];
const ID: u16 = 41;

/// What an operation hands back: the live containers on success, the error on failure.
type Outcome = Result<Box<dyn Any>, MemError>;

/// Runs `op` on a budget of exactly `need` bytes (must succeed, charging exactly `need`, then release
/// everything on drop) and on one of `need - 1` bytes (must fail with that budget's limit). `op` itself
/// asserts that a failed step left its container unchanged.
fn exact(what: &str, need: u64, op: impl Fn(&Budget) -> Outcome) {
    let ok = root(ID, need);
    let held = op(&ok).unwrap_or_else(|e| panic!("{what}: cap = need must succeed, got {e:?}"));
    assert_eq!(ok.used(), need, "{what}: exactly the needed bytes are charged");
    drop(held);
    assert_eq!(ok.used(), 0, "{what}: released on drop");
    let tight = root(ID, need - 1);
    match op(&tight) {
        Err(e) => assert_eq!(e, MemError::Limit(LimitId(ID)), "{what}: wrong error"),
        Ok(_) => panic!("{what}: cap = need - 1 must fail"),
    }
    assert!(tight.used() <= need - 1, "{what}");
}

fn rc_size() -> u64 {
    let probe = root(98, BIG);
    let _x = KRc::try_new_in([0u8; 64], &probe).unwrap();
    probe.used()
}

#[test]
fn budgets() {
    let base = start();
    let rc = rc_size();
    assert!(rc >= 64 + core::mem::size_of::<usize>() as u64);

    for n in NS {
        let nb = n as u64;
        // KVec<u8> and KVec<u64>: n pushes (amortized growth falls back to the exact need), reserve_exact,
        // with_capacity_in, extend_from_slice.
        exact("KVec<u8> push", nb, |b| {
            let mut v: KVec<u8> = KVec::new_in(b);
            for i in 0..n {
                let snap = (v.len(), v.capacity(), b.used());
                if let Err(e) = v.try_push(i as u8) {
                    assert_eq!(snap, (v.len(), v.capacity(), b.used()), "unchanged on failure");
                    return Err(e);
                }
            }
            Ok(Box::new(v))
        });
        exact("KVec<u64> push", nb * 8, |b| {
            let mut v: KVec<u64> = KVec::new_in(b);
            for i in 0..n {
                let snap = (v.len(), v.capacity(), b.used());
                if let Err(e) = v.try_push(i as u64) {
                    assert_eq!(snap, (v.len(), v.capacity(), b.used()));
                    return Err(e);
                }
            }
            Ok(Box::new(v))
        });
        exact("KVec<u8> reserve_exact", nb, |b| {
            let mut v: KVec<u8> = KVec::new_in(b);
            let snap = (v.len(), v.capacity(), b.used());
            match v.try_reserve_exact(n) {
                Ok(()) => {
                    assert_eq!(v.capacity(), n);
                    Ok(Box::new(v))
                }
                Err(e) => {
                    assert_eq!(snap, (v.len(), v.capacity(), b.used()));
                    Err(e)
                }
            }
        });
        exact("KVec<u64> with_capacity_in", nb * 8, |b| KVec::<u64>::with_capacity_in(n, b).map(|v| Box::new(v) as Box<dyn Any>));
        exact("KVec<u8> extend_from_slice", nb, |b| {
            let data = vec![7u8; n];
            let mut v: KVec<u8> = KVec::new_in(b);
            let snap = (v.len(), v.capacity(), b.used());
            match v.try_extend_from_slice(&data) {
                Ok(()) => Ok(Box::new(v)),
                Err(e) => {
                    assert_eq!(snap, (v.len(), v.capacity(), b.used()));
                    Err(e)
                }
            }
        });
        exact("KVec<u8> resize", nb, |b| {
            let mut v: KVec<u8> = KVec::new_in(b);
            v.try_resize(n, 3).map(|()| Box::new(v) as Box<dyn Any>)
        });

        // KString.
        exact("KString push_str", nb, |b| {
            let text = "a".repeat(n);
            let mut s = KString::new_in(b);
            let snap = (s.len(), b.used());
            match s.try_push_str(&text) {
                Ok(()) => Ok(Box::new(s)),
                Err(e) => {
                    assert_eq!(snap, (s.len(), b.used()));
                    Err(e)
                }
            }
        });
        exact("KString push chars", nb, |b| {
            let mut s = KString::new_in(b);
            for _ in 0..n {
                let snap = (s.len(), b.used());
                if let Err(e) = s.try_push('x') {
                    assert_eq!(snap, (s.len(), b.used()));
                    return Err(e);
                }
            }
            Ok(Box::new(s))
        });
        for len in 1..=3usize {
            let tiny = root(96, len as u64);
            let s = KString::try_from_str_in(&"q".repeat(len), &tiny).expect("a short text fits exactly its length");
            assert_eq!(tiny.used(), len as u64);
            drop(s);
        }
        exact("KString from_str", nb, |b| KString::try_from_str_in(&"z".repeat(n), b).map(|s| Box::new(s) as Box<dyn Any>));

        // KBox<[u8; 64]> and KRc<[u8; 64]>: n of them.
        exact("KBox<[u8;64]>", nb * 64, |b| {
            let holder = root(99, BIG);
            let mut kept: KVec<KBox<[u8; 64]>> = KVec::new_in(&holder);
            for i in 0..n {
                match KBox::try_new_in([i as u8; 64], b) {
                    Ok(x) => kept.try_push(x).unwrap(),
                    Err((v, e)) => {
                        assert_eq!(v, [i as u8; 64], "the value comes back");
                        return Err(e);
                    }
                }
            }
            Ok(Box::new(kept))
        });
        exact("KRc<[u8;64]>", nb * rc, |b| {
            let holder = root(99, BIG);
            let mut kept: KVec<KRc<[u8; 64]>> = KVec::new_in(&holder);
            for i in 0..n {
                match KRc::try_new_in([i as u8; 64], b) {
                    Ok(x) => kept.try_push(x).unwrap(),
                    Err((v, e)) => {
                        assert_eq!(v, [i as u8; 64], "the value comes back");
                        return Err(e);
                    }
                }
            }
            Ok(Box::new(kept))
        });

        // KMap<u32, u32>: the exact need is probed (entry overhead depends on the map layout), and is at
        // least the payload.
        let fill = |cap: u64| -> bool {
            let p = root(97, cap);
            let mut m: KMap<u32, u32> = KMap::new_in(&p);
            (0..n as u32).all(|i| m.try_insert(i.wrapping_mul(2654435761), i).is_ok())
        };
        let (mut lo, mut hi) = (0u64, BIG);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if fill(mid) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let map_need = lo;
        assert!(map_need >= nb * 8, "an entry holds at least its key and value");
        exact("KMap<u32,u32>", map_need, |b| {
            let mut m: KMap<u32, u32> = KMap::new_in(b);
            for i in 0..n as u32 {
                let snap = (m.len(), b.used());
                if let Err(e) = m.try_insert(i.wrapping_mul(2654435761), i) {
                    assert_eq!(snap, (m.len(), b.used()));
                    assert!(m.get(&i.wrapping_mul(2654435761)).is_none());
                    return Err(e);
                }
            }
            Ok(Box::new(m))
        });
    }
    assert_eq!(usage().used, base, "no leak");

    nested();
    overflow();
    preservation();
    assert_eq!(usage().used, base, "no leak");
}

/// Gate 3.
fn nested() {
    let parent = root(10, 1000);
    let child = parent.child(LimitId(11), 600).unwrap();
    assert_eq!((child.limit(), parent.limit()), (LimitId(11), LimitId(10)));
    // A successful charge appears in both.
    child.charge(400).unwrap();
    assert_eq!((child.used(), parent.used()), (400, 400));
    // Refused by the child: the child's id.
    assert_eq!(child.charge(201), Err(MemError::Limit(LimitId(11))));
    // Refused by the parent: the parent's id, child and parent unchanged.
    parent.charge(500).unwrap();
    assert_eq!((child.used(), parent.used()), (400, 900));
    assert_eq!(child.charge(150), Err(MemError::Limit(LimitId(10))));
    assert_eq!((child.used(), parent.used()), (400, 900));
    // Release restores both.
    child.release(400);
    assert_eq!((child.used(), parent.used()), (0, 500));
    parent.release(500);
    assert_eq!(parent.used(), 0);
    // A grandchild charges the whole chain; the nearest refusing ancestor reports.
    let grand = child.child(LimitId(12), 1000).unwrap();
    assert_eq!(grand.charge(700), Err(MemError::Limit(LimitId(11))));
    grand.charge(600).unwrap();
    assert_eq!((grand.used(), child.used(), parent.used()), (600, 600, 600));
    drop(child);
    // The node lives while a descendant refers to it.
    assert_eq!(grand.charge(1), Err(MemError::Limit(LimitId(11))));
    grand.release(600);
    assert_eq!(parent.used(), 0);
    // set_cap below used: allowed, later charges fail.
    grand.charge(10).unwrap();
    grand.set_cap(5);
    assert_eq!(grand.cap(), 5);
    assert_eq!(grand.charge(1), Err(MemError::Limit(LimitId(12))));
    grand.release(10);
    grand.charge(5).unwrap();
    grand.release(5);
    // A container on a child budget charges the chain.
    let c2 = parent.child(LimitId(13), 100).unwrap();
    let mut v: KVec<u8> = KVec::new_in(&c2);
    v.try_reserve_exact(64).unwrap();
    assert_eq!((c2.used(), parent.used()), (64, 64));
    assert_eq!(v.try_reserve_exact(101), Err(MemError::Limit(LimitId(13))));
    drop(v);
    assert_eq!((c2.used(), parent.used()), (0, 0));
}

/// Gate 6.
fn overflow() {
    let b = root(ID, u64::MAX);
    let before = usage().used;
    assert!(matches!(KVec::<[u8; 16]>::with_capacity_in(usize::MAX, &b), Err(MemError::Overflow)));
    assert!(matches!(KVec::<u8>::with_capacity_in(usize::MAX, &b), Err(MemError::Overflow)));
    assert!(matches!(KVec::<u64>::with_capacity_in(usize::MAX / 2, &b), Err(MemError::Overflow)));
    let mut v: KVec<u64> = KVec::new_in(&b);
    v.try_push(1).unwrap();
    let cap = v.capacity();
    let (used, usage_before) = (b.used(), usage().used);
    assert_eq!(v.try_reserve(usize::MAX), Err(MemError::Overflow), "len + additional overflows");
    assert_eq!(v.try_reserve_exact(usize::MAX), Err(MemError::Overflow));
    assert_eq!(v.try_reserve(usize::MAX / 8 + 1), Err(MemError::Overflow), "bytes overflow");
    assert_eq!(v.try_reserve(usize::MAX / 8), Err(MemError::Overflow), "layout above isize::MAX");
    assert_eq!((v.len(), v.capacity(), b.used(), usage().used), (1, cap, used, usage_before), "nothing allocated or charged");
    let mut s = KString::new_in(&b);
    assert_eq!(s.try_push_str("ab"), Ok(()));
    drop(v);
    drop(s);
    assert_eq!(usage().used, before);
}

/// Gate 7 (the containers that hand a value back or drop it).
fn preservation() {
    use std::rc::Rc;
    let b = root(ID, 40);
    // KBox / KRc: the original value comes back, with a non-allocating error.
    let tracker = Rc::new(());
    let r = KBox::try_new_in((tracker.clone(), [0u8; 64]), &b);
    let (value, e) = r.err().expect("refused");
    assert_eq!(e, MemError::Limit(LimitId(ID)));
    assert_eq!(Rc::strong_count(&tracker), 2, "returned, not dropped");
    drop(value);
    let (value, _) = KRc::try_new_in((tracker.clone(), [0u8; 64]), &b).err().expect("refused");
    drop(value);
    assert_eq!(Rc::strong_count(&tracker), 1);
    // try_push: the value is dropped, the vector is unchanged.
    let b8 = root(ID, 8);
    let mut v: KVec<(Rc<()>, u32)> = KVec::new_in(&b8);
    assert_eq!(v.try_push((tracker.clone(), 1)), Err(MemError::Limit(LimitId(ID))));
    assert_eq!((v.len(), v.capacity(), b8.used()), (0, 0, 0));
    assert_eq!(Rc::strong_count(&tracker), 1, "dropped");
    // try_insert on a map: unchanged, key and value dropped; replacing never allocates.
    let b16 = root(ID, BIG);
    let mut m: KMap<u32, Rc<()>> = KMap::new_in(&b16);
    assert_eq!(m.try_insert(1, tracker.clone()).map(|o| o.is_none()), Ok(true));
    let tight = root(ID, b16.used());
    drop(m);
    let mut m: KMap<u32, Rc<()>> = KMap::new_in(&tight);
    assert!(m.try_insert(1, tracker.clone()).is_ok());
    let used = tight.used();
    let old = m.try_insert(1, tracker.clone()).unwrap();
    assert!(old.is_some());
    assert_eq!(tight.used(), used, "replacement does not allocate");
    drop(old);
    for k in 2..40 {
        if m.try_insert(k, tracker.clone()).is_err() {
            break;
        }
    }
    let (len, used) = (m.len(), tight.used());
    assert_eq!(m.try_insert(1000, tracker.clone()).map(|_| ()), Err(MemError::Limit(LimitId(ID))));
    assert_eq!((m.len(), tight.used()), (len, used));
    assert!(!m.contains_key(&1000));
    assert_eq!(Rc::strong_count(&tracker), 1 + len, "the refused pair was dropped");
    drop(m);
    assert_eq!(Rc::strong_count(&tracker), 1);
}
