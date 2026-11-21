//! `KMap`: ordered map (spec L1a-1 5.6, L1a-2 section 6), a B-tree with nodes of up to 16 entries.
//!
//! Every non-root node holds between 8 and 16 entries, so the bytes charged to the budget (the node
//! allocations) stay within twice the entries' own size plus a small per-node header. Insert, lookup and remove
//! are O(log n); iteration is O(n) and allocates nothing. Hash tables are deliberately not used: keys come from
//! untrusted scripts and an ordered tree has no collision attack.
//!
//! Failure atomicity: `try_insert` finds, before touching the tree, how many nodes the insertion will split,
//! allocates exactly those nodes first, and only then mutates. Removal never allocates.
//!
//! Invariants relied on by the `unsafe` code: a `Node` points to a live allocation made by `alloc_node` with
//! the layout of `Leaf` (height 0) or `Internal` (height > 0, which starts with its `Leaf` part); entries
//! `0..len` are initialised; an internal node has edges `0..=len` initialised, each pointing to a node one
//! level lower; the tree owns every node and entry.
use super::alloc::alloc::{alloc, dealloc, Layout};
use super::budget::Budget;
use super::error::{MemError, MemResult};
use core::borrow::Borrow;
use core::cmp::Ordering;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::{self, NonNull};

const CAP: usize = 16;
const MIN: usize = CAP / 2;
/// Deepest tree an iterator or an insertion pool handles (8^20 entries: unreachable).
const MAX_HEIGHT: usize = 20;

#[repr(C)]
struct Leaf<K, V> {
    len: u16,
    keys: [MaybeUninit<K>; CAP],
    values: [MaybeUninit<V>; CAP],
}

#[repr(C)]
struct Internal<K, V> {
    leaf: Leaf<K, V>,
    edges: [MaybeUninit<NonNull<Leaf<K, V>>>; CAP + 1],
}

type Node<K, V> = NonNull<Leaf<K, V>>;

/// Ordered map charged to its budget one node at a time. Iteration is always in ascending key order.
/// Lookups, inserts and removals are O(log n). Not `Send`.
pub struct KMap<K: Ord, V> {
    root: Option<Node<K, V>>,
    height: usize,
    len: usize,
    nodes: usize,
    budget: Budget,
    _own: PhantomData<(K, V)>,
}

// ---- raw node access ----

unsafe fn nlen<K, V>(n: Node<K, V>) -> usize {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe { (*n.as_ptr()).len as usize }
}
unsafe fn set_nlen<K, V>(n: Node<K, V>, len: usize) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        (*n.as_ptr()).len = len as u16;
    }
}
unsafe fn key_ptr<K, V>(n: Node<K, V>, i: usize) -> *mut K {
    // SAFETY: the caller supplies a live node and an index at most CAP; no reference to the array is made.
    unsafe { ptr::addr_of_mut!((*n.as_ptr()).keys).cast::<K>().add(i) }
}

unsafe fn value_ptr<K, V>(n: Node<K, V>, i: usize) -> *mut V {
    // SAFETY: the caller supplies a live node and an index at most CAP; no reference to the array is made.
    unsafe { ptr::addr_of_mut!((*n.as_ptr()).values).cast::<V>().add(i) }
}

unsafe fn read_entry<K, V>(n: Node<K, V>, i: usize) -> (K, V) {
    // SAFETY: the caller transfers ownership of both initialized fields at this index.
    unsafe { (key_ptr(n, i).read(), value_ptr(n, i).read()) }
}

unsafe fn write_entry<K, V>(n: Node<K, V>, i: usize, (key, value): (K, V)) {
    // SAFETY: the caller owns the destination slot and has removed any previously owned entry.
    unsafe {
        key_ptr(n, i).write(key);
        value_ptr(n, i).write(value);
    }
}
unsafe fn edg<K, V>(n: Node<K, V>, i: usize) -> *mut Node<K, V> {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        ptr::addr_of_mut!((*n.cast::<Internal<K, V>>().as_ptr()).edges)
            .cast::<Node<K, V>>()
            .add(i)
    }
}

fn node_layout<K, V>(internal: bool) -> Layout {
    if internal {
        Layout::new::<Internal<K, V>>()
    } else {
        Layout::new::<Leaf<K, V>>()
    }
}

