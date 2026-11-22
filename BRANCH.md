# feat: L2c default snapshot, minified packages and Gate L2 rerun

## Objective
Implement the Wasm-owned default snapshot restore path, share and minify the facade, package Option D separately as `@sentropic/bash-jco`, and rerun the Gate L2 protocol on the packed default package with the exact verdict in `plan/reports/GATE_L2.md`.

## Scope / Guardrails
- Scope limited to `spec/lots/L2c.md` allowed paths; keep prior Gate L2 history and append the new measurement and verdict.
- Make-only workflow; branch work in worktree `tmp/l2c-snapshot`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `host/**` (default selection, shared facade, entry and loader)
  - `kernel/src/glue/**` (internal snapshot/capture hooks if required)
  - `kernel/src/mem/mod.rs` (crate-visible restore entry into the private allocator)
  - `kernel/src/mem/ceiling.rs` (read-only freshness check on the private configuration and usage state)
  - `kernel/src/mem/heap.rs` (allocator-owned checked page growth for restore only)
  - `tools/**` (snapshot, build stages, gate/pack walkers, measurement)
  - `packages/bash-jco/**` (companion manifest and staging inputs)
  - `test/**` (snapshot, consumers, guard and decision fixtures)
  - `plan/reports/GATE_L2.md`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `package.json` via `BR<n>-EX1` (route normal pack and consumers to the built entry graphs)
  - `spec/SPEC_EVOL_BASH_SANDBOX.md` via `BR23-EX2` (apply the 22/11 owner decision on JS budgets in Annex H and record it in Annex J)
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Exceptions
- `BR<n>-EX1` (`package.json`): route normal pack and consumers to the built entry graphs. Same specifiers, declarations and engine support, empty dependencies. Rollback: restore the preceding manifest and matching pack recipe.
- `BR23-EX2` (`spec/SPEC_EVOL_BASH_SANDBOX.md`): apply the 22/11 owner decision splitting cold-path JS budgets (`B_cold <= 58,000 B`, `Z_cold <= 30,000 B`) from total shipped JS budgets (`B_total <= 75,000 B`, `Z_total <= 50,000 B`, `T <= 110,000 B`) in Annex H and record it in Annex J. Impact: normative Annex H JS budget row/prose and Annex J reconciliation table only; no kernel, WIT or public API change. Rollback: revert the Annex H, Section 3 and Annex J edits in `spec/SPEC_EVOL_BASH_SANDBOX.md`.

## Feedback Loop
- `BR23-EX2` (`spec/SPEC_EVOL_BASH_SANDBOX.md`): 22/11 owner decision (PR #23 review) — update Annex H JS budgets and record in Annex J.
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l2c-snapshot` and `harness check branch`.
  - [x] Confirm scope boundaries and declare `BRxx-EXn` exceptions if needed.
- [x] **Lot 1 — Deterministic snapshot and restoration**
  - [x] Capture the real default; prove quiescence; encode the difference inside the core.
  - [x] Private core ABI `__restore_default`, restore-only `pub(crate)` helper, allocator-owned page growth.
- [x] **Lot 2 — Predicate, provenance, pipeline order**
  - [x] `isDefaultSnapshotConfig` predicate; provenance selection on both entries; image before component embed/new.
- [x] **Lot 3 — Package interface and size accounting**
  - [x] Minified facade via `oxc-minify`, shared chunk per package, staged declarations, size split B/N/T/Z vs both caps.
- [x] **Lot 4 — Binaryen measurement and Gate L2 rerun**
  - [x] `wasm-opt` behind the pin (report not measured when unpinned); Gate L2 on the packed default package.
- [ ] **Lot N — Final validation**
  - [x] `make check`
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.
  - [ ] Owner sign-off; remove `BRANCH.md` in the last commit before merge.
