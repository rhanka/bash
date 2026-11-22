# docs: Define the L3 and L8a oracle branch scope

## Objective

Record the L3 and L8a oracle snapshots (Room 1) against `just-bash@3.6.0`.

## Scope / Guardrails

- Scope limited to oracle fixtures: case and snapshot JSON only.
- Make-only workflow; branch work in worktree `tmp/oracle-w4`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.
- Never copy reference source into the repo: only `test/cases/l3/**` and
  `test/cases/l8a/**` case files plus their recorded snapshots.

## Branch Scope Boundaries (MANDATORY)

- **Allowed Paths (implementation scope)**:
  - `test/cases/l3/**`
  - `test/cases/l8a/**`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `.track/**`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop

- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`
- No exceptions declared. No `harness`/`track` CLI is installed in this
  environment, so no `.track/**` event was written; conformance is by
  scoped `make commit` paths only.

## Plan / Todo (lot-based)

- [ ] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/oracle-w4` and spec sections 8 (L3) / 7.3 (L8a).
  - [x] Confirm scope boundaries; no exceptions needed.
- [ ] **Lot 1 — L3 cases and snapshots**
  - [ ] Write 36 `test/cases/l3/*.case.json` files per L3 section 8.
  - [ ] Record `test/cases/l3/*.snap.json` with the Room-1 adapter.
  - [ ] Lot gate: `parity-record --check` stable, `make check` green.
- [ ] **Lot 2 — L8a cases and snapshots**
  - [ ] Write 30 `test/cases/l8a/*.case.json` files per L8a section 7.3.
  - [ ] Record `test/cases/l8a/*.snap.json` with the Room-1 adapter.
  - [ ] Lot gate: `parity-record --check` stable, `make check` green.
- [ ] **Lot N — Final validation**
  - [ ] `make check` green on a clean worktree.
  - [ ] Report in `conductor/lanes/oracle-w4-report.md` plus `oracle-w4.done`.
  - [ ] Remove `BRANCH.md` in the last commit before merge.
