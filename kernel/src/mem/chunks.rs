//! `KChunks` (spec L1a-2 5.3): a growable byte sequence stored in chunks of 64 KiB, so that files, output
//! buffers and pipeline data never live in one contiguous growing block.
//!
//! Layout: `dir` holds one pointer per chunk. All chunks but the last are full (`CHUNK` bytes); the last holds
//! the tail in a block of `last_cap` bytes, the used length rounded up to a power of two (at least 64), so a
//! small sequence costs a small block. `head` is the number of bytes already drained from the front of the
//! first chunk. The bytes are `[head, head + len)` of the chunk space.
//!
//! Failure atomicity: capacity is reserved first (all new blocks allocated before the old last block grows),
//! and the copy that follows cannot fail.
use super::budget::Budget;
use super::clone::TryClone;
use super::error::{MemError, MemResult};
use super::vec::KVec;
use alloc::alloc::{alloc, dealloc, realloc, Layout};
use core::ptr::{self, NonNull};

/// Growable byte sequence stored in chunks; never reallocates existing full chunks. Not `Send`.
pub struct KChunks {
    dir: KVec<NonNull<u8>>,
    head: usize,
    len: u64,
    last_cap: usize,
    budget: Budget,
}

const MIN_BLOCK: usize = 64;

fn cap_for(used: usize) -> usize {
    used.next_power_of_two().max(MIN_BLOCK)
}

fn layout(cap: usize) -> Layout {
    // SAFETY: `cap` is at least 64 and a power of two or `CHUNK`, so the size is non-zero and the alignment 1.
    unsafe { Layout::from_size_align_unchecked(cap, 1) }
}

impl KChunks {
    /// Bytes per full chunk.
    pub const CHUNK: usize = 65_536;

    /// An empty sequence on `budget`. Allocates nothing.
    pub fn new_in(budget: &Budget) -> KChunks {
        KChunks { dir: KVec::new_in(budget), head: 0, len: 0, last_cap: 0, budget: budget.clone() }
    }

