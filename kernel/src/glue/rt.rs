//! Runtime of the generated glue (EVOL D16): the few items `wit-bindgen` expects from its runtime path,
//! written here so that no crate is needed. Everything goes through the global allocator, which is
//! `kernel::mem`.
use crate::fiber::TRACE_BUF_CAP;
use crate::mem::abi::{alloc, dealloc, Layout};
use crate::mem::{MemError, MemResult};

// `take_trace` lowers up to TRACE_BUF_CAP TraceEvents plus one outer list (257 lists).
const LOWERING_SLOTS: usize = TRACE_BUF_CAP + 1;
static mut LOWERING: [Option<Cleanup>; LOWERING_SLOTS] = [const { None }; LOWERING_SLOTS];

/// True when no lowering slot is held: the quiescence a snapshot capture requires.
pub(crate) fn lowering_clean() -> bool {
    // SAFETY: the Wasm instance is single-threaded; reading the option slots cannot race.
    unsafe { &*(&raw const LOWERING) }.iter().all(Option::is_none)
}

/// Allocate the generated record arrays while the adapter can still report a refusal. Keep the
/// actual blocks until `Cleanup::new` takes them, even if the ordinary heap is fragmented.
pub fn prepare_lowering(lists: &[(usize, usize, usize)]) -> MemResult<()> {
    let mut blocks = [const { None }; LOWERING_SLOTS];
    assert!(lists.len() <= blocks.len());
    for (slot, &(count, stride, align)) in blocks.iter_mut().zip(lists) {
        let size = count.checked_mul(stride).ok_or(MemError::Overflow)?;
        let layout = Layout::from_size_align(size, align).map_err(|_| MemError::Overflow)?;
        *slot = Cleanup::try_new(layout)?.1;
    }
    // SAFETY: the Wasm instance is single-threaded and cannot re-enter during result lowering.
    // No reference to the slots escapes this block.
    let slots = unsafe { &mut *(&raw mut LOWERING) };
    assert!(slots.iter().all(Option::is_none));
    *slots = blocks;
    Ok(())
}

/// Allocate a resource representation fallibly with the layout its generated destructor expects.
pub fn new_resource<T, R>(value: T, register: impl FnOnce(*mut u8) -> R) -> MemResult<R> {
    let (ptr, cleanup) = Cleanup::try_new(Layout::new::<T>())?;
    // SAFETY: the block has exactly the layout of T (or its aligned dangling pointer for a ZST).
    // The generated resource constructor takes ownership. Its destructor drops it as a Box<T>.
    unsafe { ptr.cast::<T>().write(value) };
    if let Some(cleanup) = cleanup {
        cleanup.forget();
    }
    Ok(register(ptr))
}

/// A resource implemented by this component. `Rep` is how the value is stored behind the handle.
pub trait Resource: Sized + 'static {
    type Rep: ResourceRep<Self>;
}

/// Storage of a resource value. The kernel is single-threaded and stores the value itself.
pub trait ResourceRep<T> {
    fn rep_new(val: T) -> Self;
    unsafe fn rep_as_ref<'a>(ptr: *const Self) -> &'a T;
    unsafe fn rep_as_mut<'a>(ptr: *mut Self) -> &'a mut T;
    unsafe fn rep_take(ptr: *mut Self) -> T;
}

impl<T> ResourceRep<T> for T {
    fn rep_new(val: T) -> T {
        val
    }
    unsafe fn rep_as_ref<'a>(ptr: *const T) -> &'a T {
        // SAFETY: the canonical ABI hands back the pointer made by `resource_into_raw_`, live until `dtor`.
        unsafe { &*ptr }
    }
    unsafe fn rep_as_mut<'a>(ptr: *mut T) -> &'a mut T {
        // SAFETY: as above; the kernel is single-threaded and the borrow is not kept across calls.
        unsafe { &mut *ptr }
    }
    unsafe fn rep_take(ptr: *mut T) -> T {
        // SAFETY: as above; the caller consumes the resource.
        unsafe { ptr.read() }
    }
}

/// Memory allocated for lowering that is freed when dropped, unless `forget` is called.
pub struct Cleanup {
    ptr: *mut u8,
    layout: Layout,
}

impl Cleanup {
    pub fn new(layout: Layout) -> (*mut u8, Option<Cleanup>) {
        if layout.size() == 0 {
            return (layout.align() as *mut u8, None);
        }
        // SAFETY: the Wasm instance is single-threaded and cannot re-enter during result lowering.
        // No reference to the slots escapes this block.
        let slots = unsafe { &mut *(&raw mut LOWERING) };
        let block = slots.iter_mut().find(|slot| slot.as_ref().is_some_and(|b| b.layout == layout))
            .and_then(Option::take).expect("result record array was not prepared");
        (block.ptr, Some(block))
    }

    fn try_new(layout: Layout) -> MemResult<(*mut u8, Option<Cleanup>)> {
        if layout.size() == 0 {
            return Ok((layout.align() as *mut u8, None));
        }
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { alloc(layout) };
        if ptr.is_null() {
            return Err(MemError::Ceiling);
        }
        Ok((ptr, Some(Cleanup { ptr, layout })))
    }

    pub fn forget(self) {
        core::mem::forget(self);
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        // SAFETY: `ptr` was allocated in `new` with `layout`.
        unsafe { dealloc(self.ptr, self.layout) };
    }
}

pub fn run_ctors_once() {}

pub mod bitflags {
    //! The subset of the `bitflags` macro that the generated code uses.
    macro_rules! bitflags {
        (
            $(#[$outer:meta])*
            $vis:vis struct $Name:ident: $T:ty {
                $( const $Flag:ident = $value:expr; )*
            }
        ) => {
            $(#[$outer])*
            $vis struct $Name($T);
            #[allow(dead_code)]
            impl $Name {
                $( pub const $Flag: $Name = $Name($value); )*
                pub const fn empty() -> $Name { $Name(0) }
                pub const fn bits(&self) -> $T { self.0 }
                pub const fn from_bits_retain(bits: $T) -> $Name { $Name(bits) }
                pub const fn contains(&self, other: $Name) -> bool { self.0 & other.0 == other.0 }
            }
            impl core::ops::BitOr for $Name {
                type Output = $Name;
                fn bitor(self, rhs: $Name) -> $Name { $Name(self.0 | rhs.0) }
            }
        };
    }
    pub(crate) use bitflags;
}
