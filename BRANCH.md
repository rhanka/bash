# feat: L8a linear-time regex and glob/extglob engines

## Objective
Deliver `kernel::regex` (Thompson NFA/PikeVM plus byte-capped lazy DFA, bounded backtracker, BRE/ERE/Fixed/Perl modes) and `kernel::glob` (linear-time glob plus full bash extglob without regex lookahead, strip/replace, `GLOBIGNORE` split), wired to `kernel-test` (`__regex`, `__glob`) and proven on the `just-bash@3.6.0` pattern snapshots in `test/cases/l8a/`.

## Scope / Guardrails
- Scope limited to lot L8a (spec `spec/lots/L8a.md`).
- Make-only workflow; branch work in worktree `tmp/l8a-regex`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.
- Clean room: never read `tmp/refs/` or `tmp/bench-engines/`; no network.
- Never write to `/tmp`; scratch files under git-ignored `tmp/`.
- Never use `git stash`.
- `unsafe` only with a `// SAFETY:` comment, complying with `deny(unsafe_op_in_unsafe_fn)` (zero `unsafe` added in L8a).

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/lib.rs` (`pub mod glob;` and `pub mod regex;` only)
  - `kernel/src/regex/**`
  - `kernel/src/glob/**`
  - `kernel/src/fiber/testcmd.rs` (`__regex` and `__glob` under `test-commands`)
  - `harness/parity/l8a-adapter.mjs`
  - `test/parity/l8a.test.mjs`
  - `test/l8a-*.test.mjs`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `DIVERGENCES.md`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- `BR8a-EX1` (approved by lane brief, owner decision 21/11): append-only `DIVERGENCES.md` row (`V10`) for the
  glob throw surfaces that diverge from `just-bash` (never throw out of `exec`; tagged
  `upstream-defect`). Reason: the lane brief orders the listing and the file is outside the lot's
  allowed paths. Impact: one table row, no behavior change. Rollback: delete the row.
- Spec vs oracle notes observed during verification:
  1. Regex bracket `[!a]`: the committed `just-bash@3.6.0` oracle snapshot `test/cases/l8a/bracket-edge-cases.snap.json`
     step 3 (0-indexed step 2) records `"!\nb\n"` for `grep -E -- '[!a]'` on `"!\na\nb\n"` (leading `!` negates in regex brackets,
     whereas glob `[!a]` treats `!` as a literal member in `l8a/glob-brackets-negation.snap.json`). Implemented
     to match the committed snapshots 30/30 (`D1`); `spec/lots/L8a.md` is outside allowed paths and noted for follow-up.
  2. Gate 3 trap `([a-zA-Z]+)*$` on `"a".repeat(50_000) + "!"`: unanchored `(...)*$` matches the empty string
     at EOF (`exitCode: 0`, 1.66 ms); anchored `^([a-zA-Z]+)*$` fails to match (`exitCode: 1`, 1.22 ms). Both are
     covered in `test/l8a-redos.test.mjs` and native tests.
  3. Gate 6 nested extglob `!(!(a)|b)`: by exact bash De Morgan semantics (`!( (!a) | b ) = a & !b`), `"a"` is
     `true` and `"b"`/`"c"` are `false` (verified against GNU bash `-O extglob`). Both `!(!(a)|b)` and
     `!(!(a|b))` (`"a"`/`"b"` `true`, `"c"` `false`) are covered in native and Wasm unit tests.
- PR #22 adversarial review fixes (Findings 1–6):
  1. **Blocker — nested `!(...)` exponential blow-up:** replaced recursive `GlobAst` with a flat post-order
     `GlobNode` arena, iterative `parse_glob` with `O(N)` extglob parenthesis pre-pass, iterative `compile_nfa` +
     maximal `!(...)`-free sub-NFAs, and an iterative packed-`u64`-bitset Reachability DP caching `(node_id, start_idx)`
     once per `DpCtx` (shared across `strip_suffix` and `replace_glob`).
  2. **Major — bounded backtracker nullable capture loops:** interned active capture slot slices (`slots[2..]`) in
     `backref_at`, keeping `cap_id` unchanged when `Inst::Save` writes an identical offset and restoring `prev_cap_id`
     on `RestoreSlot` so `(a*)*\1` and `((a?)+)*\1` succeed cleanly without `BacktrackLimitExceeded`.
  3. **Major — stacked postfix quantifiers (S3 trap risk):** enforced `max_nesting_depth` on stacked quantifiers in
     `parse_suffix`, replaced recursive `Ast` with a flat post-order `AstNode` arena (iterative `Drop`), bottom-up
     `compute_min_insts`, and explicit-stack `compile_root` and `collect_factors`.
  4. **Minor — dead branch in `find_all`:** removed redundant `zero_progress`/`empty_reported` branch.
  5. **Minor — spec-vs-oracle prose:** clarified step numbering (`step 3 / 0-indexed step 2`) and added explicit
     native/Wasm assertions for regex `[!a]` negation and `!(!(a)|b)` De Morgan semantics.
  6. **Minor — two-dot diff artifact:** `feat/l8a-regex` branched before `spec/lots/L1d.md` and `spec/lots/L8b.md`
     merged to `main` (`1f90267`); three-dot diff confirms zero overlap — rebase onto `main` before merge.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l8a-regex` and record baseline Wasm sizes.
  - [x] Confirm scope boundaries and declare `BR8a-EX1` for `DIVERGENCES.md`.
- [x] **Lot 1 — `kernel::regex`**
  - [x] Parsers/compilers for ERE, BRE (`Grep`/`Sed`/`Expr`), Fixed, Perl; POSIX brackets, caps.
  - [x] Linear PikeVM plus byte-capped lazy DFA cache, literal prefilter, bounded backtracker.
  - [x] `find_all`/`replace_all` with `Standard`/`PosixSed`/`ShellExpansion` policies.
  - [x] Lot gate: 36 native regex unit tests green.
- [x] **Lot 2 — `kernel::glob`**
  - [x] Glob/extglob compiler and matcher (`*`, `?`, `[...]`, `@()`, `*()`, `+()`, `?()`, `!()`), `**` cap.
  - [x] `strip_prefix`/`strip_suffix`/`replace_glob`/`split_globignore`/`check_globstar_segments`.
  - [x] Lot gate: 34 native glob unit tests green (70 combined native L8a tests >= 65 gate).
- [x] **Lot 3 — Test commands, adapter, suites**
  - [x] `__regex`/`__glob` in `kernel/src/fiber/testcmd.rs`, `harness/parity/l8a-adapter.mjs`.
  - [x] `test/parity/l8a.test.mjs`, `test/l8a-redos.test.mjs`, `test/l8a-fuzz.test.mjs`.
  - [x] Lot gate: L8a 30/30 replay, L2 30/30 parity, S9 gates, fuzz slice green.
- [x] **Lot 4 — Final validation**
  - [x] `make check` green (G1-G8, cargo, node tests); scope audited against `BRANCH.md` and `harness.profile.json` (`harness` binary not installed in lane PATH).
  - [x] `DIVERGENCES.md` row `V10` (`BR8a-EX1`); report Wasm deltas, S9 timings, DFA peak, downstream checklist.
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.

## Points to confirm and report (spec section 9)
1. **Core Wasm size delta:**
   - `dist/js/kernel/kernel.core.wasm`: `247,498` -> `247,506` B raw (`+8 B`), `89,259` -> `89,273` B gzip (`+14 B`) (unreachable in production kernel until L4/L9 wire them).
   - `dist/kernel.core.wasm`: `253,304` -> `253,312` B raw (`+8 B`), `91,965` -> `91,982` B gzip (`+17 B`).
   - `dist/test/kernel-test.core.wasm` (both modules fully live via `__regex`/`__glob`): `275,536` -> `397,931` B raw (`+122,395 B`), `99,385` -> `143,163` B gzip (`+43,778 B`).
2. **S9 blow-up timings and DFA cache cap:**
   - `ere (a+)+b` (50k `'a'` + `'!'`): `exitCode: 1`, `1.33 ms` (< 50 ms); scaling 50k/5k = `1.62x` (<= 15x).
   - `ere (a|a)*b` (50k `'a'` + `'!'`): `exitCode: 1`, `0.50 ms` (< 50 ms).
   - `ere ^([a-zA-Z]+)*$` (50k `'a'` + `'!'`): `exitCode: 1`, `1.22 ms` (unanchored `([a-zA-Z]+)*$`: `exitCode: 0`, `1.66 ms`).
   - `perl (.*a){25}b` (20k `'a'` + `'!'`): `exitCode: 1`, `0.97 ms` (< 50 ms).
   - Compile caps (`a{1000}{1000}`, `a{1001}`, 201-deep parens, 101 captures): `0.24–0.34 ms` (< 20 ms), `exitCode: 2`.
   - DFA state-explosion `a[ab]{20}` / `a[ab]{20}c` over 64 KiB random `"ab"`: flushes cleanly, `dfa_cached_bytes() <= 262,144` B (256 KiB) throughout, bit-for-bit identical to PikeVM.
   - Bounded backreference `([a-z]+)\1+b` on 500 `'a'` + `'!'` with `maxWorkUnits: 5_000`: `exitCode: 126`, `20.20 ms` (< 50 ms), `Bash` instance remains usable (`echo ok` -> `0`).
3. **Downstream integration checklist for L4, L9a-1, L13, L14a:**
   - `kernel::regex`: `CompiledRegex::{compile, capture_count, group_index, has_backrefs, dfa_cached_bytes, prefilter, is_match, find_at, captures_at, find_all, replace_all}`, `RegexMode::{Extended, Basic(BreDialect), Fixed, Perl}`, `BreDialect::{Grep, Sed, Expr}`, `RegexFlags` (including `allow_backrefs`, `comma_first_interval`), `EmptyAdvance::{Standard, PosixSed, ShellExpansion}`, `ReplaceStyle::{Sed, Dollar, Literal}`, `Prefilter::{None, Exact, Substring}`.
   - `kernel::glob`: `CompiledGlob::{compile, is_match}`, `GlobOptions`, `GlobError`, `GlobReplaceMode::{First, All, AnchorHead, AnchorTail}`, `check_globstar_segments`, `strip_prefix`, `strip_suffix`, `replace_glob`, `split_globignore`.
4. **Upstream `!(...)` and backreference findings (EVOL Annex E.7):**
   - `kernel::glob` evaluates `!(...)` natively via exact reachability DP without regex lookahead, avoiding the reference's unhandled throw on `!(...)` in `case`, `[[ == ]]`, and `${var##!(...)}` (`DIVERGENCES.md` row `V10`, `upstream-defect`).
   - `RegexFlags::allow_backrefs` supports both strict `just-bash@3.6.0` parity (`false` -> `SyntaxKind::BackrefUnsupported`) and EVOL D12 fuel-charged bounded backreferences (`true`).
