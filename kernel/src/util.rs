//! Small helpers over `kernel::mem`: byte copies, the environment table and message formatting.
use crate::mem::{Budget, KBytes, KString, KVec, MemResult, TryClone};
use core::fmt::Write;

/// A copy of `s` with exactly its length reserved.
pub fn bytes_in(s: &[u8], b: &Budget) -> MemResult<KBytes> {
    let mut v = KBytes::with_capacity_in(s.len(), b)?;
    v.try_extend_from_slice(s)?;
    Ok(v)
}

/// A copy of `s` as text.
pub fn str_in(s: &str, b: &Budget) -> MemResult<KString> {
    KString::try_from_str_in(s, b)
}

/// Formats into a budgeted string; the closure writes with `write!`.
pub fn fmt_in(b: &Budget, f: impl FnOnce(&mut KString) -> core::fmt::Result) -> MemResult<KString> {
    let mut s = KString::new_in(b);
    if f(&mut s).is_err() {
        if let Some(e) = s.take_fmt_error() {
            return Err(e);
        }
    }
    Ok(s)
}

/// Appends the decimal text of `n`.
pub fn push_u64(out: &mut KString, n: u64) -> core::fmt::Result {
    write!(out, "{n}")
}

/// Parses an unsigned decimal; `None` when empty, not digits or too large.
pub fn parse_u64(s: &[u8]) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((c - b'0') as u64)?;
    }
    Some(n)
}

/// One variable of the shell environment. Names and values are bytes.
pub struct EnvEntry {
    pub name: KBytes,
    pub value: KBytes,
    pub exported: bool,
}

impl TryClone for EnvEntry {
    fn try_clone(&self) -> MemResult<Self> {
        Ok(EnvEntry { name: self.name.try_clone()?, value: self.value.try_clone()?, exported: self.exported })
    }
}

/// An ordered environment: insertion order is kept, lookups are linear (environments are small).
pub struct Env {
    pub v: KVec<EnvEntry>,
}

impl Env {
    pub fn new_in(b: &Budget) -> Env {
        Env { v: KVec::new_in(b) }
    }

    pub fn budget(&self) -> &Budget {
        self.v.budget()
    }

    pub fn get(&self, name: &[u8]) -> Option<&EnvEntry> {
        self.v.iter().find(|e| e.name.as_slice() == name)
    }

    /// Sets a variable: replaces the value of an existing one in place, or appends.
    pub fn set(&mut self, name: &[u8], value: &[u8], exported: bool) -> MemResult<()> {
        let b = self.v.budget().clone();
        let value = bytes_in(value, &b)?;
        if let Some(e) = self.v.iter_mut().find(|e| e.name.as_slice() == name) {
            e.value = value;
            e.exported = exported;
            return Ok(());
        }
        let name = bytes_in(name, &b)?;
        self.v.try_push(EnvEntry { name, value, exported })
    }

    pub fn remove(&mut self, name: &[u8]) {
        if let Some(i) = self.v.iter().position(|e| e.name.as_slice() == name) {
            self.v.remove(i);
        }
    }
}

impl TryClone for Env {
    fn try_clone(&self) -> MemResult<Self> {
        Ok(Env { v: self.v.try_clone()? })
    }
}
