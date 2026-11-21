//! Runtime of the generated glue (EVOL D16): the few items `wit-bindgen` expects from its runtime path,
//! written here so that no crate is needed. Everything goes through the global allocator, which is
//! `kernel::mem`.
use crate::mem::abi::{alloc, dealloc, handle_alloc_error, Layout};
use crate::mem::{MemError, MemResult};

/// Tests a generated lowering layout after the adapter has built its return value. The single-threaded
/// bump allocator reuses this last block immediately, at the same or a smaller alignment.
pub fn check_lowering(size: usize, align: usize) -> MemResult<()> {
    if size == 0 {
        return Ok(());
    }
    let layout = Layout::from_size_align(size, align).map_err(|_| MemError::Overflow)?;
    // SAFETY: the layout has a non-zero size.
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        return Err(MemError::Ceiling);
    }
    // SAFETY: ptr is the block just allocated with this layout and was never exposed.
    unsafe { dealloc(ptr, layout) };
    Ok(())
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
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { alloc(layout) };
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        (ptr, Some(Cleanup { ptr, layout }))
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