fn alloc_node<K, V>(budget: &Budget, internal: bool) -> MemResult<Node<K, V>> {
    let layout = node_layout::<K, V>(internal);
    budget.charge(layout.size() as u64)?;
    // SAFETY: `Leaf` and `Internal` have a non-zero size (the `len` field).
    let raw = unsafe { alloc(layout) } as *mut Leaf<K, V>;
    match NonNull::new(raw) {
        Some(n) => {
            // SAFETY: a fresh allocation of at least the size of a `Leaf`; only `len` needs initialising.
            unsafe { ptr::addr_of_mut!((*n.as_ptr()).len).write(0) };
            Ok(n)
        }
        None => {
            budget.release(layout.size() as u64);
            Err(MemError::Ceiling)
        }
    }
}

unsafe fn free_node<K, V>(budget: &Budget, n: Node<K, V>, internal: bool) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let layout = node_layout::<K, V>(internal);
        dealloc(n.as_ptr() as *mut u8, layout);
        budget.release(layout.size() as u64);
    }
}

/// Drops every entry below `n` and frees its nodes.
unsafe fn free_subtree<K, V>(budget: &Budget, n: Node<K, V>, height: usize) -> usize {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let len = nlen(n);
        let mut freed = 1;
        for i in 0..len {
            drop(read_entry(n, i));
        }
        if height > 0 {
            for i in 0..=len {
                freed += free_subtree(budget, *edg(n, i), height - 1);
            }
        }
        free_node(budget, n, height > 0);
        freed
    }
}

unsafe fn search<K: Ord + Borrow<Q>, V, Q: Ord + ?Sized>(
    n: Node<K, V>,
    key: &Q,
) -> Result<usize, usize> {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let (mut lo, mut hi) = (0, nlen(n));
        while lo < hi {
            let mid = (lo + hi) / 2;
            match (*key_ptr(n, mid)).borrow().cmp(key) {
                Ordering::Less => lo = mid + 1,
                Ordering::Equal => return Ok(mid),
                Ordering::Greater => hi = mid,
            }
        }
        Err(lo)
    }
}

/// Moves `count` entries `src[from..]` to `dst[to..]` (distinct nodes, or non-overlapping ranges).
unsafe fn move_entries<K, V>(
    src: Node<K, V>,
    from: usize,
    dst: Node<K, V>,
    to: usize,
    count: usize,
) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        ptr::copy_nonoverlapping(key_ptr(src, from), key_ptr(dst, to), count);
        ptr::copy_nonoverlapping(value_ptr(src, from), value_ptr(dst, to), count);
    }
}

/// Shifts initialized entries within a node. The source and destination may overlap.
unsafe fn shift_entries<K, V>(n: Node<K, V>, from: usize, to: usize, count: usize) {
    // SAFETY: the caller keeps both ranges within the arrays and moves ownership to the destination range.
    unsafe {
        ptr::copy(key_ptr(n, from), key_ptr(n, to), count);
        ptr::copy(value_ptr(n, from), value_ptr(n, to), count);
    }
}

unsafe fn move_edges<K, V>(src: Node<K, V>, from: usize, dst: Node<K, V>, to: usize, count: usize) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        ptr::copy_nonoverlapping(edg(src, from), edg(dst, to), count);
    }
}

/// Inserts `entry` at `pos` of a node with room, with `right` as the edge just after it (internal nodes).
unsafe fn insert_nonfull<K, V>(
    n: Node<K, V>,
    pos: usize,
    entry: (K, V),
    right: Option<Node<K, V>>,
) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let len = nlen(n);
        shift_entries(n, pos, pos + 1, len - pos);
        write_entry(n, pos, entry);
        if let Some(r) = right {
            ptr::copy(edg(n, pos + 1), edg(n, pos + 2), len - pos);
            edg(n, pos + 1).write(r);
        }
        set_nlen(n, len + 1);
    }
}

