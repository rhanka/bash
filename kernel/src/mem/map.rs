//! `KMap`: ordered map (spec 5.6). In this lot a sorted vector with binary search; L1a-2 may replace the
//! implementation without changing a signature.
use super::budget::Budget;
use super::error::MemResult;
use super::vec::KVec;
use core::borrow::Borrow;
use core::cmp::Ordering;

/// Ordered map charged like a `KVec<(K, V)>`. Iteration is always in ascending key order. Lookups are
/// O(log n); inserts and removals are O(n) (shifting). Not `Send`.
pub struct KMap<K: Ord, V> {
    items: KVec<(K, V)>,
}

impl<K: Ord, V> KMap<K, V> {
    /// An empty map on `budget`. Allocates nothing.
    pub fn new_in(budget: &Budget) -> KMap<K, V> {
        KMap { items: KVec::new_in(budget) }
    }

    fn search<Q: Ord + ?Sized>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
    {
        self.items.binary_search_by(|(k, _)| match k.borrow().cmp(key) {
            Ordering::Less => Ordering::Less,
            Ordering::Equal => Ordering::Equal,
            Ordering::Greater => Ordering::Greater,
        })
    }

    /// Inserts a pair and returns the previous value for the key. Replacing a value never allocates.
    /// On failure (`Overflow`, `Limit(id)` or `Ceiling`) key and value are dropped and the map is unchanged.
    pub fn try_insert(&mut self, key: K, value: V) -> MemResult<Option<V>> {
        match self.search(&key) {
            Ok(i) => Ok(Some(core::mem::replace(&mut self.items.as_mut_slice()[i].1, value))),
            Err(i) => {
                self.items.try_insert(i, (key, value))?;
                Ok(None)
            }
        }
    }

    /// The value for `key`.
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.search(key).ok().map(|i| &self.items.as_slice()[i].1)
    }

    /// The value for `key`, mutably.
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        match self.search(key) {
            Ok(i) => Some(&mut self.items.as_mut_slice()[i].1),
            Err(_) => None,
        }
    }

    /// True when the map has `key`.
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.search(key).is_ok()
    }

    /// Removes `key` and returns its value. The capacity stays.
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        match self.search(key) {
            Ok(i) => Some(self.items.remove(i).1),
            Err(_) => None,
        }
    }

    /// Number of pairs.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when the map is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Removes every pair; the capacity stays.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Pairs in ascending key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.items.as_slice().iter().map(|e| (&e.0, &e.1))
    }

    /// Pairs in ascending key order, values mutable.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.items.as_mut_slice().iter_mut().map(|e| (&e.0, &mut e.1))
    }

    /// Keys in ascending order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.items.as_slice().iter().map(|e| &e.0)
    }

    /// Values in ascending key order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.items.as_slice().iter().map(|e| &e.1)
    }

    /// The pair with the smallest key.
    pub fn first_key_value(&self) -> Option<(&K, &V)> {
        self.items.as_slice().first().map(|e| (&e.0, &e.1))
    }

    /// The pair with the largest key.
    pub fn last_key_value(&self) -> Option<(&K, &V)> {
        self.items.as_slice().last().map(|e| (&e.0, &e.1))
    }

    /// The budget this map charges.
    pub fn budget(&self) -> &Budget {
        self.items.budget()
    }
}

impl<K: Ord + core::fmt::Debug, V: core::fmt::Debug> core::fmt::Debug for KMap<K, V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Ord + super::TryClone, V: super::TryClone> super::TryClone for KMap<K, V> {
    fn try_clone(&self) -> MemResult<Self> {
        Ok(KMap { items: self.items.try_clone()? })
    }
}

