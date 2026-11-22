# Lot L1d — Observability (`logger`, `trace`, `coverage`)

Lot spec for the build lane (Room 2). Sources of authority: `spec/SPEC_EVOL_BASH_SANDBOX.md` ("EVOL":
decisions D1-D4, D6, D9, D14, D15, D19-D21, D23; security invariants S1, S2, S3, S10, S11, S14; Annexes A,
D.1, D.6, D.7, E, F, G, J) and `plan/PLAN_BASH_SANDBOX_V1.md` (lot L1d, sections 3, 4, 6 and 8). It builds on
`spec/lots/L0.md` (gates G1-G8), `spec/lots/L1a-1.md` and `spec/lots/L1a-2.md` (`kernel::mem`),
`spec/lots/L1b.md` (WIT resources, driver loop, facade), `spec/lots/L2.md` (`COVERAGE_FEATURES`, `Fiber`,
`sentropic-adapter`), `spec/lots/L2b.md` (first-party canonical-ABI binding), `spec/lots/L8b.md` (preceding W5
Slot B lot), and `spec/lots/L10a.md` (parity harness `trace` and `coverage` comparators). Where this document
and EVOL differ, stop and raise a spec issue.

- **Size.** 2 points, Mid. **Builder.** Muse 1.3 xhigh (Codex Sol 6.2 xhigh from 27/11; plan Mid slot: Codex
  Sol 6.1 medium, fallback Sonnet 5.5). **Review.** Carbon, full review of every PR, dispatched on a green
  `make check`.
- **Branch.** `feat/l1d-observability`, worktree `tmp/l1d-observability`.
- **Outcome.** Completes v1 observability across kernel, ABI adapter, driver, facade, browser entry, and parity
  adapter: host-side `logger` with exact Annex D.1 call ordering across top-level and nested `ctx.exec` runs,
  bounded per-fiber `trace` and `coverage` buffers yielding `out-of-fuel` when full without dropping events or
  hits, monotonic `trace-clock` host request answered without `real-clock` grant and invisible to scripts (S11),
  ordered draining on `out-of-fuel`, before host callbacks, on `cancel`, and before `exec` resolves, `./browser`
  filtering of `cmd:flag:*` hits, `CmdCtx` hooks for downstream command lots (`find`, `sed`, `awk`, `jq`), and
  `sentropic-adapter` `takeTrace`/`takeCoverage` parity capture.

## 1. Lane rules

1. No reference source and no reference package in the worktree. No network. The conductor (Room 1) writes the
   16 case files `test/cases/l1d/*.case.json` from section 6.3, records `test/cases/l1d/*.snap.json` against
   `just-bash@3.6.0` (with `defenseInDepth: false` when `capture.coverage` is enabled, as `adapter-just-bash.mjs`
   already does), and merges both to `main` before the worktree is created (PLAN section 6, steps 3-4). The
   builder never edits a committed case or snapshot and never runs or imports the reference engine.
2. Kernel: `no_std` on `wasm32-wasip2`, zero external crates in `Cargo.lock`, zero Wasm imports (G1), all heap
   allocations through `kernel::mem` against `&Budget` (G6), `unsafe` only in `kernel/src/mem/` and
   `kernel/src/bindings/`. `wit/sentropic-bash.wit` already declares every observability type and method and is
   **read-only** in this lot.
3. Host and tests: plain ES modules with JSDoc, Node built-ins only under `host/node/**`, `tools/**`, `test/**`,
   and `harness/**` (gate G2 checks that `host/browser.js` reaches zero `node:` modules), no Python and no Docker.
4. Never write to `/tmp`; scratch files go under the git-ignored `tmp/` of the worktree.
5. Commits through `make commit`, in English, without attribution trailers.

## 2. Scope

What is **already built on `main`** (keep and extend in place, do not duplicate):
- `wit/sentropic-bash.wit`: `shell-config { trace, coverage }`, `clock-reading { wall-ms, monotonic-ns }`,
  `host-req::trace-clock`, `host-outcome::clock(clock-reading)`, `trace-value`, `trace-detail`, `trace-event`,
  `engine.coverage-features()`, `fiber.take-trace(max)`, `fiber.take-coverage(max)`.
- `kernel/build.rs` and `kernel/src/cmd/mod.rs`: generated sorted `COVERAGE_FEATURES` table and
  `coverage_id(feature: &str) -> Option<u16>`.
- `kernel/src/glue/adapt.rs`: `Guest::coverage_features()`, `GuestFiber::take_coverage(max)`, and the
  `oneshot` guard rejecting `req.config.trace || req.config.coverage` with `KernelErrorCode::InvalidConfig`.
- `host/core/facade.js`: constructor option validation for `logger`, `trace`, and `coverage`, passing `trace`
  and `coverage` booleans in `shellConfig`; `host/core/driver.js`: `ctx.trace` and `ctx.coverage` exposure to
  custom commands in `serveCustom`.

In scope for L1d:
- **Kernel `Shell` and `Fiber` (`kernel/src/shell.rs`, `kernel/src/fiber.rs`, `kernel/src/fiber/vm.rs`):**
  - Add `pub trace: bool` to `shell::Config` and `Shell` (wired from `T::ShellConfig.trace` in `adapt.rs`).
  - Add `HostReq::TraceClock` and extend `Outcome::Clock` to carry `{ wall_ms: i64, monotonic_ns: u64 }` so
    `trace-clock` readings reach the fiber without `G_REAL_CLOCK` and without touching `Shell::virt_ms` or any
    script-visible state (S11).
  - Bound both per-fiber observability buffers in `&self.live`: `COVERAGE_BUF_CAP = 4_096` (`u16` IDs) and
    `TRACE_BUF_CAP = 256` (`TraceEvent` records). Reaching either cap zeroes the current slice fuel
    (`fuel = 0, fuel_credit = 0`) so the fiber returns `Step::OutOfFuel` at the slice boundary and the driver
    drains the buffer before resuming — **zero dropped hits or events**, and both buffers stay intact across
    `Fiber::finish` and `Fiber::cancel` until drained by `take_coverage` / `take_trace`!
