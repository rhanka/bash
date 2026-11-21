# Gate L2

**No-go (kernel miss).** The shipping build exceeds the M1, M2, M3a, and M5 kernel budgets. Shipped JavaScript also exceeds the 60,000 B target. L2b owns the first-party glue. The build-lot pause under PLAN section 5 remains in effect.

## Revision and protocol

| Field | Measured value |
|---|---|
| OS kernel | Linux 7.0.0-43-generic |
| CPU | AMD RYZEN AI MAX+ 395 w/ Radeon 8060S |
| Node | v22.22.1 |
| Rust | rustc 1.96.0 (ac68faa20 2026-05-25) |
| wasm-tools | wasm-tools 1.263.0 (7a2fa422c 2026-10-30) |
| Binaryen | 132.0.0 in the transpiler tool tree, unpinned for core optimization |
| Git | `b2563ca2783d6ffb42ae08f5ea2d9dcd211dba81` plus the PR #11 remediation changes, `dirty = true` |
| Loader Wasm SHA-256 | `42a505a374300bae1cf1e3ed5a0388166ceb76a8230bb0670d759e20d676234a` |
| Child environment | `LC_ALL=C`, `TZ=UTC`, worktree-local `TMPDIR` and npm cache, offline npm |

Each candidate passes an `echo hi` assertion before sampling. Each of three repetitions uses 30 seeded rounds. Each round shuffles fresh baseline, import-only, and import-plus-construction-plus-first-exec processes. The seeds are `0x6c320001`, `0x6c320002`, and `0x6c320003`. Cold timings exclude Node process startup. M5 subtracts the median baseline RSS from the median first-exec RSS in each repetition.

M2, M3a, M3aSync, and M_host each use their own process per repetition. Each process discards 20 warmup iterations and measures at least 1,000 iterations over at least 2,000 ms. Async and sync echo each reuse one shell. The host-call process checks the noop result before warmup. Timings use `performance.now()`, an interpolated median, and nearest-rank p95. Each reported statistic is the median of the three repetition values.

The prior Option D timing series shared a warm process across workloads. The values below come from the corrected process layout. Measurements use the worktree containing the remediation because this lane has read-only Git metadata.

## Timing and memory

Timing cells show median / p95 / minimum in milliseconds. The reference values come from the L2 specification.

| Metric | just-bash 3.6.0 | Target | Unminified | Shipping minified | Shipping result |
|---|---:|---:|---:|---:|---|
| M1 cold total | 118.8 | 25.0 (stretch 15.0) | 55.669383 / 63.372313 / 50.742769 | 51.894006 / 57.738879 / 47.960591 | Miss |
| M1 import (cold-full) | — | — | 33.278524 / 39.770234 / 30.132963 | 30.431610 / 35.849955 / 28.033270 | — |
| M1 construction | — | — | 11.621100 / 14.401931 / 10.834445 | 11.107985 / 13.307430 / 10.447065 | — |
| M1 first exec | — | — | 9.936995 / 12.703113 / 9.369667 | 9.612259 / 11.338523 / 9.016392 | — |
| M1 import-only child | — | — | 33.555875 / 37.299613 / 30.103958 | 30.123199 / 34.484423 / 27.842150 | — |
| M2 warm construction | 0.37 | 0.20 | 0.704210 / 1.976711 / 0.602755 | 0.718914 / 1.825015 / 0.611622 | Miss |
| M3a warm async exec | 0.35 | 0.07 | 0.077426 / 0.156645 / 0.070843 | 0.072847 / 0.143981 / 0.068409 | Miss |
| M3a warm sync exec | — | — | 0.075232 / 0.151936 / 0.068639 | 0.073985 / 0.146546 / 0.068829 | — |
| M_host warm custom call | — | — | 0.169103 / 0.366360 / 0.146265 | 0.172580 / 0.379535 / 0.145344 | — |
| M5 RSS delta, B | 35,500,000 | 10,000,000 | 13,293,568 | 12,812,288 | Miss |

### Repetition medians

