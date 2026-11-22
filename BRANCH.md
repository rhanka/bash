# feat: Full bash lexer and parser (L3)

## Objective

Replace the L2 subset `kernel::fiber::syntax` module with the complete bash lexer and
explicit-budget recursive-descent parser over a flat-arena AST, and extend the resumable
explicit-stack VM plus word expansion for `If`, `For`, `While`, `Until`, `Subshell`, `Group`,
`FunctionDef`, `time [-p]`, fd-0 heredocs/herestrings, deferred statement syntax errors, and
ANSI-C `$'...'` decoding.

## Scope / Guardrails

- Scope limited to the L3 lot (`spec/lots/L3.md`): full lexer/parser, arena AST, control-flow
  VM, `$'...'` expansion, parity/caps/coverage/fuzz suites.
- Make-only workflow; branch work in worktree `tmp/l3-parser`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.
- Clean room: never read `tmp/refs/` or `tmp/bench-engines/`; behavior comes from
  `spec/lots/L3.md` and the committed `test/cases/l3/` cases/snapshots only.
- No network. Never write outside the worktree and `conductor/lanes/`. Never write to `/tmp`.
  Never use `git stash`.
- `unsafe` only in `kernel/src/mem/` and `kernel/src/bindings/` with `// SAFETY:` comments.

## Branch Scope Boundaries (MANDATORY)

- **Allowed Paths (implementation scope)**:
  - `kernel/src/fiber.rs`
  - `kernel/src/fiber/syntax.rs`
  - `kernel/src/fiber/syntax/**`
  - `kernel/src/fiber/vm.rs`
  - `kernel/src/fiber/expand.rs`
  - `test/parity/l3.test.mjs`
  - `test/l3-*.test.mjs`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `kernel/src/lib.rs`
  - `kernel/src/mem/**`
  - `kernel/src/fiber/testcmd.rs`
  - `kernel/src/limits.rs`
  - `kernel/src/cmd/**`
  - `host/**`
  - `types/**`
  - `wit/**`
  - `tools/**`
  - `harness/**`
  - `test/cases/l3/**`
  - `test/parity/exclusions.json`
  - `test/l2-limits.test.mjs`
  - `DIVERGENCES.md`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop

- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`
- `BRANCH.md` itself is written in the first commit (allowed path, no exception).
- `BR3-EX1` (`test/l2-limits.test.mjs`):
  - **Reason:** L3 adds the `just-bash@3.6.0` top-level statement iteration cap (`> 10_000` statements -> exit `2`, `Parser stuck: too many iterations (>10000)` per `spec/lots/L3.md` sections 6.1 and 9.3) and includes the synthetic `EOF` token in the top-level `TOKEN_CAP` count per `spec/lots/L3.md` section 6.1.2. In `test/l2-limits.test.mjs`, `commandPressure(cap)` previously used a flat `"echo x\n".repeat(cap + 1)` for `hardened` (`10_001` statements) and `micro` (`25_001` statements), and line 201 asserted `100002` (excluding `EOF`) for `"echo x\n".repeat(33_334)`.
  - **Impact:** Two single-line updates in `test/l2-limits.test.mjs`: `commandPressure` (`cap + 1 <= 10_000 && ...`) so `hardened` and `micro` use the existing binary-tree call generator (`f0..fN`) keeping all 4 profiles testing `maxCommandCount` end-to-end, and line 201 expecting `100003` (`33_334 * 3 + 1` including synthetic `EOF`).
  - **Rollback:** Revert the two lines in `test/l2-limits.test.mjs`.
- `DIVERGENCES.md` (conditional path, additive rows only): no `BRxx-EXn` needed unless a
  shared-file edit is required; L3 records the `time`-zeros shape divergence and the
  never-throw-out-of-exec glob policy there, both additive single-row edits. Owner decision
  (21/11, Track): glob throw surfaces diverge from just-bash; never throw out of exec;
  tag them `upstream-defect`.

## Plan / Todo (lot-based)

- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l3-parser` and `harness check branch`.
  - [x] Confirm scope boundaries and declare `BRxx-EXn` exceptions if needed (none).
  - [x] Write `BRANCH.md` (this file) and commit it first.
- [x] **Lot 1 — `syntax` arena AST + full lexer**
  - [x] Flat-arena `Ast` per spec section 4 (all `KVec` tables, `Copy` id handles).
  - [x] Multi-heredoc `normalize` FIFO queue (spec 5.2).
  - [x] Full lexer: operators, reserved words, comments, continuations, all quoting
    forms, `${...}`/`$(...)`/`$((...))`/backtick/process-sub scanning, heredoc bodies.
  - [x] Lot gate: `cargo test --lib` new AST unit tests green.
- [x] **Lot 2 — Full parser + budgets + exact diagnostics**
  - [x] All 12 command kinds, pipelines, lists, deferred root `}`/`)` errors.
  - [x] Word/parameter/brace/tilde/glob, arithmetic, and `[[ ]]` conditional ASTs.
  - [x] Token/depth/iteration caps with exact messages; `Error::Lexer`/`Error::Parser`
    display lines and `Fiber::prepare` mapping.
  - [x] Lot gate: error-case oracle snapshots match byte-for-byte.
- [x] **Lot 3 — VM + expansion extensions**
  - [x] `If`/`For`/`While`/`Until`/`Subshell`/`Group`/`FunctionDef`/`time` frames,
    fd-0 heredoc/herestring stdin, empty `Simple` no-op, deferred-error trap,
    `$'...'` expansion, `bash:cmd:*` coverage hits.
  - [x] Lot gate: runtime oracle cases pass (`if`, `for`, loops, subshell, functions,
    heredoc stdin, ANSI-C).
