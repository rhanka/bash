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
- `unsafe` only with a `// SAFETY:` comment, complying with `deny(unsafe_op_in_unsafe_fn)`.

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
- `BR8a-EX1` (approved by lane brief, owner decision 21/11): append-only `DIVERGENCES.md` row for the
  glob throw surfaces that diverge from `just-bash` (never throw out of `exec`; tagged
  `upstream-defect`). Reason: the lane brief orders the listing and the file is outside the lot's
  allowed paths. Impact: one table row, no behavior change. Rollback: delete the row.
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l8a-regex` and record baseline Wasm sizes.
  - [x] Confirm scope boundaries and declare `BR8a-EX1` for `DIVERGENCES.md`.
- [ ] **Lot 1 — `kernel::regex`**
  - [ ] Parsers/compilers for ERE, BRE (`Grep`/`Sed`/`Expr`), Fixed, Perl; POSIX brackets, caps.
  - [ ] Linear PikeVM plus lazy DFA cache, literal prefilter, bounded backtracker.
  - [ ] `find_all`/`replace_all` with `Standard`/`PosixSed`/`ShellExpansion` policies.
  - [ ] Lot gate: native regex unit tests green.
- [ ] **Lot 2 — `kernel::glob`**
  - [ ] Glob/extglob compiler and matcher (`*`, `?`, `[...]`, `@()`, `*()`, `+()`, `?()`, `!()`), `**` cap.
  - [ ] `strip_prefix`/`strip_suffix`/`replace_glob`/`split_globignore`/`check_globstar_segments`.
  - [ ] Lot gate: native glob unit tests green (65+ combined with Lot 1).
- [ ] **Lot 3 — Test commands, adapter, suites**
  - [ ] `__regex`/`__glob` in `kernel/src/fiber/testcmd.rs`, `harness/parity/l8a-adapter.mjs`.
  - [ ] `test/parity/l8a.test.mjs`, `test/l8a-redos.test.mjs`, `test/l8a-fuzz.test.mjs`.
  - [ ] Lot gate: L8a 30/30 replay, L2 30/30 parity, S9 gates, fuzz slice green.
- [ ] **Lot 4 — Final validation**
  - [ ] `make check` green (G1-G8, cargo, node tests); `harness check scope` (or documented equivalent).
  - [ ] `DIVERGENCES.md` row (`BR8a-EX1`); report Wasm deltas, S9 timings, DFA peak, downstream checklist.
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.

## Points to confirm and report (spec section 9)
- Core Wasm size delta (`dist/kernel.core.wasm` raw/gzip, plus `dist/test/kernel-test.core.wasm`).
- S9 blow-up timings and work units for the four matching traps, compile-cap rejections, DFA peak bytes
  on `a[ab]{20}`, bounded-backreference termination.
- Downstream integration checklist for L4, L9a-1, L13, L14a (`CompiledRegex`, `CompiledGlob` signatures).
- Upstream `!(...)` and backreference findings (EVOL Annex E.7): native `!(...)` without regex lookahead;
  `allow_backrefs=false` for strict 3.6.0 parity, `true` for EVOL D12 bounded backreferences.
