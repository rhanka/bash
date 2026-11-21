# Test: L2 oracle snapshots (Room 1)

## Objective
L2 oracle snapshots (Room 1)

## Scope / Guardrails
- Scope limited to L2 oracle cases and snapshots, this branch plan, and Track metadata.
- Use the published reference through its public API. Keep the adapter and reference package outside the worktree.
- Work offline with zero Python. All new text is in English.
- Hand off atomic commits to the conductor because lane Git metadata is read-only.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `test/cases/l2/**`
  - `BRANCH.md`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - None.
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional or forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- The conductor applies the commit plan and sequences the Room 2 worktree after the oracle commits reach main.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree, branch, and scope with the harness.
- [x] **Lot 1 — L2 oracle fixtures**
  - [x] Write all 30 case files from section 8.2 and record their snapshots in `test/cases/l2/`.
  - [x] Report reference defects and spec discrepancies to the conductor.
- [x] **Lot 2 — Final validation**
  - [x] Run the recorder in `--check` mode and run `harness check scope`.
  - [x] Run `npm_config_cache=$PWD/tmp/npmcache make check`.
  - [x] Write the commit plan and report to the lanes directory.