/// Inserts `entry` (and the edge `right` after it, for internal nodes) at `pos` of the node at `height`. A
/// full node is split using the sibling taken from `pool[height]`; the median entry and the new right sibling
/// are returned for the parent.
unsafe fn insert_here<K, V>(
    n: Node<K, V>,
    height: usize,
    pos: usize,
    entry: (K, V),
    right: Option<Node<K, V>>,
    pool: &mut [Option<Node<K, V>>; MAX_HEIGHT + 2],
) -> Option<((K, V), Node<K, V>)> {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        if nlen(n) < CAP {
            insert_nonfull(n, pos, entry, right);
            return None;
        }
        let sib = pool[height]
            .take()
            .expect("a sibling was preallocated for every split");
        let internal = height > 0;
        match pos.cmp(&MIN) {
            Ordering::Less => {
                let med = read_entry(n, MIN - 1);
                move_entries(n, MIN, sib, 0, CAP - MIN);
                if internal {
                    move_edges(n, MIN, sib, 0, CAP - MIN + 1);
                }
                set_nlen(sib, CAP - MIN);
                set_nlen(n, MIN - 1);
                insert_nonfull(n, pos, entry, right);
                Some((med, sib))
            }
            Ordering::Equal => {
                move_entries(n, MIN, sib, 0, CAP - MIN);
                if internal {
                    edg(sib, 0).write(right.expect("internal insert carries an edge"));
                    move_edges(n, MIN + 1, sib, 1, CAP - MIN);
                }
                set_nlen(sib, CAP - MIN);
                set_nlen(n, MIN);
                Some((entry, sib))
            }
            Ordering::Greater => {
                let med = read_entry(n, MIN);
                move_entries(n, MIN + 1, sib, 0, CAP - MIN - 1);
                if internal {
                    move_edges(n, MIN + 1, sib, 0, CAP - MIN);
                }
                set_nlen(sib, CAP - MIN - 1);
                set_nlen(n, MIN);
                insert_nonfull(sib, pos - MIN - 1, entry, right);
                Some((med, sib))
            }
        }
    }
}

unsafe fn insert_rec<K: Ord, V>(
    n: Node<K, V>,
    height: usize,
    entry: (K, V),
    pool: &mut [Option<Node<K, V>>; MAX_HEIGHT + 2],
) -> Option<((K, V), Node<K, V>)> {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let pos = match search::<K, V, K>(n, &entry.0) {
            Err(p) => p,
            Ok(_) => unreachable!("the key was checked absent"),
        };
        if height == 0 {
            return insert_here(n, 0, pos, entry, None, pool);
        }
        let child = *edg(n, pos);
        match insert_rec(child, height - 1, entry, pool) {
            None => None,
            Some((med, right)) => insert_here(n, height, pos, med, Some(right), pool),
        }
    }
}

/// Removes entry `i` of a leaf.
unsafe fn leaf_remove<K, V>(n: Node<K, V>, i: usize) -> (K, V) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let len = nlen(n);
        let out = read_entry(n, i);
        shift_entries(n, i + 1, i, len - i - 1);
        set_nlen(n, len - 1);
        out
    }
}

/// Restores the minimum fill of child `i` of `parent` (at `height`) after it lost an entry.
unsafe fn fix_child<K, V>(budget: &Budget, parent: Node<K, V>, height: usize, i: usize) -> usize {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        let child = *edg(parent, i);
        if nlen(child) >= MIN {
            return 0;
        }
        let internal = height > 1;
        let n = nlen(parent);
        if i > 0 && nlen(*edg(parent, i - 1)) > MIN {
            // Borrow from the left sibling through the separator.
            let left = *edg(parent, i - 1);
            let (lc, cc) = (nlen(left), nlen(child));
            shift_entries(child, 0, 1, cc);
            write_entry(child, 0, read_entry(parent, i - 1));
            write_entry(parent, i - 1, read_entry(left, lc - 1));
            if internal {
                ptr::copy(edg(child, 0), edg(child, 1), cc + 1);
                edg(child, 0).write(*edg(left, lc));
            }
            set_nlen(left, lc - 1);
            set_nlen(child, cc + 1);
            0
        } else if i < n && nlen(*edg(parent, i + 1)) > MIN {
            // Borrow from the right sibling through the separator.
            let right = *edg(parent, i + 1);
            let (rc, cc) = (nlen(right), nlen(child));
            write_entry(child, cc, read_entry(parent, i));
            write_entry(parent, i, read_entry(right, 0));
            shift_entries(right, 1, 0, rc - 1);
            if internal {
                edg(child, cc + 1).write(*edg(right, 0));
                ptr::copy(edg(right, 1), edg(right, 0), rc);
            }
            set_nlen(right, rc - 1);
            set_nlen(child, cc + 1);
            0
        } else {
            // Merge with a sibling (both are at the minimum): right node `j + 1` into left node `j`.
            let j = if i > 0 { i - 1 } else { i };
            let (left, right) = (*edg(parent, j), *edg(parent, j + 1));
            let (lc, rc) = (nlen(left), nlen(right));
            write_entry(left, lc, read_entry(parent, j));
            move_entries(right, 0, left, lc + 1, rc);
            if internal {
                move_edges(right, 0, left, lc + 1, rc + 1);
            }
            set_nlen(left, lc + 1 + rc);
            free_node(budget, right, internal);
            shift_entries(parent, j + 1, j, n - j - 1);
            ptr::copy(edg(parent, j + 2), edg(parent, j + 1), n - j - 1);
            set_nlen(parent, n - 1);
            1
        }
    }
}

