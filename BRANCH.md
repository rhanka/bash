# Docs: Specify the L2 binding, Gate L2 and shared filesystem modes

## Objective

Turn the 21 November owner decisions and the Gate L2 measurements into executable Room 2 contracts for L2b, L2c and L7c.

## Scope / Guardrails

- Design documents only. Keep the L2 component and the L7b mount-host contract as the implementation baselines.
- Make-only checks in `tmp/lot-specs-l2bc`. Work offline. Keep temporary output under the worktree.
- The conductor owns commits. Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)

- **Allowed Paths (implementation scope)**:
  - `spec/lots/**`
  - `spec/SPEC_EVOL_BASH_SANDBOX.md`
  - `BRANCH.md`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
  - `docker-compose*.yml`
  - `Dockerfile*`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `spec/SPEC_EVOL_BASH_SANDBOX.md` (BR02c-EX1)
- **Exception process**:
  - Declare ID, reason, impact and rollback in the feedback loop before editing a conditional path.

## Feedback Loop

- BR02c-EX1 — Record the owner decisions and the two open measurement gates in the EVOL. Impact is limited to the design and its revision history. Rollback restores the preceding EVOL revision.
- The lane reports blocked / deferred / cancelled / attention. The conductor can clarify, acknowledge or refuse.

## Plan / Todo (lot-based)

- [x] **Lot 0 — Baseline and constraints**
  - [x] Confirm the worktree and branch. Declare scope and the EVOL exception.
- [x] **Lot 1 — Specify L2b**
  - [x] Reconcile the WIT surface, ABI validators, lifetimes and jco differential fixtures in `spec/lots/L2b.md`.
- [x] **Lot 2 — Specify L2c**
  - [x] Reconcile snapshot restore, packaging, Annex H accounting and the Gate L2 decision in `spec/lots/L2c.md`.
- [x] **Lot 3 — Specify L7c**
  - [x] Cross-check execution stores, synchronous filesystem batches and both public modes against L7b in `spec/lots/L7c.md`.
- [x] **Lot 4 — Amend EVOL**
  - [x] Align D15, D16, D18, D27 and Annexes H, I and J with the three contracts.
- [x] **Lot 5 — Final validation**
  - [x] Run `npm_config_cache=$PWD/tmp/npmcache make check` and `harness check scope`.
  - [x] Deliver the commit manifest and lane report to the conductor.
