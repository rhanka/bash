# Feature: L10a parity harness

## Objective
Two engine-agnostic tools, a recorder and a replay runner, with the case and snapshot formats they share,
proven on a mock engine. Spec: `spec/lots/L10a.md`. Stacked on `feat/l0-pipeline`.

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
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `.track/**`, `harness.profile.json`, `package.json`
  - the rest of `spec/**`, `plan/**`, `kernel/**`, `probe/**`, `wit/**`, `host/**`, `types/**`
- **Conditional Paths (allowed only with explicit exception)**:
  - `tools/check.mjs` (BR10a-EX1), `Makefile` (BR10a-EX2)
- **Exception process**:
  - Declared in `## Feedback Loop` before touching the path.

## Feedback Loop
- BR10a-EX1 `tools/check.mjs`. Reason: the test glob `test/*.test.mjs` does not reach `test/parity/` and `test/api/`, so the lot tests would not run in `make check`. Impact: glob list extended with `test/*/*.test.mjs`; nothing else changes. Rollback: revert the file.
- BR10a-EX2 `Makefile`. Reason: same glob in the `test` target. Impact: same. Rollback: revert the file.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec copy, scope exceptions
- [x] **Lot 1 — Formats and core**: canonical JSON, validation, case runner, mock adapter
- [x] **Lot 2 — Recorder and replay**: library, CLI, report
- [x] **Lot 3 — Sample, API skeleton, tests**
- [x] **Lot N — Final validation**: `make check`
