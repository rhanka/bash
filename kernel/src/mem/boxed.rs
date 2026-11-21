//! `KBox`: a fallible owning pointer charged to a budget (spec 5.6).
use super::budget::Budget;
use super::error::{MemError, MemResult};
use super::alloc::alloc::{alloc, dealloc, Layout};
use core::mem::{size_of, ManuallyDrop};
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};

/// Owning pointer to a heap value. Charges the size of the allocation actually requested
/// (nothing for a zero-sized value). Not `Send`.
pub struct KBox<T: ?Sized> {
    ptr: NonNull<T>,
    budget: Budget,
}

impl<T> KBox<T> {
    /// Moves `value` to the heap. On failure the value is returned to the caller with the error
    /// (`Limit(id)` or `Ceiling`).
    pub fn try_new_in(value: T, budget: &Budget) -> Result<KBox<T>, (T, MemError)> {
        let size = size_of::<T>();
        if size == 0 {
            let ptr = NonNull::<T>::dangling();
            // SAFETY: a zero-sized value is "written" through a dangling, aligned pointer: no memory is touched.
            unsafe { ptr.as_ptr().write(value) };
            return Ok(KBox { ptr, budget: budget.clone() });
        }
        if let Err(e) = budget.charge(size as u64) {
            return Err((value, e));
        }
        // SAFETY: `size` is non-zero and the layout of `T` is valid.
        let raw = unsafe { alloc(Layout::new::<T>()) } as *mut T;
        match NonNull::new(raw) {
            Some(ptr) => {
                // SAFETY: `raw` is a fresh allocation for one `T`.
                unsafe { raw.write(value) };
                Ok(KBox { ptr, budget: budget.clone() })
            }
            None => {
                budget.release(size as u64);
                Err((value, MemError::Ceiling))
            }
        }
    }

    /// Moves the value out and frees the allocation.
    pub fn into_inner(self) -> T {
        let this = ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped, so the value is read exactly once; the budget handle is read
        // exactly once and dropped below.
        let (value, budget) = unsafe { (this.ptr.as_ptr().read(), ptr::read(&this.budget)) };
        let size = size_of::<T>();
        if size != 0 {
            // SAFETY: the block was allocated in `try_new_in` with the layout of `T`.
            unsafe { dealloc(this.ptr.as_ptr() as *mut u8, Layout::new::<T>()) };
            budget.release(size as u64);
        }
        value
    }
}

impl<T: ?Sized> KBox<T> {
    /// The budget this box charges.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Gives up ownership: the caller must give both parts back to `from_raw` (or free them by hand).
    pub fn into_raw(self) -> (*mut T, Budget) {
        let this = ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped, so the budget handle is moved out exactly once.
        (this.ptr.as_ptr(), unsafe { ptr::read(&this.budget) })
    }

    /// Rebuilds a box from the parts of `into_raw`.
    ///
    /// # Safety
    /// `ptr` and `budget` come from `into_raw` of a `KBox` whose pointee layout is unchanged.
    pub unsafe fn from_raw(ptr: *mut T, budget: Budget) -> KBox<T> {
        // SAFETY: the caller guarantees `ptr` is the non-null pointer of a live `KBox`.
        KBox { ptr: unsafe { NonNull::new_unchecked(ptr) }, budget }
    }
}

impl<T: ?Sized> Drop for KBox<T> {
    fn drop(&mut self) {
        // SAFETY: the pointee is live and owned by this box; its size and layout are read before it is dropped.
        let layout = unsafe { Layout::for_value(self.ptr.as_ref()) };
        // SAFETY: the value is initialised and dropped exactly once.
        unsafe { ptr::drop_in_place(self.ptr.as_ptr()) };
        if layout.size() != 0 {
            // SAFETY: the block was allocated with this layout (the layout of the original sized type).
            unsafe { dealloc(self.ptr.as_ptr() as *mut u8, layout) };
            self.budget.release(layout.size() as u64);
        }
    }
}

impl<T: ?Sized> Deref for KBox<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the pointer is valid and the value initialised for the life of the box.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for KBox<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as `deref`, and `&mut self` guarantees exclusive access.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized + core::fmt::Debug> core::fmt::Debug for KBox<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T: super::TryClone> super::TryClone for KBox<T> {
    fn try_clone(&self) -> MemResult<Self> {
        let value = (**self).try_clone()?;
        KBox::try_new_in(value, &self.budget).map_err(|(_, e)| e)
    }
}

/// Converts a `KBox<Concrete>` into a `KBox<dyn Trait>` (or `KBox<[T]>` from an array).
/// Usage: `kbox_unsize!(boxed, dyn Command)`. Expands to `into_raw`, a raw-pointer unsizing cast,
/// and `from_raw`; it allocates nothing and cannot fail. The target must be an unsizing coercion of the
/// source type: the pointer goes through a typed `let`, so a mere reinterpreting cast does not compile.
///
/// ```
/// use sentropic_bash_kernel::mem::{Budget, KBox, LimitId};
/// let budget = Budget::root(LimitId(1), 1 << 20).unwrap();
/// let b = KBox::try_new_in(5u8, &budget).unwrap();
/// let d: KBox<dyn core::fmt::Debug> = sentropic_bash_kernel::kbox_unsize!(b, dyn core::fmt::Debug);
/// assert_eq!(format!("{:?}", &*d), "5");
/// ```
///
/// A reinterpreting cast is rejected at compile time:
///
/// ```compile_fail
/// use sentropic_bash_kernel::mem::{Budget, KBox, LimitId};
/// let budget = Budget::root(LimitId(1), 1 << 20).unwrap();
/// let b = KBox::try_new_in(5u8, &budget).unwrap();
/// let _wrong: KBox<u64> = sentropic_bash_kernel::kbox_unsize!(b, u64);
/// ```
///
/// ```compile_fail
/// use sentropic_bash_kernel::mem::{Budget, KBox, LimitId};
/// let budget = Budget::root(LimitId(1), 1 << 20).unwrap();
/// let b = KBox::try_new_in([1u8; 4], &budget).unwrap();
/// let _wrong: KBox<[u64]> = sentropic_bash_kernel::kbox_unsize!(b, [u64]);
/// ```
#[macro_export]
macro_rules! kbox_unsize {
    ($boxed:expr, $($target:tt)+) => {{
        let (raw, budget) = $crate::mem::KBox::into_raw($boxed);
        let coerced: *mut $($target)+ = raw;
        // SAFETY: `raw` and `budget` come straight from `into_raw`, and the unsizing coercion keeps the
        // address and the allocation layout (the size is read back with `Layout::for_value`).
        unsafe { $crate::mem::KBox::from_raw(coerced, budget) }
    }};
}
