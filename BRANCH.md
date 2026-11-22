# Test: L4 oracle snapshots (Room 1)

## Objective
L4 oracle snapshots (Room 1)

## Scope / Guardrails
- Scope limited to `test/cases/l4/**`, `BRANCH.md`, and `.track/**`.
- Make-only workflow; branch work in worktree `tmp/oracle-l4`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `test/cases/l4/**`
  - `BRANCH.md`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `DIVERGENCES.md`
  - `test/parity/exclusions.json`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/oracle-l4` and `harness check scope`.
  - [x] Confirm scope boundaries.
- [x] **Lot 1 — Record L4 oracle cases & snapshots**
  - [x] Write all 52 case files in `test/cases/l4/`.
  - [x] Record snapshots via `tools/parity-record.mjs` and `/home/antoinefa/src/bash/tmp/bench-engines/adapter-just-bash.mjs`.
  - [x] Verify `--check` stability (51/52 unchanged; `l4/div-prompt-clock` `V11` varies by wall-clock second).
- [x] **Lot 2 — Final validation**
  - [x] `npm_config_cache=$PWD/tmp/npmcache make check`
  - [x] Write commit plan and report to lanes directory.
