# Lot L8a — Linear-time regex engine and glob/extglob matcher

Lot spec for the build lane (Room 2). Sources of authority: `spec/SPEC_EVOL_BASH_SANDBOX.md` ("EVOL":
decisions D1, D2, D6, D7, D12, D19-D21, D23; security invariants S3, S4, S9, S10; Annexes B, E, F, G) and
`plan/PLAN_BASH_SANDBOX_V1.md` (lot L8a, sections 4, 6 and 8). It builds on `spec/lots/L0.md` (gates G1-G8),
`spec/lots/L1a-1.md` and `spec/lots/L1a-2.md` (`kernel::mem` allocator, `Budget`, `KVec`, `KBox`, `KMap`),
`spec/lots/L1b.md`, `spec/lots/L2.md` (`kernel-test` and `kernel/src/fiber/testcmd.rs`), `spec/lots/L2b.md`,
and `spec/lots/L10a.md` (parity recorder and replay runner). Where this document and EVOL differ, stop and raise
a spec issue.

- **Size.** 8 points, Strong. **Builder.** Sonnet 5.5; fallback Gemini 4 Argon. **Review.** Carbon, full
  review of every PR, dispatched on a green `make check`.
- **Branch.** `feat/l8a-regex-glob`, worktree `tmp/l8a-regex-glob`.
- **Outcome.** Delivers two zero-dependency, `no_std`, budget-allocated kernel engines — `kernel::regex`
  (Thompson NFA / PikeVM with a byte-capped lazy DFA for uncaptured search, compile-time size/repetition/depth
  caps, fuel-charged bounded backtracker for `\1`..`\9` backreferences, BRE/ERE/Fixed/Perl modes, all 14 POSIX
  bracket classes plus `[[:<:]]`/`[[:>:]]`, replacement formatter, and literal prefilter extraction) and
  `kernel::glob` (linear-time glob and full bash extglob `@(...)`, `*(...)`, `+(...)`, `?(...)`, `!(...)`
  without regex lookahead, `**` segment-cap check, shortest/longest prefix and suffix stripping, pattern
  replacement spans, and `GLOBIGNORE` splitting) — wired to `kernel-test` (`__regex`, `__glob`) and proven on
  `just-bash@3.6.0` pattern snapshots (`test/cases/l8a/*`) and S9 blow-up gates.

## 1. Lane rules

1. No reference source and no reference package in the worktree. No network. The conductor (Room 1) writes the
   30 case files `test/cases/l8a/*.case.json` from section 7.3, records `test/cases/l8a/*.snap.json` against
   `just-bash@3.6.0`, and merges both to `main` before the worktree is created (PLAN section 6, steps 3-4). The
   builder never edits a committed case or snapshot and never runs or imports the reference engine.
2. Kernel: `no_std` on `wasm32-wasip2`, zero external crates in `Cargo.lock`, zero host `RegExp` calls across the
   WIT boundary (EVOL D12, G1), all heap allocations through `kernel::mem` against `&Budget`, `unsafe` only in
   `kernel/src/mem/` and `kernel/src/bindings/`.
3. Host and tests: plain ES modules with JSDoc, Node built-ins only in `tools/**` and `test/**` / test-only
   parity adapters, no Python and no Docker.
4. Never write to `/tmp`; scratch files go under the git-ignored `tmp/` of the worktree.
5. Commits through `make commit`, in English, without attribution trailers.

## 2. Scope

In scope:
- `kernel::regex` (`kernel/src/regex/**`):
  - Pattern parsers and compilers for **ERE** (`grep -E`, `awk`, `sed -E`, `[[ =~ ]]`), **BRE** (both `grep` BRE
    and `sed`/`expr` BRE dialects), **Fixed** (`grep -F` multi-line literal needles), and **Perl** (`grep -P`
    subset: `\d \D \w \W \s \S \b \B`, `\Q...\E`, `\xHH` / `\x{H..H}`, `\0`..`\777`, inline flags `(?i)` /
    `(?ims-ims:...)`, named captures `(?P<name>...)` and `(?<name>...)`, `\K` match-start reset, non-greedy
    quantifiers `*? +? ?? {n,m}?`, and explicit `LookaroundUnsupported` diagnostic on `(?=...)`, `(?!...)`,
    `(?<=...)`, `(?<!...)`).
  - Complete POSIX bracket expression parser (`[:alnum:]`, `[:alpha:]`, `[:ascii:]`, `[:blank:]`, `[:cntrl:]`,
    `[:digit:]`, `[:graph:]`, `[:lower:]`, `[:print:]`, `[:punct:]`, `[:space:]`, `[:upper:]`, `[:word:]`,
    `[:xdigit:]`, plus `[[:<:]]` and `[[:>:]]` word-boundary brackets), range validation, and ASCII/Unicode
    case-folding (`ignore_case`).
  - Compile-time structural caps (`MAX_PROGRAM_INSTS = 65_536`, `MAX_REPEAT_EXPANSION = 1_000`,
    `MAX_NESTING_DEPTH = 200`, `MAX_CAPTURE_GROUPS = 100`) returning typed `RegexError::CompileCap` with zero
    panics or exponential memory growth on nested quantifiers such as `a{1000}{1000}`.
  - Linear-time execution core: PikeVM (ordered Thompson NFA simulation tracking capture slots in `O(|input| * |insts|)`
    worst-case time with zero backtracking), paired with an on-demand lazy DFA cache capped at
    `MAX_DFA_CACHE_BYTES = 262_144` (256 KiB; flushed and rebuilt cleanly on cap reach, never failing a match)
    for fast `is_match` and uncaptured `find` scans, plus literal prefix/substring prefilter extraction for
    `grep`/`rg` (L9a-1, L16d).
  - Bounded backtracker for `\1`..`\9` backreferences when `RegexFlags::allow_backrefs` is `true` (EVOL D12):
    every step charges the caller's `work` fuel closure and consults a bounded visited table / step ceiling so
    catastrophic backreference inputs terminate deterministically within fuel without trapping. When
    `allow_backrefs` is `false` (default for `just-bash@3.6.0` parity where the reference engine rejects pattern
    backreferences), `\1`..`\9` in a pattern returns `RegexError::Syntax(SyntaxKind::BackrefUnsupported(n))`.
  - Match iteration (`find_iter`, `captures_iter`) and replacement helpers supporting all three zero-length
    advancement policies needed by downstream lots (`Standard` global scan, `PosixSed` post-non-empty skip, and
    `ShellExpansion` no-trailing-empty-after-non-empty).
