# feat: L1d observability (logger, trace, coverage)

## Objective
Complete v1 observability across the kernel, ABI adapter, driver, facade, browser entry, and parity adapter: ordered host-side `logger`, bounded per-fiber `trace` and `coverage` buffers with `OutOfFuel` backpressure and multi-slice draining, non-observable `trace-clock` host requests (S2/S11), `./browser` `cmd:flag:*` filtering, `CmdCtx` observability hooks for downstream command lots, and `sentropic-adapter` `takeTrace`/`takeCoverage` parity capture.

## Scope / Guardrails
- Scope limited to `spec/lots/L1d.md` allowed paths (plus `kernel/src/fiber/tests.rs` for the `Outcome::Clock` variant update).
- Make-only workflow; branch work in worktree `tmp/l1d-observability`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `kernel/src/shell.rs`
  - `kernel/src/fiber.rs`
  - `kernel/src/fiber/vm.rs`
  - `kernel/src/fiber/testcmd.rs`
  - `kernel/src/fiber/tests.rs`
  - `kernel/src/cmd/mod.rs`
  - `kernel/src/glue/adapt.rs`
  - `kernel/src/glue/rt.rs`
  - `host/core/driver.js`
  - `host/core/facade.js`
  - `host/index.js`
  - `host/browser.js`
  - `harness/parity/sentropic-adapter.mjs`
  - `test/parity/l1d.test.mjs`
  - `test/l1d-*.test.mjs`
  - `test/l1b-membrane.test.mjs`
  - `BRANCH.md`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
  - `wit/**`
  - `types/**`
  - `kernel/src/lib.rs`
  - `kernel/src/mem/**`
  - `kernel/src/limits.rs`
  - `kernel/src/fs/**`
  - `kernel/build.rs`
  - `tools/**`
  - `harness/parity/record.mjs`
  - `harness/parity/replay.mjs`
  - `harness/parity/run.mjs`
  - `harness/parity/compare.mjs`
  - `harness/parity/validate.mjs`
  - `test/cases/l1d/**`
  - `test/parity/exclusions.json`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - None
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- subagent/agent: `blocked` / `deferred` / `cancelled` / `attention`
- conductor/owner: `clarification` / `acknowledge` / `refuse`

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Confirm worktree `tmp/l1d-observability` and `harness check scope`.
  - [x] Confirm scope boundaries (no `BR<n>-EX<k>` exception needed).
- [x] **Lot 1 — Kernel observability (`shell`, `fiber`, `vm`, `cmd`, `glue`, `testcmd`)**
  - [x] Wire `Shell::trace`, `HostReq::TraceClock`, `Outcome::Clock { wall_ms, monotonic_ns }`, bounded `COVERAGE_BUF_CAP = 4_096` and `TRACE_BUF_CAP = 256` buffers, `CmdCtx` hooks, `take_trace` ABI lowering with `LOWERING = 257`, and `__cov`/`__trace` test commands.
  - [x] Lot gate: `cargo test` green (both default and `test-commands`).
- [x] **Lot 2 — Host driver, facade, browser filter, and parity adapter**
  - [x] Synchronous `trace-clock` handling before `onHostCall`, `drain(x)` for coverage/trace, `{ browser }` filter for `cmd:flag:*`, uniform pre/post `logger` ordering across `exec`/`execSync`/`#runChild`, and `sentropic-adapter.mjs` `takeTrace`/`takeCoverage`.
- [x] **Lot 3 — Exit gates & validation**
  - [x] Add `test/parity/l1d.test.mjs`, `test/l1d-logger.test.mjs`, `test/l1d-buffers.test.mjs`, `test/l1d-browser.test.mjs`, `test/l1d-security.test.mjs`.
  - [x] Confirm buffer caps, ABI lowering footprint, L2c JS budgets (`tools/measure-l2.mjs`), drain order, and `CmdCtx` readiness.
  - [x] Lot gate: `make check`, `harness check scope`, and `make check-attribution ATTRIBUTION_RANGE=origin/main..HEAD`.
- [ ] **Lot N — Final validation**
  - [x] `make check`
  - [ ] PR body = this `BRANCH.md`; CI green on the PR.
  - [ ] Owner sign-off; remove `BRANCH.md` in the last commit before merge.

## Section 8 Confirmations
- **Buffer caps & ABI lowering:** `COVERAGE_BUF_CAP = 4_096`, `TRACE_BUF_CAP = 256`, `LOWERING` widened to `257` slots in `kernel/src/glue/rt.rs` with per-event `(events.len(), 32, 8)` + `(ev.details.len(), 24, 8)` `prepare_lowering` accounting and `reserve_scope` single-event fallback in `take_trace`. `dist/js/kernel/kernel.core.wasm`: `268,025` B raw / `93,780` B gzip (+4,142 B raw / +1,587 B gzip vs main); L2c split: `B_cold = 57,856` B (<= 58,000), `Z_cold = 22,188` B (<= 30,000), `B_total = 69,081` B, `Z_total = 35,621` B, `T = 97,704` B.
- **Drain ordering:** Verified at `out-of-fuel` (`__cov 12000`, `__trace flood 1000`), pre-`host-call` (`echo -n before; mycmd; echo -e after`), `cancel` (`AbortController`), and `done` (`exec`/`execSync`), plus nested `ctx.exec` logger/coverage interleaving.
- **Downstream `CmdCtx` readiness:** `live_budget`, `should_yield`, `coverage_enabled`, `hit_coverage`, `trace_enabled`, `trace_clock_req`, `emit_trace`, and `emit_find_trace` exposed on `CmdCtx` in `kernel/src/cmd/mod.rs`.
