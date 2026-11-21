# Feature: First-party canonical ABI binding

## Objective
Ship a synchronous first-party binding for the Bash engine in both public entries. Run the guarded jco adapter independently as the conformance oracle.

## Scope / Guardrails
- Implement lot L2b on the integrated L2 stack in `tmp/l2b-binding`.
- Work offline from the committed contract and snapshots. Keep runtime dependencies empty.
- Use Make for validation. The conductor owns Git writes and commits through `make commit`.
- Zero Python. All new text is in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `BRANCH.md`
  - `host/core/instance.js`
  - `host/core/abi/**`
  - `tools/**` (build integration, descriptor emission, signature checks and oracle staging)
  - `test/**` (ABI fixtures and tests, kernel-test and pack expectations)
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `package-lock.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
  - `wit/**`
  - `kernel/**`
  - `test/cases/**`
  - `test/snapshots/**`
  - `test/parity/exclusions.json`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - None.
- **Exception process**:
  - Declare an exception ID `BRxx-EXn` in the feedback loop before touching a conditional or forbidden path. Include reason, impact and rollback.

## Feedback Loop
- No scope exception required.
- Spec issue for D6. The L2 test guest can return a 5,000,000-byte `execOneshot` stdout in the micro profile. The binding checks the 4 MiB combined transfer budget before allocating host buffers. The lane report records the probe and the guest-side follow-up.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline and constraints**
  - [x] Confirm wire layout and scope on the integrated L2 worktree.
- [x] **Lot 1 — Binding and oracle**
  - [x] Implement the codecs, resource lifetimes and trap boundary.
  - [x] Check WIT descriptors and core signatures, and stage the guarded oracle.
  - [x] Exercise codec boundaries, differential transcripts, lifetime and parity.
- [x] **Lot 2 — Final validation**
  - [x] Pass `npm_config_cache=$PWD/tmp/npmcache make check` and `harness check scope`.
  - [x] Hand off ordered commit paths and the verification report to the conductor.
