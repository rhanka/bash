//! Gates 8 and 9: container semantics against reference models, `kbox_unsize!`, formatting, TryClone.
mod common;
use common::*;
use sentropic_bash_kernel::kbox_unsize;
use sentropic_bash_kernel::mem::*;
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[test]
fn semantics() {
    let base = start();
    map_matches_a_model();
    rc_semantics();
    unsize();
    formatting();
    vec_semantics();
    try_clone();
    assert_eq!(usage().used, base, "no leak");
    panics();
}

/// Last: the first panic allocates lazily inside the standard library, which is not a leak of ours.
fn panics() {
    let b = root(78, BIG);
    let mut v = KString::try_from_str_in("\u{e9}", &b).unwrap();
    let r = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| v.truncate(1)));
    assert!(r.is_err(), "truncate off a char boundary panics");
    let mut w: KVec<u8> = KVec::new_in(&b);
    let r = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
        let _ = w.try_insert(5, 0);
    }));
    assert!(r.is_err(), "insert out of bounds panics");
    let r = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| w.remove(0)));
    assert!(r.is_err(), "remove out of bounds panics");
    drop((v, w));
    assert_eq!(b.used(), 0);
}

fn map_matches_a_model() {
    let b = root(71, BIG);
    let mut map: KMap<u32, u32> = KMap::new_in(&b);
    let mut model: BTreeMap<u32, u32> = BTreeMap::new();
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    for i in 0..4000u32 {
        let key = rng.below(700) as u32;
        match rng.below(5) {
            0..=2 => assert_eq!(map.try_insert(key, i).unwrap(), model.insert(key, i)),
            3 => assert_eq!(map.remove(&key), model.remove(&key)),
            _ => {
                assert_eq!(map.get(&key), model.get(&key));
                assert_eq!(map.contains_key(&key), model.contains_key(&key));
                if let Some(v) = map.get_mut(&key) {
                    *v += 1;
                    *model.get_mut(&key).unwrap() += 1;
                }
            }
        }
        assert_eq!(map.len(), model.len());
    }
    assert!(map.len() > 100);
    assert!(map.iter().map(|(k, v)| (*k, *v)).eq(model.iter().map(|(k, v)| (*k, *v))), "same pairs, ascending");
    assert!(map.keys().is_sorted());
    assert!(map.keys().copied().eq(model.keys().copied()));
    assert!(map.values().copied().eq(model.values().copied()));
    for (_, v) in map.iter_mut() {
        *v = v.wrapping_add(1);
    }
    for v in model.values_mut() {
        *v = v.wrapping_add(1);
    }
    assert!(map.iter().map(|(k, v)| (*k, *v)).eq(model.iter().map(|(k, v)| (*k, *v))));
    assert_eq!(map.first_key_value().map(|(k, v)| (*k, *v)), model.iter().next().map(|(k, v)| (*k, *v)));
    assert_eq!(map.last_key_value().map(|(k, v)| (*k, *v)), model.iter().next_back().map(|(k, v)| (*k, *v)));
    map.clear();
    assert!(map.is_empty() && map.first_key_value().is_none() && map.last_key_value().is_none());
    drop(map);
    assert_eq!(b.used(), 0);

    // Text keys are looked up by &str without allocating.
    let mut names: KMap<KString, u32> = KMap::new_in(&b);
    for (i, n) in ["pear", "apple", "fig", "banana"].iter().enumerate() {
        names.try_insert(KString::try_from_str_in(n, &b).unwrap(), i as u32).unwrap();
    }
    let used = usage().used;
    assert_eq!(names.get("fig"), Some(&2));
    assert!(!names.contains_key("kiwi"));
    assert_eq!(usage().used, used);
    assert!(names.keys().map(|k| k.as_str()).eq(["apple", "banana", "fig", "pear"]));
    // Byte keys by slice.
    let mut by_bytes: KMap<KBytes, u8> = KMap::new_in(&b);
    let mut k = KBytes::new_in(&b);
    k.try_extend_from_slice(b"key").unwrap();
    by_bytes.try_insert(k, 9).unwrap();
    assert_eq!(by_bytes.get(&b"key"[..]), Some(&9));
}

