# Feature: L1b WIT resources and JS facade

## Objective
The kernel exports the whole Annex A contract (resources `vfs`, `shell`, `fiber`, bootstrap interpreter,
filesystem slice, limit plumbing) and the package exposes the real facade (`Bash`, `InMemoryFs`, custom
commands, driver loop, membrane, loaders). Spec: `spec/lots/L1b.md`.

## Scope / Guardrails
- Scope limited to the L1b lot spec section 3.
- Make-only workflow; branch work in worktree `tmp/l1b-wit-facade`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.
- `kernel/src/mem/**` is read-only for this lot (owned by L1a-1 and L1a-2).

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/**` (except `kernel/src/mem/**`), `kernel/Cargo.toml` (features only), `Cargo.lock`
  - `host/**`, `types/**`, `test/**`
  - `tools/**` (build learns the two components, the committed bindings check and the `[export]` imports)
  - `wit/**` (one line: the world also exports `types`, see the PR description)
  - `spec/lots/L1b.md` (the lot spec, carried by the branch), `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `.track/**`, `harness.profile.json`
  - `Makefile`, `package.json`, `.github/workflows/**`, `Cargo.toml`
  - `kernel/src/mem/**`, the rest of `spec/**` and `plan/**`
- **Conditional Paths**: none. No scope exception is needed.

## Feedback Loop
- none.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec
- [x] **Lot 1 — Kernel**: bindings (generated, committed), limits, filesystem slice, shell, fiber, bootstrap interpreter, test commands
- [x] **Lot 2 — Build**: components `kernel` and `kernel-test`, bindings check, canonical-builtin imports in the core module
- [x] **Lot 3 — Facade**: loaders, driver, membrane, `Bash`, `InMemoryFs`, types
- [x] **Lot 4 — Tests**: the 22 exit gates
- [x] **Lot N — Final validation**: `make check`
