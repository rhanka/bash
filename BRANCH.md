# Chore: Finish harness adoption (Makefile + Track)

## Objective
Complete the harness adoption of `@sentropic/bash`: a minimal Makefile for the repo gates and an initialized Track store.

## Scope / Guardrails
- Scope limited to the Makefile, the Track store and this BRANCH.md.
- Make-only workflow; branch work in worktree `tmp/harness-adoption`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `BRANCH.md`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `Makefile` (BR00-EX1)
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- conductor: `acknowledge` — BR00-EX1: create `Makefile` (default-forbidden). Reason: the repo has no command runner yet and the gates (hooks, check-attribution, check-no-python, check-no-deps, test, bench) need one entry point. Impact: new file only, no existing behaviour changes. Rollback: delete `Makefile`.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/harness-adoption` and `harness check branch`.
  - [x] Confirm scope boundaries and declare `BR00-EX1`.
- [x] **Lot 1 — Makefile + Track**
  - [x] `Makefile` with `hooks`, `check-attribution`, `check-no-python`, `check-no-deps`, `test`, `bench`, `check`, `commit`.
  - [x] `track init`.
  - [x] Lot gate: `make check`.
- [ ] **Lot N — Final validation**
  - [ ] Peer review; owner sign-off; remove `BRANCH.md` in the last commit before merge.