- **Command-context observability API (`kernel/src/cmd/mod.rs`) and ABI lowering (`kernel/src/glue/adapt.rs`, `kernel/src/glue/rt.rs`):**
  - Export `TraceValue`, `TraceDetail`, `TraceEvent`, and `FindTraceStats` / `TraceSpan` helpers on `CmdCtx`
    (`coverage_enabled`, `hit_coverage`, `trace_enabled`, `emit_trace`, `should_yield`) so L4 (at exit) and
    command lots L9a-2 (`find`), L13 (`sed`), L14a (`awk`), and L15a-1 (`jq`) emit trace events and coverage
    hits without editing `fiber.rs` or `adapt.rs`.
  - Implement `GuestFiber::take_trace(max: u32) -> Vec<T::TraceEvent>` in `adapt.rs` (widening `LOWERING` in
    `kernel/src/glue/rt.rs` to `TRACE_BUF_CAP + 1` = `257` slots) with per-event `rt::prepare_lowering`
    headroom accounting (`(events.len(), 32, 8)` plus one `(ev.details.len(), 24, 8)` per event with non-empty
    details) and single-event `reserve_scope` fallback.
- **Host driver, facade, and `./browser` filter (`host/core/driver.js`, `host/core/facade.js`, `host/index.js`, `host/browser.js`):**
  - Answer `host-req::trace-clock` synchronously in `driver.js` `serve()` **before** `B.hooks?.onHostCall?.(req)`
    (`monotonicNs = BigInt(Math.round(now() * 1e6))`, with optional `B.hooks?.traceClockNs?.()` override for
    deterministic S11 tests) so `trace: true` never emits internal clock plumbing to capability hooks (S2) and
    `execSync` works when `trace` and/or `coverage` are enabled!
  - Extend `drain(x)` in `driver.js` to drain `takeCoverage` (mapping `u16` IDs via an instance-cached
    `coverageFeatures()` array) and `takeTrace` (lifting `details` into null-prototype objects per S10), running
    on every `out-of-fuel`, before `serve()` on every `host-call`, on `cancel()`, and in `finish()` before
    `exec` / `execSync` returns.
  - Pass `{ browser: true }` from `host/browser.js` (and `{ browser: false }` from `host/index.js`) into
    `createFacade`, filtering kernel-emitted `"cmd:flag:"` hits in `drain(x)` on the browser entry only (EVOL
    Annex D.7, Annex J R2.3).
  - Fix uniform `logger` ordering in `host/core/facade.js` across `exec`, `execSync`, and `#runChild` (passing
    `B.logger` context into `#runChild`, where logging is currently absent): emit `logger.info("exec", { command })`
    **before** running `execLoop`, then `debug("stdout", { output })` (if non-empty), `info("stderr", { output })`
    (if non-empty), and `info("exit", { exitCode })` **after** `execLoop` finishes (excluding internal `o.bypass`
    `ctx.origCommand` delegations and skipping all four calls when `command.trim() === ""`, EVOL Annex D.1).
- **Test commands and parity wiring (`kernel/src/fiber/testcmd.rs`, `harness/parity/sentropic-adapter.mjs`):**
  - Add `__cov` and `__trace` in `testcmd.rs` (`cfg(feature = "test-commands")`) to exercise multi-slice buffer
    backpressure (`> 10 000` hits, `> 1 000` trace events), `trace-clock` duration computation, and `find`
    `summary`/`searchPath` event formatting before L9a-2 lands.
  - Wire `init.capture.trace` and `init.capture.coverage` in `harness/parity/sentropic-adapter.mjs`.

Out of scope:
- Emitting `bash:expansion:*` hits for L4's new parameter/brace/tilde operators (L4 wires those at L4 exit on
  top of L1d), `bash:cmd:*` for L5's remaining control-flow/builtins, `find` traversal logic (L9a-2), and
  `sed`/`awk`/`jq` node hits (L13, L14a, L15a-1).

## 3. Allowed paths

```
kernel/src/shell.rs
kernel/src/fiber.rs
kernel/src/fiber/vm.rs         (route built `hit` / builtin coverage push through bounded `Fiber::hit_coverage_id`)
kernel/src/cmd/mod.rs
kernel/src/glue/adapt.rs
kernel/src/glue/rt.rs          (widen `LOWERING` to `TRACE_BUF_CAP + 1` = `257` slots for per-event `take_trace` lowering)
kernel/src/fiber/testcmd.rs    (update `T_CLOCK` match; add `__cov` and `__trace` under test-commands)
host/core/driver.js
host/core/facade.js
host/index.js
host/browser.js
harness/parity/sentropic-adapter.mjs
test/parity/l1d.test.mjs
test/l1d-*.test.mjs
BRANCH.md
```

Read-only for this lot: `wit/**`, `types/**`, `kernel/src/lib.rs`, `kernel/src/mem/**`, `kernel/src/limits.rs`,
`kernel/src/fs/**`, `kernel/build.rs`, `tools/**`, `harness/parity/{record,replay,run,compare,validate}.mjs`,
`test/cases/l1d/**`, and `test/parity/exclusions.json`. No `BR<n>-EX<k>` scope exception is needed.

### 3.1 W5 merge order with L8b and L4

