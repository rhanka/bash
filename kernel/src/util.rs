//! Small helpers over `kernel::mem`: byte copies, the environment table and message formatting.
use crate::mem::{Budget, KBytes, KMap, KRc, KString, KVec, MemError, MemResult, TryClone};
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

/// A shared map key with byte-slice lookup.
#[derive(Clone)]
pub(crate) struct Name(KRc<KBytes>);

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.bytes().cmp(other.bytes())
    }
}

impl core::borrow::Borrow<[u8]> for Name {
    fn borrow(&self) -> &[u8] {
        self.bytes()
    }
}

impl Name {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// The index stores stable sequence numbers, so removing an entry never needs to
/// reindex the keys that follow it. The vector keeps insertion order and lets a
/// pipeline resume cloning at its next slot without rescanning earlier entries.
pub struct OrderedNames<V> {
    index: KMap<Name, usize>,
    entries: KVec<(usize, Name, V)>,
}

impl<V> OrderedNames<V> {
    pub fn new_in(budget: &Budget) -> Self {
        Self { index: KMap::new_in(budget), entries: KVec::new_in(budget) }
    }

    pub fn budget(&self) -> &Budget {
        self.entries.budget()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn reserve_entries(&mut self, len: usize) -> MemResult<()> {
        self.entries.try_reserve_exact(len)
    }

    pub(crate) fn position(&self, key: &[u8]) -> Option<usize> {
        let seq = self.index.get(key)?;
        self.entries.binary_search_by_key(seq, |entry| entry.0).ok()
    }

    pub(crate) fn entry(&self, position: usize) -> Option<(&Name, &V)> {
        self.entries.get(position).map(|(_, key, value)| (key, value))
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = (&Name, &V)> {
        self.entries.iter().map(|(_, key, value)| (key, value))
    }

    pub fn get(&self, key: &[u8]) -> Option<&V> {
        self.position(key).map(|pos| &self.entries[pos].2)
    }

    pub(crate) fn replace_at(&mut self, pos: usize, value: V) -> V {
        core::mem::replace(&mut self.entries.as_mut_slice()[pos].2, value)
    }

    pub(crate) fn insert(&mut self, key: &[u8], value: V) -> MemResult<&Name> {
        let bytes = bytes_in(key, self.budget())?;
        let key = Name(KRc::try_new_in(bytes, self.budget()).map_err(|(_, e)| e)?);
        self.insert_shared(&key, value)
    }

    /// Appends a key known to be absent. Reserve both allocations before appending.
    pub(crate) fn insert_shared(&mut self, key: &Name, value: V) -> MemResult<&Name> {
        let seq = match self.entries.last() {
            None => 0,
            Some(entry) => entry.0.checked_add(1).ok_or(MemError::Overflow)?,
        };
        self.entries.try_reserve(1)?;
        let replaced = self.index.try_insert(key.clone(), seq)?;
        debug_assert!(replaced.is_none());
        self.entries.try_push((seq, key.clone(), value))?;
        Ok(&self.entries.last().expect("just appended").1)
    }

    pub(crate) fn removal_bytes(&self, key: &Name) -> usize {
        // Temporary variables are often still at the tail and share their
        // allocation with the undo entry. Removing that slot shifts no bytes.
        let at_tail = self.entries.last().is_some_and(|(_, last, _)| {
            core::ptr::eq(key.bytes(), last.bytes())
        });
        if at_tail { 0 } else { self.len() * core::mem::size_of::<(usize, Name, V)>() }
    }

    pub(crate) fn remove_at(&mut self, pos: usize) -> (Name, V) {
        let (_, name, value) = self.entries.remove(pos);
        self.index.remove(name.bytes());
        (name, value)
    }

    pub(crate) fn pop(&mut self) -> Option<(Name, V)> {
        let (_, name, value) = self.entries.pop()?;
        self.index.remove(name.bytes());
        Some((name, value))
    }
}

impl<V: Clone> TryClone for OrderedNames<V> {
    fn try_clone(&self) -> MemResult<Self> {
        let mut result = Self::new_in(self.budget());
        result.reserve_entries(self.len())?;
        for (key, value) in self.iter() {
            result.insert_shared(key, value.clone())?;
        }
        Ok(result)
    }
}

#[derive(Clone)]
pub struct EnvValue {
    pub value: KRc<KBytes>,
    pub exported: bool,
}

pub type Env = OrderedNames<EnvValue>;

impl Env {
    /// Host requests only need the insertion-order entries. Sharing their bytes
    /// avoids rebuilding a search index that the adapter will never use.
    pub(crate) fn snapshot(&self) -> MemResult<KVec<(KRc<KBytes>, EnvValue)>> {
        let mut vars = KVec::with_capacity_in(self.len(), self.budget())?;
        for (name, value) in self.iter() {
            vars.try_push((name.0.clone(), value.clone()))?;
        }
        Ok(vars)
    }

    /// Sets a variable, preserving its insertion position and name allocation.
    pub fn set(&mut self, name: &[u8], value: &[u8], exported: bool) -> MemResult<()> {
        let value = bytes_in(value, self.budget())?;
        let value = self.own_value(value, exported)?;
        self.replace_shared(name, value)
    }

    pub(crate) fn own_value(&self, value: KBytes, exported: bool) -> MemResult<EnvValue> {
        let value = KRc::try_new_in(value, self.budget()).map_err(|(_, e)| e)?;
        Ok(EnvValue { value, exported })
    }

    pub(crate) fn replace_shared(&mut self, name: &[u8], value: EnvValue) -> MemResult<()> {
        if let Some(pos) = self.position(name) {
            self.replace_at(pos, value);
        } else {
            self.insert(name, value)?;
        }
        Ok(())
    }

    pub fn copy_from(&mut self, source: &Self) -> MemResult<()> {
        for (name, entry) in source.iter() {
            self.set(name.bytes(), &entry.value, entry.exported)?;
        }
        Ok(())
    }

    /// Merge immutable entries whose byte allocations are already charged.
    pub(crate) fn merge_shared(&mut self, source: &Self) -> MemResult<()> {
        for (name, entry) in source.iter() {
            if let Some(pos) = self.position(name.bytes()) {
                self.replace_at(pos, entry.clone());
            } else {
                self.insert_shared(name, entry.clone())?;
            }
        }
        Ok(())
    }
}
