//! `KVec`: a growable array whose every growth is fallible and charged to a budget (spec 5.4, 5.6).
use super::budget::Budget;
use super::error::{MemError, MemResult};
use alloc::alloc::{alloc, dealloc, realloc, Layout};
use core::marker::PhantomData;
use core::mem::{align_of, size_of};
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};

/// Minimum capacity of an amortized growth, in elements.
pub const MIN_GROWTH: usize = 4;

/// Growable array. Charges `capacity * size_of::<T>()` to its budget (zero-sized elements charge
/// nothing), charges the difference before growing, releases on shrink and on drop. Not `Send`.
pub struct KVec<T> {
    ptr: NonNull<T>,
    cap: usize,
    len: usize,
    budget: Budget,
    _owns: PhantomData<T>,
}

/// Byte buffers are the kernel's native data type.
pub type KBytes = KVec<u8>;

impl<T> KVec<T> {
    const ZST: bool = size_of::<T>() == 0;

    /// An empty vector on `budget`. Allocates nothing.
    pub fn new_in(budget: &Budget) -> KVec<T> {
        KVec { ptr: NonNull::dangling(), cap: 0, len: 0, budget: budget.clone(), _owns: PhantomData }
    }

    /// A vector with room for exactly `capacity` elements. Fails with `Overflow`, `Limit(id)` or `Ceiling`.
    pub fn with_capacity_in(capacity: usize, budget: &Budget) -> MemResult<KVec<T>> {
        let mut v = KVec::new_in(budget);
        v.grow_to(capacity)?;
        Ok(v)
    }

    /// Grows the buffer to exactly `new_cap` elements: checks the size, charges the difference, then
    /// allocates. On failure nothing is charged and the vector is unchanged.
    fn grow_to(&mut self, new_cap: usize) -> MemResult<()> {
        if Self::ZST || new_cap <= self.cap {
            return Ok(());
        }
        let bytes = new_cap.checked_mul(size_of::<T>()).ok_or(MemError::Overflow)?;
        let layout = Layout::from_size_align(bytes, align_of::<T>()).map_err(|_| MemError::Overflow)?;
        let delta = (bytes - self.cap * size_of::<T>()) as u64;
        self.budget.charge(delta)?;
        let raw = if self.cap == 0 {
            // SAFETY: `layout` has a non-zero size (`new_cap > cap >= 0` and `T` is not zero-sized).
            unsafe { alloc(layout) }
        } else {
            // SAFETY: `ptr` is the live block allocated with layout (`cap`, align of T); the new size is
            // non-zero and the layout was checked above to be valid for it.
            unsafe { realloc(self.ptr.as_ptr() as *mut u8, Layout::from_size_align_unchecked(self.cap * size_of::<T>(), align_of::<T>()), bytes) }
        };
        match NonNull::new(raw as *mut T) {
            Some(p) => {
                self.ptr = p;
                self.cap = new_cap;
                Ok(())
            }
            None => {
                self.budget.release(delta);
                Err(MemError::Ceiling)
            }
        }
    }

    fn grow_amortized(&mut self, need: usize) -> MemResult<()> {
        let amortized = need.max(self.cap.saturating_add(self.cap.div_ceil(2))).max(MIN_GROWTH);
        match self.grow_to(amortized) {
            Err(MemError::Limit(_) | MemError::Overflow) if amortized != need => self.grow_to(need),
            r => r,
        }
    }

    /// Makes room for `additional` more elements, growing by max(need, 1.5 x capacity, 4); when the budget
    /// cannot cover that, the exact need is tried. Fails with `Overflow`, `Limit(id)` or `Ceiling`; the
    /// vector is unchanged on failure.
    pub fn try_reserve(&mut self, additional: usize) -> MemResult<()> {
        let need = self.len.checked_add(additional).ok_or(MemError::Overflow)?;
        if need <= self.capacity() {
            return Ok(());
        }
        self.grow_amortized(need)
    }

    /// Makes room for exactly `additional` more elements. Fails like `try_reserve`.
    pub fn try_reserve_exact(&mut self, additional: usize) -> MemResult<()> {
        let need = self.len.checked_add(additional).ok_or(MemError::Overflow)?;
        self.grow_to(need)
    }

    /// Appends a value. On failure the value is dropped and the vector is unchanged.
    pub fn try_push(&mut self, value: T) -> MemResult<()> {
        if self.len == self.capacity() {
            self.try_reserve(1)?;
        }
        // SAFETY: `len < capacity`, so the slot is inside the allocation and unoccupied.
        unsafe { self.ptr.as_ptr().add(self.len).write(value) };
        self.len += 1;
        Ok(())
    }

    /// Inserts at `index`, shifting the tail. Panics if `index > len`. On failure the value is dropped.
    pub fn try_insert(&mut self, index: usize, value: T) -> MemResult<()> {
        assert!(index <= self.len, "insertion index out of bounds");
        if self.len == self.capacity() {
            self.try_reserve(1)?;
        }
        // SAFETY: `len < capacity` and `index <= len`: shifting `len - index` elements one slot up stays
        // inside the allocation, and the freed slot at `index` is then written.
        unsafe {
            let p = self.ptr.as_ptr().add(index);
            ptr::copy(p, p.add(1), self.len - index);
            p.write(value);
        }
        self.len += 1;
        Ok(())
    }