fn rc_semantics() {
    let b = root(72, BIG);
    let mut a = KRc::try_new_in(String::from("shared"), &b).unwrap();
    assert_eq!(KRc::strong_count(&a), 1);
    assert!(KRc::get_mut(&mut a).is_some());
    KRc::get_mut(&mut a).unwrap().push('!');
    let used = b.used();
    let c = a.clone();
    assert_eq!(b.used(), used, "a clone allocates nothing");
    assert_eq!((KRc::strong_count(&a), KRc::strong_count(&c)), (2, 2));
    assert!(KRc::ptr_eq(&a, &c));
    assert!(KRc::get_mut(&mut a).is_none());
    assert_eq!(&**c, "shared!");
    let other = KRc::try_new_in(String::from("shared!"), &b).unwrap();
    assert!(!KRc::ptr_eq(&a, &other));
    let a = match KRc::try_unwrap(a) {
        Err(back) => back,
        Ok(_) => panic!("not unique"),
    };
    drop(c);
    assert_eq!(KRc::strong_count(&a), 1);
    assert_eq!(KRc::try_unwrap(a).ok().as_deref(), Some("shared!"));
    drop(other);
    assert_eq!(b.used(), 0, "released when the last handle goes");
    let t = a_try_clone_rc(&b);
    drop(t);
    assert_eq!(b.used(), 0);
}

fn a_try_clone_rc(b: &Budget) -> (KRc<u32>, KRc<u32>) {
    let x = KRc::try_new_in(5u32, b).unwrap();
    let y = x.try_clone().unwrap();
    assert!(KRc::ptr_eq(&x, &y));
    (x, y)
}

static DROPS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct Counted(u32);
impl Drop for Counted {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Relaxed);
    }
}

trait Command {
    fn run(&self) -> u32;
}
impl Command for Counted {
    fn run(&self) -> u32 {
        self.0 * 2
    }
}

fn unsize() {
    let b = root(73, BIG);
    DROPS.store(0, Relaxed);
    let boxed = KBox::try_new_in(Counted(21), &b).unwrap();
    let charged = b.used();
    assert_eq!(charged, core::mem::size_of::<Counted>() as u64);
    let dynamic: KBox<dyn Command> = kbox_unsize!(boxed, dyn Command);
    assert_eq!(b.used(), charged, "unsizing allocates and charges nothing");
    assert_eq!(dynamic.run(), 42);
    drop(dynamic);
    assert_eq!(DROPS.load(Relaxed), 1, "the concrete destructor ran exactly once");
    assert_eq!(b.used(), 0, "the budget is released");
    // Arrays become slices.
    let arr = KBox::try_new_in([1u8, 2, 3, 4], &b).unwrap();
    let slice: KBox<[u8]> = kbox_unsize!(arr, [u8]);
    assert_eq!((&*slice, b.used()), (&[1u8, 2, 3, 4][..], 4));
    drop(slice);
    assert_eq!(b.used(), 0);
    // into_inner and into_raw/from_raw round trips.
    let x = KBox::try_new_in(Counted(1), &b).unwrap();
    let inner = x.into_inner();
    assert_eq!((inner.0, b.used()), (1, 0));
    drop(inner);
    assert_eq!(DROPS.load(Relaxed), 2);
    let y = KBox::try_new_in(7u64, &b).unwrap();
    let (raw, bud) = y.into_raw();
    // SAFETY: `raw` and `bud` come from `into_raw` just above, unchanged.
    let y = unsafe { KBox::from_raw(raw, bud) };
    assert_eq!((*y, b.used()), (7, 8));
    drop(y);
    // Zero-sized values charge nothing.
    let z = KBox::try_new_in((), &b).unwrap();
    assert_eq!(b.used(), 0);
    z.into_inner();
}

fn formatting() {
    let big = root(74, BIG);
    let mut s = KString::new_in(&big);
    let (n, w, k) = (std::hint::black_box(12345), std::hint::black_box("abc"), std::hint::black_box(7));
    write!(s, "{}-{}:{:>4}|{:?}", n, w, k, "q").unwrap();
    assert_eq!(s.as_str(), "12345-abc:   7|\"q\"");
    assert_eq!(s.take_fmt_error(), None);
    assert_eq!(format!("{s}"), s.as_str());

    let small = root(74, 10);
    let mut t = KString::new_in(&small);
    // Runtime values: the compiler would fold literal arguments into one string piece.
    let (n, word) = (std::hint::black_box(12345), std::hint::black_box("abcdefgh"));
    let r = write!(t, "{}-{}", n, word);
    assert!(r.is_err(), "fmt::Error on refusal");
    assert_eq!(t.take_fmt_error(), Some(MemError::Limit(LimitId(74))));
    assert_eq!(t.take_fmt_error(), None, "taking clears it");
    assert!(t.as_str().starts_with("12345-") && t.len() <= 10, "only what fitted: {:?}", t.as_str());
    assert_eq!(t.as_str(), "12345-");
    assert!(small.used() >= t.len() as u64 && small.used() <= 10);

    // Text operations.
    let mut u = KString::try_from_str_in("h\u{e9}llo", &big).unwrap();
    assert_eq!((u.len(), u.chars().count()), (6, 5));
    u.try_push('\u{4e2d}').unwrap();
    u.truncate(6);
    assert_eq!(u.as_str(), "h\u{e9}llo");
    assert!(KString::try_from_str_in("a", &big).unwrap() < KString::try_from_str_in("b", &big).unwrap());
    let bytes = u.into_bytes();
    assert_eq!(bytes.as_slice(), "h\u{e9}llo".as_bytes());
}

