# Feature: L1a-1 memory module interface

## Objective
The module `kernel::mem` with its final public interface (errors, instance ceiling, budgets, fallible
containers, transfer hook) and a deliberately simple implementation. Spec: `spec/lots/L1a-1.md`.

## Scope / Guardrails
- Scope limited to the L1a-1 lot spec section 3.
- Make-only workflow; branch work in worktree `tmp/l1a1-mem-interface`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/mem/**`, `kernel/src/lib.rs`, `kernel/tests/**`
  - `tools/**` (only the two pipeline extensions of spec section 7)
  - `test/**`
  - `spec/lots/L1a-1.md` (the lot spec, carried by the branch), `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `.track/**`, `harness.profile.json`
  - `Makefile`, `package.json`, `.github/workflows/**`, `Cargo.toml`, `kernel/Cargo.toml`
  - the rest of `spec/**` and `plan/**`
- **Conditional Paths**: none. No scope exception is needed.

## Feedback Loop
- none.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec
- [x] **Lot 1 — Pipeline extensions** (spec 7): native tests in `make check`, kernel core module build and zero-import check
- [x] **Lot 2 — Module**: errors, ceiling and allocators, budgets, containers, transfer hook
- [x] **Lot 3 — Tests**: interface, budget exactness, nested budgets, leak run, ceiling, overflow, preservation, semantics, formatting, transfer, stray-unsafe check
- [x] **Lot N — Final validation**: `make check`
