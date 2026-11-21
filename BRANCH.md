# Fix: Make timing gates robust on CI

## Objective
Keep performance gates meaningful on shared CI runners and retain the exact local limits.

## Scope / Guardrails
- Scope limited to timing assertions, their diagnostics, and this branch plan.
- Make-only workflow in `tmp/fix-ci-speed`.
- Hand off atomic commits to the conductor for staging and committing.
- No network, reference checkout access, or Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `test/**`
  - `kernel/tests/**`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - None.
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- Lane: Use the conductor handoff for commit and verification results.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm branch and worktree with `harness check branch`.
  - [x] Audit allocator, native kernel, L1b, and L10a timing gates.
- [x] **Lot 1 — CI timing limits**
  - [x] Adjust absolute CI limits and log each measurement against its limit.
  - [x] Preserve absolute local limits and functional assertions.
  - [x] Lot gate: `harness check scope`.
- [x] **Lot 2 — Final validation**
  - [x] Run `npm_config_cache=$PWD/tmp/npmcache make check` with CI unset and with `CI=true`.
  - [x] Write the commit manifest and report for the conductor.