fn vec_semantics() {
    let b = root(75, BIG);
    let mut v: KVec<u32> = KVec::new_in(&b);
    assert_eq!((v.capacity(), b.used()), (0, 0), "new_in allocates nothing");
    v.try_push(1).unwrap();
    assert_eq!(v.capacity(), 4, "minimum growth is 4 elements");
    for i in 2..=5 {
        v.try_push(i).unwrap();
    }
    assert_eq!(v.capacity(), 6, "1.5 x 4");
    for i in 6..=7 {
        v.try_push(i).unwrap();
    }
    assert_eq!(v.capacity(), 9, "1.5 x 6");
    assert_eq!(b.used(), 36);
    v.try_insert(0, 100).unwrap();
    assert_eq!(v.as_slice(), &[100, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!((v.remove(0), v.swap_remove(0), v.pop()), (100, 1, Some(6)));
    assert_eq!(v.as_slice(), &[7, 2, 3, 4, 5]);
    v.shrink_to_fit();
    assert_eq!((v.capacity(), b.used()), (5, 20));
    v.clear();
    v.shrink_to_fit();
    assert_eq!((v.capacity(), b.used()), (0, 0));
    for x in &mut v {
        *x += 1;
    }
    // reserve(n) on a fresh vector takes max(n, 4) elements; try_reserve_exact takes n.
    v.try_reserve(2).unwrap();
    assert_eq!(v.capacity(), 4);
    v.try_reserve_exact(10).unwrap();
    assert_eq!(v.capacity(), 10);
    // Zero-sized elements charge nothing and never fail.
    let mut z: KVec<()> = KVec::new_in(&root(76, 0));
    for _ in 0..1000 {
        z.try_push(()).unwrap();
    }
    assert_eq!((z.len(), z.budget().used()), (1000, 0));
    // try_clone charges the same budget as the source.
    let mut src: KVec<u32> = KVec::new_in(&b);
    src.try_extend_from_slice(&[1, 2, 3]).unwrap();
    let before = b.used();
    let copy = src.try_clone().unwrap();
    assert_eq!(copy.as_slice(), src.as_slice());
    assert_eq!(b.used(), before + 12);
}

fn try_clone() {
    let b = root(77, BIG);
    assert_eq!((3u8, 2.5f64, true, 'x').try_clone().unwrap(), (3u8, 2.5f64, true, 'x'));
    assert_eq!(Some(4u16).try_clone().unwrap(), Some(4));
    let s = KString::try_from_str_in("clone me", &b).unwrap();
    let t = s.try_clone().unwrap();
    assert_eq!((s.as_str(), t.as_str(), b.used()), ("clone me", "clone me", 16));
    let mut m: KMap<u32, KString> = KMap::new_in(&b);
    m.try_insert(2, s).unwrap();
    m.try_insert(1, t).unwrap();
    let m2 = m.try_clone().unwrap();
    assert!(m.iter().map(|(k, v)| (*k, v.as_str())).eq(m2.iter().map(|(k, v)| (*k, v.as_str()))));
    let bx = KBox::try_new_in(KString::try_from_str_in("in a box", &b).unwrap(), &b).unwrap();
    let bx2 = bx.try_clone().unwrap();
    assert_eq!(bx2.as_str(), "in a box");
    drop((m, m2, bx, bx2));
    assert_eq!(b.used(), 0);
    // A clone refused by the budget leaves nothing behind.
    let tight = root(77, 25);
    let src = KString::try_from_str_in("0123456789", &tight).unwrap();
    let first = src.try_clone().unwrap();
    assert_eq!(tight.used(), 20);
    assert_eq!(src.try_clone().err(), Some(MemError::Limit(LimitId(77))));
    assert_eq!(tight.used(), 20, "a refused clone leaves nothing charged");
    drop((src, first));
    assert_eq!(tight.used(), 0);
}
