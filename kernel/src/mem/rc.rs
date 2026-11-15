//! `KRc`: single-threaded reference-counted pointer on a budget (spec 5.6). No weak references;
//! reference cycles are a bug of the caller.
use super::budget::Budget;
use super::error::{MemError, MemResult};
use alloc::alloc::{alloc, dealloc, Layout};
use core::cell::Cell;
use core::mem::{size_of, ManuallyDrop};
use core::ops::Deref;
use core::ptr::{self, NonNull};

struct RcInner<T: ?Sized> {
    strong: Cell<usize>,
    budget: Budget,
    value: T,
}

/// Shared pointer. The allocation (count, budget handle and value) is charged once, to the budget it was
/// created with. Cloning is a handle copy: it never fails and allocates nothing. Not `Send`.
pub struct KRc<T: ?Sized> {
    ptr: NonNull<RcInner<T>>,
}

impl<T> KRc<T> {
    /// Moves `value` to the heap. On failure the value is returned with the error (`Limit(id)` or `Ceiling`).
    pub fn try_new_in(value: T, budget: &Budget) -> Result<KRc<T>, (T, MemError)> {
        let size = size_of::<RcInner<T>>();
        if let Err(e) = budget.charge(size as u64) {
            return Err((value, e));
        }
        // SAFETY: `RcInner` holds a `usize`, so its size is non-zero and the layout is valid.
        let raw = unsafe { alloc(Layout::new::<RcInner<T>>()) } as *mut RcInner<T>;
        match NonNull::new(raw) {
            Some(ptr) => {
                // SAFETY: `raw` is a fresh allocation for one `RcInner<T>`.
                unsafe { raw.write(RcInner { strong: Cell::new(1), budget: budget.clone(), value }) };
                Ok(KRc { ptr })
            }
            None => {
                budget.release(size as u64);
                Err((value, MemError::Ceiling))
            }
        }
    }

    /// Takes the value out when this is the only handle; otherwise gives the handle back.
    pub fn try_unwrap(this: KRc<T>) -> Result<T, KRc<T>> {
        if KRc::strong_count(&this) != 1 {
            return Err(this);
        }
        let this = ManuallyDrop::new(this);
        // SAFETY: the count is 1, so this handle owns the block; the value and the budget handle are each
        // read exactly once and the handle is never dropped.
        let (value, budget) = unsafe {
            let inner = this.ptr.as_ptr();
            (ptr::read(&(*inner).value), ptr::read(&(*inner).budget))
        };
        let layout = Layout::new::<RcInner<T>>();
        // SAFETY: the block was allocated in `try_new_in` with this layout.
        unsafe { dealloc(this.ptr.as_ptr() as *mut u8, layout) };
        budget.release(layout.size() as u64);
        Ok(value)
    }
}

impl<T: ?Sized> KRc<T> {
    fn inner(&self) -> &RcInner<T> {
        // SAFETY: the block is live while a handle exists.
        unsafe { self.ptr.as_ref() }
    }

    /// Number of handles to the value.
    pub fn strong_count(this: &KRc<T>) -> usize {
        this.inner().strong.get()
    }

    /// True when both handles point to the same allocation.
    pub fn ptr_eq(a: &KRc<T>, b: &KRc<T>) -> bool {
        core::ptr::addr_eq(a.ptr.as_ptr(), b.ptr.as_ptr())
    }

    /// Mutable access, only when this is the only handle.
    pub fn get_mut(this: &mut KRc<T>) -> Option<&mut T> {
        if KRc::strong_count(this) == 1 {
            // SAFETY: the count is 1 and `this` is borrowed mutably, so access is exclusive.
            Some(unsafe { &mut this.ptr.as_mut().value })
        } else {
            None
        }
    }
}

impl<T: ?Sized> Clone for KRc<T> {
    /// A handle copy: never fails, allocates nothing. The count traps on overflow.
    fn clone(&self) -> KRc<T> {
        let strong = &self.inner().strong;
        match strong.get().checked_add(1) {
            Some(n) => strong.set(n),
            None => panic!("KRc count overflow"),
        }
        KRc { ptr: self.ptr }
    }
}

impl<T: ?Sized> Drop for KRc<T> {
    fn drop(&mut self) {
        let inner = self.ptr.as_ptr();
        // SAFETY: the block is live while a handle exists; the count is only touched on this thread.
        let left = unsafe { (*inner).strong.get() } - 1;
        // SAFETY: as above.
        unsafe { (*inner).strong.set(left) };
        if left != 0 {
            return;
        }
        // SAFETY: this was the last handle, so the value is dropped exactly once and the budget handle is
        // read exactly once; the layout is read before the block is freed.
        unsafe {
            let layout = Layout::for_value(&*inner);
            ptr::drop_in_place(ptr::addr_of_mut!((*inner).value));
            let budget = ptr::read(ptr::addr_of!((*inner).budget));
            dealloc(inner as *mut u8, layout);
            budget.release(layout.size() as u64);
        }
    }
}

impl<T: ?Sized> Deref for KRc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner().value
    }
}

impl<T: ?Sized + core::fmt::Debug> core::fmt::Debug for KRc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized> super::TryClone for KRc<T> {
    fn try_clone(&self) -> MemResult<Self> {
        Ok(self.clone())
    }
}
