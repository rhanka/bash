//! `KInsertMap` (spec L1a-2 5.2): a map that iterates in insertion order, for shell variables and associative
//! arrays.
//!
//! Two B-trees: `index` maps a key to the sequence number it was inserted under, and `order` maps sequence
//! numbers (ascending, hence insertion order) to the entry. Overwriting finds the old sequence number and keeps
//! it; removal deletes from both trees. No tombstones, so removal closes the gap at once and memory in use is
//! always that of the live entries (two `KMap`s, each within twice its entries' size). Lookup, insert and remove
//! are O(log n); iteration is O(n). The key is stored twice (hence `K: TryClone`).
use super::budget::Budget;
use super::clone::TryClone;
use super::error::MemResult;
use super::map::KMap;
use core::borrow::Borrow;

/// Map that iterates in insertion order. Lookup, insert and remove are O(log n). Overwriting an existing key
/// keeps its original position. Removal leaves no gap in iteration. Not `Send`.
pub struct KInsertMap<K: Ord + TryClone, V> {
    index: KMap<K, u64>,
    order: KMap<u64, (K, V)>,
    next: u64,
}

impl<K: Ord + TryClone, V> KInsertMap<K, V> {
    /// An empty map on `budget`. Allocates nothing.
    pub fn new_in(budget: &Budget) -> KInsertMap<K, V> {
        KInsertMap { index: KMap::new_in(budget), order: KMap::new_in(budget), next: 0 }
    }

    /// Returns the previous value. On failure (`Limit(id)` or `Ceiling`) the map is unchanged; key and value
    /// are dropped. Replacing a value never allocates.
    pub fn try_insert(&mut self, key: K, value: V) -> MemResult<Option<V>> {
        if let Some(&seq) = self.index.get(&key) {
            let entry = self.order.get_mut(&seq).expect("every indexed key has an entry");
            return Ok(Some(core::mem::replace(&mut entry.1, value)));
        }
        let seq = self.next;
        self.order.try_insert(seq, (key, value))?;
        let cloned = self.order.get(&seq).expect("just inserted").0.try_clone();
        let indexed = match cloned {
            Ok(k) => self.index.try_insert(k, seq).map(|_| ()),
            Err(e) => Err(e),
        };
        if let Err(e) = indexed {
            self.order.remove(&seq);
            return Err(e);
        }
        self.next += 1;
        Ok(None)
    }

    /// The value for `key`.
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let seq = self.index.get(key)?;
        self.order.get(seq).map(|e| &e.1)
    }

    /// The value for `key`, mutably.
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let seq = *self.index.get(key)?;
        self.order.get_mut(&seq).map(|e| &mut e.1)
    }

    /// True when the map has `key`.
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.index.contains_key(key)
    }

    /// Removes `key` and returns its value; later entries keep their order.
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let seq = self.index.remove(key)?;
        self.order.remove(&seq).map(|e| e.1)
    }

    /// Number of pairs.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// True when the map is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Removes every pair and releases every node.
    pub fn clear(&mut self) {
        self.index.clear();
        self.order.clear();
    }

    /// Pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.order.values().map(|e| (&e.0, &e.1))
    }

    /// Pairs in insertion order, values mutable.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.order.iter_mut().map(|(_, e)| {
            let (k, v) = e;
            let k: &K = k;
            (k, v)
        })
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.order.values().map(|e| &e.0)
    }

    /// Values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.order.values().map(|e| &e.1)
    }

    /// The budget this map charges.
    pub fn budget(&self) -> &Budget {
        self.index.budget()
    }

    /// Number of tree nodes currently allocated (for the memory-bound test).
    #[doc(hidden)]
    pub fn node_count(&self) -> usize {
        self.index.node_count() + self.order.node_count()
    }
}

impl<K: Ord + TryClone + core::fmt::Debug, V: core::fmt::Debug> core::fmt::Debug for KInsertMap<K, V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Ord + TryClone, V: TryClone> TryClone for KInsertMap<K, V> {
    fn try_clone(&self) -> MemResult<Self> {
        let mut out = KInsertMap::new_in(self.budget());
        for (k, v) in self.iter() {
            out.try_insert(k.try_clone()?, v.try_clone()?)?;
        }
        Ok(out)
    }
}