L1d runs in W5 Slot B immediately **after** L8b merges to `main`, so `kernel/src/fiber/testcmd.rs` already
contains L8b's test commands when L1d branches. In `kernel/src/fiber/vm.rs`, keep the edit surgical (routing the
existing `self.hit(...)` and builtin-ID `self.coverage.try_push(...)` sites through `self.hit_coverage_id(...)`)
so L4 (Slot A) merges L1d cleanly before L4's final PR.

## 4. Exact interfaces

### 4.1 Kernel types and `Fiber` / `CmdCtx` methods (`kernel::shell`, `kernel::fiber`, `kernel::cmd`)

In `kernel/src/shell.rs`, add `pub trace: bool` to both `Config<'a>` and `Shell` (copied in `Shell::create`).

In `kernel/src/fiber.rs` (with `TraceValue`, `TraceDetail`, `TraceEvent`, `FindTraceStats` re-exported from
`kernel/src/cmd/mod.rs`):

```rust
pub const COVERAGE_BUF_CAP: usize = 4_096;
pub const TRACE_BUF_CAP: usize = 256;

pub enum HostReq {
    // ... existing variants unchanged ...
    Clock,
    TraceClock,
    Entropy(u32),
}

pub enum Outcome<'a> {
    // ... existing variants unchanged ...
    Clock { wall_ms: i64, monotonic_ns: u64 },
    // ...
}

pub enum TraceValue {
    Number(f64),
    Text(KBytes),
}

pub struct TraceDetail {
    pub key: KString,
    pub value: TraceValue,
}

pub struct TraceEvent {
    pub category: KString,
    pub name: KString,
    pub duration_ms: f64,
    pub details: KVec<TraceDetail>,
}

/// Exact counter & fuel-apportionment payload for `find` (`summary` + `searchPath`, EVOL Annex D.6).
#[derive(Clone, Copy, Debug, Default)]
pub struct FindTraceStats {
    pub readdir_calls: u64,
    pub stat_calls: u64,
    pub eval_calls: u64,
    pub node_count: u64,
    pub batch_count: u64,
    pub early_prunes: u64,
    /// Fuel units spent in each phase; apportioned across `duration_ms` per EVOL Annex D.6:
    /// `phase_ms = if total_fuel > 0 { duration_ms * (phase_fuel as f64 / total_fuel as f64) } else { 0.0 }`.
    pub readdir_fuel: u64,
    pub stat_fuel: u64,
    pub eval_fuel: u64,
    pub batch_fuel: u64,
    pub total_fuel: u64,
    pub results_found: u64,
}

impl Fiber {
    pub fn coverage_enabled(&self) -> bool { self.shell.coverage }
    pub fn trace_enabled(&self) -> bool { self.shell.trace }

    /// True when slice fuel is exhausted or either observability buffer has reached its slice cap.
    pub fn should_yield_observability(&self) -> bool {
        self.coverage.len() >= COVERAGE_BUF_CAP || self.trace.len() >= TRACE_BUF_CAP
    }

    /// Appends `id` when `shell.coverage` is true; whenever `coverage.len() >= COVERAGE_BUF_CAP`
    /// (`4_096`), zeroes **both** `self.fuel` and `self.fuel_credit` so the next `take_fuel()` or
    /// `tick()` checkpoint yields `Step::OutOfFuel` cleanly between VM opcodes for draining.
    pub(crate) fn hit_coverage_id(&mut self, id: u16) -> Result<(), Fail>;

    /// Looks up `coverage_id(feature)` and delegates to `hit_coverage_id`.
    pub(crate) fn hit_coverage(&mut self, feature: &str) -> Result<(), Fail>;

    /// Appends `event` when `shell.trace` is true; whenever `trace.len() >= TRACE_BUF_CAP` (`256`),
    /// zeroes **both** `self.fuel` and `self.fuel_credit` so the next `take_fuel()` or `tick()`
    /// checkpoint yields `Step::OutOfFuel` cleanly for draining.
    pub(crate) fn push_trace(&mut self, event: TraceEvent) -> Result<(), Fail>;

    /// Formats and pushes the two `find` trace events (`summary` then `searchPath`) for `raw_search_path`
    /// using `start_ns` and `end_ns` (`duration_ms = (end_ns.saturating_sub(start_ns) as f64) / 1_000_000.0`).
    pub(crate) fn emit_find_trace(
        &mut self,
        raw_search_path: &[u8],
        start_ns: u64,
        end_ns: u64,
        stats: &FindTraceStats,
    ) -> Result<(), Fail>;
}
```

And on `CmdCtx<'_>` in `kernel/src/cmd/mod.rs`:

```rust
impl CmdCtx<'_> {
    pub fn live_budget(&self) -> &Budget { &self.fiber.live }
    pub fn should_yield(&self) -> bool {
        self.fiber.should_yield_observability() || !self.fiber.tick()
    }
    pub fn coverage_enabled(&self) -> bool { self.fiber.coverage_enabled() }
    pub fn hit_coverage(&mut self, feature: &str) -> Result<(), Fail> {
        self.fiber.hit_coverage(feature)
    }
    pub fn trace_enabled(&self) -> bool { self.fiber.trace_enabled() }
    pub fn trace_clock_req(&self) -> Option<HostReq> {
        self.fiber.trace_enabled().then_some(HostReq::TraceClock)
    }
    pub fn emit_trace(&mut self, event: TraceEvent) -> Result<(), Fail> {
        self.fiber.push_trace(event)
    }
    pub fn emit_find_trace(
        &mut self,
        raw_search_path: &[u8],
        start_ns: u64,
        end_ns: u64,
        stats: &FindTraceStats,
    ) -> Result<(), Fail> {
        self.fiber.emit_find_trace(raw_search_path, start_ns, end_ns, stats)
    }
}
```

