//! Gate 13 companion: names every public item added by lot L1a-2 (spec 5.1-5.3) with its full signature.
//! The L1a-1 interface test (`interface.rs`) is unchanged and still compiles against the new implementations.
use sentropic_bash_kernel::mem::*;
use std::fmt::Debug;
use std::marker::PhantomData;

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

fn single_threaded<T: ?Sized>(_: PhantomData<T>) {
    <T as AmbiguousIfSend<_>>::probe();
    <T as AmbiguousIfSync<_>>::probe();
}
fn is_debug<T: Debug + ?Sized>() {}
fn is_try_clone<T: TryClone>() {}

#[test]
fn interface() {
    // 5.1 accounting
    let _: fn() -> u64 = footprint;
    let _: fn() -> u64 = footprint_high_water;
    let _: fn(u64) -> Result<(), ConfigError> = set_headroom;
    let _: fn() -> Budget = headroom;
    let _: u8 = reserve_scope(|| 1u8);
    assert_eq!(reserve_scope(|| reserve_scope(|| 7)), 7, "nested scopes just run");
    assert_eq!(headroom().limit(), LimitId::HEADROOM);

    // 5.2 KInsertMap
    let b = Budget::root(LimitId(1000), 1 << 20).unwrap();
    let _: fn(&Budget) -> KInsertMap<u32, u32> = KInsertMap::<u32, u32>::new_in;
    let _: fn(&mut KInsertMap<u32, u32>, u32, u32) -> MemResult<Option<u32>> = KInsertMap::<u32, u32>::try_insert;
    let _: for<'a, 'b> fn(&'a KInsertMap<u32, u32>, &'b u32) -> Option<&'a u32> = KInsertMap::<u32, u32>::get::<u32>;
    let _: for<'a, 'b> fn(&'a mut KInsertMap<u32, u32>, &'b u32) -> Option<&'a mut u32> = KInsertMap::<u32, u32>::get_mut::<u32>;
    let _: fn(&KInsertMap<u32, u32>, &u32) -> bool = KInsertMap::<u32, u32>::contains_key::<u32>;
    let _: fn(&mut KInsertMap<u32, u32>, &u32) -> Option<u32> = KInsertMap::<u32, u32>::remove::<u32>;
    let _: fn(&KInsertMap<u32, u32>) -> usize = KInsertMap::<u32, u32>::len;
    let _: fn(&KInsertMap<u32, u32>) -> bool = KInsertMap::<u32, u32>::is_empty;
    let _: fn(&mut KInsertMap<u32, u32>) = KInsertMap::<u32, u32>::clear;
    let _: fn(&KInsertMap<u32, u32>) -> &Budget = KInsertMap::<u32, u32>::budget;
    let mut m = KInsertMap::<KString, u32>::new_in(&b);
    let _: Option<(&KString, &u32)> = m.iter().next();
    let _: Option<(&KString, &mut u32)> = m.iter_mut().next();
    let _: Option<&KString> = m.keys().next();
    let _: Option<&u32> = m.values().next();
    let _: Option<&u32> = m.get("borrowed lookup");
    is_debug::<KInsertMap<u32, u32>>();
    is_try_clone::<KInsertMap<u8, u8>>();
    assert!(core::mem::needs_drop::<KInsertMap<u32, u32>>());
    single_threaded::<KInsertMap<u8, u8>>(PhantomData);

    // 5.3 KChunks
    let _: usize = KChunks::CHUNK;
    assert_eq!(KChunks::CHUNK, 65_536);
    let _: fn(&Budget) -> KChunks = KChunks::new_in;
    let _: fn(&KChunks) -> u64 = KChunks::len;
    let _: fn(&KChunks) -> bool = KChunks::is_empty;
    let _: fn(&mut KChunks, &[u8]) -> MemResult<()> = KChunks::try_append;
    let _: fn(&mut KChunks, u64, &[u8]) -> MemResult<()> = KChunks::try_write_at;
    let _: fn(&KChunks, u64, &mut [u8]) -> usize = KChunks::read_at;
    let _: fn(&mut KChunks, u64) = KChunks::truncate;
    let _: fn(&mut KChunks) = KChunks::clear;
    let _: fn(&KChunks) -> &Budget = KChunks::budget;
    let mut c = KChunks::new_in(&b);
    let _: usize = c.drain_front(10, |_slice: &[u8]| {});
    let _: Option<&[u8]> = c.chunks().next();
    is_debug::<KChunks>();
    is_try_clone::<KChunks>();
    assert!(core::mem::needs_drop::<KChunks>());
    single_threaded::<KChunks>(PhantomData);
}
