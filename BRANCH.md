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
  - `DIVERGENCES.md`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop

- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`
- `BRANCH.md` itself is written in the first commit (allowed path, no exception).
- `DIVERGENCES.md` (conditional path, additive rows only): no `BRxx-EXn` needed unless a
  shared-file edit is required; L3 records the `time`-zeros shape divergence and the
  never-throw-out-of-exec glob policy there, both additive single-row edits. Owner decision
  (21/11, Track): glob throw surfaces diverge from just-bash; never throw out of exec;
  tag them `upstream-defect`.

## Plan / Todo (lot-based)

- [ ] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l3-parser` and `harness check branch`.
  - [x] Confirm scope boundaries and declare `BRxx-EXn` exceptions if needed (none).
  - [ ] Write `BRANCH.md` (this file) and commit it first.
- [ ] **Lot 1 — `syntax` arena AST + full lexer**
  - [ ] Flat-arena `Ast` per spec section 4 (all `KVec` tables, `Copy` id handles).
  - [ ] Multi-heredoc `normalize` FIFO queue (spec 5.2).
  - [ ] Full lexer: operators, reserved words, comments, continuations, all quoting
    forms, `${...}`/`$(...)`/`$((...))`/backtick/process-sub scanning, heredoc bodies.
  - [ ] Lot gate: `cargo test --lib` new AST unit tests green.
- [ ] **Lot 2 — Full parser + budgets + exact diagnostics**
  - [ ] All 12 command kinds, pipelines, lists, deferred root `}`/`)` errors.
  - [ ] Word/parameter/brace/tilde/glob, arithmetic, and `[[ ]]` conditional ASTs.
  - [ ] Token/depth/iteration caps with exact messages; `Error::Lexer`/`Error::Parser`
    display lines and `Fiber::prepare` mapping.
  - [ ] Lot gate: error-case oracle snapshots match byte-for-byte.
- [ ] **Lot 3 — VM + expansion extensions**
  - [ ] `If`/`For`/`While`/`Until`/`Subshell`/`Group`/`FunctionDef`/`time` frames,
    fd-0 heredoc/herestring stdin, empty `Simple` no-op, deferred-error trap,
    `$'...'` expansion, `bash:cmd:*` coverage hits.
  - [ ] Lot gate: runtime oracle cases pass (`if`, `for`, loops, subshell, functions,
    heredoc stdin, ANSI-C).
- [ ] **Lot 4 — Suites, gates, and report points**
  - [ ] `test/parity/l3.test.mjs` (36/36, 0 stale, 0 excluded, 0 not-covered),
    `test/l3-caps.test.mjs`, `test/l3-coverage.test.mjs`,
    `test/l3-parser-fuzz.test.mjs` + native fuzz slice (>= 60 AST unit tests,
    >= 2 500 fuzz inputs).
  - [ ] Record in this file: arena footprint (`size_of` on `wasm32`), core Wasm size
    delta, parity/fuzz counts, L4/L5 handoff table.
  - [ ] Lot gate: `make check` green + `harness check scope` pass.
- [ ] **Lot N — Final validation**
  - [ ] `make check`
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.
  - [ ] Owner sign-off; remove `BRANCH.md` in the last commit before merge.

## Points the builder must confirm and report (spec section 10)

1. **AST arena footprint and zero-recursion drop:** exact `size_of` of `Command`, `Stmt`,
   `WordPart`, `ArithExpr`, and `CondId`-target `CondExpr` on `wasm32`; `Drop for Ast`
   frees flat vectors without recursion; 200-deep scripts run inside the Wasm call stack.
2. **Core Wasm size delta:** `dist/js/kernel/kernel.core.wasm` raw and gzip byte delta added
   by L3 against the Gate L2 `1 500 000 B raw / 700 000 B gzip` budget.
3. **Parity and fuzz counts:** `36/36` on `test/parity/l3.test.mjs`, `30/30` on
   `test/parity/l2.test.mjs`, native AST test count, seeded fuzz input count + wall-clock
   runtime in `make check`.
4. **Handoff table for L4 and L5:** exact `syntax::Ast` entry points L4 will consume
   (`ParamExpand`, `CommandSubst`, `ArithExpand`, `ProcessSubst`, `BraceExpand`,
   `TildeExpand`, `Glob`, `ArithExpr`) and L5 will consume (`Redir`,
   `Assign::subscript`/`array`, `Stmt::source_span`, `CStyleFor`, `Case`, `Conditional`).