### 4.2 Facade loader signature (`host/core/facade.js`, `host/index.js`, `host/browser.js`)

```js
// host/core/facade.js
export function createFacade(loader, { browser = false } = {}) { ... }

// host/index.js
const { Bash, InMemoryFs } = createFacade(nodeLoader, { browser: false });

// host/browser.js
const { Bash, InMemoryFs } = createFacade(loader, { browser: true });
```

`types/core.d.ts` already defines `BashLogger`, `TraceEvent`, `BashOptions.{logger,trace,coverage}`, and
`CommandContext.{trace,coverage}`; no `.d.ts` change is needed (`tsc --noEmit` must stay green).

## 5. Complete behavioural specification

### 5.1 `logger` contract and nested `ctx.exec` ordering (EVOL Annex D.1, verified on `just-bash@3.6.0`)

1. **Blank-script and `ctx.origCommand` suppression:**
   - Before starting an execution (`bash.exec`, `bash.execSync`, or nested `#runChild` in `host/core/facade.js`),
     compute `const shouldLog = Boolean(B.logger) && !o.bypass && commandLine.trim() !== ""` (using standard JS
     `String.prototype.trim()`).
   - When `commandLine.trim() === ""` (empty `""` or whitespace-only `"   \n\t\u00a0"`), or when `o.bypass` is
     set (`ctx.origCommand`, which delegates directly to the shadowed built-in without running a shell `exec` in
     `just-bash@3.6.0`), **zero** calls are made to `logger.info` or `logger.debug`.
   - A comment-only script such as `"# comment\n"` has `commandLine.trim() !== ""` and **does** log `"exec"`
     and `"exit"` (`{ exitCode: 0 }`).
2. **Uniform pre/post call sequence across `exec`, `execSync`, and `#runChild` (`host/core/facade.js`):**
   - Split `#log` in `host/core/facade.js` into `#logStart(B, commandLine, o)` and `#logEnd(B, commandLine, o, result)`
     (or wrap `execLoop` uniformly in `exec`, `execSync`, and `#runChild`, passing `B` with `B.logger` into
     `#runChild`, which currently omits logging on `main`):
     1. **Before** running `execLoop`: if `shouldLog`, call `B.logger.info("exec", { command: commandLine })`
        (with the raw un-normalized `commandLine` string as passed to `exec` / `execSync` / `ctx.exec`).
     2. Run `execLoop` (`await runAsync(...)` in `exec` and `#runChild`; `runSync(...)` in `execSync`) to obtain
        `result` (`{ stdout, stderr, exitCode, ... }`, with `stdout` and `stderr` decoded to JS strings as
        returned to the caller).
     3. If `shouldLog && result.stdout !== ""` (non-empty string): `B.logger.debug("stdout", { output: result.stdout })`.
     4. If `shouldLog && result.stderr !== ""` (non-empty string): `B.logger.info("stderr", { output: result.stderr })`.
     5. If `shouldLog`: `B.logger.info("exit", { exitCode: result.exitCode })`.
3. **Interleaving with custom commands, `ctx.exec`, and limit results:**
   - Because step 1 runs **before** `execLoop` and steps 3-5 run **after** `execLoop` finishes, a custom command
     invoked during the script observes the parent's `info("exec", ...)` already logged; if the custom command
     calls `await ctx.exec(subCmd)` (or `ctx.execWithInheritedStdin`), `#runChild` logs the child's full
     `info("exec")` -> `debug("stdout")` -> `info("stderr")` -> `info("exit")` sequence **before** `ctx.exec`
     resolves back to the custom command, and the parent logs its `stdout`/`stderr`/`exit` after the outer
     `execLoop` finishes (matching `just-bash@3.6.0`).
   - Per EVOL D1, D14, Annex D.1, and Annex D.2, this exact sequence also holds on syntax errors (`exitCode: 2`),
     unknown commands (`exitCode: 127`), execution limits including `maxSourceBytes` and `maxCommandCount`
     (`exitCode: 126`, returning a result rather than throwing), and abort/deadline (`exitCode: 124`).

### 5.2 Bounded `coverage` and `trace` buffers, drain order, and ABI lowering (EVOL Annex A)

1. **Collection gating:**
   - When `Shell.coverage == false`, `hit_coverage_id` is a `O(1)` no-op (allocates 0 bytes) and `takeCoverage`
     always returns `[]`.
   - When `Shell.trace == false`, `push_trace` and `emit_find_trace` are `O(1)` no-ops, `trace_clock_req()`
     returns `None` (zero `trace-clock` host calls are ever emitted!), and `takeTrace` always returns `[]`.
2. **Bounded per-fiber buffers and `OutOfFuel` backpressure between VM opcodes:**
   - Both `Fiber.coverage` (`KVec<u16>`) and `Fiber.trace` (`KVec<TraceEvent>`) allocate from `&self.live` (so
     an adversarial custom/kernel trace flood cannot breach `maxLiveBytes`).
   - Route every coverage push in `kernel/src/fiber/vm.rs` (both `self.hit(...)` and the builtin-ID
     `self.coverage.try_push(id as u16)` in `Fiber::dispatch`) through `self.hit_coverage_id(id)`.
   - Whenever `hit_coverage_id` pushes an ID and `self.coverage.len() >= COVERAGE_BUF_CAP` (`4_096`), or
     `push_trace` pushes an event and `self.trace.len() >= TRACE_BUF_CAP` (`256`), immediately set **both**
     `self.fuel.set(0)` **and** `self.fuel_credit.set(0)`. Because `Fiber::tick()` decrements `fuel_credit` and
     falls back to `Fiber::take_fuel()` when `fuel_credit == 0` (which otherwise grants `16` credits per fuel
     unit), zeroing **both** `fuel` and `fuel_credit` guarantees that the very next `take_fuel()` or `tick()`
     checkpoint in `vm.rs` / `expand.rs` / `CmdCtx::should_yield()` returns `false`, parking the active VM or
     command frame cleanly between VM opcodes and yielding `Step::OutOfFuel` without dropping any hit or event!
   - Additionally, at the top of `Fiber::advance`, if `self.coverage.len() >= COVERAGE_BUF_CAP * 2` or
     `self.trace.len() >= TRACE_BUF_CAP * 2` (for example if a raw WIT caller invokes `fiber.run(fuel)`
     repeatedly without draining), return `Step::OutOfFuel` immediately without advancing the VM until the
     caller drains!
   - `Fiber::finish` and `Fiber::cancel` **must not** clear `self.coverage` or `self.trace`: all buffered items
     remain readable via `take_coverage` and `take_trace` after `Step::Done` and after `cancel` until consumed
     or until the `Fiber` resource is disposed.
