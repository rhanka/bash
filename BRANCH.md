# Feature: L2 vertical slice and Gate L2

## Objective

Replace the bootstrap with the L2 shell, command registry, and `echo`. Replay the committed L2 oracle cases and measure Gate L2.

## Scope / Guardrails

- Worktree `tmp/l2-vertical-slice`, branch `feat/l2-vertical-slice`.
- Clean-room inputs are the specs and committed snapshots. Build and verify offline with the pinned tools.
- Zero Python and third-party dependencies. All new text is in English.
- Git metadata is read-only in this lane. The conductor receives the ordered commit plan and lane report.

## Branch Scope Boundaries (MANDATORY)

- **Allowed Paths (implementation scope)**:
  - `BRANCH.md`
  - `kernel/Cargo.toml`
  - `kernel/build.rs`
  - `kernel/src/**`
  - `host/**`
  - `types/**`
  - `tools/components.json`
  - `tools/build.mjs`
  - `tools/gates.mjs` (reproducibility hashes for relocated probe JavaScript)
  - `tools/transpile.mjs`
  - `tools/measure-l2.mjs`
  - `harness/parity/sentropic-adapter.mjs`
  - `test/**`
  - `plan/reports/GATE_L2.md`
  - `spec/lots/L2.md` (conductor-authorized snapshot path correction)
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `LICENSE`
  - `.github/workflows/**`
  - `.githooks/**`
  - `kernel/src/mem/**`
  - `wit/**`
  - `test/cases/l2/**`
  - `test/snapshots/l2/**`
  - `test/parity/exclusions.json`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - None.
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in Feedback Loop before touching conditional or forbidden paths, with reason, impact, and rollback.

## Feedback Loop

- L2 requires no scope exception. The conductor authorized the L2 spec snapshot path correction.

## Plan / Todo (lot-based)

- [x] Confirm worktree and scope boundaries.
- [x] Implement parser, VM, registry, layout, and allocator wiring.
- [x] Retire the public probe and verify the packed consumer.
- [x] Pass the 30-case replay and the four-profile suite in `make check`.
- [x] Check scope and record Gate L2 measurements and the lane handoff.

## Handover

- L2 merges before L7b. L7b owns the rebase and uses COMMAND_NAMES for host-backed command stubs.
- The include! registry passes G3 on Rust 1.96. G3 includes the relocated test-only JS.
- Hardened ceiling exhaustion returned exit 126 with stderr and a usable instance.
- The shipping minified build passed G1–G8 and the full test suite.
- Raw / gzip JS group sizes, unminified then shipping minified
  - Generated glue 565,157 / 63,043 B and 273,336 / 45,167 B.
  - Registry 54 / 74 B and 54 / 74 B.
  - Other browser-reachable entry code 69,100 / 22,669 B and 69,100 / 22,669 B.
  - Node-only 1,790 / 853 B and 1,790 / 853 B.
  - All shipped JS (M6-js) 636,101 / 86,639 B and 344,280 / 68,763 B.
- Binaryen 132.0.0 is unpinned for core optimization. The driver reports wasm-opt as not measured. Optimization flags and gain are pending. wasmOpt stays disabled.
- Each warm workload runs in a separate process and collects garbage once after warmup. M6-js counts each candidate's packed JavaScript files.
- Upstream-defect candidates are VFS quota exceptions, lone-surrogate echo escapes, and mixed valid and invalid UTF-8 output.
- Coverage snapshots use defenseInDepth: false because the reference can block the first lazy command import.
- The parser token cap requires a function call tree to reach the contain and normal command caps. The deadline suite advances the driver clock while retaining the profile values.
- No-go (kernel miss). L2b owns the first-party glue. See plan/reports/GATE_L2.md for the current measurements.
