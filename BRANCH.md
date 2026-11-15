# Docs: Lot specs for wave W1 (L1a-1, L10a)

## Objective
Write the Room-1 lot specs of wave W1 of `plan/PLAN_BASH_SANDBOX_V1.md`: `spec/lots/L1a-1.md` (memory
module interface) and `spec/lots/L10a.md` (parity harness), per plan section 6.

## Scope / Guardrails
- Scope limited to lot specs under `spec/lots/`.
- Make-only workflow; branch work in worktree `tmp/lot-specs-w1`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.
- Reference-reading lane (Room 1): specs describe behaviour and interfaces, carry no reference code and no
  reference path a builder would need to follow.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `spec/lots/**`
  - `BRANCH.md`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `spec/SPEC_EVOL_BASH_SANDBOX.md`
  - `plan/PLAN_BASH_SANDBOX_V1.md`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- none yet

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Worktree `tmp/lot-specs-w1` from `main`.
- [ ] **Lot 1 — Specs**
  - [ ] `spec/lots/L1a-1.md`
  - [ ] `spec/lots/L10a.md`
- [ ] **Lot N — Final validation**
  - [ ] Review of both specs by the review lane; `make check`.
  - [ ] Remove `BRANCH.md` in the last commit before merge.