3. **`GuestFiber::take_trace(max: u32)` in `kernel/src/glue/adapt.rs` (and `kernel/src/glue/rt.rs`):**
   - In `kernel/src/glue/rt.rs`, widen `LOWERING` (and the local `blocks` array in `prepare_lowering`) from `3`
     to `TRACE_BUF_CAP + 1` (`257` slots: `[const { None }; 257]`).
   - Clamp `limit = (max as usize).min(TRACE_BUF_CAP)`. Take up to `limit` front events from `Fiber`.
   - Convert each `TraceEvent` to `T::TraceEvent { category: copy_text(&e.category)?, name: copy_text(&e.name)?, duration_ms: e.duration_ms, details }` where each `TraceDetail` maps `TraceValue::Number(n)` -> `T::TraceValue::Number(n)` and `TraceValue::Text(b)` -> `T::TraceValue::Text(copy(&b)?)`.
   - **Per-event `rt::prepare_lowering` accounting:** In `kernel/src/bindings/kernel.rs`
     (`_export_method_fiber_take_trace_cabi`), `wit-bindgen` allocates the outer `Vec<TraceEvent>` via one
     `Cleanup::new` call of `(events.len(), 32, 8)` and then allocates **each** `TraceEvent.details` list via
     its own exact-layout `Cleanup::new` lookup of `(ev.details.len(), 24, 8)` (`size == 0` bypasses `LOWERING`).
     Therefore, build a stack slice `specs` containing `(events.len(), 32, 8)` (when `!events.is_empty()`)
     **plus one `(ev.details.len(), 24, 8)` tuple per event with `!ev.details.is_empty()`**, and pass
     `&specs[..spec_count]` to a single `rt::prepare_lowering` call (**never** coalesce into `(total_details, 24, 8)`!).
     If allocation or `prepare_lowering` fails at the full `limit`, retry with `limit = 1` inside
     `mem::reserve_scope` (mirroring `take_output` in `adapt.rs`), and call `f.consume_trace(out.len())` only
     for the events actually lowered!
4. **Driver drain order (`host/core/driver.js`):**
   - Cache `inst.coverageFeatures ??= inst.call(() => inst.engine.coverageFeatures())` on the `Instance` (or a
     `WeakMap<Instance, string[]>`).
   - Extend `drain(x)` (after `takeStdout` and `takeStderr`):
     - If `x.B.coverage`: loop calling `x.fiber.takeCoverage(4096)` until empty; for each `id`, look up
       `const feature = features[id]`; if `feature !== undefined && !(x.B.browser && feature.startsWith("cmd:flag:"))`,
       invoke `x.B.coverage.hit(feature)`.
     - If `x.B.trace`: loop calling `x.fiber.takeTrace(256)` until empty; for each `ev`, build
       `const out = { category: ev.category, name: ev.name, durationMs: ev.durationMs }`; if
       `ev.details.length > 0`, set `out.details = nullObject()` and populate each `d.key` with
       `d.value.tag === "number" ? d.value.val : decodeUtf8(d.value.val)`; invoke `x.B.trace(out)`.
   - Because `execLoop` already calls `drain(x)` before `cancelReason`, before `yield { tick: true }`, **before**
     `serve(x, step.val)` on every `host-call`, and at the start of `finish(x, er)` on both `done` and `cancel`:
     1. Every parent hit/event emitted before a `custom-command` call is delivered **before** the custom
        command's `execute()` runs (so custom `ctx.coverage.hit(...)`, `ctx.trace(...)`, or child `ctx.exec(...)`
        hits never precede buffered parent hits!).
     2. Every hit/event emitted before an abort, deadline, or limit cancellation is delivered before `exec`
        resolves.
     3. Every hit/event of the exec is delivered in exact emission order before `exec` / `execSync` returns.

### 5.3 `trace-clock`, S11 non-observability, and `find` trace schema (EVOL Annex D.6, S11)

1. **`trace-clock` execution, S2 hook isolation, and S11 guarantee:**
   - In `host/core/driver.js` `serve(x, pc)`, dispatch `req.tag === "trace-clock"` **before**
     `B.hooks?.onHostCall?.(req)`:
     - Read `const ns = B.hooks?.traceClockNs ? BigInt(B.hooks.traceClockNs()) : BigInt(Math.round(now() * 1e6));`
     - Return `{ outcome: { tag: "clock", val: { wallMs: 0n, monotonicNs: ns } } }` immediately (no `yield`, no
       grant check, and **never** invoking `B.hooks?.onHostCall?.(req)`).
   - Handling `trace-clock` prior to `onHostCall` ensures that enabling `trace: true` on an ungranted shell
     never leaks internal observability plumbing to capability-recording hooks or fails invariant S2, while
     `HostReq::Clock` remains denied inside the kernel when `real-clock` is not granted!
   - Inside the kernel, `monotonic_ns` from a `TraceClock` outcome is stored **only** in the command/test trace
     frame to compute `TraceEvent.duration_ms` and phase `*TimeMs` values. It is **never** written to
     `Shell::virt_ms`, `$SECONDS`, `$EPOCHSECONDS`, `$EPOCHREALTIME`, `printf %(...)T`, `time`, stdout, stderr,
     exit code, environment variables, or VFS mtimes (S11).
   - When `bash.execSync` is called on a shell with `trace`, `coverage`, or `logger` configured (and no async
     capability `fs`/`fetch`/`sleep`/`customCommands`/`network`), `execSync` **succeeds** without throwing
     `AsyncCapabilityError`!
