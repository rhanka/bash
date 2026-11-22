//! Linear-time glob and bash `extglob` matcher (lot L8a).
//!
//! Supports `*`, `?`, `[...]` (with `^` negation, literal `!`, literal `'`, 14 POSIX
//! classes where unknown `[:name:]` contributes zero characters, and `\` escapes), plus
//! full bash `extglob` (`@(...)`, `*(...)`, `+(...)`, `?(...)`, `!(...)` without regex
//! lookahead), `**` globstar segment capping, prefix/suffix stripping, leftmost-longest
//! glob replacement, and bracket-aware `GLOBIGNORE` splitting.

mod engine;
mod parser;

#[cfg(test)]
mod tests;

use crate::mem::{Budget, KVec, MemError};

pub const MAX_GLOBSTAR_SEGMENTS: usize = 5;
pub const DEFAULT_MAX_EXTGLOB_DEPTH: u32 = 100;

#[derive(Clone, Copy, Debug)]
pub struct GlobOptions {
    pub extglob: bool,
    pub nocase: bool,
    /// When `true` (pathname segment matching), `*`, `?`, `[...]`, and extglob groups
    /// never match `/`, and a leading `.` in the subject (or right after `/`) is matched
    /// only by a literal `.` unless `dotglob` is `true`.
    pub pathname: bool,
    pub dotglob: bool,
    pub max_extglob_depth: u32,
}

impl Default for GlobOptions {
    fn default() -> Self {
        Self {
            extglob: false,
            nocase: false,
            pathname: false,
            dotglob: false,
            max_extglob_depth: DEFAULT_MAX_EXTGLOB_DEPTH,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GlobError {
    TooManyGlobstars { max: usize },
    ExtglobDepthExceeded { max: u32 },
    InvalidCharacterRange,
    FuelExhausted,
    Mem(MemError),
}

impl From<MemError> for GlobError {
    fn from(e: MemError) -> Self {
        Self::Mem(e)
    }
}

pub struct CompiledGlob {
    pub(crate) opts: GlobOptions,
    pub(crate) parsed: parser::ParsedGlob,
    pub(crate) fwd: KVec<engine::GlobInst>,
    pub(crate) rev: KVec<engine::GlobInst>,
}

impl CompiledGlob {
    pub fn compile(
        pattern: &[u8],
        opts: GlobOptions,
        b: &Budget,
    ) -> Result<Self, GlobError> {
        let parsed = parser::parse_glob(pattern, opts, b)?;
        let (fwd, rev) = if parsed.has_not {
            (KVec::new_in(b), KVec::new_in(b))
        } else {
            (
                engine::compile_nfa(&parsed.root, false, b)?,
                engine::compile_nfa(&parsed.root, true, b)?,
            )
        };
        Ok(Self {
            opts,
            parsed,
            fwd,
            rev,
        })
    }

    pub fn is_match(
        &self,
        subject: &[u8],
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<bool, GlobError> {
        engine::is_match(self, subject, b, work)
    }
}

/// Validates that a slash-separated path pattern contains at most `MAX_GLOBSTAR_SEGMENTS` (5)
/// `**` path segments; returns `Err(GlobError::TooManyGlobstars { max: 5 })` when exceeded.
pub fn check_globstar_segments(path_pattern: &[u8]) -> Result<usize, GlobError> {
    parser::check_globstar_segments(path_pattern)
}

/// `${var#pat}` (`greedy: false`) and `${var##pat}` (`greedy: true`) in linear time.
/// On invalid range `[z-a]`, returns `Ok(None)` so the caller keeps `subject` unchanged.
pub fn strip_prefix(
    subject: &[u8],
    pattern: &[u8],
    greedy: bool,
    opts: GlobOptions,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    let compiled = match CompiledGlob::compile(pattern, opts, b) {
        Ok(c) => c,
        Err(GlobError::InvalidCharacterRange) => return Ok(None),
        Err(e) => return Err(e),
    };
    engine::strip_prefix(&compiled, subject, greedy, b, work)
}

/// `${var%pat}` (`greedy: false`, shortest suffix = rightmost start) and
/// `${var%%pat}` (`greedy: true`, longest suffix = leftmost start) in linear time.
pub fn strip_suffix(
    subject: &[u8],
    pattern: &[u8],
    greedy: bool,
    opts: GlobOptions,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError> {
    let compiled = match CompiledGlob::compile(pattern, opts, b) {
        Ok(c) => c,
        Err(GlobError::InvalidCharacterRange) => return Ok(None),
        Err(e) => return Err(e),
    };
    engine::strip_suffix(&compiled, subject, greedy, b, work)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobReplaceMode {
    First,
    All,
    AnchorHead,
    AnchorTail,
}

/// Computes `${var/pat/rep}` replacement using bash leftmost-longest greedy glob semantics.
pub fn replace_glob(
    subject: &[u8],
    pattern: &[u8],
    replacement: &[u8],
    mode: GlobReplaceMode,
    opts: GlobOptions,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, GlobError> {
    if subject.is_empty() {
        return Ok(KVec::new_in(b));
    }
    let compiled = match CompiledGlob::compile(pattern, opts, b) {
        Ok(c) => c,
        Err(GlobError::InvalidCharacterRange) => {
            let mut out = KVec::with_capacity_in(subject.len(), b)?;
            out.try_extend_from_slice(subject)?;
            return Ok(out);
        }
        Err(e) => return Err(e),
    };
    engine::replace_glob(&compiled, subject, replacement, mode, b, work)
}

/// Splits `GLOBIGNORE` on `:` outside bracket expressions (`[:alnum:]` stays intact),
/// dropping empty items.
pub fn split_globignore<'a>(raw: &'a [u8], b: &Budget) -> Result<KVec<&'a [u8]>, MemError> {
    parser::split_globignore(raw, b)
}