/// Removes the largest entry below `n`. `freed` counts the nodes released.
unsafe fn remove_max<K, V>(
    budget: &Budget,
    n: Node<K, V>,
    height: usize,
    freed: &mut usize,
) -> (K, V) {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        if height == 0 {
            return leaf_remove(n, nlen(n) - 1);
        }
        let i = nlen(n);
        let out = remove_max(budget, *edg(n, i), height - 1, freed);
        *freed += fix_child(budget, n, height, i);
        out
    }
}

unsafe fn remove_rec<K: Ord + Borrow<Q>, V, Q: Ord + ?Sized>(
    budget: &Budget,
    n: Node<K, V>,
    height: usize,
    key: &Q,
    freed: &mut usize,
) -> Option<(K, V)> {
    // SAFETY: the caller upholds this function's contract (live nodes of the tree, indexes within the node
    // bounds, ranges that fit), which is what every operation below relies on.
    unsafe {
        match search::<K, V, Q>(n, key) {
            Ok(i) => {
                if height == 0 {
                    return Some(leaf_remove(n, i));
                }
                let pred = remove_max(budget, *edg(n, i), height - 1, freed);
                let old = read_entry(n, i);
                write_entry(n, i, pred);
                *freed += fix_child(budget, n, height, i);
                Some(old)
            }
            Err(i) => {
                if height == 0 {
                    return None;
                }
                let out = remove_rec(budget, *edg(n, i), height - 1, key, freed);
                if out.is_some() {
                    *freed += fix_child(budget, n, height, i);
                }
                out
            }
        }
    }
}

impl<K: Ord, V> KMap<K, V> {
    /// An empty map on `budget`. Allocates nothing.
    pub fn new_in(budget: &Budget) -> KMap<K, V> {
        KMap {
            root: None,
            height: 0,
            len: 0,
            nodes: 0,
            budget: budget.clone(),
            _own: PhantomData,
        }
    }