2. **`find` trace event format (`emit_find_trace`, EVOL Annex D.6, verified on `just-bash@3.6.0`):**
   - Path normalization for `details.path`: if `raw_search_path` is empty, use `b"."`; if `raw_search_path.len() > 1`
     and ends with `b'/'`, strip **one** trailing `b'/'` (`"/dir/"` -> `"/dir"`, `"/"` -> `"/"`, `"/dir//"` ->
     `"/dir/"`).
   - Compute `duration_ms = (end_ns.saturating_sub(start_ns) as f64) / 1_000_000.0`.
   - Apportion phase times from fuel counts (`apportion(f) = if stats.total_fuel > 0 { duration_ms * (f as f64) / (stats.total_fuel as f64) } else { 0.0 }`):
     `readdir_ms`, `stat_ms`, `eval_ms`, `batch_ms`, and
     `other_ms = duration_ms - (readdir_ms + stat_ms + eval_ms + batch_ms)` (may be negative when `batch_fuel`
     overlaps the other three phases, matching EVOL Annex D.6!).
   - Emit two `TraceEvent` records in exact order:
     1. `category: "find"`, `name: "summary"`, `duration_ms`, with **11** `details` entries in order:
        `readdirCalls` (`Number`), `readdirTimeMs` (`Number`), `statCalls` (`Number`), `statTimeMs` (`Number`),
        `evalCalls` (`Number`), `evalTimeMs` (`Number`), `nodeCount` (`Number`), `batchCount` (`Number`),
        `batchTimeMs` (`Number`), `earlyPrunes` (`Number`), `otherTimeMs` (`Number`).
     2. `category: "find"`, `name: "searchPath"`, `duration_ms`, with **2** `details` entries in order:
        `path` (`Text(normalized_path)`), `resultsFound` (`Number`).

### 5.4 Browser `cmd:flag:*` filter (EVOL Annex D.7, Annex J R2.3)

- Verified against `just-bash@3.6.0` (`dist/bundle/index.js` vs `dist/bundle/browser.js`):
  - On `echo -n hi`, the Node entry emits `["bash:cmd:SimpleCommand", "bash:expansion:word_glob", "bash:expansion:word_glob", "bash:builtin:echo", "cmd:flag:echo:-n"]`.
  - On `echo -n hi`, the `./browser` entry (`host/browser.js`) emits `["bash:cmd:SimpleCommand", "bash:expansion:word_glob", "bash:expansion:word_glob", "bash:builtin:echo"]` — every `bash:cmd:*`, `bash:expansion:*`, `bash:builtin:*`, `sed:cmd:*`, `awk:*`, and `jq:node:*` hit is preserved, and only kernel hits starting with `"cmd:flag:"` are omitted!
  - Custom commands calling `ctx.coverage.hit(...)` invoke `B.coverage.hit` directly in `serveCustom` and are
    **not** filtered on either entry.

## 6. Test commands, parity adapter wiring, and Room-1 oracle cases

### 6.1 `kernel-test` commands (`kernel/src/fiber/testcmd.rs`)

