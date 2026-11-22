# Lot L8b — Shared leaf kernels (`num`, `format`, `hash`, `base64`)

Lot spec for the build lane (Room 2). Sources of authority: `spec/SPEC_EVOL_BASH_SANDBOX.md` ("EVOL":
decisions D1, D2, D6, D7, D9, D19-D21, D23; security invariants S1, S3, S4, S10, S11; Annexes B, D.2, E, F, G)
and `plan/PLAN_BASH_SANDBOX_V1.md` (lot L8b, sections 3, 4, 6 and 8). It builds on `spec/lots/L0.md` (gates
G1-G8), `spec/lots/L1a-1.md` and `spec/lots/L1a-2.md` (`kernel::mem` allocator, `Budget`, `KVec`, `KBytes`,
`KString`), `spec/lots/L1b.md`, `spec/lots/L2.md` (`kernel-test` and `kernel/src/fiber/testcmd.rs`),
`spec/lots/L2b.md`, and `spec/lots/L10a.md` (parity recorder and replay runner). Where this document and EVOL
differ, stop and raise a spec issue.

- **Size.** 3 points, Mid. **Builder.** Muse 1.3 xhigh (Codex Sol 6.2 xhigh from 27/11; plan Mid slot: Codex
  Sol 6.1 medium, fallback Sonnet 5.5). **Review.** Carbon, full review of every PR, dispatched on a green
  `make check`.
- **Branch.** `feat/l8b-leaf-kernels`, worktree `tmp/l8b-leaf-kernels`.
- **Outcome.** Delivers four zero-dependency, `no_std`, budget-allocated leaf kernel modules — `kernel::num`
  (ECMA-262 shortest round-trip `f64` decimal formatter plus fixed/exponential/precision formatting and shell
  arithmetic, `printf`, and float number parsers), `kernel::format` (complete `printf` engine with format and
  `%b` escape decoders, `%q` shell quoting, prospective byte-limit gates, `awk` `sprintf`/`OFMT` dialect, and
  proleptic-Gregorian `strftime` engine), `kernel::hash` (streaming and one-shot `Md5`, `Sha1`, and `Sha256`),
  and `kernel::base64` (line-wrapping encoder and permissive/strict decoders with prospective size gates) —
  wired to `kernel-test` (`__num`, `__printf`, `__strftime`, `__hash`, `__base64`) and verified against the
  Room-1 JavaScript double vector file (`test/fixtures/l8b-js-numbers.json`), RFC standard vectors, and 30
  `just-bash@3.6.0` snapshots (`test/cases/l8b/*`).

## 1. Lane rules

1. No reference source and no reference package in the worktree. No network. Before the worktree is created
   (PLAN section 6, steps 3-4), Room 1 commits:
   - `test/fixtures/l8b-js-numbers.json` (pre-generated IEEE-754 `f64` bit patterns and their expected
     JavaScript string forms from Node/V8);
   - the 30 case files `test/cases/l8b/*.case.json` and recorded snapshots `test/cases/l8b/*.snap.json` against
     `just-bash@3.6.0`.
   The builder never edits a committed fixture, case, or snapshot and never runs or imports the reference
   engine.
2. Kernel: `no_std` on `wasm32-wasip2`, zero external crates in `Cargo.lock`, zero host imports (EVOL D2, G1),
   all heap allocations through `kernel::mem` against `&Budget` (G6), `unsafe` only in `kernel/src/mem/` and
   `kernel/src/bindings/`.
3. Host and tests: plain ES modules with JSDoc, Node built-ins only in `tools/**`, `test/**`, and test-only
   parity adapters; no Python and no Docker.
4. Never write to `/tmp`; scratch files go under the git-ignored `tmp/` of the worktree.
5. Commits through `make commit`, in English, without attribution trailers.

## 2. Scope

In scope:
- `kernel::num` (`kernel/src/num.rs` or `kernel/src/num/**`):
  - Exact ECMA-262 `Number::toString(x, 10)` shortest round-trip decimal formatter (`fmt_f64_js`,
    `f64_to_js_string`) implemented in pure `no_std` Rust (Dragonbox, Ryū, or Grisu3 with exact big-integer
    Dragon4 fallback on stack buffers) so shell arithmetic (`$((2**70))`), `awk`, `seq`, and `jq` match V8
    `String(x)` character-for-character across all IEEE-754 `f64` values (EVOL D1).
  - Exact ECMA-262 `toFixed(p)`, `toExponential(p)`, and `toPrecision(p)` decimal formatters (`fmt_f64_fixed`,
    `fmt_f64_exp`, `fmt_f64_prec`) operating on the exact rational value of the IEEE-754 `f64` with
    ties-away-from-zero rounding on the decimal digit cut.
  - Number parsers for all four consumer rules:
    1. `parse_arith_literal` for shell arithmetic (`2..=64` `base#digits`, `0x`/`0X` hex, `0`-prefixed octal
       with `8`/`9` rejection, decimal, clamping above `2^53 - 1 = 9_007_199_254_740_991` to `9_007_199_254_740_991.0`).
    2. `parse_printf_int` for `printf` integer specifiers (`'c` / `"c` / `\'c` / `\"c` character code units,
       leading/trailing whitespace diagnostics, `+` prefix, `0x` hex, positive `0`-prefixed octal, `N#...`
       prefix recovery with error, partial leading decimal `3abc` recovery with error).
    3. `parse_f64_prefix` (JS `parseFloat(s)` prefix parser for `printf` float specifiers and `awk` string-to-number
       coercion).
    4. `parse_f64_strict` (JS `Number(s)` full-string parser for `seq` and numeric comparisons).
- `kernel::format` (`kernel/src/format.rs` or `kernel/src/format/**`):
  - Escape processors for `printf` format strings (`process_format_escapes`) and `%b` operand strings
    (`process_b_escapes`), including `\c` early stop and UTF-8 byte-run decoding differences.
  - `%q` shell-quoting formatter (`shell_quote_printf`) with prospective byte-limit checks.
  - Complete `printf` engine (`format_printf`) supporting `%%`, `%d`, `%i`, `%o`, `%u`, `%x`, `%X`, `%f`, `%F`,
    `%e`, `%E`, `%g`, `%G`, `%c`, `%s`, `%q`, `%b`, `%(fmt)T`, flags `- + ' ' # 0 '`, dynamic `*` width and
    `.*` precision, length modifiers `h l L`, multi-pass argument reuse, soft parse-error reporting (`exit 1`),
    and prospective byte-budget enforcement (`exit 126`).
  - `awk` `sprintf`/`printf`/`OFMT` formatter (`format_awk_printf`) supporting `%n$` 1-based positional
    arguments, `hh ll l z j h` length modifiers, signed `-%x`/`-%o`, `100`-digit float precision limit, and
    `min(10_000, max_bytes)` width/precision cap.
  - Civil-calendar `strftime` engine (`format_strftime`, `parse_tz_spec`) over 64-bit Unix epoch seconds in
    proleptic Gregorian calendar with all 39 directives (`%a`..`%Z`, `%%`), UTC default (EVOL D9, S11),
    `Etc/GMT±N` and fixed-offset `TZ` parsing, and UTC fallback on unknown `TZ`.
- `kernel::hash` (`kernel/src/hash.rs` or `kernel/src/hash/**`):
  - Streaming and one-shot `Md5` (RFC 1321), `Sha1` (FIPS 180-4), and `Sha256` (FIPS 180-4) over raw byte
    slices with lowercase hexadecimal encoding.
- `kernel::base64` (`kernel/src/base64.rs` or `kernel/src/base64/**`):
  - RFC 4648 encoder with configurable line wrapping (`wrap_cols`, default `76`, `0` unwrapped) and
    prospective line-count / string-length / output-size checks.
  - Whitespace-stripping decoder supporting both `Permissive` mode (`base64 -d` parity) and `Strict` mode
    (RFC 4648 validation for `jq @base64d`), with prospective decoded-size / output-size checks.
- Test-only commands `__num`, `__printf`, `__strftime`, `__hash`, and `__base64` in
  `kernel/src/fiber/testcmd.rs` (`cfg(feature = "test-commands")` only, zero bytes in production
  `kernel.core.wasm`), parity adapter `harness/parity/l8b-adapter.mjs`, parity test `test/parity/l8b.test.mjs`,
  vector/limit/fuzz tests `test/l8b-*.test.mjs`.

Out of scope:
- Registering `printf`, `date`, `seq`, `base64`, `md5sum`, `sha1sum`, or `sha256sum` in `kernel/src/cmd/`
  (`COMMAND_NAMES` remains `["echo"]` in W5 so L2 registry assertions stay green; command files and `-v`
  variable assignment land in L5, L9b, and L16c).
- Runtime evaluation of `$((...))` operators, `${var@Q}` single-quote transformation, and `${var@...}`
  parameter operators (L4, consuming `kernel::num` and `kernel::format` at L4 exit).
- Full historical/DST IANA timezone database (`America/New_York`, etc.) inside the Wasm binary (UTC and fixed
  offsets are supported in v1; see section 5.4).

## 3. Allowed paths

```
kernel/src/lib.rs              (add `pub mod base64; pub mod format; pub mod hash; pub mod num;`)
kernel/src/num.rs
kernel/src/num/**              (optional submodule split)
kernel/src/format.rs
kernel/src/format/**           (optional submodule split)
kernel/src/hash.rs
kernel/src/hash/**             (optional submodule split)
kernel/src/base64.rs
kernel/src/base64/**           (optional submodule split)
kernel/src/fiber/testcmd.rs    (add `__num`, `__printf`, `__strftime`, `__hash`, `__base64` under test-commands)
harness/parity/l8b-adapter.mjs
test/parity/l8b.test.mjs
test/l8b-*.test.mjs
BRANCH.md
```

Read-only for this lot: `kernel/src/mem/**`, `kernel/src/limits.rs`, `kernel/src/shell.rs`,
`kernel/src/fiber.rs`, `kernel/src/fiber/{syntax,vm,expand}*`, `kernel/src/cmd/**`, `kernel/src/glue/**`,
`host/**`, `types/**`, `wit/**`, `tools/**`, `test/fixtures/l8b-js-numbers.json`, `test/cases/l8b/**`, and
`test/parity/exclusions.json`. No `BR<n>-EX<k>` scope exception is needed.

