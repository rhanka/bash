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
- `kernel/src/mem/**` is read-only for this lot outside the exceptions below (owned by L1a-1 and L1a-2).

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/**` (except `kernel/src/mem/**`), `kernel/Cargo.toml` (features only), `Cargo.lock`
  - `host/**`, `types/**`, `test/**`
  - `tools/**` (build learns the two components, the committed bindings check and the `[export]` imports)
  - `wit/**` (export `types` from the world and carry binary stdout through the kernel contract)
  - `spec/lots/L1b.md` (the lot spec, carried by the branch), `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `.track/**`, `harness.profile.json`
  - `Makefile`, `package.json`, `.github/workflows/**`, `Cargo.toml`
  - `kernel/src/mem/**`, the rest of `spec/**` and `plan/**`
- **Conditional Paths**:
  - `kernel/src/mem/mod.rs` under `BR1b-EX1` and `BR1b-EX2`.
  - `kernel/src/mem/bump.rs` and `kernel/src/mem/ceiling.rs` under `BR1b-EX2`.

## Feedback Loop
- **BR1b-EX1** — `kernel/src/mem/mod.rs` gains a small `pub mod abi` that re-exports `Vec`, `String`, `vec!` and the
  allocator entry points.
  - Rationale: main hardened G6 (alloc paths allowed only in `kernel/src/mem/` and the declared bindings
    directory) and the bindings check (that directory holds exactly the generated `kernel.rs`). The hand-written
    glue (`kernel/src/glue/`) names the heap types of the generated traits, and the generated `_rt` module is
    private, so `mem` is the one place that may expose them.
  - Impact: purely additive, no behaviour change; L1a-2 rewrites `mem/mod.rs`, so the merge keeps this block.
  - Rollback: remove the `abi` module and move the glue back under a declared bindings directory.
- **BR1b-EX2** — `kernel/src/mem/mod.rs`, `kernel/src/mem/bump.rs` and `kernel/src/mem/ceiling.rs` expose and enforce a 64 KiB ABI reserve.
  - Rationale: L1a-2's reserve is specified but absent on this base. Ordinary allocations can use the whole ceiling, leaving no space to report a refusal or drain pending bytes.
  - Impact: the Wasm allocator leaves one page for small diagnostics and drain copies. Native ceilings and the configured Wasm ceiling remain the same.
  - Rollback: revert the reserve and adapter changes together. When L1a-2 lands, replace the temporary reserve with its allocator reserve API.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec
- [x] **Lot 1 — Kernel**: bindings (generated, committed), limits, filesystem slice, shell, fiber, bootstrap interpreter, test commands
- [x] **Lot 2 — Build**: components `kernel` and `kernel-test`, bindings check, canonical-builtin imports in the core module
- [x] **Lot 3 — Facade**: loaders, driver, membrane, `Bash`, `InMemoryFs`, types
- [x] **Lot 4 — Tests**: the 22 exit gates
- [x] **Lot N — Final validation**: `make check`
