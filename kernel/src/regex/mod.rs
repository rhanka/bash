//! Linear-time regex engine (lot L8a, EVOL D12).
//!
//! Thompson NFA compiled from ERE, BRE (`Grep`/`Sed`/`Expr`), `Fixed` and Perl-subset
//! patterns, executed by an ordered PikeVM simulation (`O(|input| * |insts|)`, zero
//! backtracking) paired with an on-demand byte-capped lazy DFA cache for capture-free
//! scans, plus a fuel-charged bounded backtracker for `\1`..`\9` when
//! [`RegexFlags::allow_backrefs`] is set. All heap allocations go through [`Budget`].
//! Matching steps over Unicode scalars (one scalar per atom, one byte for invalid
//! UTF-8 lead bytes), with ASCII/Unicode case folding under `ignore_case`.

mod compiler;
mod engine;
mod parser;

#[cfg(test)]
mod tests;

pub(crate) use engine::{char_matches_icase, decode_scalar, RAW_BYTE_BASE};
pub(crate) use parser::posix_ranges;

use crate::mem::{Budget, KBytes, KVec, MemError};

pub const MAX_PROGRAM_INSTS: usize = 65_536;
pub const MAX_REPEAT_EXPANSION: u32 = 1_000;
pub const MAX_NESTING_DEPTH: u32 = 200;
pub const MAX_CAPTURE_GROUPS: u32 = 100;
pub const MAX_DFA_CACHE_BYTES: usize = 262_144;
pub const MAX_BACKREF_VISITED_STATES: usize = 262_144;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegexMode {
    Extended,
    Basic(BreDialect),
    Fixed,
    Perl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreDialect {
    Grep,
    Sed,
    Expr,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RegexFlags {
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub whole_word: bool,
    pub line_regexp: bool,
    pub allow_backrefs: bool,
    /// `sed` only (`sed` BRE `\{,m\}` and `sed -E` `{,m}`): a comma-first interval
    /// means `{0,m}`. Everywhere else `{,m}` is literal text.
    pub comma_first_interval: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CompileLimits {
    pub max_program_insts: usize,
    pub max_repeat_expansion: u32,
    pub max_nesting_depth: u32,
    pub max_capture_groups: u32,
    pub max_dfa_cache_bytes: usize,
}

impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            max_program_insts: MAX_PROGRAM_INSTS,
            max_repeat_expansion: MAX_REPEAT_EXPANSION,
            max_nesting_depth: MAX_NESTING_DEPTH,
            max_capture_groups: MAX_CAPTURE_GROUPS,
            max_dfa_cache_bytes: MAX_DFA_CACHE_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxKind {
    UnclosedGroup,
    UnmatchedCloseParen,
    UnclosedBracket,
    InvalidCharacterRange,
    UnknownPosixClass,
    InvalidRepeatRange,
    DanglingQuantifier,
    TrailingBackslash,
    InvalidEscape,
    DuplicateCaptureName,
    InvalidCaptureName,
    LookaroundUnsupported,
    BackrefUnsupported(u8),
    InvalidBackref(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapKind {
    ProgramTooLarge { limit: usize },
    RepeatTooLarge { limit: u32 },
    NestingTooDeep { limit: u32 },
    TooManyCaptures { limit: u32 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegexError {
    Syntax(SyntaxKind),
    CompileCap(CapKind),
    Mem(MemError),
}

impl From<MemError> for RegexError {
    fn from(e: MemError) -> Self {
        Self::Mem(e)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MatchError {
    FuelExhausted,
    BacktrackLimitExceeded,
    Mem(MemError),
}

impl From<MemError> for MatchError {
    fn from(e: MemError) -> Self {
        Self::Mem(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

pub struct Captures {
    slots: KVec<Option<Span>>,
}

impl Captures {
    pub fn full(&self) -> Span {
        self.slots.as_slice()[0].unwrap_or(Span { start: 0, end: 0 })
    }

    pub fn group(&self, index: usize) -> Option<Span> {
        self.slots.as_slice().get(index).copied().flatten()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prefilter<'a> {
    None,
    Exact(&'a [u8]),
    Substring(&'a [u8]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyAdvance {
    Standard,
    PosixSed,
    ShellExpansion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceStyle {
    Sed,
    Dollar,
    Literal,
}

pub struct CompiledRegex {
    insts: KVec<compiler::Inst>,
    classes: KVec<parser::CharClass>,
    names: KVec<(KBytes, u32)>,
    prefilter_bytes: KBytes,
    prefilter_kind: u8, // 0 = none, 1 = exact, 2 = substring
    groups: u32,
    has_backrefs: bool,
    dfa: engine::DfaCache,
}

impl CompiledRegex {
    pub fn compile(
        pattern: &[u8],
        mode: RegexMode,
        flags: RegexFlags,
        limits: CompileLimits,
        b: &Budget,
    ) -> Result<Self, RegexError> {
        let parsed = parser::parse(pattern, mode, flags, limits, b)?;
        let compiled = compiler::compile(&parsed, flags, limits, b)?;
        Ok(Self {
            insts: compiled.insts,
            classes: parsed.classes,
            names: parsed.names,
            prefilter_bytes: compiled.prefilter_bytes,
            prefilter_kind: compiled.prefilter_kind,
            groups: parsed.groups,
            has_backrefs: parsed.has_backrefs,
            dfa: engine::DfaCache::new_in(limits.max_dfa_cache_bytes, b),
        })
    }

    pub fn capture_count(&self) -> usize {
        self.groups as usize
    }

    pub fn group_index(&self, name: &[u8]) -> Option<usize> {
        self.names
            .as_slice()
            .iter()
            .find(|(n, _)| n.as_slice() == name)
            .map(|(_, i)| *i as usize)
    }

    pub fn has_backrefs(&self) -> bool {
        self.has_backrefs
    }

    pub fn dfa_cached_bytes(&self) -> usize {
        self.dfa.cached_bytes()
    }

    pub fn prefilter(&self) -> Prefilter<'_> {
        match self.prefilter_kind {
            1 => Prefilter::Exact(self.prefilter_bytes.as_slice()),
            2 => Prefilter::Substring(self.prefilter_bytes.as_slice()),
            _ => Prefilter::None,
        }
    }

    fn exec_ctx<'x>(
        &'x mut self,
        haystack: &'x [u8],
        b: &'x Budget,
        work: &'x mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> engine::ExecCtx<'x> {
        let prefilter = match self.prefilter_kind {
            1 => Prefilter::Exact(self.prefilter_bytes.as_slice()),
            2 => Prefilter::Substring(self.prefilter_bytes.as_slice()),
            _ => Prefilter::None,
        };
        engine::ExecCtx {
            insts: self.insts.as_slice(),
            classes: self.classes.as_slice(),
            names: self.names.as_slice(),
            prefilter,
            groups: self.groups,
            has_backrefs: self.has_backrefs,
            haystack,
            dfa: &mut self.dfa,
            b,
            work,
        }
    }

    pub fn is_match(
        &mut self,
        haystack: &[u8],
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<bool, MatchError> {
        self.exec_ctx(haystack, b, work).is_match()
    }

    pub fn find_at(
        &mut self,
        haystack: &[u8],
        at: usize,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Option<Span>, MatchError> {
        self.exec_ctx(haystack, b, work).find_at(at)
    }

    pub fn captures_at(
        &mut self,
        haystack: &[u8],
        at: usize,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Option<Captures>, MatchError> {
        let Some(slots) = self.exec_ctx(haystack, b, work).captures_at(at)? else {
            return Ok(None);
        };
        Ok(Some(Captures { slots }))
    }

    pub fn find_all(
        &mut self,
        haystack: &[u8],
        policy: EmptyAdvance,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<KVec<Span>, MatchError> {
        self.exec_ctx(haystack, b, work).find_all(policy)
    }

    pub fn replace_all(
        &mut self,
        haystack: &[u8],
        replacement: &[u8],
        style: ReplaceStyle,
        nth: Option<usize>,
        policy: EmptyAdvance,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<(KVec<u8>, bool), MatchError> {
        self.exec_ctx(haystack, b, work)
            .replace_all(replacement, style, nth, policy)
    }

    #[cfg(test)]
    pub(crate) fn is_match_pikevm_only(
        &mut self,
        haystack: &[u8],
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<bool, MatchError> {
        self.exec_ctx(haystack, b, work).is_match_pikevm_only()
    }

    #[cfg(test)]
    pub(crate) fn is_match_dfa_only(
        &mut self,
        haystack: &[u8],
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Option<bool>, MatchError> {
        self.exec_ctx(haystack, b, work).is_match_dfa_only()
    }
}
