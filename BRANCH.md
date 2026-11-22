# feat: L7c execution isolation for a shared InMemoryFs

## Objective
Give one shared `InMemoryFs` volume two execution modes: the default `separate` (a private execution store per `Bash`, synchronous in-memory root mount) and opt-in `shared` (one common store), preserving identity, sync access and per-shell capabilities.

## Scope / Guardrails
- Scope limited to `spec/lots/L7c.md` section 3 allowed paths; L7b mount/fs-batch signatures suffice.
- Make-only workflow; branch work in worktree `tmp/l7c-shared-fs`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `host/core/facade.js`
  - `host/core/holder.js`
  - `host/core/instance.js`
  - `host/core/fs.js`
  - `host/core/driver.js`
  - `host/core/fs-batch.js`
  - `types/core.d.ts`
  - `types/index.d.ts`
  - `types/browser.d.ts` (the lot text names `types/browser.d.ts`; this is that file)
  - `kernel/src/fs.rs`
  - `kernel/src/shell.rs`
  - `kernel/src/fiber/**`
  - `kernel/src/fiber.rs` (only mounted-root dispatch/layout as needed)
  - `test/**` (two-store and one-store suites, declarations, consumers)
  - `DIVERGENCES.md`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - (none)
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Exceptions
None. The L7b `mount-host`, `unmount` and `fs-batch` signatures suffice; no `BRxx-EXn` needed.

## Feedback Loop
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l7c-shared-fs`.
  - [x] Confirm scope boundaries; no `BRxx-EXn` exceptions.
- [ ] **Lot 1 — `isolation` option and construction**
  - [ ] Validate `isolation: "separate" | "shared"` synchronously; default `separate`.
  - [ ] Separate: fresh execution holder + local root, `mountHost("/", 1)`; shared: attach to the volume's holder.
  - [ ] Seed layout through the owning `Vfs`; align `InMemoryFs` sync methods with declarations.
  - [ ] Lot gate: `make check` green.
- [ ] **Lot 2 — Synchronous mount dispatch**
  - [ ] Serve `fs-batch` mount 1 through the volume owner's `vfs.op` with request/response byte bounds.
  - [ ] Trap domains: poison semantics per store; `KernelPoisonedError` vs `io` results.
  - [ ] Lot gate: `make check` green.
- [ ] **Lot 3 — Tests and declarations**
  - [ ] Two-store and one-store suites, declaration consumers, browser staging order.
  - [ ] `DIVERGENCES.md` D27/V9 entry without a new oracle exclusion.
- [ ] **Lot N — Final validation**
  - [ ] `make check` (30/30 parity on both bindings).
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.
  - [ ] Owner sign-off; remove `BRANCH.md` in the last commit before merge.