    /// Inserts a pair and returns the previous value for the key. Replacing a value never allocates.
    /// On failure (`Limit(id)` or `Ceiling`) key and value are dropped and the map is unchanged.
    pub fn try_insert(&mut self, key: K, value: V) -> MemResult<Option<V>> {
        let Some(root) = self.root else {
            let n = alloc_node::<K, V>(&self.budget, false)?;
            // SAFETY: `n` is a fresh empty leaf with room for one entry.
            unsafe { insert_nonfull(n, 0, (key, value), None) };
            self.root = Some(n);
            self.nodes = 1;
            self.len = 1;
            return Ok(None);
        };
        // Scan: replace in place when the key exists, else count the nodes that will split.
        let mut run = 0;
        let mut node = root;
        let mut h = self.height;
        loop {
            // SAFETY: `node` is a live node of this tree at height `h`.
            unsafe {
                match search::<K, V, K>(node, &key) {
                    Ok(i) => return Ok(Some(value_ptr(node, i).replace(value))),
                    Err(i) => {
                        run = if nlen(node) == CAP { run + 1 } else { 0 };
                        if h == 0 {
                            break;
                        }
                        node = *edg(node, i);
                        h -= 1;
                    }
                }
            }
        }
        // Preallocate: one sibling per splitting node, plus a new root when the root splits.
        let new_root = run == self.height + 1;
        let want = run + usize::from(new_root);
        let mut pool: [Option<Node<K, V>>; MAX_HEIGHT + 2] = [None; MAX_HEIGHT + 2];
        for level in 0..want {
            match alloc_node::<K, V>(&self.budget, level > 0) {
                Ok(n) => pool[level] = Some(n),
                Err(e) => {
                    for (l, slot) in pool.iter().enumerate() {
                        if let Some(n) = slot {
                            // SAFETY: allocated just above by `alloc_node` with this kind, still empty.
                            unsafe { free_node(&self.budget, *n, l > 0) };
                        }
                    }
                    return Err(e);
                }
            }
        }
        // SAFETY: the tree is intact; the pool holds exactly the nodes the splits will consume.
        let split = unsafe { insert_rec(root, self.height, (key, value), &mut pool) };
        if let Some((med, right)) = split {
            let top = pool[self.height + 1]
                .take()
                .expect("a new root was preallocated");
            // SAFETY: `top` is a fresh empty internal node; the old root and `right` are at the same height.
            unsafe {
                write_entry(top, 0, med);
                edg(top, 0).write(root);
                edg(top, 1).write(right);
                set_nlen(top, 1);
            }
            self.root = Some(top);
            self.height += 1;
        }
        self.nodes += want;
        self.len += 1;
        Ok(None)
    }

    /// The value for `key`.
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let mut node = self.root?;
        let mut h = self.height;
        loop {
            // SAFETY: `node` is a live node of this tree at height `h`; the shared borrow of the map keeps
            // it alive and unmodified for the returned lifetime.
            unsafe {
                match search::<K, V, Q>(node, key) {
                    Ok(i) => return Some(&*value_ptr(node, i)),
                    Err(i) => {
                        if h == 0 {
                            return None;
                        }
                        node = *edg(node, i);
                        h -= 1;
                    }
                }
            }
        }
    }

    /// The value for `key`, mutably.
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let mut node = self.root?;
        let mut h = self.height;
        loop {
            // SAFETY: as for `get`, with the exclusive borrow of the map.
            unsafe {
                match search::<K, V, Q>(node, key) {
                    Ok(i) => return Some(&mut *value_ptr(node, i)),
                    Err(i) => {
                        if h == 0 {
                            return None;
                        }
                        node = *edg(node, i);
                        h -= 1;
                    }
                }
            }
        }
    }

    /// True when the map has `key`.
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }

    /// Removes `key` and returns its value. Never allocates; nodes left under the minimum fill are merged
    /// or refilled and the freed ones are released to the budget.
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let root = self.root?;
        let mut freed = 0;
        let (budget, height) = (&self.budget, self.height);
        // SAFETY: the tree is intact and `root` is at `self.height`.
        let removed = unsafe { remove_rec::<K, V, Q>(budget, root, height, key, &mut freed) };
        let out = removed?;
        self.len -= 1;
        self.nodes -= freed;
        // SAFETY: `root` is still live (only merges below it free nodes, and a merge never frees the root).
        unsafe {
            if nlen(root) == 0 {
                if self.height > 0 {
                    let child = *edg(root, 0);
                    free_node(&self.budget, root, true);
                    self.root = Some(child);
                    self.height -= 1;
                } else {
                    free_node(&self.budget, root, false);
                    self.root = None;
                }
                self.nodes -= 1;
            }
        }
        Some(out.1)
    }

    /// Number of pairs.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when the map is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes every pair and releases every node.
    pub fn clear(&mut self) {
        if let Some(root) = self.root.take() {
            // SAFETY: `root` was the live root at `self.height`; it is no longer reachable from the map.
            unsafe { free_subtree(&self.budget, root, self.height) };
        }
        self.height = 0;
        self.len = 0;
        self.nodes = 0;
    }

    fn raw_iter(&self) -> RawIter<K, V> {
        let mut it = RawIter {
            stack: [None; MAX_HEIGHT + 1],
            depth: 0,
            remaining: self.len,
            leaf: self.height + 1,
        };
        if let Some(root) = self.root {
            let mut node = root;
            let mut h = self.height;
            loop {
                it.stack[it.depth] = Some((node, 0));
                it.depth += 1;
                if h == 0 {
                    break;
                }
                // SAFETY: an internal node has edge 0.
                node = unsafe { *edg(node, 0) };
                h -= 1;
            }
        }
        it
    }

    /// Pairs in ascending key order.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        let it = self.raw_iter();
        // SAFETY: the shared borrow of the map keeps every entry alive and unmodified.
        it.map(|(key, value)| unsafe { (&*key, &*value) })
    }

    /// Pairs in ascending key order, values mutable.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        let it = self.raw_iter();
        // SAFETY: the exclusive borrow of the map gives access to each entry once; keys stay shared.
        it.map(|(key, value)| unsafe { (&*key, &mut *value) })
    }

    /// Keys in ascending order.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    /// Values in ascending key order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    /// The pair with the smallest key.
    pub fn first_key_value(&self) -> Option<(&K, &V)> {
        let mut node = self.root?;
        // SAFETY: walking edge 0 of internal nodes down to a leaf; the borrow keeps the tree alive.
        unsafe {
            for _ in 0..self.height {
                node = *edg(node, 0);
            }
            Some((&*key_ptr(node, 0), &*value_ptr(node, 0)))
        }
    }

    /// The pair with the largest key.
    pub fn last_key_value(&self) -> Option<(&K, &V)> {
        let mut node = self.root?;
        // SAFETY: walking the last edge of internal nodes down to a leaf; the borrow keeps the tree alive.
        unsafe {
            for _ in 0..self.height {
                node = *edg(node, nlen(node));
            }
            let i = nlen(node) - 1;
            Some((&*key_ptr(node, i), &*value_ptr(node, i)))
        }
    }

    /// The budget this map charges.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Number of nodes currently allocated (for the memory-bound test).
    #[doc(hidden)]
    pub fn node_count(&self) -> usize {
        self.nodes
    }
}