    /// Number of bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The budget this sequence charges.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    fn end(&self) -> u64 {
        self.head as u64 + self.len
    }

    fn block_alloc(&self, cap: usize) -> MemResult<NonNull<u8>> {
        self.budget.charge(cap as u64)?;
        // SAFETY: `cap` is non-zero.
        let p = unsafe { alloc(layout(cap)) };
        match NonNull::new(p) {
            Some(p) => Ok(p),
            None => {
                self.budget.release(cap as u64);
                Err(MemError::Ceiling)
            }
        }
    }

    fn block_free(&self, p: NonNull<u8>, cap: usize) {
        // SAFETY: `p` is a live block of `cap` bytes obtained from `block_alloc`/`block_resize`.
        unsafe { dealloc(p.as_ptr(), layout(cap)) };
        self.budget.release(cap as u64);
    }

    /// Grows a block to `new` bytes (charging first).
    fn block_grow(&self, p: NonNull<u8>, old: usize, new: usize) -> MemResult<NonNull<u8>> {
        self.budget.charge((new - old) as u64)?;
        // SAFETY: `p` is a live block of `old` bytes; `new` is larger and non-zero.
        let q = unsafe { realloc(p.as_ptr(), layout(old), new) };
        match NonNull::new(q) {
            Some(q) => Ok(q),
            None => {
                self.budget.release((new - old) as u64);
                Err(MemError::Ceiling)
            }
        }
    }

    /// Shrinks a block to `new` bytes; returns the block and its capacity (unchanged if the allocator refused).
    fn block_shrink(&self, p: NonNull<u8>, old: usize, new: usize) -> (NonNull<u8>, usize) {
        if new >= old {
            return (p, old);
        }
        // SAFETY: `p` is a live block of `old` bytes; `new` is smaller and non-zero.
        let q = unsafe { realloc(p.as_ptr(), layout(old), new) };
        match NonNull::new(q) {
            Some(q) => {
                self.budget.release((old - new) as u64);
                (q, new)
            }
            None => (p, old),
        }
    }

    /// Makes room for `new_end` bytes of chunk space. On failure nothing changed.
    fn reserve_to(&mut self, new_end: u64) -> MemResult<()> {
        if new_end <= self.end() && !self.dir.is_empty() {
            return Ok(());
        }
        let chunk = Self::CHUNK as u64;
        let n_old = self.dir.len();
        let n_new = new_end.div_ceil(chunk);
        if n_new > (usize::MAX / 32) as u64 {
            return Err(MemError::Overflow);
        }
        let n_new = n_new as usize;
        if n_new == 0 {
            return Ok(());
        }
        let used_last = (new_end - (n_new as u64 - 1) * chunk) as usize;
        let cap_last = cap_for(used_last);
        if n_new == n_old {
            if cap_last > self.last_cap {
                let last = self.dir.len() - 1;
                let p = self.block_grow(self.dir[last], self.last_cap, cap_last)?;
                self.dir.as_mut_slice()[last] = p;
                self.last_cap = cap_last;
            }
            return Ok(());
        }
        let add = n_new - n_old;
        if n_old + add <= 8 {
            self.dir.try_reserve_exact(add)?;
        } else {
            self.dir.try_reserve(add)?;
        }
        for k in 0..add {
            let cap = if k + 1 == add { cap_last } else { Self::CHUNK };
            match self.block_alloc(cap) {
                Ok(p) => {
                    // Room was reserved, so the push cannot fail.
                    let _ = self.dir.try_push(p);
                }
                Err(e) => {
                    self.free_blocks_from(n_old, |_| Self::CHUNK);
                    self.dir.shrink_to_fit();
                    return Err(e);
                }
            }
        }
        if n_old > 0 && self.last_cap < Self::CHUNK {
            let last = n_old - 1;
            match self.block_grow(self.dir[last], self.last_cap, Self::CHUNK) {
                Ok(p) => self.dir.as_mut_slice()[last] = p,
                Err(e) => {
                    self.free_blocks_from(n_old, |i| if i + 1 == n_new { cap_last } else { Self::CHUNK });
                    self.dir.shrink_to_fit();
                    return Err(e);
                }
            }
        }
        self.last_cap = cap_last;
        Ok(())
    }

    /// Frees and removes the blocks from index `from` on; `cap_of` gives the capacity of block `i`.
    fn free_blocks_from(&mut self, from: usize, cap_of: impl Fn(usize) -> usize) {
        while self.dir.len() > from {
            let i = self.dir.len() - 1;
            let p = self.dir.pop().expect("non-empty");
            self.block_free(p, cap_of(i));
        }
    }

    fn cap_of(&self, i: usize) -> usize {
        if i + 1 == self.dir.len() {
            self.last_cap
        } else {
            Self::CHUNK
        }
    }

    /// Copies `data` to chunk-space position `abs` (capacity must be reserved).
    fn write_abs(&mut self, mut abs: u64, mut data: &[u8]) {
        while !data.is_empty() {
            let (i, off) = ((abs / Self::CHUNK as u64) as usize, (abs % Self::CHUNK as u64) as usize);
            let n = data.len().min(Self::CHUNK - off);
            // SAFETY: reserved capacity covers chunk `i` up to `off + n` (the chunk is full-size, or the last
            // block was sized for the new end), and `data` cannot overlap the chunk storage owned by `self`.
            unsafe { ptr::copy_nonoverlapping(data.as_ptr(), self.dir[i].as_ptr().add(off), n) };
            abs += n as u64;
            data = &data[n..];
        }
    }

    fn zero_abs(&mut self, mut abs: u64, mut count: u64) {
        while count > 0 {
            let (i, off) = ((abs / Self::CHUNK as u64) as usize, (abs % Self::CHUNK as u64) as usize);
            let n = (count.min((Self::CHUNK - off) as u64)) as usize;
            // SAFETY: as for `write_abs`.
            unsafe { ptr::write_bytes(self.dir[i].as_ptr().add(off), 0, n) };
            abs += n as u64;
            count -= n as u64;
        }
    }

    /// Appends `data`. Fails with `Limit(id)` or `Ceiling` (or `Overflow` past 2^64 bytes); the content is
    /// unchanged on failure.
    pub fn try_append(&mut self, data: &[u8]) -> MemResult<()> {
        let at = self.end();
        let new_end = at.checked_add(data.len() as u64).ok_or(MemError::Overflow)?;
        self.reserve_to(new_end)?;
        self.write_abs(at, data);
        self.len += data.len() as u64;
        Ok(())
    }

    /// Writes at `offset`, extending with zero bytes if `offset` is past the end. Fails like `try_append`; the
    /// content is unchanged on failure.
    pub fn try_write_at(&mut self, offset: u64, data: &[u8]) -> MemResult<()> {
        let end = offset.checked_add(data.len() as u64).ok_or(MemError::Overflow)?;
        if end > self.len {
            let abs_end = (self.head as u64).checked_add(end).ok_or(MemError::Overflow)?;
            self.reserve_to(abs_end)?;
            if offset > self.len {
                let from = self.end();
                self.zero_abs(from, offset - self.len);
            }
            self.len = self.len.max(end);
        }
        self.write_abs(self.head as u64 + offset, data);
        Ok(())
    }

    /// Copies up to `out.len()` bytes starting at `offset`; returns the count (0 at or past the end).
    pub fn read_at(&self, offset: u64, out: &mut [u8]) -> usize {
        if offset >= self.len {
            return 0;
        }
        let total = (out.len() as u64).min(self.len - offset) as usize;
        let mut abs = self.head as u64 + offset;
        let mut done = 0;
        while done < total {
            let (i, off) = ((abs / Self::CHUNK as u64) as usize, (abs % Self::CHUNK as u64) as usize);
            let n = (total - done).min(Self::CHUNK - off);
            // SAFETY: `abs + n` is within the stored bytes, which lie inside chunk `i`'s block.
            unsafe { ptr::copy_nonoverlapping(self.dir[i].as_ptr().add(off), out.as_mut_ptr().add(done), n) };
            abs += n as u64;
            done += n;
        }
        total
    }

    /// Shortens to `len` bytes; no effect if already shorter. Releases the chunks beyond.
    pub fn truncate(&mut self, len: u64) {
        if len >= self.len {
            return;
        }
        if len == 0 {
            self.clear();
            return;
        }
        let chunk = Self::CHUNK as u64;
        let new_end = self.head as u64 + len;
        let n_new = new_end.div_ceil(chunk) as usize;
        let old_last = self.dir.len() - 1;
        let old_cap_last = self.last_cap;
        self.free_blocks_from(n_new, |i| if i == old_last { old_cap_last } else { Self::CHUNK });
        let used_last = (new_end - (n_new as u64 - 1) * chunk) as usize;
        let cur = if n_new - 1 == old_last { old_cap_last } else { Self::CHUNK };
        let (p, cap) = self.block_shrink(self.dir[n_new - 1], cur, cap_for(used_last));
        self.dir.as_mut_slice()[n_new - 1] = p;
        self.last_cap = cap;
        self.len = len;
    }

    /// Removes every byte and releases all storage.
    pub fn clear(&mut self) {
        let last = self.dir.len().wrapping_sub(1);
        let cap_last = self.last_cap;
        self.free_blocks_from(0, |i| if i == last { cap_last } else { Self::CHUNK });
        self.dir.shrink_to_fit();
        self.head = 0;
        self.len = 0;
        self.last_cap = 0;
    }

    /// Removes up to `max` bytes from the front, passing them to `sink` in order, slice by slice. Returns the
    /// number of bytes removed. Memory of fully consumed chunks is released.
    pub fn drain_front(&mut self, max: usize, mut sink: impl FnMut(&[u8])) -> usize {
        let m = (max as u64).min(self.len) as usize;
        if m == 0 {
            return 0;
        }
        let mut abs = self.head;
        let mut left = m;
        while left > 0 {
            let (i, off) = (abs / Self::CHUNK, abs % Self::CHUNK);
            let n = left.min(Self::CHUNK - off);
            // SAFETY: the bytes `[abs, abs + n)` are stored bytes inside chunk `i`'s block.
            sink(unsafe { core::slice::from_raw_parts(self.dir[i].as_ptr().add(off), n) });
            abs += n;
            left -= n;
        }
        self.len -= m as u64;
        if self.len == 0 {
            self.clear();
            return m;
        }
        self.head += m;
        while self.head >= Self::CHUNK {
            let p = self.dir.remove(0);
            self.block_free(p, Self::CHUNK);
            self.head -= Self::CHUNK;
        }
        if self.dir.len() == 1 && self.head > 0 {
            // A single partial chunk: slide the content to the front and shrink the block to fit.
            let used = self.len as usize;
            // SAFETY: `[head, head + used)` and `[0, used)` lie inside the block; `copy` handles the overlap.
            unsafe { ptr::copy(self.dir[0].as_ptr().add(self.head), self.dir[0].as_ptr(), used) };
            self.head = 0;
            let (p, cap) = self.block_shrink(self.dir[0], self.last_cap, cap_for(used));
            self.dir.as_mut_slice()[0] = p;
            self.last_cap = cap;
        }
        m
    }

    /// The stored bytes as consecutive slices.
    pub fn chunks(&self) -> impl Iterator<Item = &[u8]> {
        let n = self.dir.len();
        let end = self.end();
        (0..n).map(move |i| {
            let start = if i == 0 { self.head } else { 0 };
            let stop = if i + 1 == n { (end - (i as u64) * Self::CHUNK as u64) as usize } else { Self::CHUNK };
            // SAFETY: `[start, stop)` is within chunk `i`'s block and holds stored bytes; the shared borrow of
            // `self` keeps them alive and unmodified.
            unsafe { core::slice::from_raw_parts(self.dir[i].as_ptr().add(start), stop - start) }
        })
    }

    #[doc(hidden)]
    pub fn chunk_count(&self) -> usize {
        self.dir.len()
    }

    #[doc(hidden)]
    pub fn capacity_of_chunk(&self, i: usize) -> usize {
        self.cap_of(i)
    }
}

impl Drop for KChunks {
    fn drop(&mut self) {
        self.clear();
    }
}

impl core::fmt::Debug for KChunks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KChunks").field("len", &self.len).finish()
    }
}

impl TryClone for KChunks {
    fn try_clone(&self) -> MemResult<Self> {
        let mut out = KChunks::new_in(&self.budget);
        for s in self.chunks() {
            out.try_append(s)?;
        }
        Ok(out)
    }
}