| Metric | Unminified runs 1–3 | Shipping minified runs 1–3 |
|---|---:|---:|
| M1 cold total | 55.669383 / 55.180461 / 56.557815 | 51.390223 / 52.142483 / 51.894006 |
| M1 import (cold-full) | 32.793216 / 33.278524 / 33.924775 | 30.431610 / 30.722151 / 30.236241 |
| M1 construction | 11.621100 / 11.459857 / 11.691749 | 11.107985 / 11.039275 / 11.343358 |
| M1 first exec | 10.135874 / 9.918216 / 9.936995 | 9.448330 / 9.942536 / 9.612259 |
| M1 import-only child | 32.484949 / 33.555875 / 33.777938 | 30.123199 / 30.065501 / 31.022026 |
| M2 warm construction | 0.704210 / 0.703966 / 0.718733 | 0.723202 / 0.703283 / 0.718914 |
| M3a warm async exec | 0.078297 / 0.076133 / 0.077426 | 0.074370 / 0.072847 / 0.072466 |
| M3a warm sync exec | 0.077375 / 0.073749 / 0.075232 | 0.072356 / 0.075542 / 0.073985 |
| M_host warm custom call | 0.169103 / 0.166563 / 0.173617 | 0.171563 / 0.172966 / 0.172580 |
| M5 RSS delta, B | 13,510,656 / 13,244,416 / 13,293,568 | 12,707,840 / 12,812,288 / 12,941,312 |

## Artifact sizes

Each pair gives raw / gzip bytes. The driver lists packaged JavaScript by running offline `npm pack --dry-run --json` in each candidate tree. It applies gzip level 9 to each file separately. Both candidates share the same Wasm build.

| Wasm artifact | Raw / gzip bytes | Result |
|---|---:|---|
| Loader core (M6-wasm) | 229,104 / 83,193 | Pass |
| Linked core | 234,910 / 85,918 | — |
| Component | 259,365 / 91,153 | — |

| Shipped JS group | Unminified | Shipping minified |
|---|---:|---:|
| Generated glue | 565,157 / 63,043 | 273,336 / 45,167 |
| Registry | 54 / 74 | 54 / 74 |
| Other browser-reachable entry code | 69,135 / 22,681 | 69,135 / 22,681 |
| Node-only | 1,790 / 853 | 1,790 / 853 |
| All shipped JS (M6-js) | 636,136 / 86,651 | 344,315 / 68,775 |

The minified build stays selected. It reduced median cold start by 3.775377 ms in this series. The Annex H provisional browser-reachable budget is 45,000 B raw against 634,346 / 342,525 B unminified / minified. Node-only JS is within 40,000 B raw. Both whole-package gzip totals exceed 30,000 B.

### wasm-opt

**Not measured.** The owner decision on the Binaryen optimizer pin is pending. `--variant wasm-opt` emits this status and writes it to `tmp/bench-l2/wasm-opt/results.json`. Optimization flags, size deltas, and timing deltas are pending. The build leaves `wasmOpt` disabled.

### v1 size projection

The additions are planning estimates in bytes. Command rows count shared runners and parsers once.

| Kernel contribution | Raw bytes | Gzip bytes |
|---|---:|---:|
| L2 loader core, measured | 229,104 | 83,193 |
| L3–L6 language and VFS | 400,000 | 160,000 |
| Tier-1 commands | 280,000 | 112,000 |
| awk, sed and jq | 400,000 | 160,000 |
| yq formats | 230,000 | 92,000 |
| Archives and compression | 180,000 | 72,000 |
| Projected v1 loader core | 1,719,104 | 679,193 |

Recommend owner review of a Cargo-feature `slim` profile. The projected raw size exceeds 1,500,000 B.

## Verification

- `make check` passed G1–G8, K1, native Rust tests, bindings regeneration, the packed consumers, and the Node suite with 246 passes and 9 existing TODOs.
- The 30 committed L2 oracle cases passed. Child-process regressions enforce bounded wall time for the large assignment tree, the long PATH search, and the large IFS split.
- Raw samples are in `tmp/bench-l2/committed/results.json` and `tmp/bench-l2/unminified/results.json`. The reference timings use the L2 specification. This lane replayed the committed oracle snapshots.
