//! `KString`: fallible UTF-8 text for identifiers and messages (spec 5.6). Shell data is `KBytes`.
use super::budget::Budget;
use super::error::{MemError, MemResult};
use super::vec::{KBytes, KVec};
use core::fmt;
use core::ops::Deref;

/// UTF-8 text on a budget; the bytes are charged like a `KVec<u8>`. Not `Send`.
pub struct KString {
    bytes: KVec<u8>,
    fmt_error: Option<MemError>,
}

impl KString {
    /// An empty string on `budget`. Allocates nothing.
    pub fn new_in(budget: &Budget) -> KString {
        KString { bytes: KVec::new_in(budget), fmt_error: None }
    }

    /// A copy of `text`, with exactly its length reserved. Fails with `Overflow`, `Limit(id)` or `Ceiling`.
    pub fn try_from_str_in(text: &str, budget: &Budget) -> MemResult<KString> {
        let mut s = KString::new_in(budget);
        s.try_push_str(text)?;
        Ok(s)
    }

    /// Appends a character. Fails like `try_push_str`; the string is unchanged on failure.
    pub fn try_push(&mut self, c: char) -> MemResult<()> {
        let mut buf = [0u8; 4];
        self.try_push_str(c.encode_utf8(&mut buf))
    }

    /// Appends `text`. Fails with `Overflow`, `Limit(id)` or `Ceiling`; the string is unchanged on failure.
    pub fn try_push_str(&mut self, text: &str) -> MemResult<()> {
        self.bytes.try_extend_from_slice(text.as_bytes())
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        // SAFETY: the bytes only ever come from `&str` and `char` encodings, so they are valid UTF-8.
        unsafe { core::str::from_utf8_unchecked(self.bytes.as_slice()) }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// True when the string is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Empties the string; the capacity stays.
    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    /// Shortens to `len` bytes. Panics if `len` is not on a character boundary. No effect if longer.
    pub fn truncate(&mut self, len: usize) {
        if len < self.len() {
            assert!(self.as_str().is_char_boundary(len), "truncate off a char boundary");
            self.bytes.truncate(len);
        }
    }

    /// The same bytes as a byte buffer (same budget, nothing reallocated).
    pub fn into_bytes(self) -> KBytes {
        self.bytes
    }

    /// The memory error behind the last failed `core::fmt::Write` call, if any; clears it.
    pub fn take_fmt_error(&mut self) -> Option<MemError> {
        self.fmt_error.take()
    }

    /// The budget this string charges.
    pub fn budget(&self) -> &Budget {
        self.bytes.budget()
    }
}

impl fmt::Write for KString {
    /// Appends fallibly. On failure returns `fmt::Error` and records the `MemError` for `take_fmt_error`.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        match self.try_push_str(s) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.fmt_error = Some(e);
                Err(fmt::Error)
            }
        }
    }
}

impl Deref for KString {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl core::borrow::Borrow<str> for KString {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for KString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for KString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl PartialEq for KString {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for KString {}

impl PartialOrd for KString {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KString {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}
