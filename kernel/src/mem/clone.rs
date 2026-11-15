//! `TryClone` (spec 5.5): a clone that can fail on memory. A clone is charged to the budget of its source.
use super::error::MemResult;
use super::string::KString;
use super::vec::KVec;

/// A clone that reports memory failure instead of aborting.
pub trait TryClone: Sized {
    /// Clones the value, charging the same budget as the source. Fails with `Overflow`, `Limit(id)` or
    /// `Ceiling`; nothing stays allocated on failure.
    fn try_clone(&self) -> MemResult<Self>;
}

macro_rules! copy_try_clone {
    ($($t:ty),* $(,)?) => {$(
        impl TryClone for $t {
            fn try_clone(&self) -> MemResult<Self> {
                Ok(*self)
            }
        }
    )*};
}

copy_try_clone!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64, bool, char, ());

macro_rules! tuple_try_clone {
    ($(($($name:ident $idx:tt),+)),+ $(,)?) => {$(
        impl<$($name: TryClone),+> TryClone for ($($name,)+) {
            fn try_clone(&self) -> MemResult<Self> {
                Ok(($(self.$idx.try_clone()?,)+))
            }
        }
    )+};
}

tuple_try_clone!((A 0), (A 0, B 1), (A 0, B 1, C 2), (A 0, B 1, C 2, D 3));

impl<T: TryClone> TryClone for Option<T> {
    fn try_clone(&self) -> MemResult<Self> {
        match self {
            Some(v) => Ok(Some(v.try_clone()?)),
            None => Ok(None),
        }
    }
}

impl<T: TryClone> TryClone for KVec<T> {
    fn try_clone(&self) -> MemResult<Self> {
        let mut out = KVec::with_capacity_in(self.len(), self.budget())?;
        for item in self.as_slice() {
            out.try_push(item.try_clone()?)?;
        }
        Ok(out)
    }
}

impl TryClone for KString {
    fn try_clone(&self) -> MemResult<Self> {
        KString::try_from_str_in(self.as_str(), self.budget())
    }
}