    /// Appends a copy of `items`. Fails like `try_reserve`; the vector is unchanged on failure.
    pub fn try_extend_from_slice(&mut self, items: &[T]) -> MemResult<()>
    where
        T: Copy,
    {
        self.try_reserve(items.len())?;
        // SAFETY: room for `items.len()` more elements was reserved, and `items` cannot alias the
        // vector's spare capacity (it is a shared slice of initialised memory).
        unsafe { ptr::copy_nonoverlapping(items.as_ptr(), self.ptr.as_ptr().add(self.len), items.len()) };
        self.len += items.len();
        Ok(())
    }

    /// Resizes to `new_len`, filling with copies of `value`. Fails like `try_reserve` when growing.
    pub fn try_resize(&mut self, new_len: usize, value: T) -> MemResult<()>
    where
        T: Copy,
    {
        if new_len <= self.len {
            self.truncate(new_len);
            return Ok(());
        }
        self.try_reserve(new_len - self.len)?;
        while self.len < new_len {
            // SAFETY: `len < new_len <= capacity` after the reservation.
            unsafe { self.ptr.as_ptr().add(self.len).write(value) };
            self.len += 1;
        }
        Ok(())
    }

    /// Removes and returns the last element.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        // SAFETY: the element at the old `len - 1` is initialised and no longer counted.
        Some(unsafe { self.ptr.as_ptr().add(self.len).read() })
    }

    /// Removes the element at `index`, shifting the tail. Panics if out of bounds.
    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "removal index out of bounds");
        // SAFETY: `index < len`: the element is initialised; the tail moves down over its slot.
        unsafe {
            let p = self.ptr.as_ptr().add(index);
            let out = p.read();
            ptr::copy(p.add(1), p, self.len - index - 1);
            self.len -= 1;
            out
        }
    }

    /// Removes the element at `index`, replacing it with the last one. Panics if out of bounds.
    pub fn swap_remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "removal index out of bounds");
        let last = self.len - 1;
        self.as_mut_slice().swap(index, last);
        self.pop().expect("non-empty")
    }

    /// Shortens to `len` elements, dropping the rest. No effect if `len >= self.len()`.
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        let tail = self.len - len;
        self.len = len;
        // SAFETY: elements `len..len + tail` are initialised and no longer counted by the vector.
        unsafe { ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.ptr.as_ptr().add(len), tail)) };
    }

    /// Drops every element; the capacity stays.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Gives back unused capacity. Best effort, never fails.
    pub fn shrink_to_fit(&mut self) {
        if Self::ZST || self.cap == self.len {
            return;
        }
        let old_bytes = self.cap * size_of::<T>();
        let new_bytes = self.len * size_of::<T>();
        if self.len == 0 {
            // SAFETY: the block was allocated with exactly this layout.
            unsafe { dealloc(self.ptr.as_ptr() as *mut u8, Layout::from_size_align_unchecked(old_bytes, align_of::<T>())) };
            self.ptr = NonNull::dangling();
        } else {
            // SAFETY: the block was allocated with the old layout; `new_bytes` is non-zero and smaller.
            let raw = unsafe { realloc(self.ptr.as_ptr() as *mut u8, Layout::from_size_align_unchecked(old_bytes, align_of::<T>()), new_bytes) };
            match NonNull::new(raw as *mut T) {
                Some(p) => self.ptr = p,
                None => return,
            }
        }
        self.cap = self.len;
        self.budget.release((old_bytes - new_bytes) as u64);
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when there is no element.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacity in elements (`usize::MAX` for zero-sized elements).
    pub fn capacity(&self) -> usize {
        if Self::ZST {
            usize::MAX
        } else {
            self.cap
        }
    }

    /// The elements as a slice.
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: `ptr` is non-null and aligned, and the first `len` elements are initialised.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    /// The elements as a mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: as `as_slice`, and `&mut self` guarantees exclusive access.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// The budget this vector charges.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }
}

impl<T> Drop for KVec<T> {
    fn drop(&mut self) {
        self.clear();
        if !Self::ZST && self.cap != 0 {
            let bytes = self.cap * size_of::<T>();
            // SAFETY: the block was allocated with exactly this layout and is live.
            unsafe { dealloc(self.ptr.as_ptr() as *mut u8, Layout::from_size_align_unchecked(bytes, align_of::<T>())) };
            self.budget.release(bytes as u64);
        }
    }
}

impl<T> Deref for KVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> DerefMut for KVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for KVec<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

impl<'a, T> IntoIterator for &'a KVec<T> {
    type Item = &'a T;
    type IntoIter = core::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, T> IntoIterator for &'a mut KVec<T> {
    type Item = &'a mut T;
    type IntoIter = core::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

impl<T> core::borrow::Borrow<[T]> for KVec<T> {
    fn borrow(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T: PartialEq> PartialEq for KVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for KVec<T> {}

impl<T: PartialOrd> PartialOrd for KVec<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}

impl<T: Ord> Ord for KVec<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}