### 3.1 W5 merge touchpoints with L4 and L1d

In Wave W5 (PLAN section 3), L4 runs in Slot A while Slot B runs **L8b then L1d** sequentially; both L8b and
L1d merge to `main` before L4's final PR:

| Path | L4 (Slot A) | L8b (Slot B, first) | L1d (Slot B, second) |
|---|---|---|---|
| `kernel/src/lib.rs` | untouched | adds `base64`, `format`, `hash`, `num` | untouched |
| `kernel/src/{num,format,hash,base64}*` | consumes `num` & `format` at exit | owns | untouched |
| `kernel/src/fiber/{syntax,expand}*` | owns | untouched | untouched |
| `kernel/src/fiber/vm.rs` | owns expansion & command hooks | untouched | bounded coverage helper call |
| `kernel/src/{shell,fiber,cmd/mod,glue/adapt}.rs` | consumes `hit_coverage` at exit | untouched | owns trace/coverage/clock plumbing |
| `kernel/src/fiber/testcmd.rs` | untouched | adds `__num`, `__printf`, `__strftime`, `__hash`, `__base64` | branches after L8b; adds `__trace`, `__cov` |
| `host/**`, `harness/parity/sentropic-adapter.mjs` | untouched | untouched | owns logger, drain, browser filter, adapter capture |

## 4. Exact interfaces

### 4.1 `kernel::num`

```rust
use crate::mem::{Budget, KString, MemError};

/// `Number.MAX_SAFE_INTEGER` (`2^53 - 1`), the literal ceiling in `just-bash@3.6.0` arithmetic (EVOL D1).
pub const MAX_SAFE_INT: u64 = 9_007_199_254_740_991;
pub const MAX_SAFE_INT_F64: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithNumError {
    /// Base outside `2..=64`, digit `>= base` (`08`, `2#102`, `16#g`), or malformed `#` literal.
    ValueTooGreatForBase,
    /// Empty or non-numeric token.
    InvalidNumber,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrintfInt {
    pub value: f64,
    /// `true` when `printf` must emit `printf: <raw>: invalid number\n` and exit `1` while still formatting `value`.
    pub invalid: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FloatFmtError {
    /// Precision outside supported bounds (`> 100` in ECMA-262 / `awk`, or `> max_precision` in `printf`).
    PrecisionRange,
    Mem(MemError),
}

impl From<MemError> for FloatFmtError {
    fn from(e: MemError) -> Self { Self::Mem(e) }
}

/// Appends the exact ECMA-262 `Number::toString(val, 10)` representation to `out`.
pub fn fmt_f64_js(val: f64, out: &mut KString) -> Result<(), MemError>;

/// Convenience wrapper returning an owned `KString` in `b`.
pub fn f64_to_js_string(val: f64, b: &Budget) -> Result<KString, MemError>;

/// ECMA-262 `Number.prototype.toFixed(precision)` (`0 <= precision <= 100`, or up to `1_024` for `printf`
/// zero-extension after the significant decimal digits). Rounds exact binary float ties away from zero.
pub fn fmt_f64_fixed(val: f64, precision: usize, b: &Budget) -> Result<KString, FloatFmtError>;

/// ECMA-262 `Number.prototype.toExponential(precision)`. When `two_digit_exp` is `true` (bash `printf`),
/// single-digit exponents are zero-padded to two digits (`e+00`, `E-05`); when `false` (`awk` `sprintf`),
/// JS single-digit exponents (`e+0`, `e-5`) are preserved.
pub fn fmt_f64_exp(
    val: f64,
    precision: usize,
    upper: bool,
    two_digit_exp: bool,
    b: &Budget,
) -> Result<KString, FloatFmtError>;

/// ECMA-262 `Number.prototype.toPrecision(significant)` (`significant >= 1`), with optional trailing-zero
/// stripping (`!keep_zeros`, used by `%g`/`%G` without `#`) and optional 2-digit minimum exponent.
pub fn fmt_f64_prec(
    val: f64,
    significant: usize,
    upper: bool,
    keep_zeros: bool,
    two_digit_exp: bool,
    b: &Budget,
) -> Result<KString, FloatFmtError>;

/// Parses an unsigned arithmetic literal token (`10`, `010`, `0x1f`, `64#@_`) per EVOL D1, clamping values
/// above `MAX_SAFE_INT` to `MAX_SAFE_INT_F64`. Leading `+`/`-` signs are unary operators in the caller.
pub fn parse_arith_literal(raw: &[u8]) -> Result<f64, ArithNumError>;

/// Parses a `printf` integer argument (`%d`, `%i`, `%o`, `%u`, `%x`, `%X`), reproducing character constants,
/// whitespace rules, and partial-parse error recovery.
pub fn parse_printf_int(raw: &[u8]) -> PrintfInt;

/// JavaScript `Number.parseFloat` prefix parser: skips leading JS whitespace, parses the longest valid
/// decimal/scientific prefix or case-sensitive `[+-]?Infinity`, and ignores trailing garbage (`"3.5abc"` -> `Some(3.5)`).
pub fn parse_f64_prefix(raw: &[u8]) -> Option<f64>;

/// JavaScript `Number(s)` full-string parser: trims leading/trailing JS whitespace; empty/blank -> `Some(0.0)`;
/// accepts `0x`/`0X`, `0b`/`0B`, `0o`/`0O` unsigned integer literals and decimal floats or `[+-]?Infinity`;
/// returns `None` (`NaN`) if any unparsed byte remains.
pub fn parse_f64_strict(raw: &[u8]) -> Option<f64>;
```

### 4.2 `kernel::format`

```rust
use crate::mem::{Budget, KString, KVec, MemError};

#[derive(Debug, PartialEq, Eq)]
pub enum FormatLimitKind {
    /// `escaped string size limit exceeded (<rem> bytes)`
    EscapedStringSize { limit_bytes: usize },
    /// `format width limit exceeded (<rem> bytes)` (or `printf width limit exceeded (<cap> bytes)` in `awk`)
    Width { limit_bytes: usize },
    /// `format precision limit exceeded (<rem> bytes)` (or `printf precision limit exceeded (<cap> bytes)` in `awk`)
    Precision { limit_bytes: usize },
    /// `printf floating-point precision limit exceeded (100 digits)` (`awk` only)
    AwkFloatPrecision,
    /// `formatted value size limit exceeded (<rem> bytes)` (or `formatted string size limit exceeded (<cap> bytes)` in `awk`)
    FormattedValueSize { limit_bytes: usize },
    /// `printf: output size limit exceeded (<max> bytes)`
    OutputSize { limit_bytes: usize },
    /// `strftime: output size limit exceeded (<rem> bytes)`
    StrftimeOutputSize { limit_bytes: usize },
    /// `strftime: iteration limit exceeded (<max_ops>)`
    StrftimeIterations { max_ops: usize },
    Mem(MemError),
}

impl From<MemError> for FormatLimitKind {
    fn from(e: MemError) -> Self { Self::Mem(e) }
}

pub struct BEscapeOut {
    pub bytes: KVec<u8>,
    pub stopped: bool,
}

pub struct PrintfOpts<'a> {
    /// Value of `$TZ` from the fiber environment (`None` when unset -> UTC).
    pub tz: Option<&'a [u8]>,
    /// Current Unix timestamp in seconds (virtual clock `0` unless `real-clock` granted), used for `%(...)T` on `""` or `"-1"`.
    pub now_sec: i64,
    /// Shell start Unix timestamp in seconds, used for `%(...)T` on `"-2"`.
    pub start_sec: i64,
    /// Effective cap `min(maxStringLength, maxOutputSize)` in bytes.
    pub max_output_bytes: usize,
}

pub struct PrintfOutput {
    pub bytes: KVec<u8>,
    /// `true` when any integer conversion failed or an invalid placeholder occurred (`exit 1`).
    pub had_error: bool,
    /// Diagnostic line to write to stderr when `had_error` is `true` (e.g. `printf: abc: invalid number\n`).
    pub stderr: KString,
    /// `true` when `%b` encountered `\c`.
    pub stopped: bool,
}

