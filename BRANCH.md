# Docs: W4 lot specs (L3 full parser, L8a regex and glob)

## Objective
Room-1 lot specs for wave W4: L3 (full lexer and parser) written by Carbon, L8a (regex and glob engines) written by Carbon, both reviewed and fixed by Muse wonder-full against the published just-bash 3.6.0. Owner decision 21/11: glob throw surfaces diverge (Track).

## Scope / Guardrails
- Scope limited to <areas>.
- Make-only workflow; branch work in worktree `tmp/<slug>`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `BRANCH.md`
  - `spec/lots/**`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `rules/**`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [ ] **Lot 0 — Baseline & constraints**
  - [ ] Confirm worktree `tmp/<slug>` and `harness check branch`.
  - [ ] Confirm scope boundaries and declare `BRxx-EXn` exceptions if needed.
- [ ] **Lot 1 — <Main change>**
  - [ ] <Task>
  - [ ] Lot gate: `make lint` + `make test` + `make check-attribution ATTRIBUTION_RANGE=origin/main..HEAD`
- [ ] **Lot N — Final validation**
  - [ ] `make check`
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.
  - [ ] Owner sign-off; remove `BRANCH.md` in the last commit before merge.