- `kernel::glob` (`kernel/src/glob/**`):
  - Standalone linear-time glob and bash `extglob` compiler/matcher (`*`, `?`, `[...]` with `^` negation
    (`!` is a literal member in the reference), POSIX classes, `\` escapes, and `@(p1|p2)`, `*(...)`,
    `+(...)`, `?(...)`, `!(...)` including arbitrary suffixes such as `!(foo)bar` and nested `!(...)` without
    translating to regex negative lookahead!).
  - Pathname mode (`/` never matched by `*`, `?`, `[...]`, or extglob bodies) vs string mode (`[[ == ]]`,
    `case`, `${var#/%}`, `${var/pat/rep}` where `*` and `?` match `/`), `nocase`, `dot_protective` (leading `.`
    rule), `extglob` nesting cap (`max_extglob_depth`, default `100` or caller's `maxCallDepth`), and `**`
    globstar segment counter (`<= 5` `**` segments, returning `GlobError::TooManyGlobstars { max: 5 }`).
  - Linear-time `strip_prefix` (`#`, `##`), `strip_suffix` (`%`, `%%`), `replace` (`/`, `//`, `/#`, `/%`), and
    bracket-aware `split_globignore` (`:` separator preserving `[:class:]`).
- Test-only Wasm harness commands `__regex` and `__glob` in `kernel/src/fiber/testcmd.rs` (`kernel-test` only,
  zero bytes in production `kernel.core.wasm`), test parity adapter `harness/parity/l8a-adapter.mjs`, parity
  suite `test/parity/l8a.test.mjs`, S9 blow-up suite `test/l8a-redos.test.mjs`, and seeded fuzz slice.

Out of scope:
- Shell parser changes (L3, running in parallel in W4).
- VFS directory traversal for pathname expansion (`nullglob`, `failglob`, `dotglob`, `globskipdots` over VFS
  entries) and `BASH_REMATCH` shell array population (L4/L5, consuming `kernel::glob` and `kernel::regex`).
- Full `grep`, `rg`, `sed`, `awk`, and `expr` command implementations (L9a-1, L16d, L13, L14a, L9b).

## 3. Allowed paths

```
kernel/src/lib.rs              (add `pub mod glob;` and `pub mod regex;` only)
kernel/src/regex/**
kernel/src/glob/**
kernel/src/fiber/testcmd.rs    (add `__regex` and `__glob` under test-commands feature)
harness/parity/l8a-adapter.mjs
test/parity/l8a.test.mjs
test/l8a-*.test.mjs
BRANCH.md
```

Read-only for this lot: `kernel/src/mem/**`, `kernel/src/limits.rs`, `kernel/src/fiber.rs`,
`kernel/src/fiber/{syntax,vm,expand}.rs`, `kernel/src/cmd/**`, `host/**`, `types/**`, `wit/**`, `tools/**`,
`test/cases/l8a/**`, and `test/parity/exclusions.json`. No `BR<n>-EX<k>` scope exception is needed.

### 3.1 W4 merge touchpoints with L3

L8a and L3 run in parallel on disjoint files (see L3 section 3.1): L3 owns `kernel/src/fiber.rs` and
`kernel/src/fiber/{syntax,vm,expand}*`; L8a owns `kernel/src/lib.rs` (`pub mod glob; pub mod regex;`),
`kernel/src/regex/**`, `kernel/src/glob/**`, `kernel/src/fiber/testcmd.rs`, `harness/parity/l8a-adapter.mjs`,
and `test/l8a*` / `test/parity/l8a.test.mjs`. If L3 merges first, L8a rebases cleanly onto `main` with zero file
overlap and re-runs `make check`.

## 4. Exact interfaces (`kernel::regex` and `kernel::glob`)

### 4.1 `kernel::regex`

```rust
pub const MAX_PROGRAM_INSTS: usize = 65_536;
pub const MAX_REPEAT_EXPANSION: u32 = 1_000;
pub const MAX_NESTING_DEPTH: u32 = 200;
pub const MAX_CAPTURE_GROUPS: u32 = 100;
pub const MAX_DFA_CACHE_BYTES: usize = 262_144; // 256 KiB
pub const MAX_BACKREF_VISITED_STATES: usize = 262_144;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegexMode {
    Extended,               // ERE (`grep -E`, `awk`, `sed -E`, `[[ =~ ]]`)
    Basic(BreDialect),      // BRE (`grep` vs `sed`/`expr`)
    Fixed,                  // Literal line/needle list (`grep -F`, `\n`-separated)
    Perl,                   // Perl/PCRE subset (`grep -P`)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreDialect {
    /// `grep` default BRE: `\(...\)`, `\|`, `\{d,d\}` (first char after `\{` must be ASCII digit),
    /// `\+` and `\?` literal, and `*` at start or right after `^`, `\(`, `\|` literal.
    Grep,
    /// `sed` BRE: `\(`, `\)`, `\{n,m\}` and GNU `\{,m\}`, plus `\+`, `\?`, `\|`, `\t`, `\n`, `\r`,
    /// and `^`/`$` literal unless at start/after `\(` or end/before `\)`.
    Sed,
    /// `expr STRING : REGEXP`: ERE operators (unescaped `(`, `)`, `{n,m}`; `\(` is a literal paren),
    /// implicitly anchored at byte 0. The reference does not give `expr` BRE escapes.
    Expr,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RegexFlags {
    pub ignore_case: bool,    // -i / (?i) / nocasematch
    pub multiline: bool,      // ^ and $ match line boundaries `\n`
    pub dot_all: bool,        // . matches `\n`
    pub whole_word: bool,     // -w: match must be bounded by non-word bytes or input ends (word = [A-Za-z0-9_])
    pub line_regexp: bool,    // -x: match must cover the whole input / line
    pub allow_backrefs: bool, // false -> SyntaxKind::BackrefUnsupported; true -> bounded backtracker
    /// `sed` only (`sed` BRE `\{,m\}` and `sed -E` `{,m}`): a comma-first interval means `{0,m}`.
    /// Everywhere else (`grep`, `awk`, `[[ =~ ]]`, `expr`) `{,m}` is literal text.
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

impl Default for CompileLimits { /* constants above */ }

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
    LookaroundUnsupported,     // (?=...), (?!...), (?<=...), (?<!...)
    BackrefUnsupported(u8),    // \1..\9 when allow_backrefs == false
    InvalidBackref(u8),        // \k referencing non-existent group when allow_backrefs == true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapKind {
    ProgramTooLarge { limit: usize },
    RepeatTooLarge { limit: u32 },
    NestingTooDeep { limit: u32 },
    TooManyCaptures { limit: u32 },
}

#[derive(Debug)]
pub enum RegexError {
    Syntax(SyntaxKind),
    CompileCap(CapKind),
    Mem(MemError),
}

#[derive(Debug, PartialEq, Eq)]
pub enum MatchError {
    FuelExhausted,
    BacktrackLimitExceeded,
    Mem(MemError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

pub struct Captures {
    slots: KVec<Option<Span>>, // slot 0 = full match (after `\K` adjustment), 1..=groups = captures
}

impl Captures {
    pub fn full(&self) -> Span;
    pub fn group(&self, index: usize) -> Option<Span>;
    pub fn len(&self) -> usize;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prefilter<'a> {
    None,
    Exact(&'a [u8]),      // pattern is or starts with an exact literal byte slice
    Substring(&'a [u8]),  // required literal factor inside every match
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyAdvance {
    /// Advance 1 UTF-8 scalar (1 byte if non-UTF-8) on empty match and allow matching at the next offset (`grep -o`, global scan).
    Standard,
    /// POSIX `sed s/.../.../g`: after a NON-empty match ending at `p`, skip an immediate zero-length match at `p`.
    PosixSed,
    /// Bash `${var//pat/rep}`: after a NON-empty match that reaches `input.len()`, do not emit a trailing empty match at `input.len()`.
    ShellExpansion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceStyle {
    /// `&` = full match, `\1`..`\9` = capture group, `\n` `\t` `\r` `\\` `\&` escapes (`sed`).
    Sed,
    /// `$&` = full match, `$1`..`$9` / `${name}` = capture, `$$` = literal `$`.
    Dollar,
    /// Literal replacement bytes (no `&` or `\1` interpretation).
    Literal,
}

pub struct CompiledRegex { /* opaque: bytecode in KVec, named-group table, lazy DFA cache in KVec, prefilter */ }

impl CompiledRegex {
    pub fn compile(
        pattern: &[u8],
        mode: RegexMode,
        flags: RegexFlags,
        limits: CompileLimits,
        b: &Budget,
    ) -> Result<Self, RegexError>;

    pub fn capture_count(&self) -> usize;
    pub fn group_index(&self, name: &[u8]) -> Option<usize>;
    pub fn has_backrefs(&self) -> bool;
    pub fn dfa_cached_bytes(&self) -> usize;
    pub fn prefilter(&self) -> Prefilter<'_>;

    pub fn is_match(
        &mut self,
        haystack: &[u8],
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<bool, MatchError>;

    pub fn find_at(
        &mut self,
        haystack: &[u8],
        at: usize,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Option<Span>, MatchError>;

    pub fn captures_at(
        &mut self,
        haystack: &[u8],
        at: usize,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<Option<Captures>, MatchError>;

    pub fn find_all(
        &mut self,
        haystack: &[u8],
        policy: EmptyAdvance,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<KVec<Span>, MatchError>;

    pub fn replace_all(
        &mut self,
        haystack: &[u8],
        replacement: &[u8],
        style: ReplaceStyle,
        nth: Option<usize>, // None = global (`g`), Some(1) = first, Some(k) = k-th match (1-based, `sed s///k`)
        policy: EmptyAdvance,
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<(KVec<u8>, bool), MatchError>;
}
```

### 4.2 `kernel::glob`

```rust
pub const MAX_GLOBSTAR_SEGMENTS: usize = 5;
pub const DEFAULT_MAX_EXTGLOB_DEPTH: u32 = 100;

#[derive(Clone, Copy, Debug)]
pub struct GlobOptions {
    pub extglob: bool,
    pub nocase: bool,
    /// When `true` (pathname segment matching), `*`, `?`, `[...]`, and extglob groups never match `/`,
    /// and a leading `.` in the subject (or right after `/` if multi-segment) is matched only by a
    /// literal `.` unless `dotglob` is `true`.
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
            dotglob: false, // bash default; only consulted when `pathname` is true
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

pub struct CompiledGlob { /* opaque linear-time automaton / bytecode in KVec */ }

impl CompiledGlob {
    pub fn compile(
        pattern: &[u8],
        opts: GlobOptions,
        b: &Budget,
    ) -> Result<Self, GlobError>;

    pub fn is_match(
        &self,
        subject: &[u8],
        b: &Budget,
        work: &mut dyn FnMut(u64) -> Result<(), ()>,
    ) -> Result<bool, GlobError>;
}

/// Validates that a slash-separated path pattern contains at most `MAX_GLOBSTAR_SEGMENTS` (5)
/// `**` path segments; returns `Err(GlobError::TooManyGlobstars { max: 5 })` when exceeded.
pub fn check_globstar_segments(path_pattern: &[u8]) -> Result<usize, GlobError>;

/// `${var#pat}` (`greedy: false`) and `${var##pat}` (`greedy: true`) in linear time.
/// On invalid range `[z-a]`, returns `Ok(None)` so the caller keeps `subject` unchanged.
pub fn strip_prefix(
    subject: &[u8],
    pattern: &[u8],
    greedy: bool,
    opts: GlobOptions,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError>; // returns byte end of matched prefix

/// `${var%pat}` (`greedy: false`, shortest suffix = rightmost start) and
/// `${var%%pat}` (`greedy: true`, longest suffix = leftmost start) in linear time.
pub fn strip_suffix(
    subject: &[u8],
    pattern: &[u8],
    greedy: bool,
    opts: GlobOptions,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<Option<usize>, GlobError>; // returns byte start of matched suffix

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobReplaceMode {
    First,      // ${var/pat/rep}
    All,        // ${var//pat/rep}
    AnchorHead, // ${var/#pat/rep}
    AnchorTail, // ${var/%pat/rep}
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
) -> Result<KVec<u8>, GlobError>;

/// Splits `GLOBIGNORE` on `:` outside bracket expressions (`[:alnum:]` stays intact), dropping empty items.
pub fn split_globignore<'a>(raw: &'a [u8], b: &Budget) -> Result<KVec<&'a [u8]>, MemError>;
```

## 5. Regex engine specification (`kernel::regex`)

### 5.1 Dialect and mode rules (verified against `just-bash@3.6.0`)

1. **Extended (`RegexMode::Extended`):**
   - Operators: `.` (any scalar/byte except `\n` unless `dot_all`), `^`, `$`, `*`, `+`, `?`, `{n}`, `{n,}`,
     `{n,m}`, `|` alternation (empty branches allowed: `(|a)`, `(a|)`, `()`), `(...)` numbered capture groups
     (`1..`), `[...]` bracket expressions. A `{` that does not form a valid interval is literal text
     (`grep -E 'a{b'` matches `a{b`); this includes `{,m}` unless `comma_first_interval` is set (`sed -E`
     only), while a well-formed but inverted interval (`a{2,1}`) is `SyntaxKind::InvalidRepeatRange` and a
     leading quantifier (`*a` in ERE) is `SyntaxKind::DanglingQuantifier`.
   - Escapes outside `[...]`: `\` before any ERE metacharacter (`. ^ $ * + ? ( ) [ ] { } | \`) matches it
     literally; `\t` -> `0x09`, `\n` -> `0x0A`, `\r` -> `0x0D`, `\b` -> word boundary, `\B` -> non-word
     boundary, `\w` -> `[A-Za-z0-9_]`, `\W` -> `[^A-Za-z0-9_]`, `\s` -> `[ \t\n\r\f\v]`, `\S` -> negated `\s`,
     `\d` -> `[0-9]`, `\D` -> `[^0-9]`; `\1`..`\9` -> backreference (or `BackrefUnsupported` when
     `!allow_backrefs`); trailing `\` -> `SyntaxKind::TrailingBackslash`.
2. **Basic (`RegexMode::Basic`):**
   - Unescaped `+`, `?`, `|`, `(`, `)`, `{`, `}` are **literal** characters!
   - **`BreDialect::Grep`** (matches `just-bash@3.6.0` `grep` without `-E`/`-F`/`-P`):
     - `\(` and `\)` open and close capture groups; `\|` is alternation; `\{n\}`, `\{n,\}`, `\{n,m\}` are
       interval quantifiers **only** when the character after `\{` is an ASCII digit `[0-9]` (`a\{,2\}` in `grep`
       BRE is literal `a{,2}`).
     - `\+` and `\?` in `grep` BRE match literal `+` and `?` in `just-bash@3.6.0` (`grep 'a\+b'` and `grep 'a\?b'`
       match literal `a+b` and `a?b`).
     - Unescaped `*` at the very start of the pattern, or immediately after `^`, `\(`, or `\|` (`*abc`, `^*abc`,
       `\(*abc\)`), is a **literal `*`**, never a dangling quantifier.
   - **`BreDialect::Sed`** (matches `just-bash@3.6.0` `sed` without `-E`/`-r`):
     - `\(`, `\)`, `\|`, `\+` (one-or-more), `\?` (zero-or-one), `\{n\}`, `\{n,\}`, `\{n,m\}`, and `\{,m\}`
       (`{0,m}`) are active operators; `\t`, `\n`, `\r` match tab, LF, CR.
     - `^` is an anchor only at pattern start or immediately after `\(` (elsewhere literal `^`); `$` is an anchor
       only at pattern end or immediately before `\)` (elsewhere literal `$`).
   - **`BreDialect::Expr`** (`expr str : pat`): ERE operator set (unescaped `(...)` captures and `{n,m}`
     intervals; `\(` and `\{` match literal `(` and `{`), implicitly anchored at byte `0`. Verified:
     `expr abc : 'a(b)c'` prints `b`, `expr abc : 'a\(b\)c'` does not match, `expr aab : 'a{2}b'` prints `3`.
   - In every `Basic` dialect the class escapes of the ERE list also apply outside brackets (`\t`, `\n`,
     `\r`, `\b`, `\B`, `\w`, `\W`, `\s`, `\S`, `\d`, `\D`), in addition to each dialect's operator escapes.
   - **Pattern backreferences per surface in the reference** (with `allow_backrefs: false` the engine
     returns `BackrefUnsupported`; downstream lots map that one error to each surface's result): `grep`
     (BRE, ERE and `-P`) and `expr` fail with exit `2` and an invalid-regex message; `sed` accepts the
     pattern and never matches it (`s/\(a\)\1/X/` leaves `aa` unchanged, exit `0`); `awk` never matches;
     `[[ lhs =~ (a)\1 ]]` returns status `1` with empty stderr. Replacement-side `\1`..`\9` and `&` in `sed`
     are not pattern backreferences and work normally (`ReplaceStyle::Sed`).
3. **Fixed (`RegexMode::Fixed`):**
   - Splits `pattern` on `\n` into literal needles. Empty pattern (`b""`) or any empty line inside `pattern`
     matches every line (`0..0`). With `line_regexp` (`-x`), needle must equal the full line; with `whole_word`
     (`-w`), needle match boundaries must satisfy word-boundary rules; with `ignore_case` (`-i`), ASCII/Unicode
     case-insensitive literal comparison.
4. **Perl (`RegexMode::Perl`, `grep -P`):**
   - Everything in `Extended`, plus:
     - Non-greedy quantifiers `*?`, `+?`, `??`, `{n,m}?`, `{n,}?` (in PikeVM, flip split priority so the
       exit branch has higher priority than the loop branch — still strictly linear time!).
     - `\Q...\E` literal quoting (an unclosed `\Q` quotes through the end of the pattern).
     - `\xHH` (exactly 2 hex digits; `\x4` is `InvalidEscape`), `\x{H...}` (1 to 6 hex digits, invalid >
       `0x10FFFF` -> `InvalidEscape`), `\0` / `\NNN` octal (`\101` matches `A`), `\f` (`0x0C`), `\v`
       (`0x0B`). `\e` is a compile error in the reference (`InvalidEscape`); `\a` is outside the verified
       subset and also returns `InvalidEscape` (recorded for the `grep` lot to snapshot before relying on it).
     - Inline flag groups `(?ims-ims)` (scoped to enclosing group) and `(?ims-ims:...)` non-capturing scoped
       groups, `(?:...)` non-capturing group, `(?P<name>...)` and `(?<name>...)` named capture groups (`name`
       matching `[A-Za-z_][A-Za-z0-9_]*`).
     - `\K` match-start reset: records the current offset as the reported `full().start` on acceptance without
       employing lookbehind (linear-time slot update in PikeVM!).
     - Lookaround assertions `(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)`: **rejected at compile time** with
       `RegexError::Syntax(SyntaxKind::LookaroundUnsupported)` per EVOL D12 and `just-bash@3.6.0`.

### 5.2 Bracket expressions and POSIX character classes

1. Inside `[...]`:
   - Leading `^` negates the bracket class (`!` inside regex `[...]` is a **literal `!`**, unlike glob `[!...]`,
     matching POSIX regex and `just-bash@3.6.0`).
   - An immediate `]` right after `[` or `[^` (`[]a]` or `[^]a]`) is a **literal `]`** and does not close the
     bracket.
   - `-` at the start (after optional `^` and optional initial `]`), or immediately before closing `]`, is a
     **literal `-`**. Elsewhere `c1-c2` defines an inclusive range; if `c1 > c2` (`[z-a]`), fail with
     `SyntaxKind::InvalidCharacterRange`.
   - Standalone word-boundary bracket tokens `[[:<:]]` (start of word: preceded by non-word or start of text,
     followed by word char `[A-Za-z0-9_]`) and `[[:>:]]` (end of word: preceded by word char, followed by
     non-word or end of text) compile to zero-width start-of-word / end-of-word assertions (`grep -E '[[:<:]]cat[[:>:]]'`).
   - Named POSIX classes `[:name:]` inside `[...]`: recognize **exact** 14 names (any unknown `[:bogus:]` fails
     with `SyntaxKind::UnknownPosixClass` in regex mode, matching `just-bash@3.6.0` `grep -E '[[:bogus:]a]'` ->
     exit `2`!):

| Class | Exact ASCII byte set |
|---|---|
| `alnum` | `0-9`, `A-Z`, `a-z` |
| `alpha` | `A-Z`, `a-z` |
| `ascii` | `0x00..=0x7F` |
| `blank` | `' '` (`0x20`), `'\t'` (`0x09`) |
| `cntrl` | `0x00..=0x1F`, `0x7F` |
| `digit` | `0-9` |
| `graph` | `0x21..=0x7E` |
| `lower` | `a-z` (under `ignore_case`, matches `A-Z` as well) |
| `print` | `0x20..=0x7E` |
| `punct` | `0x21..=0x2F`, `0x3A..=0x40`, `0x5B..=0x60`, `0x7B..=0x7E` |
| `space` | `' '`, `'\t'`, `'\n'`, `'\r'`, `'\x0C'` (`\f`), `'\x0B'` (`\v`) |
| `upper` | `A-Z` (under `ignore_case`, matches `a-z` as well) |
| `word` | `0-9`, `A-Z`, `a-z`, `'_'` |
| `xdigit` | `0-9`, `A-F`, `a-f` |

### 5.3 Execution guarantees, compile caps, lazy DFA, and bounded backtracker (EVOL D12, S9)

1. **Compile caps:**
   - Every `{n,m}` or `{n,}` or `{n}` with `n > MAX_REPEAT_EXPANSION` (`1_000`) or `m > MAX_REPEAT_EXPANSION`
     (`1_000`) fails with `CompileCap(CapKind::RepeatTooLarge { limit: 1_000 })`.
   - Unrolling counted repetitions emits NFA instructions linearly; the moment the instruction buffer exceeds
     `MAX_PROGRAM_INSTS` (`65_536`), compilation aborts immediately (before allocating further) with
     `CompileCap(CapKind::ProgramTooLarge { limit: 65_536 })` — so `a{1000}{1000}` (`1_000_000` instructions)
     is rejected in `< 1 ms`!
   - Group/alternation nesting `> MAX_NESTING_DEPTH` (`200`) fails with `NestingTooDeep { limit: 200 }`;
     numbered captures `> MAX_CAPTURE_GROUPS` (`100`) fails with `TooManyCaptures { limit: 100 }`.
2. **Linear-time PikeVM + capped lazy DFA (`!has_backrefs()`):**
   - For patterns without backreferences, matching NEVER backtracks:
     - `is_match` and uncaptured `find_at` (when no `\K`, word-boundary assertions can be tracked via 1-bit
       previous-byte word-class in DFA state) use a lazy DFA whose transition table + state set byte footprint is
       tracked in `dfa_cached_bytes`. When adding a state would exceed `MAX_DFA_CACHE_BYTES` (`262_144` B), clear
       the DFA cache in `O(1)` (resetting to the start state) and continue stepping (falling back to PikeVM if a
       single transition cannot fit).
     - `captures_at` runs two-pass or single-pass PikeVM (find match end/bounds via DFA/PikeVM, then extract
       leftmost-first / POSIX-ERE capture slots in `O(|span| * |insts|)`), deduplicating NFA threads per `pc` at
       each byte offset via a sparse-set generation counter so each byte executes at most `|insts|` state steps!
   - Charge `work` (`1` work unit per `64` NFA thread transitions or `256` DFA byte steps, minimum `1` per call)
     so `maxWorkUnits` bounds long haystacks cleanly.
3. **Fuel-charged bounded backtracker (`has_backrefs() == true`, `allow_backrefs == true`):**
   - When a pattern contains `\1`..`\9`, execute an explicit-stack VM (`KVec<BtFrame>`, zero Rust call recursion)
     charging `1` work unit per `16` backtrack steps to `work` and maintaining a visited bitset/hash capped at
     `MAX_BACKREF_VISITED_STATES` (`262_144` entries; `(pc, offset)` for states where capture slots `1..=max_ref`
     have not changed, plus a hard per-match step ceiling `min(4_000_000, remaining_fuel * 16)`).
   - Exceeding `work` returns `Err(MatchError::FuelExhausted)`; exceeding the per-match step ceiling returns
     `Err(MatchError::BacktrackLimitExceeded)`. Zero traps, zero unbounded loops.

## 6. Glob and extglob specification (`kernel::glob`)

1. **Core wildcards and brackets:**
   - `*`: matches zero or more characters (excluding `/` when `opts.pathname == true`).
   - `?`: matches any single character (excluding `/` when `opts.pathname == true`).
   - `\c`: outside `'...'`, escapes `c` so it matches literally (`\*`, `\?`, `\[`, `\\`).
   - `[...]`:
     - If no closing `]` exists before the end of `pattern`, `[` matches a literal `[`!
     - Negation: only a leading `^` negates the bracket class. A leading `!` is an ordinary member: `[!a]`
       matches `!` or `a` and does not match `b` (verified on `case` and `[[ == ]]`; this differs from GNU
       bash, and D1 keeps the reference behavior). A negated class never matches `/` when
       `opts.pathname == true`.
     - Initial `]` right after `[` or `[^` is a literal `]`.
     - `'` inside brackets is an ordinary member byte and quotes nothing: `['a-z']` is the set
       `{', a..z}` (it matches `m`, not `-`), and `[a'-'z]` likewise still forms the range `a..z`.
     - POSIX classes `[:name:]`: use the same 14-entry table as section 5.2. An unknown `[:bogus:]` inside a
       glob bracket contributes 0 characters without failing compilation in this engine (`strip_prefix` on
       `xb` with `[[:bogus:]x]` strips `x` in the reference). A reversed range `[z-a]` returns
       `Err(GlobError::InvalidCharacterRange)`. Reference behavior for these two inputs is surface-dependent
       and partly an internal error: `case` with `[[:bogus:]b]` or `[z-a]` throws out of `exec`, `[[ == ]]`
       returns status `2` with a conditional-expression message, `${var#[z-a]}` throws, and pathname
       expansion of `[z-a]` falls back to the literal word. Those surfaces are `upstream-defect` candidates
       (EVOL Annex E.7), excluded from the section 7.3 parity cases; downstream lots map `GlobError` to a
       shell result (never a throw) per their own snapshots.
2. **Full `extglob` (`opts.extglob == true`):**
   - Operators `@(p1|p2|...)` (exactly 1), `*(p1|...)` (0 or more), `+(p1|...)` (1 or more), `?(p1|...)` (0 or 1),
     and `!(p1|...)` (any string — excluding `/` when `opts.pathname == true` — that does **not** match any
     branch `p_i`).
   - Track nesting depth of extglob groups; if depth exceeds `opts.max_extglob_depth`, fail with
     `Err(GlobError::ExtglobDepthExceeded { max: opts.max_extglob_depth })`.
   - **Linear-time `!(...)` without regex lookahead:** In bash extglob concatenation `!(P)Q`, a subject `S`
     matches if and only if there exists a split `S = U ++ V` (at character/byte boundaries, with `U` containing
     no `/` when `opts.pathname == true`) such that `U` does **not** match `P` and `V` matches `Q`. Thus
     `!(foo)bar` is `false` on `"foobar"` (the only split ending in `"bar"` has `U = "foo"`, which matches `foo`)
     and `true` on `"xbar"` and `"bar"`, while `!(a*)b` is `false` on `"ab"` and `"aab"` and `true` on `"bb"`.
     Implement `CompiledGlob` so `!(p1|...)` is exact and polynomial/linear-time on all inputs (for instance via
     dynamic programming / bitset reachability over pattern positions `0..=M` across subject positions `0..=N`,
     computing exact `(start, end)` substring matches of each inner `!(...)` sub-automaton via multi-start NFA
     stepping, or subset-complement DFA in `O(N)` when state count `<= 256`). Every transition step charges `work`
     so nested `!(...)` inputs honor `maxGlobOperations` / `maxWorkUnits` with zero exponential blow-up and zero
     lookahead errors.
3. **Prefix/suffix stripping and replacement (`strip_prefix`, `strip_suffix`, `replace_glob`):**
   - `strip_prefix(subject, pat, greedy)`: run the compiled glob automaton forward from offset `0`, recording the
     first (`!greedy`, `${var#pat}`) and last (`greedy`, `${var##pat}`) UTF-8 character boundary where the accept
     state is active — strictly `O(|subject| * |pat|)` in a single pass!
   - `strip_suffix(subject, pat, greedy)`: run the reversed glob automaton backward from `subject.len()` (or
     single backward DP pass) to find the rightmost (`!greedy`, `${var%pat}`) and leftmost (`greedy`,
     `${var%%pat}`) UTF-8 boundary matching through `subject.len()` in linear time (unless `!(...)` uses the
     bounded bitset pass).
   - `replace_glob`: leftmost-longest greedy match (`First`, `All` with `EmptyAdvance::ShellExpansion` so
     `v=abc; ${v//*/X}` yields `X` and NOT `XX`!, `AnchorHead` matching from `0`, `AnchorTail` matching through
     `subject.len()`). On an empty subject every mode returns the empty string (no empty match is replaced).
4. **Leading-dot rule (`opts.pathname && !opts.dotglob`) and `GLOBIGNORE`:**
   - When `opts.pathname && !opts.dotglob`, a subject starting with `.` matches only if the pattern itself starts
     with a literal `.` (or `@(.foo|...)` branch starting with literal `.`).
   - `split_globignore`: split on `:` at bracket depth `0` (tracking `[...]` including nested `[:class:]` so
     `GLOBIGNORE='*.bak:[[:digit:]]*'` splits into `["*.bak", "[[:digit:]]*"]`). Note for L4: when `GLOBIGNORE`
     is non-empty, bash implicitly enables `dotglob` and filters `.` and `..` unless otherwise matched.

## 7. Test commands, parity adapter, and Room-1 oracle cases

### 7.1 `kernel-test` commands (`kernel/src/fiber/testcmd.rs`)

Add two test-only commands (compiled **only** under `cfg(feature = "test-commands")`, zero bytes in production
`kernel.core.wasm`). Both dispatch from the built `Fiber::exec_test` in `testcmd.rs` (extend its `CMDS`
table); their `work` closure charges the aggregate counter through `Fiber::charge_work(units)`, mapping
`Err(Fail::Work)` to `MatchError::FuelExhausted` / `GlobError::FuelExhausted`. Fuel-slice `tick()` is not
usable inside these synchronous test commands; long scans are bounded by `maxWorkUnits` instead, exactly as
gate 5 asserts (`limitName: "maxWorkUnits"`, exit `126` through the normal `Fail::Work` finish).

1. **`__regex <mode:bre-grep|bre-sed|bre-expr|ere|fixed|perl> <flags> <pattern> [replacement]`**
   - `<flags>`: `-` or any combination of `i` (`ignore_case`), `m` (`multiline`), `s` (`dot_all`), `w`
     (`whole_word`), `x` (`line_regexp`), `b` (`allow_backrefs`), `o` (only-matching), `v` (invert lines), `c`
     (count lines), `n` (line numbers), `g` (global replace when `[replacement]` is given).
   - Reads `stdin`:
     - When `[replacement]` is absent: runs line-by-line `grep`-compatible search over `stdin` (`multiline: false`
       per line), emitting matching lines (or `-o` non-empty/advancing slices, `-c` count, `-n` `1:` prefixes),
       exiting `0` if at least one line matched, `1` if zero matched, `2` on `RegexError::Syntax` or
       `CompileCap` with `grep: invalid regular expression: <pattern>\n` (and when flag `D` is passed in
       `<flags>`, prints the detailed `SyntaxKind` / `CapKind` tag for unit tests), or `126` on `Mem`/`FuelExhausted`!
     - When `[replacement]` is present: runs `replace_all` per line (`ReplaceStyle::Sed`, `EmptyAdvance::PosixSed`
       when `g` is set) and writes transformed output.
2. **`__glob <op:match|strip-prefix|strip-prefix-greedy|strip-suffix|strip-suffix-greedy|replace|replace-all|globstar-check|globignore> <flags> <pattern> [arg1] [arg2]`**
   - `<flags>`: `-` or combination of `e` (`extglob`), `i` (`nocase`), `p` (`pathname`), `d` (`dotglob`).
   - Executes the corresponding `kernel::glob` function. The `match` op prints `match\n` or `no\n` and
     exits `0` in both cases (mirroring the `case ... echo match/no` probe, whose exit is `echo`'s). Strip
     and replace ops print the resulting string + `\n` with exit `0` (`Ok(None)` prints the subject
     unchanged). `TooManyGlobstars` on any op finishes with exit `126`, empty stdout, and
     `bash: Glob pattern has too many ** segments (max 5)\n` on stderr (no `?` in `result.env`, matching the
     reference limit finish). `ExtglobDepthExceeded` and `InvalidCharacterRange` print `__glob: <tag>\n` on
     stderr with exit `2` (test-harness outcomes only; no oracle case uses them). `FuelExhausted` and `Mem`
     finish as `Fail::Work` / `Fail::Mem` (exit `126`).

### 7.2 Test-only parity adapter (`harness/parity/l8a-adapter.mjs`)

Because L8a branches from `main` in W4 before `grep`, `shopt`, `case`, and `${var#...}` exist in the shell VM,
`test/parity/l8a.test.mjs` passes `harness/parity/l8a-adapter.mjs` to `defineParityTests`:
- `create(init)` instantiates `Bash` on `kernel-test` (`import { T } from "../../test/kt.mjs"`, identical
  `files`/`env`/`cwd`/`profile` setup to `sentropic-adapter.mjs`).
- `exec(step)` recognizes exactly the three canonical step forms used in `test/cases/l8a/*.case.json`; any
  other step text fails the test (counted `notCovered`, never silently passed). Quoting helpers, used in
  both directions: `unquote(s)` decodes one shell single-quoted token (strip the outer quotes, replace each
  `'\''` sequence with `'`), and `quote(s)` encodes as `'` + `s` with every `'` replaced by `'\''` + `'`.
  Patterns and subjects are always passed to `__regex`/`__glob` through `quote(...)` in the generated
  script text, never through exec `env` (which would leak adapter variables into `result.env`):
  1. `^grep( -[EFPiwxovcn]+)* -- ('(?:[^']|'\\'')*')$` -> mode from `-E`/`-F`/`-P` (absent = `bre-grep`),
     flags from the remaining letters, pattern `unquote(group)`, stdin `step.stdin`; generated script
     `__regex <mode> <flags|-> <quote(pattern)>`.
  2. `^\( (shopt -s ([a-z ]+); )?case ('(?:[^']|'\\'')*') in (.*)\) echo match ;; \*\) echo no ;; esac \)$`
     -> subject `unquote(group)`, pattern = group 4 up to the **last** `) echo match` (extglob patterns
     contain their own `)` bytes); flags from the `shopt` names (`extglob` -> `e`, `nocasematch` -> `i`);
     generated script `__glob match <flags|-> <quote(pattern)> <quote(subject)>`.
  3. `^\( shopt -s globstar; echo (\S+) \)$` -> generated script `__glob globstar-check - <quote(path)>`.
- Because every step in `test/cases/l8a/*.case.json` either runs `grep` or runs inside a `( ... )` subshell in
  `just-bash@3.6.0`, `result.env` (`?` + the 12 default variables, with `BASHOPTS` unmutated) and `fs` (`{}`)
  match `kernel-test` identically without special-casing `env` or `fs`! Once L4, L5, and L9a-1 merge,
  `test/parity/l8a.test.mjs` can switch straight to `sentropic-adapter.mjs` with zero snapshot edits.

### 7.3 Room-1 oracle cases (`test/cases/l8a/*.case.json`, 30 cases)

| # | Case ID (`l8a/...`) | Canonical step(s) & `stdin` |
|---:|---|---|
| 1 | `bre-basic-literals` | `grep -- 'a+b?c(1)\|2'` on literal vs metachar lines (`+ ? ( ) \|` are literal in BRE) |
| 2 | `bre-groups-and-alt` | `grep -- 'a\(bc\)*d\|e'` and `grep -- '\(*abc\)'` + `grep -- '*abc'` (leading/post-`\(` `*` is literal) |
| 3 | `bre-intervals` | `grep -- 'a\{2,3\}b'` vs `grep -- 'a\{,2\}b'` (comma-first is literal in `grep` BRE) |
| 4 | `bre-plus-qmark-literal` | `grep -- 'a\+b'` and `grep -- 'a\?b'` (literal `a+b` / `a?b` in `just-bash@3.6.0` `grep` BRE) |
| 5 | `ere-quantifiers-alt` | `grep -E -- '(ab)+c?d{2,4}\|(xy\|)'` including empty alternation branch |
| 6 | `ere-comma-interval-literal` | `grep -E -- 'a{,2}b'` on `"a{,2}b\nab\naab\nb\n"` (comma-first interval is literal text outside `sed`; only the first line matches). The `sed` form is covered natively in gate 6, since the adapter has no `sed` step form |
| 7 | `ere-anchors-and-escapes` | `grep -E -- '^\[a\.b\]\$$'` and `\t`, `\w`, `\W`, `\s`, `\S`, `\d`, `\D` |
| 8 | `fixed-strings-multiline` | `grep -F -- 'a*b'` and multi-needle `-F`, `-Fi`, `-Fw`, `-Fx` |
| 9 | `flags-i-w-x-v-c-n` | `grep -Eiw -- 'cat'`, `grep -Ex -- '[0-9]+'`, `grep -Evc -- '^#'`, `grep -En -- 'hit'` |
| 10 | `only-matching-ere` | `grep -Eo -- '[a-z]+[0-9]+'` with multiple matches per line and non-matching lines (`exit 1`) |
| 11 | `posix-classes-alpha-num` | `grep -Ex -- '[[:alpha:]]+-[[:digit:]]+-[[:alnum:]]+-[[:xdigit:]]+'` |
| 12 | `posix-classes-space-punct` | `grep -Ex -- '[[:blank:]]+[[:punct:]]+[[:upper:]][[:lower:]]'` and `[[:print:]]`, `[[:graph:]]`, `[[:cntrl:]]` |
| 13 | `posix-classes-ascii-word` | `grep -Ex -- '[[:ascii:]]+'` and `grep -Ex -- '[[:word:]]+'` |
| 14 | `posix-word-boundaries` | `grep -E -- '[[:<:]]cat[[:>:]]'` on `"cat\ncats\na cat!\n_cat\n"` |
| 15 | `bracket-edge-cases` | `grep -E -- '[]a-]'`, `grep -E -- '[^]a]'`, `grep -E -- '[!a]'` (`!` literal in regex brackets) |
| 16 | `perl-shorthands-hex-quote` | `grep -Po -- '\Q(a+b)\E\x41\x{3b1}\d+\s+\w+'` |
| 17 | `perl-inline-flags-named` | `grep -Po -- '(?i:abc)-(?-i:DEF)-(?P<id>[0-9]+)'` |
| 18 | `perl-keep-and-lazy` | `grep -Po -- '(?i:ab)\d+\K[a-z]+?'` (`\K` match-start reset + lazy `+?`) |
| 19 | `err-perl-lookaround` | Steps: `grep -P -- 'a(?=b)'`, `grep -P -- 'a(?!b)'`, `grep -P -- '(?<=a)b'`, `grep -P -- '(?<!a)b'` (all `exit 2`, exact stderr) |
| 20 | `err-backref-default` | Steps: `grep -- '\(a\)\1'`, `grep -E -- '(a)\1'`, `grep -P -- '(a)\1'` (all `exit 2`, exact stderr) |
| 21 | `err-invalid-syntax` | Steps: `grep -- '['`, `grep -E -- '(abc'`, `grep -E -- '[z-a]'`, `grep -E -- '[[:bogus:]a]'` (all `exit 2`) |
| 22 | `unicode-ignore-case` | `grep -Ei -- 'café'` on `"CAFÉ\ncafé\ncafe\n"` and astral UTF-8 dot/negated-bracket matching |
| 23 | `glob-wildcards-escapes` | `( case 'a*b?c' in 'a*b'?c) echo match ;; *) echo no ;; esac )` and `*`, `?`, `\?`, `\*` |
| 24 | `glob-brackets-negation` | Steps `( case '<s>' in <p>) echo match ;; *) echo no ;; esac )` for (`b`, `[^a]`) -> `match`, (`b`, `[!a]`) -> `no` (`!` is a literal member), (`!`, `[!a]`) -> `match`, (`]`, `[]-]`) -> `match`, (`7`, `[[:digit:]]`) -> `match` |
| 25 | `glob-nocasematch` | `( shopt -s nocasematch; case 'HeLLo.TXT' in *.txt) echo match ;; *) echo no ;; esac )` |
| 26 | `extglob-at-star` | `( shopt -s extglob; case 'abab' in @(a\|ab)) echo match ;; *) echo no ;; esac )` and `*(a\|b)` |
| 27 | `extglob-plus-qmark` | `( shopt -s extglob; case 'aaab' in +(a)b) echo match ;; *) echo no ;; esac )` and `?(pre_)main` |
| 28 | `extglob-nested` | `( shopt -s extglob; case 'foo_12_bar' in @(foo\|baz)_+([0-9])_?(bar\|qux)) echo match ;; *) echo no ;; esac )` |
| 29 | `glob-bracket-quote-members` | `( case 'm' in ['a-z']) echo match ;; *) echo no ;; esac )` -> `match` and `( case '-' in ['a-z']) ... )` -> `no` (`'` quotes nothing inside brackets; the `a-z` range still forms) |
| 30 | `glob-globstar-cap` | `( shopt -s globstar; echo /**/**/**/**/**/** )` (`exit 126`, `bash: Glob pattern has too many ** segments (max 5)\n`) |

Reading the table: a `\|` sequence inside a quoted pattern is two script bytes (backslash, pipe) in BRE rows
and one pipe byte (Markdown table escaping) in ERE, Perl, `case`, and extglob rows; Room 1 writes the case
files accordingly. Deliberately absent from this list, because the reference throws an internal error out of
`exec` on them (`upstream-defect`, EVOL Annex E.7): any `case` step whose pattern contains `!(...)`, an
unknown POSIX class, or a reversed range. Our behavior for those inputs is defined by the native gates in
section 8, not by snapshots.

## 8. Executable exit gates

All gates run in `make check` (`cargo test` + `node tools/run-tests.mjs`):

1. **Repository and zero-host-RegExp gates.** G1-G8 pass (`dist/kernel.core.wasm` has zero foreign imports;
   zero external crates in `Cargo.lock`; `host/**` is untouched so zero JS `RegExp` is reachable from `kernel::regex`
   or `kernel::glob`).
2. **Parity (`test/parity/l8a.test.mjs`).** All 30 committed `test/cases/l8a/*.case.json` cases replay with **0
   failures, 0 stale hashes, 0 exclusions, and 0 `notCovered`**.
3. **S9 matching blow-up gate (`test/l8a-redos.test.mjs` + native `cargo test`).** Run each classic backtracking
   trap below through `__regex` on `kernel-test` (and natively via `CompiledRegex`):
   - `ere` pattern `(a+)+b` against `"a".repeat(50_000) + "!"` -> `exitCode: 1` (no match) in **`< 50 ms`**;
   - `ere` pattern `(a|a)*b` against `"a".repeat(50_000) + "!"` -> `exitCode: 1` in **`< 50 ms`**;
   - `ere` pattern `([a-zA-Z]+)*$` against `"a".repeat(50_000) + "!"` -> `exitCode: 1` in **`< 50 ms`**;
   - `perl` pattern `(.*a){25}b` against `"a".repeat(20_000) + "!"` -> `exitCode: 1` in **`< 50 ms`**;
   - Scaling check: wall time / work-unit count of `(a+)+b` on `50_000` `'a'`s vs `5_000` `'a'`s is `<= 15x`
     (confirming `O(N)` scaling, never `O(2^N)`).
4. **S9 compilation blow-up and DFA cache cap gate (`test/l8a-redos.test.mjs` + native `cargo test`).**
   - `a{1000}{1000}` fails compilation cleanly in `< 20 ms` with `CompileCap(ProgramTooLarge { limit: 65_536 })`
     (`exitCode: 2` in `__regex`).
   - `a{1001}` fails with `CompileCap(RepeatTooLarge { limit: 1_000 })`.
   - `(`.repeat(201) + `a` + `)`.repeat(201) fails with `CompileCap(NestingTooDeep { limit: 200 })`.
   - `(a)`.repeat(101) fails with `CompileCap(TooManyCaptures { limit: 100 })`.
   - A DFA state-explosion pattern `a[ab]{20}` scanned across `64 KiB` of pseudo-random `"ab"` bytes succeeds,
     never exceeds `dfa_cached_bytes() <= 262_144` (256 KiB) at any point, and matches PikeVM results bit-for-bit.
5. **Bounded backreference gate (EVOL D12, S9).**
   - With `allow_backrefs: true`: `([a-z]+)-\1` matches `"abc-abc"` (captures `0..7`, `0..3`) and rejects
     `"abc-def"`; `\(a*\)\1` in `BreDialect::Sed` and `BreDialect::Grep` works identically.
   - Adversarial backref pattern `([a-z]+)\1+b` against `"a".repeat(500) + "!"`: with `maxWorkUnits: 5_000` on
     `kernel-test`, terminates deterministically in `< 50 ms` with `exitCode: 126` (`limitName: "maxWorkUnits"`)
     and zero traps, leaving the `Bash` instance usable for `exec("echo ok")`.
   - With `allow_backrefs: false`: `(a)\1` returns `Syntax(BackrefUnsupported(1))`.
6. **Native `!(...)` extglob, strip, replace, and `sed` dialect unit suite (`cargo test --lib`).** At least
   **65** native unit tests covering:
   - `!(...)` extglob (including forms where lookahead-based translators fail): `!(a|b)` on `"a"` (`false`), `"x"`
     (`true`), `""` (`true`); `!(foo)bar` on `"foobar"` (`false`), `"xbar"` (`true`), `"bar"` (`true`);
     `!(a*)b` on `"ab"` (`false`), `"bb"` (`true`); nested `!(!(a)|b)` on `"a"` (`true`), `"b"` (`true`), `"c"`
     (`false`); `strip_prefix("abc", b"!(z)", true)` == `Some(3)` (strips all `"abc"`) and `!greedy` == `Some(0)`;
     pathname `!(a)` on `"x/y"` (`false` because `/` is not matched when `pathname: true`).
   - `strip_prefix` / `strip_suffix` linear-time check on `"a".repeat(50_000) + "b"` with `*a*` completing in
     `< 50 ms`.
   - `replace_glob` with `EmptyAdvance::ShellExpansion` (`v="abc"`, pattern `"*"`, replacement `"X"`, `All` ->
     `"X"`, and `v=""` -> `""`, matching the reference: an empty subject produces no replacement).
   - `BreDialect::Sed` and `comma_first_interval` (`\+`, `\?`, `\|`, `\{,2\}` in BRE and `{,2}` in `sed -E`
     form, with the same `{,2}` staying literal when the flag is off, mid-pattern literal `^` and `$`,
     `ReplaceStyle::Sed` with `&` and `\1`..`\9`, and `PosixSed` empty-match skip on `s/a*/X/g` over `"aba"`
     -> `"XbX"`), plus `BreDialect::Expr` (`a(b)c` on `abc` anchored with capture `b`; `a\(b\)c` no match).
   - `Prefilter` extraction (`Exact` on `"hello.*"` and `Substring` on `".*needle.*"`).
7. **Deterministic seeded regex/glob fuzz slice (`test/l8a-fuzz.test.mjs` + native `cargo test`).** Fixed seed
   `0x5345_4E54_4C38_415A` generating **>= 1 500** random BRE/ERE/Perl/Glob/Extglob patterns and UTF-8 + raw-byte
   haystacks in `make check`: 0 panics, 0 Wasm traps, DFA == PikeVM differential equivalence on all capture-free
   runs, `< 2 s` total runtime.

## 9. Points the builder must confirm and report

Record in `BRANCH.md` and the PR description:

1. **Core Wasm size delta:** `dist/js/kernel/kernel.core.wasm` raw and gzip byte delta added by `kernel::regex`
   and `kernel::glob` (note: production `kernel` only links what is reachable until L4/L9 wire them; also report
   `dist/test/kernel-test.core.wasm` delta where both modules are fully live).
2. **S9 blow-up timings and DFA peak bytes:** exact milliseconds and work units for the four matching traps in
   gate 3, the compile-cap rejections in gate 4, the peak `dfa_cached_bytes()` on `a[ab]{20}`, and the bounded
   backreference termination in gate 5.
3. **Downstream integration checklist for L4, L9a-1, L13, L14a:** confirm exact signatures of `CompiledRegex`
   (`BreDialect::{Grep,Sed,Expr}`, `EmptyAdvance::{Standard,PosixSed,ShellExpansion}`, `Prefilter`) and
   `CompiledGlob` (`strip_prefix`, `strip_suffix`, `replace_glob`, `split_globignore`, `check_globstar_segments`).
4. **Upstream `!(...)` and backreference findings (EVOL Annex E.7):** note in `BRANCH.md` that `kernel::glob`
   evaluates `!(...)` natively without regex lookahead (avoiding the reference's `SyntaxError` on `!(...)` in
   `[[ == ]]` and `${var##!(...)}`), and `RegexFlags::allow_backrefs` supports both strict `3.6.0` parity
   (`false`) and EVOL D12 bounded backreferences (`true`) pending owner choice for downstream consumers.
