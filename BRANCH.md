# Feature: L10a parity harness

## Objective
Two engine-agnostic tools, a recorder and a replay runner, with the case and snapshot formats they share,
proven on a mock engine. Spec: `spec/lots/L10a.md`. Based on `main` (L0 merged).

## Scope / Guardrails
- Scope limited to the L10a lot spec section 3.
- Make-only workflow; branch work in worktree `tmp/l10a-parity-harness`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English. Node built-ins only.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `harness/parity/**`
  - `tools/parity-record.mjs`
  - `test/parity/**`, `test/cases/l10a/**`, `test/snapshots/l10a/**`, `test/api/**`
  - `spec/lots/L10a.md`, `BRANCH.md`
  - `.track/**` (Track CLI updates of the L10a item only, single writer)
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `harness.profile.json`, `package.json`
  - the rest of `spec/**`, `plan/**`, `kernel/**`, `probe/**`, `wit/**`, `host/**`, `types/**`
- **Conditional Paths (allowed only with explicit exception)**:
  - `tools/gates.mjs` (BR10a-EX1, export only)
- **Exception process**:
  - Declared in `## Feedback Loop` before touching the path.

## Feedback Loop
- BR10a-EX1 `tools/gates.mjs`. Reason: the Gate 12 isolation self-test reuses `stripComments` and `scanImports`; they are already exported on main, so no edit was needed. Impact: none (read-only use). Rollback: not applicable.
- BR10a-EX2 `tools/check.mjs`, `Makefile`: no longer needed. main's `tools/run-tests.mjs` runs `test/**/*.test.mjs` recursively, so the lot tests run in `make check` unchanged. Withdrawn.
- BR10a-EX3 `.track/**`: the Track CLI (single writer) records the L10a item state; the two files changed are `.track/events.jsonl` and `.track/head.json`. Rollback: revert the Track commit.
- Review fixes (PR #5): see `tmp/conductor/lanes/review-pr5.md`; all findings addressed on this branch.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec copy, scope exceptions
- [x] **Lot 1 — Formats and core**: canonical JSON, validation, case runner, mock adapter
- [x] **Lot 2 — Recorder and replay**: library, CLI, report
- [x] **Lot 3 — Sample, API skeleton, tests**
- [x] **Lot N — Final validation**: `make check`