pub fn process_format_escapes(
    raw: &[u8],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind>;

pub fn process_b_escapes(
    raw: &[u8],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<BEscapeOut, FormatLimitKind>;

pub fn shell_quote_printf(
    raw: &[u8],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind>;

pub fn format_printf(
    format: &[u8],
    args: &[&[u8]],
    opts: &PrintfOpts<'_>,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<PrintfOutput, FormatLimitKind>;

#[derive(Clone, Copy, Debug)]
pub enum AwkArg<'a> {
    Num(f64),
    Str(&'a [u8]),
}

pub fn format_awk_printf(
    format: &[u8],
    args: &[AwkArg<'_>],
    max_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TzSpec {
    /// Offset east of UTC in seconds (e.g. `-18_000` for `-0500`, `0` for UTC).
    pub offset_east_sec: i32,
    /// Abbreviated zone label (`"UTC"` by default, up to 8 ASCII bytes).
    pub abbr: [u8; 8],
    pub abbr_len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: i64,
    pub month: u8,   // 1..=12
    pub day: u8,     // 1..=31
    pub hour: u8,    // 0..=23
    pub minute: u8,  // 0..=59
    pub second: u8,  // 0..=59
    pub weekday: u8, // 0 = Sunday .. 6 = Saturday
    pub yday: u16,   // 1..=366
}

pub fn parse_tz_spec(tz: Option<&[u8]>) -> TzSpec;
pub fn civil_from_timestamp(epoch_sec: i64, offset_east_sec: i32) -> CivilDateTime;

pub fn format_strftime(
    format: &[u8],
    epoch_sec: i64,
    tz: Option<&[u8]>,
    max_operations: usize,
    max_output_bytes: usize,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, FormatLimitKind>;
```

### 4.3 `kernel::hash`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlg {
    Md5,
    Sha1,
    Sha256,
}

#[derive(Clone)]
pub struct Md5 { /* 4 x u32 state, 64-byte block buffer, u64 byte count */ }

impl Md5 {
    pub const DIGEST_LEN: usize = 16;
    pub const HEX_LEN: usize = 32;
    pub const fn new() -> Self;
    pub fn update(&mut self, data: &[u8]);
    pub fn finalize(self) -> [u8; 16];
    pub fn digest(data: &[u8]) -> [u8; 16];
    pub fn hex_lower(digest: &[u8; 16]) -> [u8; 32];
}

#[derive(Clone)]
pub struct Sha1 { /* 5 x u32 state, 64-byte block buffer, u64 byte count */ }

impl Sha1 {
    pub const DIGEST_LEN: usize = 20;
    pub const HEX_LEN: usize = 40;
    pub const fn new() -> Self;
    pub fn update(&mut self, data: &[u8]);
    pub fn finalize(self) -> [u8; 20];
    pub fn digest(data: &[u8]) -> [u8; 20];
    pub fn hex_lower(digest: &[u8; 20]) -> [u8; 40];
}

#[derive(Clone)]
pub struct Sha256 { /* 8 x u32 state, 64-byte block buffer, u64 byte count */ }

impl Sha256 {
    pub const DIGEST_LEN: usize = 32;
    pub const HEX_LEN: usize = 64;
    pub const fn new() -> Self;
    pub fn update(&mut self, data: &[u8]);
    pub fn finalize(self) -> [u8; 32];
    pub fn digest(data: &[u8]) -> [u8; 32];
    pub fn hex_lower(digest: &[u8; 32]) -> [u8; 64];
}
```

### 4.4 `kernel::base64`

```rust
use crate::mem::{Budget, KVec, MemError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base64DecodeMode {
    /// Node `Buffer.from(cleaned, "base64")` behavior required for `base64 -d` parity:
    /// strips whitespace, ignores non-alphabet bytes outside `[A-Za-z0-9+/=]`, stops at first `=`,
    /// decodes 2-char and 3-char unpadded tail groups, and drops a trailing single-sextet orphan.
    Permissive,
    /// Strict RFC 4648 validation (after stripping ASCII whitespace): rejects non-alphabet bytes,
    /// `len % 4 == 1`, mid-stream `=`, or >2 `=` pad bytes with `Base64Error::InvalidInput`.
    Strict,
}

#[derive(Clone, Copy, Debug)]
pub struct Base64Limits {
    pub max_string_length: usize,
    pub max_output_size: usize,
    pub max_array_elements: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Base64Error {
    /// `base64: invalid input\n` (exit 1)
    InvalidInput,
    /// `base64: wrapped line limit exceeded (<limit>)\n` (exit 126, `maxArrayElements`)
    WrappedLineLimit { limit: usize },
    /// `base64: encoded size limit exceeded (<limit> bytes)\n` (exit 126, `maxStringLength`)
    EncodedSizeLimit { limit: usize },
    /// `base64: decoded size limit exceeded (<limit> bytes)\n` (exit 126, `maxStringLength`)
    DecodedSizeLimit { limit: usize },
    /// `base64: output size limit exceeded (<limit> bytes)\n` (exit 126, `maxOutputSize`)
    OutputSizeLimit { limit: usize },
    FuelExhausted,
    Mem(MemError),
}

impl From<MemError> for Base64Error {
    fn from(e: MemError) -> Self { Self::Mem(e) }
}

pub fn encoded_output_len(input_len: usize, wrap_cols: usize) -> Option<(usize, usize)>;

pub fn encode(
    input: &[u8],
    wrap_cols: usize,
    limits: Base64Limits,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, Base64Error>;

pub fn decode(
    input: &[u8],
    mode: Base64DecodeMode,
    limits: Base64Limits,
    b: &Budget,
    work: &mut dyn FnMut(u64) -> Result<(), ()>,
) -> Result<KVec<u8>, Base64Error>;
```

## 5. Complete behavioural specification

### 5.1 JavaScript-compatible `f64` formatting (`kernel::num`)

1. **Shortest round-trip formatter (`fmt_f64_js`, ECMA-262 section 6.1.6.1.20 `Number::toString(x, 10)`):**
   - Special IEEE-754 values:
     - `x.is_nan()` -> `"NaN"`.
     - `x == 0.0` (both `+0.0` and `-0.0`) -> `"0"`.
     - `x == f64::INFINITY` -> `"Infinity"`; `x == f64::NEG_INFINITY` -> `"-Infinity"`.
   - Negative finite `x < 0.0`: emit `'-'` followed by `fmt_f64_js(-x)`.
   - Positive finite `x > 0.0`: find the unique `(s, k, n)` with integers `k >= 1` and `10^(k-1) <= s < 10^k`
     such that `s * 10^(n - k)` rounds to the exact IEEE-754 `f64` bit pattern of `x` under IEEE round-to-nearest-even,
     with `k` **minimal** (shortest decimal significand) and, when two `s` of that minimal length `k` both round
     to `x`, choosing the `s` closer to the exact mathematical value of `x` (and on an exact tie — which occurs
     only when `x` has an even significand — choosing the even digit `s` per ECMA-262):
     - Implement Dragonbox or Ryū (128-bit carrier tables in `static` data, zero heap allocation), or Grisu3
       with an on-stack fixed-capacity big-uint (`[u32; 40]`) Burger-Dybvig / Dragon4 fallback for the `< 0.5%`
       uncertain midpoint cases. Rust's standard `core::fmt::Display for f64` uses Ryū for raw digits **but** never
       uses scientific notation (`write!("{val}")` prints `1180591620717411300000` for `2f64.powi(70)` instead
       of `"1.1805916207174113e+21"`!), so either extract the shortest `(digits, decimal_exponent)` from a
       self-contained Dragonbox/Ryū core or parse/reformat exact shortest digits from a stack buffer, applying
       ECMA-262's exact radix placement rules below:
     - Let `n` be the decimal exponent such that `x ≈ 0.d_1 d_2 ... d_k * 10^n` (so `1.0` has `k = 1, n = 1`;
       `1e20` has `k = 1, n = 21`; `1e21` has `k = 1, n = 22`; `1e-6` has `k = 1, n = -5`; `1e-7` has
       `k = 1, n = -6`):
       1. **`k <= n <= 21`** (`1 <= x < 10^21` with integer value at this precision, e.g. `1e20`, `2**63`):
          emit the `k` digits of `s` followed by `n - k` ASCII `'0'` characters (`1e20` -> `"100000000000000000000"`,
          `2**63` -> `"9223372036854776000"`).
       2. **`0 < n < k` and `n <= 21`** (fractional fixed form >= `1`, e.g. `12.34` has `k = 4, n = 2`):
          emit the first `n` digits of `s`, `'.'`, and the remaining `k - n` digits of `s`.
       3. **`-5 <= n <= 0`** (`10^-6 <= x < 1`, e.g. `1e-6` -> `"0.000001"`):
          emit `"0."`, `-n` ASCII `'0'` characters, and the `k` digits of `s`.
       4. **`n <= -6` or `n > 21`** (`x < 10^-6` or `x >= 10^21`, e.g. `1e-7` -> `"1e-7"`, `1e21` -> `"1e+21"`,
          `2**70` -> `"1.1805916207174113e+21"`):
          - If `k == 1`, emit `d_1`; otherwise emit `d_1`, `'.'`, and `d_2..d_k`.
          - Emit `'e'`, `'+'` if `n - 1 >= 0` else `'-'`, and the decimal digits of `|n - 1|` **without**
            leading zeros (`1e+21`, `1e-7`, `5e-324`).
2. **Exact binary-float `toFixed`, `toExponential`, and `toPrecision` (`fmt_f64_fixed`, `fmt_f64_exp`, `fmt_f64_prec`):**
   - In ECMA-262 (`toFixed`, `toExponential`, `toPrecision`), non-finite values (`NaN`, `±Infinity`) format via
     `fmt_f64_js` (`"NaN"`, `"Infinity"`, `"-Infinity"`), and `-0.0` is normalized to `+0.0` (so `(-0.0).toFixed(2)`
     is `"0.00"`, never `"-0.00"`).
   - For finite `|x| >= 10^21`, `fmt_f64_fixed` delegates to `fmt_f64_js(x)` exactly as `Number.prototype.toFixed`
     does (`(1e21).toFixed(2)` -> `"1e+21"`).
   - For finite values within fixed/exponential/precision range, compute digits from the **exact mathematical
     real value** `|x| = m * 2^e` of the IEEE-754 double (using stack big-integer arithmetic; do **not** round
     the shortest Ryū string, because `(1.15)` in IEEE-754 is `1.14999999999999991118...`, so `(1.15).toFixed(1)`
     is `"1.1"` whereas `(1.25).toFixed(1)` is `"1.3"`!). On an exact decimal half-way tie (`0.5`, `2.5`, `1.25`),
     round the magnitude **up (away from zero)** (`0.5 -> "1"`, `2.5 -> "3"`), matching ECMA-262!
   - Exponent formatting:
     - When `two_digit_exp == true` (bash `printf` `%e`/`%E`/`%g`/`%G`), pad 1-digit exponents to 2 digits
       (`1.500000e+00`, `1e+06`, `1.000000e+100`).
     - When `two_digit_exp == false` (`awk` `sprintf`), keep 1-digit exponents (`1.500000e+0`).
     - When `upper == true` (`%E`, `%G`), uppercase `'e'` -> `'E'` and `"Infinity"` -> `"INFINITY"`, `"NaN"` ->
       `"NAN"`. Note: `%F` keeps `"Infinity"` and `"-Infinity"` mixed-case in `just-bash@3.6.0` (verified:
       `printf '%F|%E|%G' Infinity Infinity Infinity` -> `Infinity|INFINITY|INFINITY`).

### 5.2 Number parsers (`kernel::num`)

1. **`parse_arith_literal(raw)` (for L4 shell arithmetic):**
   - If `raw` contains `b'#'`:
     - Split on `b'#'`; if there are not **two** non-empty parts `(base_str, digits)` or `base_str` is not all
       ASCII digits `[0-9]+`, return `Err(ArithNumError::ValueTooGreatForBase)`.
     - Parse `base` in decimal; if `base < 2 || base > 64`, return `Err(ArithNumError::ValueTooGreatForBase)`.
     - Accumulate `acc: f64 = 0.0` across every byte `ch` of `digits`:
       - `'0'..='9'` -> `0..=9`;
       - `'a'..='z'` -> `10..=35`;
       - `'A'..='Z'` -> `10..=35` when `base <= 36`, else `36..=61` when `base >= 37`;
       - `'@'` -> `62`; `'_'` -> `63`;
       - any other byte, or `digit >= base` -> `Err(ArithNumError::ValueTooGreatForBase)` (never stop at a
         valid prefix: `2#10x` is an error!).
       - `acc = acc * (base as f64) + (digit as f64)`; if `acc > MAX_SAFE_INT_F64`, return `Ok(MAX_SAFE_INT_F64)`.
     - Return `Ok(acc)`.
   - If `raw` starts with `b"0x"` or `b"0X"`:
     - Require at least one ASCII hex digit after `0x` (and all remaining bytes hex digits; otherwise
       `Err(ArithNumError::InvalidNumber)`), accumulate in base 16, clamping `> MAX_SAFE_INT_F64` to
       `MAX_SAFE_INT_F64`.
   - If `raw.len() > 1 && raw[0] == b'0'` and all bytes are ASCII digits `[0-9]`:
     - If any byte is `b'8'` or `b'9'`, return `Err(ArithNumError::ValueTooGreatForBase)` (`$((08))` ->
       `bash: value too great for base\n`, exit `1`).
     - Otherwise accumulate in base 8, clamping `> MAX_SAFE_INT_F64` to `MAX_SAFE_INT_F64`.
   - Otherwise require non-empty `[0-9]+`, accumulate in base 10, clamping `> MAX_SAFE_INT_F64` to
     `MAX_SAFE_INT_F64`.
2. **`parse_printf_int(raw)` (verified against `just-bash@3.6.0`):**
   - Decode `raw` as UTF-8 with per-byte Latin-1 fallback (same recovery table as section 5.3 item 1) into scalar
     sequence `S` so `'é` yields `233` (`U+00E9`).
   - Trim leading JS whitespace (`0x09..=0x0D`, `0x20`, `0xA0`, `0x1680`, `0x2000..=0x200A`, `0x2028`, `0x2029`,
     `0x202F`, `0x205F`, `0x3000`, `0xFEFF`). Check whether any trailing JS whitespace exists (`has_trailing_ws`),
     then trim trailing JS whitespace to obtain `A`.
   - **Character constants:**
     - If `A` starts with `'\''` or `'"'` and has at least 2 UTF-16 code units: return `PrintfInt { value: code_unit_1 as f64, invalid: false }` (trailing whitespace does **not** set `invalid` here; a lone `'` of length 1 falls through and yields `value: 0.0, invalid: true`).
     - If `A` starts with `"\\'"` or `"\\\""` and has at least 3 UTF-16 code units: return `PrintfInt { value: code_unit_2 as f64, invalid: false }`.
   - **Leading `+`:** If `A` starts with `'+'`, strip that single `'+'`.
   - **Hexadecimal (`0x` / `0X`):** Notice `-0x10` did **not** strip `'-'` before this check!
     - If `A` starts with `"0x"` or `"0X"`: parse the leading run of `[0-9a-fA-F]` after `0x` as base-16 `f64`
       (JS `parseInt(A, 16)` semantics: `"0x10"` -> `16`, `"0x10zz"` -> `16` without error!, `"0x"` -> `0` with
       `invalid: true`). If valid hex digits existed, `invalid = has_trailing_ws`. Notice `"-0x10"` does **not**
       enter this branch; it falls through to the decimal fallback, which parses `"-0"` -> `0.0` with `invalid: true`!
   - **Octal (`0[0-7]+`):**
     - If `A` starts with `'0'`, `A.len() > 1`, and `A` matches `^0[0-7]+$`: parse in base 8 as `f64`, with
       `invalid = has_trailing_ws`.
     - Notice: `+010` stripped `'+'` above so `A = "010"` -> `8`; whereas `-010` starts with `'-'`, so it skips
       this branch and parses as decimal **`-10`** (`invalid: false`), and `08` skips this branch because of `'8'`
       and parses as decimal **`8`** (`invalid: false`)!
   - **Arbitrary-base prefix (`^\d+#`):**
     - If `A` starts with one or more ASCII digits followed by `'#'` (`64#a`): parse the digits before `'#'` in
       base 10 and return `PrintfInt { value: prefix, invalid: true }`.
   - **Decimal and partial-decimal fallback:**
     - If `A` is empty `""`: return `PrintfInt { value: 0.0, invalid: has_trailing_ws }` (`printf '%d' ''` -> `0`,
       exit `0`; `printf '%d' '   '` -> `0`, exit `0` because `trimmed` is empty so `has_trailing_ws` is `false`!).
     - If `A` matches `^-?[0-9]+$`: parse in base 10 as IEEE-754 `f64` (note: numbers > `2^53` round to the
       nearest IEEE-754 `f64`, e.g. `9223372036854775807` -> `9223372036854776000`, and `10^29` formats via
       `fmt_f64_js` as `"1e+29"`!), with `invalid = has_trailing_ws`.
     - Otherwise (`"3abc"`, `"abc"`, `"-0x10"`): set `invalid = true`, and parse any leading `^[+-]?[0-9]+`
       run in base 10 (or `0.0` if none; normalize `-0.0` to `0.0`).

### 5.3 `printf` engine, escapes, `%q`, limits, and `awk` `sprintf` (`kernel::format`)

1. **UTF-8 recovery rule (`decode_utf8_recovery`):**
   - Valid UTF-8 1..=4-byte sequences (`0x00..=0x7F`, non-overlong 2-byte `0xC2..=0xDF`, non-overlong/non-surrogate
     3-byte `0xE0..=0xEF`, non-overlong `<= 0x10FFFF` 4-byte `0xF0..=0xF4`) decode to their Unicode scalar value.
   - Any ill-formed lead or lone continuation byte `b` (`0x80..=0xFF`) is preserved as scalar `U+00XX`
     (`char::from(b)`), which re-encodes in UTF-8 text output as 2 bytes (`0xC2/0xC3 0x80..0xBF`).
2. **Escape-processing comparison (`process_format_escapes` vs `process_b_escapes`):**
   - Prospective input check: if `utf8_byte_len(raw) > max_bytes`, fail immediately before scanning:
     - `process_format_escapes` -> `FormatLimitKind::EscapedStringSize { limit_bytes: max_bytes }`;
     - `process_b_escapes` -> `FormatLimitKind::OutputSize { limit_bytes: max_bytes }`.

| Sequence | In format string (`process_format_escapes`) | In `%b` argument (`process_b_escapes`) |
|---|---|---|
| `\a \b \f \n \r \t \v \\` | `0x07 0x08 0x0C 0x0A 0x0D 0x09 0x0B 0x5C` | Same |
| `\e`, `\E` | `0x1B` (ESC) | **Not recognized** — kept literally as `\e` / `\E` |
| `\c` | **Not recognized** — kept literally as `\c` | **Stops all output immediately** (`stopped = true`; rest of `%b` arg, rest of format string, and remaining args are discarded) |
| Octal | `\[0-7]{1,3}` (1 to 3 octal digits starting right after `\`, so `\0101` consumes `\010` = `0x08` + `'1'`; `\777` = scalar `U+01FF` `ǿ`) | `\0[0-7]{0,3}` (up to 3 octal digits **after** `\0`, so `\0101` = `'A'`, `\0` = NUL, `\0777` = `U+01FF` `ǿ`) **and** `\[1-7][0-7]{0,2}` (1 to 3 digits when non-zero lead) |
| `\x` hex | Requires **2** hex digits `\x[0-9a-fA-F]{2}` (`\x4` stays literal `\x4`!). Consecutive `\xHH` escapes are collected into a byte slice: if the **entire** slice is valid UTF-8, emit the decoded UTF-8 scalars; if **any** byte in the run is invalid UTF-8, emit **every** byte `b` in the run as scalar `U+00XX`! | Accepts **1 or 2** hex digits `\x[0-9a-fA-F]{1,2}` (`\x4g` -> `0x04` + `'g'`; `\xz` -> literal `\xz`). Consecutive `\x` escapes are decoded with per-sequence `decode_utf8_recovery`. |
| `\u` Unicode | `\u[0-9a-fA-F]{1,4}` -> scalar `U+XXXX` (no hex -> literal `\u`) | Same (`\u[0-9a-fA-F]{1,4}`) |
| `\U` Unicode | `\U[0-9a-fA-F]{1,8}` -> scalar `U+XXXXXXXX` (no hex -> literal `\U`) | **Not recognized** — kept literally as `\U` |
| Other `\X` or trailing `\` | Kept literally (`\` emitted, scanner advances 1 char) | Same |

3. **`%q` shell quoting (`shell_quote_printf`):**
   - Empty string `""` -> `"''"` (requires `max_bytes >= 2`, else `OutputSize { limit_bytes: max_bytes }`).
   - Safe unquoted ASCII `^[a-zA-Z0-9_./-]+$` -> returned verbatim (checked against `max_bytes`).
   - Check every scalar `c` of the decoded string: if **any** scalar has code point `c <= 0x1F` or
     `0x7F <= c <= 0xFF` (`needs_dollar_quote` — note scalars `c > 0xFF` such as `'é'` `U+00E9`? **Careful!**
     `U+00E9` IS `0xE9` (`233`), which IS in `0x7F..=0xFF`! Verified in `just-bash@3.6.0`: `printf '%q' 'é'`
     outputs `$'\351'`, whereas `c >= 0x0100` such as `'α'` or `'😀'` is `> 0xFF` and stays unescaped!):
     - When `needs_dollar_quote` is `true`, wrap in `$'...'`, escaping `'` `\` `\n` `\t` `\r` `\a`(`0x07`)
       `\b`(`0x08`) `\f`(`0x0C`) `\v`(`0x0B`) `\E`(`0x1B`) `"` with their 2-byte escape, any other `0x00..=0x1F`
       or `0x7F..=0xFF` as 3-digit octal `\NNN`, and scalars `> 0xFF` or printable ASCII verbatim.
     - When `needs_dollar_quote` is `false`, prefix each character in `` \t|&;<>()$`\"'*?[#~=%!{}`` with `\`.
   - Accumulate prospective UTF-8 byte length per character before allocating/appending; if it exceeds
     `max_bytes`, abort immediately with `OutputSize { limit_bytes: max_bytes }`.
4. **Specifier, width, precision, and limit rules in `format_printf`:**
   - First run `process_format_escapes(format, opts.max_output_bytes)`.
   - Loop `format_once` starting at `arg_pos = 0` while `arg_pos < args.len() && args_consumed_in_pass > 0`
     (so `printf 'plain' a b` runs **once** because `args_consumed == 0`, while `printf '%s\n' a b` runs twice!).
   - At each step inside `format_once`, let `rem = opts.max_output_bytes - total_emitted_utf8_bytes`:
     - Literal `%` at end of format (`i + 1 == format.len()`) -> append `"%"`.
     - `%%` -> append `"%"` (`%10%` or `%-5%` is **not** valid `%%`; it falls through to unknown placeholder
       `printf: [sprintf] unexpected placeholder\n`, exit `1`!).
     - **`%[-?\d*][.\d+](fmt)T` (`strftime`):**
       - Parse optional signed integer width and optional `.precision`, checking `|width| <= rem` (`Width { limit_bytes: rem }`)
         and `precision <= rem` (`Precision { limit_bytes: rem }`).
       - Consume 1 argument (`""` if exhausted): `""`, `"-1"`, or `"-2"` selects `opts.now_sec` (`"-2"` selects
         `opts.start_sec`); otherwise leading decimal integer (`parseInt(arg, 10) || 0`).
       - Call `format_strftime(fmt, ts, opts.tz, rem, rem)`, then apply string precision (truncate to `precision`
         UTF-16 code units when `>= 0`) and space padding to `|width|` (left-aligned if `width < 0`, checking
         prospective padded bytes `<= rem` -> `FormattedValueSize { limit_bytes: rem }`). Notice `%()T` (empty
         `fmt`) emits `""`!
     - **General specifiers `%[+-0 #']*[width|*][.[precision|*]][hlL]<spec>`:**
       - If `*` width: consume 1 arg, parse `w = leading_int_or_0(arg)`, check `|w| <= rem` (and safe integer) ->
         else `Width { limit_bytes: rem }`; negative `w` sets `-` (left-align) and `width = |w|`.
       - If `.*` precision: consume 1 arg, parse `p = leading_int_or_0(arg)`; if `p >= 0`, check `p <= rem` ->
         else `Precision { limit_bytes: rem }`; if `p < 0` (and safe integer), precision is **omitted** (`None`)!
       - Check literal `width <= rem` (`Width { limit_bytes: rem }`) and literal `precision <= rem`
         (`Precision { limit_bytes: rem }`).
       - Consume 1 value argument (`""` if exhausted):
         - **`d`, `i`:** `parse_printf_int(arg)`; `abs_str = fmt_f64_js(|val|)`; if `precision >= 0`, zero-pad
           `abs_str` on the left to `precision` chars (note: `%.0d` on `0` still emits `"0"`, **not** `""`!);
           prepend sign (`"-"` if `val < 0`, else `"+"` if `+` flag, else `" "` if space flag); pad to `width`
           (`-` -> right spaces; `0` flag **and** no precision -> zeros after sign; else left spaces).
         - **`u`:** `parse_printf_int(arg)`; if `val < 0.0`, convert to 32-bit unsigned `(val as i64 as i32 as u32) as f64`
           (`-1` -> `4294967295`); format identically to `d` (`+` and `' '` flags still apply since `unsigned >= 0`!).
         - **`o`:** `parse_printf_int(arg)`; base-8 digits of `|val| as u64` (**no** minus sign on negative inputs:
           `-8` -> `"10"`, `%#o` on `-8` -> `"010"`); zero-pad to `precision`; if `#` flag and string does not
           start with `'0'`, prepend `'0'`; apply width (`-`, `0` when no precision, else spaces).
         - **`x`, `X`:** `parse_printf_int(arg)`; base-16 digits of `|val| as u64` (lowercase for `x`, uppercase
           for `X`, **no** minus sign on negative inputs: `-16` -> `"10"`; `+` and `' '` flags ignored); zero-pad
           to `precision`; if `#` flag and `val != 0.0`, prepend `"0x"` / `"0X"`; apply width (zero-pad after
           `0x`/`0X` when `0` flag and no precision).
         - **`f`, `F`, `e`, `E`, `g`, `G`:** `val = parse_f64_prefix(arg).unwrap_or(0.0)` (if `val.is_nan()`,
           replace with `0.0`! Verified in `just-bash@3.6.0`: `printf '%f|%+10.2f' NaN NaN` ->
           `0.000000|     +0.00`, whereas `Infinity` and `-Infinity` format as `Infinity` / `-Infinity` / `INFINITY`!);
           normalize `-0.0` to `+0.0`; default precision is `6` (`%.f` / `%.e` / `%.g` -> precision `0`);
           - `f`/`F`: `fmt_f64_fixed(|val|, precision)`; if `#` and `precision == 0` and finite, append `'.'`.
           - `e`/`E`: `fmt_f64_exp(|val|, precision, spec == 'E', true)`.
           - `g`/`G`: `fmt_f64_prec(|val|, precision.max(1), spec == 'G', has_hash_flag, true)`.
           Prepend sign (`"-"` if `val < 0`, `"+"` if `+`, `" "` if `' '`); apply width (`-`, `0` after sign for
           finite or `Infinity`, else left spaces).
         - **`c`:** if `arg.is_empty()` -> `""`; otherwise take the **first byte** `b0` of `arg` and emit scalar
           `char::from(b0)` (`'é'` `[0xC3, 0xA9]` -> `U+00C3` `'Ã'`, `'😀'` `[0xF0, ...]` -> `U+00F0` `'ð'`).
         - **`s`:** ignore `0`, `+`, `' '`, `#` flags; truncate to `precision` UTF-16 code units when `precision >= 0`;
           pad with spaces to `width` (checking prospective bytes `<= rem` -> `FormattedValueSize { limit_bytes: rem }`).
         - **`q`:** run `shell_quote_printf(arg, rem)`, then pad with spaces to `width` (`-` left-aligns;
           precision is not accepted on `%q`, falling through to unknown placeholder if given).
         - **`b`:** run `process_b_escapes(arg, rem)`; if `stopped`, append `bytes` and terminate both `format_once`
           and the outer reuse loop immediately!
         - **Any other specifier:** set `had_error = true`, `stderr = "printf: [sprintf] unexpected placeholder\n"`,
           and emit `""`.
   - Whenever an integer conversion sets `invalid == true`, record `had_error = true` and `stderr = format!("printf: {arg}: invalid number\n")` (preserving the formatted fallback value in `bytes`).
5. **`awk` `sprintf`/`OFMT` mode (`format_awk_printf`):**
   - Differs from bash `format_printf` in five exact ways (verified on `just-bash@3.6.0`):
     1. Does **not** run format-string escapes upfront; only `\n`, `\t`, `\r`, `\\`, and `\X -> X` are decoded
        inline outside `%` directives, and there is **no** outer argument-reuse loop.
     2. Supports `%n$` 1-based positional argument indexing (`%2$s-%1$04d`), and skips `hh`, `ll`, `l`, `z`, `j`,
        `h` length modifiers.
     3. Field width and precision cap is `cap = min(10_000, max_bytes)` (`Width { limit_bytes: cap }` /
        `Precision { limit_bytes: cap }`), and float precision `> 100` returns `AwkFloatPrecision`
        (`printf floating-point precision limit exceeded (100 digits)`).
     4. `%x`, `%X`, and `%o` preserve `"-"` on negative numbers (`-16` -> `"-10"`), `%e`/`%E` use 1-digit
        JS exponents (`e+0`), and `%c` on `AwkArg::Num(n)` emits `char::from_u32(n as u32)` while on
        `AwkArg::Str(s)` emits the first Unicode char of `s`.
     5. Unknown `%z` emits the raw directive text (`"%z"`) literally without error.

### 5.4 Civil-calendar `strftime` and timezone rules (`kernel::format`)

1. **Timezone resolution (`parse_tz_spec`, EVOL D9 & S11):**
   - Unset (`None`), empty `""`, `"UTC"`, `"GMT"`, `"UCT"`, `"Universal"`, `"Zulu"`, `"Etc/UTC"`, `"Etc/GMT"`,
     `"Etc/GMT+0"`, `"Etc/GMT-0"`, `"Etc/GMT0"`, `"Z"`: `offset_east_sec = 0`, `abbr = "UTC"` (`%Z` = `"UTC"`,
     `%z` = `"+0000"` — note `TZ=GMT` outputs `%Z = "UTC"` in `just-bash@3.6.0`!).
   - `Etc/GMT+H` or `Etc/GMT-H` (`0 <= H <= 14`): apply POSIX-inverted IANA sign (`Etc/GMT+5` ->
     `offset_east_sec = -5 * 3600`, `abbr = "GMT-5"`, `%z = "-0500"`; `Etc/GMT-2` -> `+7200`, `%z = "+0200"`).
   - Fixed ISO / POSIX offset strings:
     - `[+-]HH[:MM]` or `(UTC|GMT)[+-]HH[:MM]` where an explicit ISO `±HH:MM` or POSIX `STD[+-]HH[:MM[:SS]]` is
       given: for POSIX `STD[+-]h[:m[:s]]` (`^[A-Za-z]{3,6}[+-]?\d{1,2}(:\d{2}(:\d{2})?)?$`, e.g. `EST5` or
       `UTC+5`), POSIX sign is west-of-UTC (`EST5` -> `-0500`, `abbr = "EST"`); for `<±HHMM>[+-]...` bracketed
       POSIX names, `abbr` is the inner `<...>` without `<`/`>`.
   - Any unrecognized `TZ` value falls back to **UTC** (`offset_east_sec = 0`, `abbr = "UTC"`), **never** to
     the host machine's timezone (enforcing EVOL S11).
2. **Proleptic Gregorian civil conversion (`civil_from_timestamp`):**
   - Shift `local_sec = epoch_sec.saturating_add(offset_east_sec as i64)`.
   - Split using Euclidean floor division: `days = local_sec.div_euclid(86_400)`,
     `rem = local_sec.rem_euclid(86_400)` (`hour = rem / 3600`, `minute = (rem % 3600) / 60`, `second = rem % 60`).
   - Compute `(year, month, day, yday)` from `days` (`1970-01-01` = day `0`) via Howard Hinnant's `civil_from_days`
     algorithm (exact across all `i64` timestamps, leap years: `(y % 4 == 0 && y % 100 != 0) || y % 400 == 0`).
   - `weekday = (days + 4).rem_euclid(7) as u8` (`0` = Sunday .. `4` = Thursday for `1970-01-01` .. `6` = Saturday).
3. **Complete 39-directive table (`format_strftime`):**
   - Charge `1` operation per loop step (`operations > max_operations` -> `StrftimeIterations { max_ops }`) and
     `work(1)` every `64` steps; prospective output > `max_output_bytes` -> `StrftimeOutputSize`.
   - Unknown `%Q` (or trailing lone `%`) emits literal `%` and advances by 1 byte so `Q` is emitted next (`"%Q"`).

| Directive | Exact output (English C/en-US locale) |
|---|---|
| `%a` | `"Sun"`, `"Mon"`, `"Tue"`, `"Wed"`, `"Thu"`, `"Fri"`, `"Sat"` |
| `%A` | `"Sunday"` .. `"Saturday"` |
| `%b`, `%h` | `"Jan"`, `"Feb"`, `"Mar"`, `"Apr"`, `"May"`, `"Jun"`, `"Jul"`, `"Aug"`, `"Sep"`, `"Oct"`, `"Nov"`, `"Dec"` |
| `%B` | `"January"` .. `"December"` |
| `%c` | `"%a %b %e %H:%M:%S %Y"` (e.g. `"Thu Jan  1 00:00:00 1970"`) |
| `%C` | `year.div_euclid(100)` zero-padded to 2 digits (`"19"`, `"20"`) |
| `%d` | Day of month `01..=31` (zero-padded) |
| `%D`, `%x` | `"%m/%d/%y"` (e.g. `"01/01/70"`) |
| `%e` | Day of month ` 1..=31` (space-padded to 2 chars) |
| `%F` | `"%Y-%m-%d"` (e.g. `"1970-01-01"`) |
| `%G` | ISO-8601 week-based full year (e.g. `"1970"` for `1969-12-31`!) |
| `%g` | `iso_week_year.rem_euclid(100)` zero-padded to 2 digits |
| `%H` | Hour `00..=23` (zero-padded) |
| `%I` | 12-hour clock `01..=12` (`0 -> 12`, zero-padded) |
| `%j` | Day of year `001..=366` (zero-padded to 3 digits) |
| `%k` | Hour ` 0..=23` (space-padded to 2 chars) |
| `%l` | 12-hour clock ` 1..=12` (space-padded to 2 chars) |
| `%m` | Month `01..=12` (zero-padded) |
| `%M` | Minute `00..=59` (zero-padded) |
| `%n` | `"\n"` |
| `%N` | `"000000000"` (9 zeros; second-granularity timestamps) |
| `%p` | `"AM"` if `hour < 12` else `"PM"` |
| `%P` | `"am"` if `hour < 12` else `"pm"` |
| `%r` | `"%I:%M:%S %p"` (e.g. `"12:00:00 AM"`) |
| `%R` | `"%H:%M"` |
| `%s` | Decimal `epoch_sec` (always true UTC epoch seconds, unaffected by `TZ`!) |
| `%S` | Second `00..=59` (zero-padded) |
| `%t` | `"\t"` |
| `%T`, `%X` | `"%H:%M:%S"` |
| `%u` | ISO weekday `1..=7` (`1` = Monday .. `7` = Sunday) |
| `%U` | Sunday-first week number `00..=53`: `((yday - 1 + (7 - weekday_sun) % 7_prev_sun) ...)` — specifically, days since first Sunday of year: if `yday - 1 < first_sunday_offset`, `00`, else `1 + (yday - 1 - first_sunday_offset) / 7`, where `jan1_wday = (weekday + 7 - (yday - 1) % 7) % 7` and `first_sunday_offset = (7 - jan1_wday) % 7` (`1970-01-01` Thu -> `"00"`) |
| `%V` | ISO-8601 week number `01..=53` (Monday start; week 1 contains Jan 4 / first Thursday; `1969-12-31` -> `"01"`, `1970-01-01` -> `"01"`) |
| `%w` | Weekday `0..=6` (`0` = Sunday .. `6` = Saturday) |
| `%W` | Monday-first week number `00..=53`: `first_monday_offset = (8 - jan1_wday) % 7` (`1970-01-01` Thu -> `"00"`) |
| `%y` | `year.rem_euclid(100)` zero-padded to 2 digits |
| `%Y` | Full year (`"1970"`, `"-1"` if negative) |
| `%z` | Offset `±HHMM` (`"+0000"`, `"-0500"`) |
| `%Z` | Zone abbreviation (`"UTC"`, `"EST"`, etc.) |
| `%%` | `"%"` |

### 5.5 Cryptographic / checksum digests (`kernel::hash`)

1. **`Md5` (RFC 1321):**
   - Initial state: `a0 = 0x67452301`, `b0 = 0xefcdab89`, `c0 = 0x98badcfe`, `d0 = 0x10325476`.
   - 64-byte blocks decoded as 16 **little-endian** `u32` words; 64 rounds with standard sine-table constants
     `K[0..64]` and left-rotation amounts `[7, 12, 17, 22]`, `[5, 9, 14, 20]`, `[4, 11, 16, 23]`, `[6, 10, 15, 21]`.
   - Finalization appends `0x80`, zero bytes to `len % 64 == 56`, and 64-bit bit-length `(total_bytes as u64).wrapping_mul(8)`
     in **little-endian**, outputting `a0, b0, c0, d0` in little-endian (`16` bytes).
2. **`Sha1` (FIPS 180-4):**
   - Initial state: `0x67452301`, `0xefcdab89`, `0x98badcfe`, `0x10325476`, `0xc3d2e1f0`.
   - 64-byte blocks decoded as 16 **big-endian** `u32` words expanded to 80 words (`W[t] = (W[t-3] ^ W[t-8] ^ W[t-14] ^ W[t-16]).rotate_left(1)`);
     80 rounds; `0x80` + zeros + **big-endian** 64-bit bit-length; big-endian `20`-byte output.
3. **`Sha256` (FIPS 180-4):**
   - Initial state `H0..H7 = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]`,
     standard 64 cube-root round constants `K[0..64]`, 64-byte big-endian blocks, big-endian 64-bit bit-length
     padding, big-endian `32`-byte output.
4. All three implementations allocate **zero** heap bytes (`new`, `update`, `finalize` live completely on the
   caller's stack/struct). Callers streaming large files charge `work(1)` per `4 KiB` fed to `update`.

### 5.6 `base64` encode and decode (`kernel::base64`)

1. **Encode (`encode`):**
   - Alphabet: `ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/` with `'='` padding.
   - `encoded_len = input.len().div_ceil(3) * 4`.
   - `newline_count = if wrap_cols > 0 && encoded_len > 0 { encoded_len.div_ceil(wrap_cols) } else { 0 }`.
   - Prospective limit checks in exact `just-bash@3.6.0` order:
     1. `newline_count > limits.max_array_elements` -> `Err(Base64Error::WrappedLineLimit { limit: limits.max_array_elements })`;
     2. `encoded_len > limits.max_string_length` -> `Err(Base64Error::EncodedSizeLimit { limit: limits.max_string_length })`;
     3. `encoded_len + newline_count > limits.max_output_size` -> `Err(Base64Error::OutputSizeLimit { limit: limits.max_output_size })`.
   - Empty `input` (`b""`) produces `b""` (zero bytes, **no** trailing newline even when `wrap_cols > 0`!).
   - When `wrap_cols > 0 && encoded_len > 0`, insert `b'\n'` after every `wrap_cols` base64 chars and ensure
     a single trailing `b'\n'` at the end; when `wrap_cols == 0`, emit no `b'\n'`.
   - Charge `work(1)` per `1_024` input bytes.
2. **Decode (`decode`):**
   - Strip ASCII/UTF-8 whitespace first when computing `cleaned_len` and prospective upper bound
     `est_len = (cleaned_len.div_ceil(4) * 3).saturating_sub(trailing_pad_count)`:
     1. `est_len > limits.max_string_length` -> `Err(Base64Error::DecodedSizeLimit { limit: limits.max_string_length })`;
     2. `est_len > limits.max_output_size` -> `Err(Base64Error::OutputSizeLimit { limit: limits.max_output_size })`.
   - **`Base64DecodeMode::Permissive` (`base64 -d` parity):**
     - Scan `input`: stop immediately on `b'='`; ignore any byte outside `[A-Za-z0-9+/]`; pack valid 6-bit
       sextets in groups of 4 -> 3 bytes; at end-of-input, if 2 sextets remain emit 1 byte (`(s0 << 2) | (s1 >> 4)`),
       if 3 sextets remain emit 2 bytes, if 1 sextet remains drop it (`"Y"` -> `b""`, `"YW"` -> `b"a"`,
       `"YWJ"` -> `b"ab"`, `"YW!Jj"` -> `b"abc"`, `"YW=Jj"` -> `b"a"`).
   - **`Base64DecodeMode::Strict`:**
     - Skip ASCII whitespace (`' '`, `'\t'`, `'\n'`, `'\r'`, `'\x0B'`, `'\x0C'`). Any other byte outside
       `[A-Za-z0-9+/=]` returns `Err(Base64Error::InvalidInput)`. Allow unpadded 2- or 3-sextet tail or
       properly padded 4-char tail (`XX==` or `XXX=`), rejecting `rem == 1` or data after `'='` with
       `Err(Base64Error::InvalidInput)`.

## 6. Test commands, parity adapter, vector fixture, and Room-1 oracle cases

### 6.1 `kernel-test` commands (`kernel/src/fiber/testcmd.rs`)

Add five test-only commands under `cfg(feature = "test-commands")` (zero bytes in production
`kernel.core.wasm`), charging `Fiber::charge_work` through `work`:

1. **`__num <op:js|fixed|exp|prec|arith|printf-int|f64-prefix|f64-strict> <arg1> [arg2] [flags]`**
   - `js <hex_u64_bits_or_f64>` -> writes `fmt_f64_js` + `\n` (`0x...` raw IEEE-754 `u64` bits when `arg1` starts
     with `0x` and flag `bits` is passed, else parsed `f64`).
   - `fixed`/`exp`/`prec` -> writes formatted output + `\n` (or exits `2` on `PrecisionRange`).
   - `arith <token>` -> writes `fmt_f64_js(val)` + `\n` (exit `0`), or `bash: value too great for base\n` /
     `bash: invalid number\n` on stderr with exit `1`.
   - `printf-int <raw>` -> writes `<fmt_f64_js(value)>|<0_or_1>\n`.
2. **`__printf [--tz <tz>] [--] <format> [args...]`**
   - If zero positional args after options (`__printf` alone): stderr `printf: usage: printf format [arguments]\n`,
     exit `2` (`exit 1` when `--` was given, matching `printf --`).
   - Reads `TZ` from `--tz` override or `self.env.get(b"TZ")`, `max_output_bytes = self.shell.lim.vals[I_STRING_LENGTH].min(self.shell.lim.vals[I_OUTPUT_SIZE]) as usize`,
     `now_sec = (self.shell.virt_ms.get() / 1000) as i64`, `start_sec = 0`.
   - Calls `format_printf`: writes `out.bytes` to `Chan::Stdout`, writes `out.stderr` to `Chan::Stderr` if
     `out.had_error`, sets `self.last_exit = i32::from(out.had_error)`.
   - Maps `FormatLimitKind::Mem` to `Flow::Fail(Fail::Mem(e))`, and all other `FormatLimitKind` variants to
     `self.finish_early(126, Status::LimitExceeded, Some("maxStringLength"), Some(format_args!("bash: {msg}\n")))`!
3. **`__strftime <epoch_sec> <format> [tz]`**
   - Calls `format_strftime(format, epoch_sec, tz.or_else(|| self.env.get(b"TZ")), max_loop, max_out)` and
     writes `bytes` + `\n` to `Chan::Stdout` (mirroring `date -u -d @<sec> +<fmt>`).
4. **`__hash <md5|sha1|sha256> [label]`**
   - Streams chunks of `self.stdin` into `Md5`/`Sha1`/`Sha256` and writes `<hex>  <label_or_->\n` to
     `Chan::Stdout` with exit `0`.
5. **`__base64 <encode|decode|decode-strict> [wrap_cols]`**
   - Reads `self.stdin` as raw bytes (`in_cap` already enforced by `Fiber`), runs `base64::encode` (default
     `wrap_cols = 76`) or `base64::decode` (`Permissive` vs `Strict`), writing `Chan::StdoutBytes` on decode and
     `Chan::Stdout` on encode. Maps `InvalidInput` to stderr `base64: invalid input\n` (`exit 1`), and limit
     variants to `finish_early(126, Status::LimitExceeded, Some(limit_name), Some(format_args!("bash: {msg}\n")))`.

### 6.2 Test-only parity adapter (`harness/parity/l8b-adapter.mjs`)

`test/parity/l8b.test.mjs` passes `harness/parity/l8b-adapter.mjs` to `defineParityTests`:
- `create(init)` constructs `Bash` on `kernel-test` (`import { T } from "../../test/kt.mjs"`), forwarding
  `files`, `env`, `cwd`, `executionLimitProfile: init.profile ?? "normal"`, and `init.engine`.
- Uses the exact single-quote `unquote(s)` / `quote(s)` helpers (never injecting adapter variables into `env`):
  `token = '(?:[^']|'\\'')*'`.
- Recognizes five canonical step forms (any non-matching step throws an error with `err.code = "UNSUPPORTED"` so
  the runner marks it `notCovered`):
  1. `^printf$`: runs `__printf` (no args -> exit `2`).
  2. `^printf( --)?((?: (?:'(?:[^']|'\\'')*'|[A-Za-z0-9_./+-]+))+)$`: tokenizes single-quoted or bare safe tokens
     and runs `__printf [--] <quoted_tokens...>`, forwarding `step.env` (e.g. `TZ`) via `exec` options when
     `step.env` is explicitly present on the step.
  3. `^date -u -d @(-?[0-9]+) '(\+(?:[^']|'\\'')*)'$`: decodes `+fmt`, strips leading `+`, and runs
     `__strftime <epoch> <quote(fmt)> 'UTC'`.
  4. `^base64(?: -w ([0-9]+)| (-d|--decode))?$`: runs `__base64 encode <w|76>` or `__base64 decode` with
     `step.stdin` / `step.stdinKind`.
  5. `^(md5sum|sha1sum|sha256sum)$`: maps command prefix (`md5` / `sha1` / `sha256`) to `__hash <alg> -` with
     `step.stdin` / `step.stdinKind`.

### 6.3 Pre-generated JavaScript double vector fixture (`test/fixtures/l8b-js-numbers.json`)

Room 1 generates and commits `test/fixtures/l8b-js-numbers.json` from Node 22 before the build worktree opens
(PLAN section 6 step 3, section 8 L8b Entry). Schema:

```json
{
  "generator": "node-v22-ieee754",
  "count": 640,
  "vectors": [
    {
      "bits": "0x445a000000000000",
      "js": "1.1805916207174113e+21",
      "fixed0": "1.1805916207174113e+21",
      "fixed6": "1.1805916207174113e+21",
      "exp6": "1.180592e+21",
      "prec6": "1.18059e+21"
    }
  ]
}
```

The `>= 600` vectors cover: `+0.0`, `-0.0`, quiet/signaling `NaN` bit patterns, `±Infinity`, `Number.MIN_VALUE`
(`5e-324`), subnormal boundaries, all powers `2^k` (`-1074 <= k <= 1023`), all powers `10^k` (`-323 <= k <= 308`)
and their `nextUp`/`nextDown` 1-ULP neighbors, exact decimal radix cutoffs (`9.999999999999999e-7`, `1e-6`,
`999999999999999900000`, `1e20`, `1e21`), `MAX_SAFE_INTEGER - 2 .. MAX_SAFE_INTEGER + 4`, `2**62`, `2**63`,
`2**64`, `2**70`, `0.1`, `0.2`, `0.1 + 0.2`, `1.15`, `1.25`, `0.5`, `2.5`, `Number.MAX_VALUE`, and 300
deterministic xorshift64 bit patterns across all exponent buckets.

### 6.4 Room-1 oracle cases (`test/cases/l8b/*.case.json`, 30 cases)

All 30 cases set `"env": { "TZ": "UTC" }` unless testing explicit `step.env`:

| # | Case ID (`l8b/...`) | Canonical step(s) & `stdin` |
|---:|---|---|
| 1 | `printf-usage-errors` | Steps: `printf` (`exit 2`), `printf --` (`exit 1`) |
| 2 | `printf-int-bases-signs` | `printf '%d\|%d\|%d\|%d\|%d\|%d\|%d' '+42' '-42' '0x10' '+0x10' '010' '+010' '-010'` (`42\|-42\|16\|16\|8\|8\|-10`) and `'08'` (`8`) |
| 3 | `printf-int-char-consts` | `printf '%d\|%d\|%d\|%d' "'a" '"A' "'é" "\\'z"` (`97\|65\|233\|122`) |
| 4 | `printf-int-invalid-recovery` | Steps: `printf '%d' '-0x10'`, `printf '%d' '64#a'`, `printf '%d' '3abc'`, `printf '%d' 'abc'`, `printf '%d' '42 '` (all `exit 1` with formatted partial/0 stdout and `printf: <arg>: invalid number\n`) |
| 5 | `printf-int-flags-width-prec` | `printf '%+d\|% d\|%+ d\|%05d\|%-5d\|%08.4d\|%.0d\|%6.d' '42' '42' '42' '42' '42' '42' '0' '0'` |
| 6 | `printf-unsigned-oct-hex` | `printf '%u\|%o\|%x\|%X\|%#o\|%#o\|%#x\|%#X\|%#x' '-1' '-8' '-16' '-16' '0' '8' '16' '16' '0'` |
| 7 | `printf-large-int-double` | `printf '%d\|%d\|%d' '9007199254740991' '9223372036854775807' '99999999999999999999999999999'` |
| 8 | `printf-float-f-e` | `printf '%f\|%.2f\|%.0f\|%#.0f\|%e\|%.2E\|%+010.2f' '1.5' '1.25' '2.5' '1' '1.5' '1234.5' '3.5'` |
| 9 | `printf-float-g-special` | `printf '%g\|%g\|%g\|%#g\|%.2g\|%F\|%E\|%f' '100000' '1000000' '0.00001' '100' '123.4' 'Infinity' '-Infinity' 'NaN'` |
| 10 | `printf-string-and-char` | `printf '%5s\|%-5s\|%05s\|%.3s\|%.2s\|%c\|%c\|%c' 'hi' 'hi' 'hi' 'héllo' '😀ab' '' 'abc' 'é'` |
| 11 | `printf-quote-q` | `printf '%q\|' '' 'abc_1-2./' 'hello world' "a'b" 'é' '~foo' 'a=b' '{a,b}' '!hist'` |
| 12 | `printf-format-escapes` | `printf '\\a\|\\b\|\\e\|\\E\|\\f\|\\n\|\\r\|\\t\|\\v\|\\\\\|\\0101\|\\101\|\\777\|\\x4\|\\x41\|\\xc3\\xa9\|\\xc3\\x28\|\\u00e9\|\\U0001f600\|\\cX'` |
| 13 | `printf-b-escapes-and-stop` | Steps: `printf '%b' '\\a\|\\e\|\\0\|\\0101\|\\101\|\\777\|\\x4g\|\\xc3\\xa9\|\\xc3\\x28\|\\u00e9\|\\U0001f600'` and `printf '%b-%s' 'abc\\cdef' 'ignored'` |
| 14 | `printf-dynamic-star` | `printf '%*.*f\|%*d\|%.*d' '-8' '2' '3.5' '-4' '7' '-2' '7'` |
| 15 | `printf-arg-reuse-and-pct` | Steps: `printf 'a:%d b:%s\\n' '1' 'one' '2' 'two' '3'`, `printf 'plain%%' 'extra1' 'extra2'`, `printf '%'` |
| 16 | `printf-bad-placeholder` | Steps: `printf '%z' '1'` and `printf '%10%' '1'` (`exit 1`, `printf: [sprintf] unexpected placeholder\n`) |
| 17 | `printf-strftime-directives` | `printf '%(%Y-%m-%d %H:%M:%S %Z %z %a %A %b %h %B %C %d %D %e %F %g %G %I %j %k %l %m %M %p %P %r %R %s %S %u %U %V %w %W %x %X %y %N %Q)T' '0'` |
| 18 | `printf-strftime-neg-and-empty` | Steps: `printf '%(%Y-%m-%d %H:%M:%S %u %w %V %G)T' '-86400'`, `printf '%12(%Y-%m-%d)T' '1700000000'`, `printf '%()T' '0'` |
| 19 | `date-utc-epochs` | Steps: `date -u -d @0 '+%Y-%m-%d %H:%M:%S %Z %z %a %b %e %j %u %w %V %G'` and `date -u -d @1700000000 '+%Y-%m-%d %H:%M:%S %c %r %R %T %D %F %p %P'` |
| 20 | `date-leap-and-iso-week` | Steps: `date -u -d @951782400 '+%F %j %U %W %V %G'` (`2000-02-29` leap day) and `date -u -d @1609459200 '+%F %u %V %G %g'` (`2021-01-01` -> ISO `53 2020 20`) |
| 21 | `printf-limits-width-prec` | `engine: { executionLimits: { maxStringLength: 16 } }`, steps: `printf '%20s' 'hi'`, `printf '%.20d' '1'`, `printf '%10s%10s' 'a' 'b'` (all `exit 126`) |
| 22 | `base64-encode-empty-and-short` | Steps: `base64` with `stdin: ""` (`""`), `"f"` (`"Zg==\n"`), `"fo"` (`"Zm8=\n"`), `"foo"` (`"Zm9v\n"`), `"hello"` (`"aGVsbG8=\n"`) |
| 23 | `base64-encode-wrap` | `stdin` = 80 `'a'` bytes: steps `base64` (76-col wrap + `\n`), `base64 -w 0` (single line, no `\n`), `base64 -w 16` |
| 24 | `base64-decode-standard-and-ws` | Steps: `base64 -d` with `"aGVs\r\n  bG8=\n"` -> `"hello"`, `base64 --decode` with `{ base64: "dGVzdA==" }` |
| 25 | `base64-decode-permissive-binary` | Steps: `base64 -d` on `"YWJj"` (unpadded -> `"abc"`), `"YW!Jj"` (`"abc"`), `"YW=Jj"` (`"a"`), and `base64` -> `base64 -d` on raw binary `{ base64: "AP9+gA==" }` (`stdoutKind: "bytes"`) |
| 26 | `md5sum-vectors` | Steps: `md5sum` on `""`, `"a"`, `"abc"`, `"message digest"`, and binary `{ base64: "AP8BAg==" }` |
| 27 | `sha1sum-vectors` | Steps: `sha1sum` on `""`, `"abc"`, `"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"`, and binary `{ base64: "AP8BAg==" }` |
| 28 | `sha256sum-vectors` | Steps: `sha256sum` on `""`, `"abc"`, `"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"`, and binary `{ base64: "AP8BAg==" }` |
| 29 | `printf-quote-control-bytes` | `printf '%q' 'a	b'` (tab -> `$'a\tb'`) and `printf '%b' '\\x01\\x7f\\x1b'` |
| 30 | `printf-output-size-limit` | `engine: { executionLimits: { maxStringLength: 12 } }`, step: `printf '%s%s' '01234567' '89abX'` (`exit 126`, `bash: formatted value size limit exceeded (4 bytes)\n` / output limit) |

## 7. Executable exit gates

All gates run in `make check` (`cargo test` + `node tools/run-tests.mjs`):

1. **Repository and zero-import / zero-crate gates.** G1-G8 pass (`dist/js/kernel/kernel.core.wasm` has zero
   foreign imports; `Cargo.lock` contains only workspace crates; `getCommandNames()` remains `["echo"]`).
2. **JavaScript number-formatter vector gate (`test/l8b-numbers.test.mjs` + native `cargo test`).**
   - Replay every entry in `test/fixtures/l8b-js-numbers.json` (`>= 600` doubles) through `fmt_f64_js`,
     `fmt_f64_fixed`, `fmt_f64_exp`, and `fmt_f64_prec` (both natively and via `__num` in Wasm): **0 mismatches**.
   - In `test/l8b-numbers.test.mjs` (Node 22 `String(x)` live comparison on `kernel-test` `__num js` in batches):
     verify **>= 10 000** deterministic seeded `f64` bit patterns (excluding `NaN` payload variants normalized to
     `0x7ff8_0000_0000_0000`) match Node's `String(x)` with **0 mismatches** in `< 1.5 s`.
3. **Parity (`test/parity/l8b.test.mjs`).** All 30 committed `test/cases/l8b/*.case.json` cases replay with
   **0 failures, 0 stale hashes, 0 exclusions, and 0 `notCovered`**.
4. **Standard RFC digest and `base64` vector gate (`cargo test` + `test/l8b-crypto-base64.test.mjs`).**
   - `Md5` passes all 7 RFC 1321 section A.5 test vectors (`""` through 80-digit numeric string);
   - `Sha1` and `Sha256` pass FIPS 180-4 empty, `"abc"`, 448-bit (`56`-byte block boundary), 896-bit, and
     **1 000 000 `'a'` bytes** (`1 MiB`, fed both in 1 chunk and in prime-sized `137`-byte streaming chunks,
     matching `cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0` for SHA-256 and
     `34aa973cd4c4daa4f61eeb2bdbad27316534016f` for SHA-1);
   - `base64` passes all RFC 4648 section 10 vectors, `1 MiB` binary round-trip (`encode` -> `decode`), and
     both `Permissive` and `Strict` decode edge cases.
5. **Prospective byte-limit and S11 timezone gate (`test/l8b-limits.test.mjs`).**
   - `%2147483647s` and `%.2147483647d` under default `contain` (`maxStringLength = 16 MiB`) fail prospectively
     in **`< 10 ms`** without allocating gigabytes (`exitCode: 126`, `Bash` instance still runs `echo ok` -> `"ok\n"`).
   - `base64` with `-w 1` on a payload exceeding `maxArrayElements` (`100` override) fails prospectively with
     `exitCode: 126` (`bash: base64: wrapped line limit exceeded (100)\n`).
   - S11 timezone non-disclosure: `__strftime 0 '%Y-%m-%d %H:%M:%S %Z %z'` with `TZ` unset, `TZ=""`, and
     `TZ="Invalid/Zone"` all output `"1970-01-01 00:00:00 UTC +0000\n"` regardless of the host `TZ` env var.
6. **Native unit suite (`cargo test --lib`).** At least **70** native unit tests across `num`, `format`, `hash`,
   and `base64`, including `parse_arith_literal` (`2..=64` bases, `36#Z` == `35` vs `37#Z` == `61`, `64#@_` ==
   `4031`, `08` error, `MAX_SAFE_INT + 1` clamping), `format_awk_printf` (`%2$s`, `ll`/`z` modifiers, `-%x`,
   `> 100` precision error), and `strftime` negative pre-1970 dates, year `2038+` (`> i32::MAX`), and `Etc/GMT±N`
   sign inversion.
7. **Deterministic seeded fuzz slice (`test/l8b-fuzz.test.mjs` + native `cargo test`).** Fixed seed
   `0x5345_4E54_4C38_4231` running **>= 2 000** random format strings, escape sequences, timestamps, number
   strings, and byte slices in `make check`: 0 panics, 0 Wasm traps, `< 2 s` total runtime.

## 8. Points the builder must confirm and report

Record in `BRANCH.md` and the PR description:

1. **JS number formatter algorithm and Wasm size delta:** name the exact `no_std` algorithm and lookup-table
   footprint used for `fmt_f64_js`, and report raw + gzip byte deltas on both `dist/js/kernel/kernel.core.wasm`
   and `dist/test/kernel-test.core.wasm`.
2. **Vector and round-trip counts:** exact count of `test/fixtures/l8b-js-numbers.json` vectors and seeded live
   `String(x)` comparisons passing with 0 mismatches.
3. **Downstream handoff verification:** confirm the exact public signatures consumed by L4 (`fmt_f64_js`,
   `parse_arith_literal`), L9b (`format_printf`, `format_strftime`, `base64::{encode,decode}`, `parse_f64_strict`),
   L14a/L14b (`format_awk_printf`, `fmt_f64_js`, `parse_f64_prefix`), L15a-1/L15a-2 (`fmt_f64_js`,
   `format_strftime`, `base64::{encode,decode}`), and L16c (`Md5`, `Sha1`, `Sha256`).
