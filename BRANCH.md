# Feature: L1a-2 memory module completion

## Objective
Production allocator for the wasm target (bounded footprint, reserve), complete byte accounting
(footprint, headroom, reserve scope), B-tree `KMap`, `KInsertMap` and `KChunks`. Spec: `spec/lots/L1a-2.md`.
Every L1a-1 signature and behaviour stays as it is.

## Scope / Guardrails
- Scope limited to the L1a-2 lot spec section 2.
- Make-only workflow; branch work in worktree `tmp/l1a2-mem-allocator`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/mem/**`
  - `kernel/tests/**`
  - `kernel/Cargo.toml` (one feature: `mem-test-exports`)
  - `test/**`
  - `tools/components.json` (only if an extra core-module build is needed)
  - `spec/lots/L1a-2.md` (the lot spec, carried by the branch), `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `.track/**`, `harness.profile.json`
  - `Makefile`, `package.json`, `.github/workflows/**`, `Cargo.toml`
  - `kernel/src/lib.rs` and everything in `kernel/src/` outside `mem/` (owned by L1b during this wave)
  - the rest of `spec/**` and `plan/**`
- **Conditional Paths**: none. No scope exception is needed.

## Feedback Loop
- none.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec
- [x] **Lot 1 — Accounting**: atomic budget counters, static headroom account, transfer charging
- [x] **Lot 2 — Allocator**: boundary-tag size-class heap, reserve, footprint, native unit tests
- [x] **Lot 3 — Containers**: B-tree `KMap`, `KInsertMap`, `KChunks`, native tests
- [x] **Lot 4 — Wasm gates**: `mem-test-exports`, node:test workloads, source check
- [x] **Lot N — Final validation**: `make check`