/// In-order traversal with an explicit stack: frame `d` is a node at height `height - d` and the index of the
/// next entry to yield from it.
struct RawIter<K, V> {
    stack: [Option<(Node<K, V>, usize)>; MAX_HEIGHT + 1],
    depth: usize,
    remaining: usize,
    leaf: usize,
}

impl<K, V> Iterator for RawIter<K, V> {
    type Item = (*mut K, *mut V);

    fn next(&mut self) -> Option<Self::Item> {
        // The leaf frame is the last pushed one; frames above it are internal.
        loop {
            if self.depth == 0 {
                return None;
            }
            let (node, i) = self.stack[self.depth - 1].expect("frame");
            // SAFETY: frames hold live nodes of a tree that the iterator's borrow keeps intact.
            unsafe {
                if i >= nlen(node) {
                    self.depth -= 1;
                    continue;
                }
                self.stack[self.depth - 1] = Some((node, i + 1));
                let item = (key_ptr(node, i), value_ptr(node, i));
                // An internal frame continues with the subtree to the right of the entry just yielded.
                if self.is_internal(self.depth - 1) {
                    let mut child = *edg(node, i + 1);
                    loop {
                        self.stack[self.depth] = Some((child, 0));
                        self.depth += 1;
                        if !self.is_internal(self.depth - 1) {
                            break;
                        }
                        child = *edg(child, 0);
                    }
                }
                self.remaining -= 1;
                return Some(item);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K, V> RawIter<K, V> {
    /// Frames are pushed root first, so a frame is a leaf exactly when it is the last of `leaf` frames.
    fn is_internal(&self, frame: usize) -> bool {
        frame + 1 < self.leaf
    }
}

impl<K: Ord, V> Drop for KMap<K, V> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<K: Ord + core::fmt::Debug, V: core::fmt::Debug> core::fmt::Debug for KMap<K, V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Ord + super::TryClone, V: super::TryClone> super::TryClone for KMap<K, V> {
    fn try_clone(&self) -> MemResult<Self> {
        let mut out = KMap::new_in(&self.budget);
        for (k, v) in self.iter() {
            out.try_insert(k.try_clone()?, v.try_clone()?)?;
        }
        Ok(out)
    }
}
