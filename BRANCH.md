# Feature: Host filesystem adapters (L7b)

## Objective

Ship Node host filesystem adapters and kernel-to-host filesystem batches with containment, inode isolation, and revocable capability checks.

## Scope / Guardrails

- Worktree `tmp/l7b-host-fs`, branch `feat/l7b-host-fs`.
- Clean-room inputs are the specification and committed snapshots. Work offline with zero Python, dependencies, or crates.
- Use `tmp/` for scratch, npm cache, and fixtures. The conductor applies the ordered commit manifest.

## Branch Scope Boundaries (MANDATORY)

- **Allowed Paths (implementation scope)**:
  - `BRANCH.md`
  - `kernel/src/fs.rs`
  - `kernel/src/fiber.rs`
  - `kernel/src/fiber/vm.rs`
  - `kernel/src/fiber/testcmd.rs`
  - `kernel/src/bindings/adapt.rs`
  - `kernel/src/glue/adapt.rs`
  - `host/index.js`
  - `host/core/facade.js`
  - `host/core/driver.js`
  - `host/core/fs-batch.js`
  - `host/node/overlay-fs.js`
  - `host/node/read-write-fs.js`
  - `host/node/host-paths.js`
  - `host/node/fs-errors.js`
  - `types/index.d.ts`
  - `test/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `LICENSE`
  - `.github/workflows/**`
  - `.githooks/**`
  - `docker-compose*.yml`
  - `Dockerfile*`
  - `kernel/src/mem/**`
  - `wit/**`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `kernel/src/shell.rs` only if the host filesystem capability check requires a helper.
  - `kernel/src/fiber/tests.rs` under `BR7b-EX1`.
- **Exception process**:
  - Record any `BR7b-EXn` exception with reason, impact, and rollback before touching a conditional or forbidden path.

## Feedback Loop

- The existing handwritten WIT adapter lives in `kernel/src/glue/adapt.rs`, as identified by the conductor.
- `BR7b-EX1` permits updating the L1b native host-call test in `kernel/src/fiber/tests.rs`. The test-commands build references the removed request and outcome types. This exception changes the test fixture and assertions only. Rollback restores the old test together with the previous request and outcome types.
- `BR7b-EX2` permits the host-mount layout guard in `kernel/src/shell.rs` (`Shell::layout` returns early when a host mount covers `/`). Reason: the kernel volume is fully shadowed by the mount, so the facade seeds the same layout into the host adapter instead of duplicating it. Impact: layout and stub code only, behind the mount check; no command or VFS semantics change. Rollback: delete the early return to restore the unconditional in-kernel layout.
- L2 has merged and this branch is rebased on `main`, which holds L2 and L2b: test commands dispatch through L2's VM, kernel command stubs seed from `COMMAND_NAMES`, and the sentinel and walk gates run on the merged tree.
- Linux Node 22 reports `ELOOP` for a symlink leaf opened with `O_NOFOLLOW`. A real directory opens with `O_RDONLY | O_DIRECTORY | O_NOFOLLOW`.
- The 1,000-file walk returns 1,010 entries per adapter. Each run makes one host call and 11 directory listings. The final check measured 163.38 ms for `OverlayFs` and 131.64 ms for `ReadWriteFs`.
- Upstream defect for the owner listing. The reference hangs on host FIFO reads and overlay FIFO append. L7b rejects both operations promptly.
- Path validation opens the canonical root directly and verifies the descriptor against `/proc/self/fd` on Linux; no ancestor above the root is ever opened, so traverse-only ancestors need no read permission. Where procfs descriptors are unavailable (macOS, Windows, procfs-less containers) the adapters fall back to validated canonical paths with `O_NOFOLLOW` and `(dev, ino)` checks per L7b §5.2.

## Plan / Todo (lot-based)

- [x] **Lot 0 — Baseline and scope**
  - [x] Confirm the branch and run `harness check scope`.
- [x] **Lot 1 — Node filesystem adapters**
  - [x] Implement path controls, overlay, read-write mutations, Node exports, and declarations.
  - [x] Verify D10, S7, and Node-only isolation.
- [x] **Lot 2 — Mounts and batches**
  - [x] Implement the mount table and typed, capability-gated batch requests and outcomes.
  - [x] Wire facade layout and the revocable driver. Exercise sentinels, all operations, and the walk gate.
- [x] **Lot 3 — Final validation**
  - [x] Run `npm_config_cache=$PWD/tmp/npmcache make check` and `harness check scope`.
  - [x] Write the conductor commit manifest and report.