- [x] **Lot 4 — Suites, gates, and report points**
  - [x] `test/parity/l3.test.mjs` (36/36, 0 stale, 0 excluded, 0 not-covered),
    `test/l3-caps.test.mjs`, `test/l3-coverage.test.mjs`,
    `test/l3-parser-fuzz.test.mjs` + native fuzz slice (82 `fiber::syntax` tests,
    2 500 JS + 2 500 native fuzz inputs).
  - [x] Record in this file: arena footprint (`size_of` on `wasm32`), core Wasm size
    delta, parity/fuzz counts, L4/L5 handoff table.
  - [x] Lot gate: `make check` green + `harness check scope` pass.
- [ ] **Lot N — Final validation**
  - [x] `make check`
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.
  - [ ] Owner sign-off; remove `BRANCH.md` in the last commit before merge.

## Points the builder must confirm and report (spec section 10)

1. **AST arena footprint and zero-recursion drop:**
   All AST node types contain only fixed-width scalars (`u32`, `i64`, `u8`, `bool`, `Span`,
   typed `*Id` handles) — zero pointers, zero `usize`, zero `KBox` trees — so sizes are
   identical on `wasm32` and host targets and `Drop for Ast` frees flat `KVec` arenas with
   O(1) stack depth:
   - `size_of::<Command>()` = **44 B**
   - `size_of::<Stmt>()` = **44 B**
   - `size_of::<WordPart>()` = **16 B**
   - `size_of::<ArithExpr>()` = **40 B**
   - `size_of::<CondExpr>()` = **12 B**
   200-deep `{ }`, `( )`, `if`, `[[ ( ... ) ]]`, and `$(( (...) ))` scripts execute cleanly
   within the Wasm call stack; depth 201 exits `2` with `Maximum parser nesting depth exceeded (200)`.
2. **Core Wasm size delta:**
   - Baseline `dist/js/kernel/kernel.core.wasm`: `247 498 B raw` / `89 259 B gzip`
   - After L3 `dist/js/kernel/kernel.core.wasm`: `438 746 B raw` / `146 543 B gzip`
   - Delta: `+191 248 B raw` / `+57 284 B gzip` (well under Gate L2 budget `1 500 000 B raw` / `700 000 B gzip`).
3. **Parity and fuzz counts:**
   - `test/parity/l3.test.mjs`: **36/36** passed (0 failed, 0 stale, 0 excluded, 0 not covered).
   - `test/parity/l2.test.mjs`: **30/30** passed (0 failed, 0 stale, 0 excluded, 0 not covered).
   - Native Rust AST & syntax tests (`fiber::syntax::tests` + `fiber::vm::l3_vm_tests`): **95** tests (92 syntax/AST/security unit + 1 native 2 500-input seeded fuzz + 2 L3 VM unit; **289** total `cargo test --lib`).
   - JS seeded parser fuzz slice (`test/l3-parser-fuzz.test.mjs`, seed `0x5345_4E54_4C33_465A`): **2 500** inputs (1 500 grammar-mutated + 1 000 arbitrary byte/UTF-8/surrogate/deep-nesting), **0 traps**, `~295 ms` total (`< 3 s` budget).
4. **Handoff table for L4 and L5:**
   | Downstream lot | `kernel::fiber::syntax::Ast` entry points |
   |---|---|
   | **L4 (Expansions & arithmetic)** | `WordPart::ParamExpand(u32)` -> `Ast::param_expands` (all 15 `ParamOp` variants), `WordPart::CommandSubst { body, backtick }`, `WordPart::ArithExpand(ArithId)`, `WordPart::ProcessSubst { body, output }`, `WordPart::BraceExpand(Span)` -> `Ast::brace_items` (`BraceItem::Word`/`Range`), `WordPart::TildeExpand { user }`, `WordPart::Glob { pattern }`, `Ast::arith_nodes` / `Ast::arith_lists` (`ArithExpr` incl. bases 2..=64, `DynamicBase`, `DynamicNumber`, `ArrayElement`, `NumberSubscript`, `DoubleSubscript`, `SingleQuote`, `Concat`, `SyntaxError`) |
   | **L5 (Redirections, arrays, control flow)** | `Ast::redirs` (`Redir { fd, fd_var, op: RedirOp, target: RedirTarget }`), `Assign { name, subscript, value, array, append }`, `Stmt::source_span` / `Stmt::background` / `Stmt::line`, `Command::CStyleFor`, `Command::Case` -> `Ast::case_items` (`CaseItem` with `DoubleSemi`/`SemiAnd`/`SemiSemiAnd`), `Command::Arithmetic`, `Command::Conditional` -> `Ast::cond_nodes` (`CondExpr` with all 25 unary & 14 binary ops, `=~`, `!(...)`) |
5. **Deviations & `DIVERGENCES.md` candidates:**
   - `time` / `time -p` prints deterministic zero timings (`0m0.000s` / `0.00`) in the exact reference shapes (spec section 7 point 9; candidate for `DIVERGENCES.md`).
   - `test/parity/l3.test.mjs` wraps `sentropic-adapter.mjs` with a 5-line stdin-only custom `cat` command for the three L3 heredoc/pipeline oracle cases until L7a adds `kernel/src/cmd/cat.rs`; `true`, `false`, `:`, `[`, `test` are dispatched as shell builtins in `vm.rs` without touching the read-only `kernel/src/cmd/` registry.