Extend `testcmd.rs` (after L8b's commands) with two resumable commands under `cfg(feature = "test-commands")`:

1. **`__cov <count> [feature]`**
   - Default `feature` is `"bash:cmd:SimpleCommand"`; if `coverage_id(feature)` is `None`, fail with `t_bad`.
   - Emits `<count>` hits of that feature ID across as many `OutOfFuel` slices as needed: in each slice, push
     while ` emitted < count && !self.should_yield_observability() `, and if `emitted < count`, park a resumable
     `CmdFrame` returning `CmdPoll::Yield` (`Step::OutOfFuel`) so the driver drains `COVERAGE_BUF_CAP` (`4_096`)
     hits and resumes until all `<count>` hits are emitted in order with `exit 0`.
2. **`__trace <op>`**
   - **`__trace flood <count>`:** emits `<count>` events `{ category: "test", name: "item", duration_ms: 0.0, details: [("seq", Number(i as f64))] }` (`0 <= i < count`), yielding `CmdPoll::Yield` whenever `should_yield_observability()` is true so `> 1_000` events stream across multiple `TRACE_BUF_CAP` (`256`) drains without loss.
   - **`__trace span <label>`:** if `trace_enabled()`, issues `HostReq::TraceClock` (`start_ns`), prints
     `<label>\n` to stdout, issues a second `HostReq::TraceClock` (`end_ns`), emits `{ category: "test", name: "span", duration_ms: (end_ns - start_ns) as f64 / 1e6, details: [("label", Text(label))] }`, and exits `0`.
     When `!trace_enabled()`, prints `<label>\n` and exits `0` with **zero** `HostReq::TraceClock` calls!
   - **`__trace find <search_path> <results_found> <readdir_calls> <stat_calls> <eval_calls> <node_count> <batch_count> <early_prunes>`:**
     takes start/end `TraceClock` readings (when `trace_enabled()`), populates `FindTraceStats` (with synthetic
     non-zero phase fuels `10, 5, 5, 20`, `total_fuel = 40`), calls `emit_find_trace`, and exits `0`.

### 6.2 Parity adapter wiring (`harness/parity/sentropic-adapter.mjs`)

Update `harness/parity/sentropic-adapter.mjs`:
- Maintain `let trace = [];` and `let coverage = [];` inside `create(init)`.
- If `init.capture?.trace` is truthy, set `options.trace = (ev) => trace.push(ev)`.
- If `init.capture?.coverage` is truthy, set `options.coverage = { hit: (feature) => coverage.push(feature) }`.
- Implement `takeTrace() { const out = trace; trace = []; return out; }` and
  `takeCoverage() { const out = coverage; coverage = []; return out; }`.
- Because `harness/parity/run.mjs` drains `session.takeTrace()` (normalizing via `filterTraceEvent`) and
  `session.takeCoverage()` after each step and compares `result.trace` and `result.coverage` against the
  snapshot, `test/parity/l1d.test.mjs` uses `sentropic-adapter.mjs` directly on production `@sentropic/bash`!

### 6.3 Room-1 oracle cases (`test/cases/l1d/*.case.json`, 16 cases)

Every case below uses grammar and commands already built on `main` (`echo`, flags, assignments, pipelines,
groups, functions, word splitting, comments, blank scripts) so `test/parity/l1d.test.mjs` runs directly against
production `sentropic-adapter.mjs` with **0 exclusions and 0 `notCovered`**. Room 1 records all 16 against
`just-bash@3.6.0`:

| # | Case ID (`l1d/...`) | `capture` | Script / step(s) |
|---:|---|---|---|
| 1 | `cov-empty-and-whitespace` | `{ "coverage": true, "trace": true }` | Steps: `""`, `"   \n\t "` (`coverage: []`, `trace: []`) |
| 2 | `cov-comment-only` | `{ "coverage": true, "trace": true }` | `"# comment only\n"` (`coverage: []`, `trace: []`) |
| 3 | `cov-echo-plain` | `{ "coverage": true }` | `"echo hello world"` (`SimpleCommand`, `word_glob` x2, `builtin:echo`) |
| 4 | `cov-echo-flags-order` | `{ "coverage": true }` | Steps: `"echo -n -e 'a\\nb'"` and `"echo hi -n -E"` (argument-order `cmd:flag:echo:*` hits) |
| 5 | `cov-echo-repeated-flags` | `{ "coverage": true }` | `"echo -n -n -e hi"` (records duplicate flag occurrences in argument order) |
| 6 | `cov-pipeline-stages` | `{ "coverage": true }` | `"echo left \| { echo right; }"` (`SimpleCommand`, `Group`, stage order) |
| 7 | `cov-group-and-andor` | `{ "coverage": true }` | `"{ echo a && echo b; } \|\| echo c"` (short-circuit skips `echo c` hits!) |
| 8 | `cov-function-def-and-call` | `{ "coverage": true }` | `"greet() { echo -n \"$1\"; }\ngreet world"` (`FunctionDef`, function call without builtin hit on `greet`, inner `builtin:echo` + `cmd:flag:echo:-n`) |
| 9 | `cov-builtin-shadowed-by-fn` | `{ "coverage": true }` | `"echo() { /bin/echo custom; }\necho hi"` (EVOL Annex D.7: `bash:builtin:echo` is hit **before** function lookup when `echo` is called!) |
| 10 | `cov-word-split-vs-quoted` | `{ "coverage": true }` | `"S='one two'\necho $S \"$S\""` (`word_split` hit on unquoted `$S`, not on `"$S"`) |
| 11 | `cov-assignment-only` | `{ "coverage": true }` | `"A=1 B=2\necho $A $B"` (`SimpleCommand` hit on assignment-only command) |
| 12 | `cov-unknown-command` | `{ "coverage": true }` | `"nosuchcmd arg1"` (`SimpleCommand`, `word_glob`, `exit 127`) |
| 13 | `cov-syntax-error` | `{ "coverage": true, "trace": true }` | `"'"` (`exit 2`, `coverage: []`, `trace: []`) |
| 14 | `cov-multistep-accumulation` | `{ "coverage": true, "trace": true }` | 3 steps: `"echo a"`, `"echo -n b"`, `"echo -e 'c\\nd'"` (verifies per-step `takeCoverage` drain without cross-step duplication) |
| 15 | `cov-command-count-limit` | `{ "coverage": true }` | `engine: { executionLimits: { maxCommandCount: 2 } }`, `"echo 1; echo 2; echo 3"` (`exit 126`, hits from first 2 commands preserved!) |
| 16 | `trace-enabled-no-find` | `{ "trace": true, "coverage": true }` | `"X=ok; echo -n $X"` (`trace: []`, `coverage` intact when both are active) |

## 7. Executable exit gates

All gates run in `make check` (`cargo test` + `node tools/run-tests.mjs`):

1. **Repository, zero-import, browser graph, and type gates.** G1-G8 pass (`kernel.core.wasm` has zero imports;
   G2 confirms `host/browser.js` reaches zero `node:` imports; L2b differential check covers `takeTrace`,
   `takeCoverage`, `coverageFeatures`, and `trace-clock`; `tsc --noEmit` passes).
2. **Parity (`test/parity/l1d.test.mjs`).** All 16 committed `test/cases/l1d/*.case.json` cases replay via
   `sentropic-adapter.mjs` with **0 failures, 0 stale hashes, 0 exclusions, and 0 `notCovered`**.
3. **`logger` call-sequence gate (`test/l1d-logger.test.mjs`).** Verify on both `bash.exec` and `bash.execSync`:
   - `""` and `"   \n\t\r "` produce **0** `logger` calls;
   - `"# comment\n"` produces `[["info", "exec", { command: "# comment\n" }], ["info", "exit", { exitCode: 0 }]]`;
   - stdout-only, stderr-only, both stdout+stderr, syntax error (`exit 2`), `maxSourceBytes` / `maxCommandCount`
     limit (`exit 126`), and `AbortSignal` (`exit 124`) produce the exact Annex D.1 sequence;
   - Custom command calling `await ctx.exec("echo sub")`: parent `info("exec")` fires **before** custom command
     body runs, child `info("exec")` -> `debug("stdout")` -> `info("exit")` fires inside `ctx.exec`, and parent
     `debug("stdout")` / `info("stderr")` / `info("exit")` fires after the custom command returns.
4. **Bounded buffer multi-slice drain and ordering gate (`test/l1d-buffers.test.mjs`).**
   - On `kernel-test`: run `__cov 12000 bash:builtin:echo` (`> 2.9x COVERAGE_BUF_CAP`) and
     `__trace flood 1000` (`> 3.9x TRACE_BUF_CAP`) in one script: **12 000 + 1** coverage hits (`SimpleCommand`
     + `word_glob`s + `12 000` `echo` hits) and **1 000** trace events (`seq == 0..999` in strict ascending
     order) are delivered before `exec` resolves, with **0 drops** and under a tight `maxLiveBytes` budget.
   - **Drain-before-host-callback gate:** run `echo -n before; mycmd; echo -e after` where `mycmd` records hits
     and calls `ctx.coverage.hit("custom:mid")`, `ctx.trace(...)`, and `await ctx.exec("echo child")`: prove
     every `before` hit (`SimpleCommand`, `word_glob`, `builtin:echo`, `cmd:flag:echo:-n`) is delivered **before**
     `mycmd` starts, `"custom:mid"` and `"child"` hits arrive in the middle, and `after` hits arrive last.
   - **Drain-on-cancel gate:** run `echo -n hi; slow_sleep` aborted mid-sleep via `AbortController`: prove `exitCode === 124` and all pre-sleep coverage hits (`cmd:flag:echo:-n`, etc.) were delivered before `exec` resolved.
   - **`find` trace contract check:** run `__trace find /dir/ 4 2 1 3 5 1 0` through `filterTraceEvent` from
     `harness/parity/run.mjs` and assert exact `summary` (11 keys, `*TimeMs` finite numbers masked to `null`,
     `otherTimeMs` finite) + `searchPath` (`{ path: "/dir", resultsFound: 4 }`) structure, plus `/` preserved as
     `"/"` and `""` normalized to `"."`, and prototypes of `ev.details` equal `null` (S10).
5. **Browser `cmd:flag:*` filter gate (`test/l1d-browser.test.mjs`).**
   - Import `Bash as BrowserBash` from `../host/browser.js` (after `await BrowserBash.precompile(wasmBytes)`):
     run `echo -n -e hi` with `coverage` enabled and assert `["bash:cmd:SimpleCommand", "bash:expansion:word_glob", "bash:expansion:word_glob", "bash:expansion:word_glob", "bash:builtin:echo"]` (zero `cmd:flag:*` hits, all `bash:*` hits intact), while `Bash` from `../host/index.js` on the identical script includes `"cmd:flag:echo:-n"` and `"cmd:flag:echo:-e"`.
6. **S2/S11 `trace-clock` non-observability, `execSync`, and `exec-oneshot` rejection gate (`test/l1d-security.test.mjs`).**
   - **S2 + S11:** Run the same multi-command script (`__clock; __trace span alpha; echo $PWD`) twice with `trace`
     enabled, `real-clock` **not** granted, and an `onHostCall` recording hook attached via `[INTERNAL]`,
     injecting two distinct `traceClockNs` sequences (`0n, 2_000_000n` -> `durationMs == 2` vs
     `100_000_000_000n, 100_055_500_000n` -> `durationMs == 55.5`): assert `durationMs` equals `2` and `55.5`
     respectively while `stdout` (`"0\nalpha\n/home/user\n"`), `stderr`, `exitCode`, and `env` are
     **byte-for-byte identical**, and `onHostCall` records **0** capability host calls!
   - **`execSync`:** After `await Bash.precompile()`, run `bash.execSync("echo -n sync", { ... })` on a `Bash`
     configured with `logger`, `trace`, and `coverage` — succeeds synchronously and delivers all three streams.
   - **`exec-oneshot`:** Call `engine.execOneshot` directly with `config.trace = true` (`coverage = false`) and
     with `config.coverage = true` (`trace = false`): both fail with `code === "invalid-config"`.

## 8. Points the builder must confirm and report

Record in `BRANCH.md` and the PR description:

1. **Buffer caps and ABI lowering footprint:** confirm `COVERAGE_BUF_CAP` (`4_096`), `TRACE_BUF_CAP` (`256`),
   `LOWERING` `257`-slot capacity in `glue/rt.rs` + per-event `rt::prepare_lowering` sizes in `take_trace`, and
   raw + gzip deltas on `dist/js/kernel/kernel.core.wasm` and packed JS host bundles (checking EVOL Annex H JS
   host caps).
2. **Drain ordering proof:** summarize test evidence for all four drain trigger points (`out-of-fuel`,
   pre-`host-call`, `cancel`, and `done`) and nested `ctx.exec` logger + coverage interleaving.
3. **Downstream `CmdCtx` readiness:** confirm `CmdCtx::{hit_coverage, trace_enabled, trace_clock_req, emit_trace, emit_find_trace, should_yield}` signatures for L4 (exit), L9a-2 (`find`), L13 (`sed`), L14a (`awk`), and
   L15a-1 (`jq`).
