# test: L8b and L1d oracle snapshots (Room 1)

## Objective
L8b and L1d oracle snapshots (Room 1).

## Scope / Guardrails
- Scope limited to `test/cases/l8b/**`, `test/cases/l1d/**`, `BRANCH.md`, and `.track/**`.
- Make-only workflow; branch work in worktree `tmp/oracle-w4b`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `test/cases/l8b/**`
  - `test/cases/l1d/**`
  - `BRANCH.md`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - None
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/oracle-w4b` and `harness check branch`.
  - [x] Confirm scope boundaries (no exceptions needed).
- [ ] **Lot 1 — L8b and L1d oracle cases and snapshots**
  - [x] Write 30 case files under `test/cases/l8b/*.case.json` per `spec/lots/L8b.md`.
  - [x] Write 16 case files under `test/cases/l1d/*.case.json` per `spec/lots/L1d.md`.
  - [x] Record snapshots with `tools/parity-record.mjs` against `adapter-just-bash.mjs` and verify `--check`.
- [x] **Lot N — Final validation**
  - [x] `SENTROPIC_TOOLS_DIR=/home/antoinefa/src/bash/tmp/tools npm_config_cache=$PWD/tmp/npmcache make check`
  - [x] `harness check scope`
