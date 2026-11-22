# feat: L8b shared leaf kernels (num, format, hash, base64)

## Objective
Implement the four zero-dependency, `no_std`, budget-allocated leaf kernel modules (`kernel::num`, `kernel::format`, `kernel::hash`, `kernel::base64`), wire test-only commands into `kernel/src/fiber/testcmd.rs`, and verify parity against the committed L8b snapshots, IEEE-754 JS number vectors, RFC vectors, limits, and seeded fuzz gates.

## Scope / Guardrails
- Scope limited to `kernel/src/{lib.rs,num*,format*,hash*,base64*,fiber/testcmd.rs}`, `harness/parity/l8b-adapter.mjs`, `test/parity/l8b.test.mjs`, `test/l8b-*.test.mjs`, `test/fixtures/l8b-js-numbers.json` (`BR8b-EX1`), and `BRANCH.md`.
- Make-only workflow; branch work in worktree `tmp/l8b-kernels`.
- No AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/lib.rs`
  - `kernel/src/num.rs`
  - `kernel/src/num/**`
  - `kernel/src/format.rs`
  - `kernel/src/format/**`
  - `kernel/src/hash.rs`
  - `kernel/src/hash/**`
  - `kernel/src/base64.rs`
  - `kernel/src/base64/**`
  - `kernel/src/fiber/testcmd.rs`
  - `harness/parity/l8b-adapter.mjs`
  - `test/parity/l8b.test.mjs`
  - `test/l8b-*.test.mjs`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `test/fixtures/l8b-js-numbers.json`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- `BR8b-EX1` (`test/fixtures/l8b-js-numbers.json`):
  - **Reason**: Room 1's `test/oracle-w4b` PR committed `test/cases/l8b/*` without `test/fixtures/l8b-js-numbers.json`, which is required by `spec/lots/L8b.md` §6.3 and Gate 2 (§7.2).
  - **Impact**: Adds the deterministic Node v22.22.1 IEEE-754 `f64` vector fixture (`>= 600` doubles) used by native `cargo test` and `test/l8b-numbers.test.mjs`. Zero impact on runtime kernel or existing fixtures.
  - **Rollback**: Delete `test/fixtures/l8b-js-numbers.json`.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l8b-kernels` and `harness check branch`.
  - [x] Confirm scope boundaries and declare `BR8b-EX1` for `test/fixtures/l8b-js-numbers.json`.
- [x] **Lot 1 — `kernel::num` (ECMA-262 float formatting & number parsers)**
  - [x] Implement `fmt_f64_js`, `f64_to_js_string`, `fmt_f64_fixed`, `fmt_f64_exp`, `fmt_f64_prec`.
  - [x] Implement `parse_arith_literal`, `parse_printf_int`, `parse_f64_prefix`, `parse_f64_strict`.
- [x] **Lot 2 — `kernel::format` (`printf`, `%q`, escapes, `awk` `sprintf`, `strftime`)**
  - [x] Implement `process_format_escapes`, `process_b_escapes`, `shell_quote_printf`.
  - [x] Implement `format_printf`, `format_awk_printf`.
  - [x] Implement `parse_tz_spec`, `civil_from_timestamp`, `format_strftime`.
- [x] **Lot 3 — `kernel::hash` & `kernel::base64`**
  - [x] Implement `Md5`, `Sha1`, `Sha256` with `new`, `update`, `finalize`, `digest`, `hex_lower`.
  - [x] Implement `base64::{encoded_output_len, encode, decode}` with `Permissive` and `Strict` modes and prospective limit checks.
- [x] **Lot 4 — `kernel-test` commands, parity adapter, and verification suites**
  - [x] Wire `__num`, `__printf`, `__strftime`, `__hash`, `__base64` in `kernel/src/fiber/testcmd.rs`.
  - [x] Add `harness/parity/l8b-adapter.mjs` and `test/parity/l8b.test.mjs`.
  - [x] Generate `test/fixtures/l8b-js-numbers.json` (`BR8b-EX1`) and add `test/l8b-{numbers,crypto-base64,limits,fuzz}.test.mjs` plus >= 70 native `cargo test` unit tests.
- [x] **Lot N — Final validation**
  - [x] `make check` green (including G1-G8, L8b 30/30 parity, L2 30/30 parity, vector/limit/fuzz gates).
  - [x] `harness check scope` passes.
  - [x] Report Wasm raw + gzip size deltas, algorithm/LUT footprint, vector counts, and downstream handoff signatures.

## Builder Confirmation (Spec §8)
1. **JS number formatter algorithm and Wasm size delta:** Exact `no_std` stack-allocated Burger-Dybvig Dragon4 (`BigUint` `[u32; 42]`, **0 bytes** static lookup table). `dist/js/kernel/kernel.core.wasm`: `247,677` B raw (+179 B vs `247,498` B on `main`), `89,323` B gzip. `dist/test/js/kernel-test/kernel-test.core.wasm`: `363,766` B raw (+94,036 B vs `269,730` B on `main`), `138,347` B gzip.
2. **Vector and round-trip counts:** `test/fixtures/l8b-js-numbers.json` holds `4,351` vectors (all replayed natively + in Wasm `__num` across `js`, `fixed0`, `fixed6`, `exp6`, `prec6` with 0 mismatches), plus `10,000` seeded live `String(x)` Wasm `__num js` comparisons with 0 mismatches.
3. **Downstream handoff verification:** Public signatures verified in `kernel::num` (`fmt_f64_js`, `f64_to_js_string`, `fmt_f64_fixed`, `fmt_f64_exp`, `fmt_f64_prec`, `parse_arith_literal`, `parse_printf_int`, `parse_f64_prefix`, `parse_f64_strict`), `kernel::format` (`process_format_escapes`, `process_b_escapes`, `shell_quote_printf`, `format_printf`, `format_awk_printf`, `parse_tz_spec`, `civil_from_timestamp`, `format_strftime`), `kernel::hash` (`Md5`, `Sha1`, `Sha256`), and `kernel::base64` (`encoded_output_len`, `encode`, `decode`).
