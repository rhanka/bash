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
  - `kernel/src/*.rs`, `kernel/src/bindings/**`, `kernel/src/fiber/**`, `kernel/src/glue/**`
  - `kernel/Cargo.toml` (features only), `Cargo.lock`
  - `host/**`, `types/**`, `test/**`
  - `tools/**` (build learns the two components, the committed bindings check and the `[export]` imports)
  - `wit/**` (export `types` from the world and carry binary stdout through the kernel contract)
  - `spec/lots/L1b.md` (the lot spec, carried by the branch), `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `.track/**`, `harness.profile.json`
  - `Makefile`, `package.json`, `.github/workflows/**`, `Cargo.toml`
  - The rest of `spec/**` and `plan/**`
- **Conditional Paths**:
  - `kernel/src/mem/mod.rs`, `kernel/src/mem/heap.rs` and `kernel/src/mem/ceiling.rs` under BR1b-EX1.

## Feedback Loop
- **BR1b-EX1** — `mem::abi` re-exports the heap types and allocator entry points used by the binding
  adapter. Its `cabi_realloc` entry tags incoming transfer blocks with a spare Wasm heap header bit.
  Restore `ceiling.rs` to L1a-2 and use its dedicated reserve through `reserve_scope`.
  - Rationale: main hardened G6 (alloc paths allowed only in `kernel/src/mem/` and the declared bindings
    directory) and the bindings check (that directory holds exactly the generated `kernel.rs`). The hand-written
    glue (`kernel/src/glue/`) names the heap types of the generated traits, and the generated `_rt` module is
    private. Generated Rust-owned inputs and generated post-return cleanup deallocate through the global
    allocator, which must release the transfer charge carried by each tagged block.
  - Impact: the L1a-2 heap keeps one reserve page for bounded diagnostics and drain copies. The transfer
    account releases each tagged charge on free or when an ordinary Rust resize takes ownership.
  - Rollback: revert the ABI glue and its heap tag together. Remove `mem::abi` when the glue no longer uses it.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline**: BRANCH.md, lot spec
- [x] **Lot 1 — Kernel**: bindings (generated, committed), limits, filesystem slice, shell, fiber, bootstrap interpreter, test commands
- [x] **Lot 2 — Build**: components `kernel` and `kernel-test`, bindings check, canonical-builtin imports in the core module
- [x] **Lot 3 — Facade**: loaders, driver, membrane, `Bash`, `InMemoryFs`, types
- [x] **Lot 4 — Tests**: the 22 exit gates
- [x] **Lot N — Final validation**: `make check`
