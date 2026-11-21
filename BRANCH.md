# Fix: L0 tooling hardening after review

## Objective
Close the residual holes found by the adversarial review of PR #6 in the L0 tooling now on main: the G2 comment stripper and evaluator bypasses (finding 2) and the tool supply-chain / G8 gaps (finding 5).

## Scope / Guardrails
- Scope limited to tools/, test/, plan/reports/ and this file.
- Make-only workflow; branch work in worktree `tmp/tools-hardening`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `tools/**`
  - `test/**`
  - `plan/reports/**`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - none
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.

## Feedback Loop
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [ ] **Lot 0 — Baseline**
  - [ ] Confirm worktree and scope.
- [ ] **Lot 1 — G2 hardening**
  - [ ] Template `${}` stack, keyword-preceded regex, `\u` escapes, banned identifiers, with negative tests.
- [ ] **Lot 2 — Supply chain**
  - [ ] Pre-scan tar, validate pin fields, npm node_modules manifest in fetchNpm and G8, scrub SENTROPIC_PINS.
- [ ] **Lot N — Final validation**
  - [ ] `make check`, CI simulation, push.
