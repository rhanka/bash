//! Gate 1: names every public item of spec 5.1-5.8 with its full signature. A signature change breaks
//! compilation. Items that return `impl Iterator` are called and their item types pinned.
use sentropic_bash_kernel::mem::*;
use std::fmt::{Debug, Display, Write};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

trait AmbiguousIfSend<A> {
    fn probe() {}
}
impl<T: ?Sized> AmbiguousIfSend<()> for T {}
impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
trait AmbiguousIfSync<A> {
    fn probe() {}
}
impl<T: ?Sized> AmbiguousIfSync<()> for T {}
impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}

/// Compiles only when `T` is neither `Send` nor `Sync`.
fn single_threaded<T: ?Sized>(_: PhantomData<T>) {
    <T as AmbiguousIfSend<_>>::probe();
    <T as AmbiguousIfSync<_>>::probe();
}

fn is_debug<T: Debug + ?Sized>() {}
fn is_display<T: Display + ?Sized>() {}
fn is_try_clone<T: TryClone>() {}
fn is_deref<T: Deref + ?Sized>() {}
fn is_deref_mut<T: DerefMut + ?Sized>() {}
fn is_ord<T: Ord>() {}
fn is_write<T: Write>() {}
fn is_borrow<T: ?Sized, B: ?Sized>()
where
    T: core::borrow::Borrow<B>,
{
}

trait Speak {
    fn id(&self) -> u8;
}
impl Speak for u8 {
    fn id(&self) -> u8 {
        *self
    }
}

#[test]
fn every_public_item_has_its_normative_signature() {
    // 5.1 errors
    let _: LimitId = LimitId(7);
    let _: [LimitId; 2] = [LimitId::CEILING, LimitId::HEADROOM];
    assert_eq!((LimitId::CEILING.0, LimitId::HEADROOM.0), (0, 1));
    let _: [MemError; 3] = [MemError::Limit(LimitId(2)), MemError::Ceiling, MemError::Overflow];
    let _: MemResult<u8> = Ok(1);

    // 5.2 ceiling
    let _: [ConfigError; 4] =
        [ConfigError::AlreadyConfigured, ConfigError::NotConfigured, ConfigError::BelowCurrent, ConfigError::BelowUsage];
    let _: fn(u64) -> Result<(), ConfigError> = configure;
    let _: fn(u64) -> Result<(), ConfigError> = raise_ceiling;
    let _: fn() -> Usage = usage;
    let u = usage();
    let _: (u64, u64, u64) = (u.used, u.ceiling, u.high_water);

    // 5.3 budgets
    let _: fn(LimitId, u64) -> MemResult<Budget> = Budget::root;
    let _: fn(&Budget, LimitId, u64) -> MemResult<Budget> = Budget::child;
    let _: fn(&Budget, u64) -> MemResult<()> = Budget::charge;
    let _: fn(&Budget, u64) = Budget::release;
    let _: fn(&Budget) -> u64 = Budget::used;
    let _: fn(&Budget) -> u64 = Budget::cap;
    let _: fn(&Budget) -> LimitId = Budget::limit;
    let _: fn(&Budget, u64) = Budget::set_cap;
    let _: fn(&Budget) -> Budget = Clone::clone;

    // 5.5 TryClone
    let _: fn(&u8) -> MemResult<u8> = <u8 as TryClone>::try_clone;
    is_try_clone::<u8>();
    is_try_clone::<u16>();
    is_try_clone::<u32>();
    is_try_clone::<u64>();
    is_try_clone::<u128>();
    is_try_clone::<usize>();
    is_try_clone::<i8>();
    is_try_clone::<i16>();
    is_try_clone::<i32>();
    is_try_clone::<i64>();
    is_try_clone::<i128>();
    is_try_clone::<isize>();
    is_try_clone::<f32>();
    is_try_clone::<f64>();
    is_try_clone::<bool>();
    is_try_clone::<char>();
    is_try_clone::<()>();
    is_try_clone::<(u8,)>();
    is_try_clone::<(u8, u16)>();
    is_try_clone::<(u8, u16, u32)>();
    is_try_clone::<(u8, u16, u32, u64)>();
    is_try_clone::<Option<u8>>();
    is_try_clone::<KVec<u8>>();
    is_try_clone::<KString>();
    is_try_clone::<KBox<u8>>();
    is_try_clone::<KRc<u8>>();
    is_try_clone::<KRc<dyn Speak>>();
    is_try_clone::<KMap<u8, u8>>();

    // 5.6 KVec
    let _: fn(&Budget) -> KVec<u8> = KVec::<u8>::new_in;
    let _: fn(usize, &Budget) -> MemResult<KVec<u8>> = KVec::<u8>::with_capacity_in;
    let _: fn(&mut KVec<u8>, usize) -> MemResult<()> = KVec::<u8>::try_reserve;
    let _: fn(&mut KVec<u8>, usize) -> MemResult<()> = KVec::<u8>::try_reserve_exact;
    let _: fn(&mut KVec<u8>, u8) -> MemResult<()> = KVec::<u8>::try_push;
    let _: fn(&mut KVec<u8>, usize, u8) -> MemResult<()> = KVec::<u8>::try_insert;
    let _: fn(&mut KVec<u8>, &[u8]) -> MemResult<()> = KVec::<u8>::try_extend_from_slice;
    let _: fn(&mut KVec<u8>, usize, u8) -> MemResult<()> = KVec::<u8>::try_resize;
    let _: fn(&mut KVec<u8>) -> Option<u8> = KVec::<u8>::pop;
    let _: fn(&mut KVec<u8>, usize) -> u8 = KVec::<u8>::remove;
    let _: fn(&mut KVec<u8>, usize) -> u8 = KVec::<u8>::swap_remove;
    let _: fn(&mut KVec<u8>, usize) = KVec::<u8>::truncate;
    let _: fn(&mut KVec<u8>) = KVec::<u8>::clear;
    let _: fn(&mut KVec<u8>) = KVec::<u8>::shrink_to_fit;
    let _: fn(&KVec<u8>) -> usize = KVec::<u8>::len;
    let _: fn(&KVec<u8>) -> bool = KVec::<u8>::is_empty;
    let _: fn(&KVec<u8>) -> usize = KVec::<u8>::capacity;
    let _: fn(&KVec<u8>) -> &[u8] = KVec::<u8>::as_slice;
    let _: fn(&mut KVec<u8>) -> &mut [u8] = KVec::<u8>::as_mut_slice;
    let _: fn(&KVec<u8>) -> &Budget = KVec::<u8>::budget;
    is_deref::<KVec<u8>>();
    is_deref_mut::<KVec<u8>>();
    is_debug::<KVec<u8>>();
    assert!(core::mem::needs_drop::<KVec<u8>>());
    let _: <&KVec<u8> as IntoIterator>::Item = &0u8;
    let _: <&mut KVec<u8> as IntoIterator>::Item = &mut 0u8;
    is_borrow::<KVec<u8>, [u8]>();
    let _: KBytes = KVec::<u8>::new_in(&Budget::root(LimitId(9), 0).unwrap());

    // 5.6 KString
    let _: fn(&Budget) -> KString = KString::new_in;
    let _: fn(&str, &Budget) -> MemResult<KString> = KString::try_from_str_in;
    let _: fn(&mut KString, char) -> MemResult<()> = KString::try_push;
    let _: fn(&mut KString, &str) -> MemResult<()> = KString::try_push_str;
    let _: fn(&KString) -> &str = KString::as_str;
    let _: fn(&KString) -> usize = KString::len;
    let _: fn(&KString) -> bool = KString::is_empty;
    let _: fn(&mut KString) = KString::clear;
    let _: fn(&mut KString, usize) = KString::truncate;
    let _: fn(KString) -> KBytes = KString::into_bytes;
    let _: fn(&mut KString) -> Option<MemError> = KString::take_fmt_error;
    is_deref::<KString>();
    is_debug::<KString>();
    is_display::<KString>();
    is_write::<KString>();
    is_ord::<KString>();
    is_borrow::<KString, str>();
    assert!(core::mem::needs_drop::<KString>());

    // 5.6 KBox
    let _: fn(u8, &Budget) -> Result<KBox<u8>, (u8, MemError)> = KBox::<u8>::try_new_in;
    let _: fn(KBox<u8>) -> u8 = KBox::<u8>::into_inner;
    let _: fn(&KBox<dyn Speak>) -> &Budget = KBox::<dyn Speak>::budget;
    let _: fn(KBox<dyn Speak>) -> (*mut dyn Speak, Budget) = KBox::<dyn Speak>::into_raw;
    let _: unsafe fn(*mut dyn Speak, Budget) -> KBox<dyn Speak> = KBox::<dyn Speak>::from_raw;
    is_deref::<KBox<dyn Speak>>();
    is_deref_mut::<KBox<dyn Speak>>();
    is_debug::<KBox<u8>>();
    assert!(core::mem::needs_drop::<KBox<u8>>());
    let b = Budget::root(LimitId(9), 64).unwrap();
    let boxed: KBox<dyn Speak> = sentropic_bash_kernel::kbox_unsize!(KBox::try_new_in(5u8, &b).unwrap(), dyn Speak);
    assert_eq!(boxed.id(), 5);

    // 5.6 KRc
    let _: fn(u8, &Budget) -> Result<KRc<u8>, (u8, MemError)> = KRc::<u8>::try_new_in;
    let _: fn(KRc<u8>) -> Result<u8, KRc<u8>> = KRc::<u8>::try_unwrap;
    let _: fn(&KRc<dyn Speak>) -> usize = KRc::<dyn Speak>::strong_count;
    let _: fn(&KRc<dyn Speak>, &KRc<dyn Speak>) -> bool = KRc::<dyn Speak>::ptr_eq;
    let _: fn(&mut KRc<dyn Speak>) -> Option<&mut (dyn Speak + 'static)> = KRc::<dyn Speak>::get_mut;
    let _: fn(&KRc<u8>) -> KRc<u8> = Clone::clone;
    is_deref::<KRc<dyn Speak>>();
    is_debug::<KRc<u8>>();
    assert!(core::mem::needs_drop::<KRc<u8>>());

    // 5.6 KMap
    let _: fn(&Budget) -> KMap<u32, u32> = KMap::<u32, u32>::new_in;
    let _: fn(&mut KMap<u32, u32>, u32, u32) -> MemResult<Option<u32>> = KMap::<u32, u32>::try_insert;
    let _: for<'a, 'b> fn(&'a KMap<u32, u32>, &'b u32) -> Option<&'a u32> = KMap::<u32, u32>::get::<u32>;
    let _: for<'a, 'b> fn(&'a mut KMap<u32, u32>, &'b u32) -> Option<&'a mut u32> = KMap::<u32, u32>::get_mut::<u32>;
    let _: fn(&KMap<u32, u32>, &u32) -> bool = KMap::<u32, u32>::contains_key::<u32>;
    let _: fn(&mut KMap<u32, u32>, &u32) -> Option<u32> = KMap::<u32, u32>::remove::<u32>;
    let _: fn(&KMap<u32, u32>) -> usize = KMap::<u32, u32>::len;
    let _: fn(&KMap<u32, u32>) -> bool = KMap::<u32, u32>::is_empty;
    let _: fn(&mut KMap<u32, u32>) = KMap::<u32, u32>::clear;
    let _: for<'a> fn(&'a KMap<u32, u32>) -> Option<(&'a u32, &'a u32)> = KMap::<u32, u32>::first_key_value;
    let _: for<'a> fn(&'a KMap<u32, u32>) -> Option<(&'a u32, &'a u32)> = KMap::<u32, u32>::last_key_value;
    let _: fn(&KMap<u32, u32>) -> &Budget = KMap::<u32, u32>::budget;
    let mut map = KMap::<KString, u32>::new_in(&b);
    let _: Option<(&KString, &u32)> = map.iter().next();
    let _: Option<(&KString, &mut u32)> = map.iter_mut().next();
    let _: Option<&KString> = map.keys().next();
    let _: Option<&u32> = map.values().next();
    let _: Option<&u32> = map.get("lookup by str, no allocation");
    let _: bool = map.contains_key("x");
    let _: Option<&mut u32> = map.get_mut("x");
    let _: Option<u32> = map.remove("x");
    is_debug::<KMap<u32, u32>>();
    assert!(core::mem::needs_drop::<KMap<u32, u32>>());

    // 5.8 transfer hook
    let _: unsafe fn(*mut u8, usize, usize, usize) -> *mut u8 = cabi_realloc;
    let _: unsafe fn(*mut u8, usize, usize) = transfer_free;

    // Single-threaded by design: no Send, no Sync.
    single_threaded::<Budget>(PhantomData);
    single_threaded::<KVec<u8>>(PhantomData);
    single_threaded::<KString>(PhantomData);
    single_threaded::<KBox<u8>>(PhantomData);
    single_threaded::<KRc<u8>>(PhantomData);
    single_threaded::<KMap<u8, u8>>(PhantomData);
}
