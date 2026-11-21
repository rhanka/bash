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
| Git | `d1e521059a37d08924c10b5b5202d66fabaa698c` plus the PR #11 remediation changes, `dirty = true` |
| Loader Wasm SHA-256 | `bcd96432a2c7f44827bcf9e1f108193b03b9e3cf482eef0147780ed79382d6d8` |
| Child environment | `LC_ALL=C`, `TZ=UTC`, worktree-local `TMPDIR` and npm cache, offline npm |

Each candidate passes `echo hi` and custom `noop` smoke checks in a separate process before sampling. Each of three repetitions uses 30 seeded rounds. Each round shuffles fresh baseline, import-only, and import-plus-construction-plus-first-exec processes. The seeds are `0x6c320001`, `0x6c320002`, and `0x6c320003`. Cold timings exclude Node process startup. M5 subtracts the median baseline RSS from the median first-exec RSS in each repetition.

M2, M3a, M3aSync, and M_host each use their own process per repetition. Each process runs 20 warmup iterations, then collects garbage once before starting the sample clock. It measures at least 1,000 iterations over at least 2,000 ms with normal garbage collection. Async echo, sync echo, and the custom call each reuse one shell within their own process. Timings use `performance.now()`, an interpolated median, and nearest-rank p95. Each reported statistic is the median of the three repetition values.

Measurements use the worktree changes on top of the listed revision because this lane has read-only Git metadata.

## Timing and memory

Timing cells show median / p95 / minimum in milliseconds. The reference values come from the L2 specification.

| Metric | just-bash 3.6.0 | Target | Unminified | Shipping minified | Shipping result |
|---|---:|---:|---:|---:|---|
| M1 cold total | 118.8 | 25.0 (stretch 15.0) | 57.763401 / 67.320344 / 52.688203 | 54.184043 / 62.512445 / 48.330450 | Miss |
| M1 import (cold-full) | — | — | 34.607620 / 40.165562 / 30.547559 | 32.314830 / 39.090117 / 28.378616 | — |
| M1 construction | — | — | 12.047356 / 14.877914 / 11.112997 | 11.538458 / 14.096353 / 10.480146 | — |
| M1 first exec | — | — | 10.371167 / 13.297389 / 9.651667 | 9.919115 / 13.170370 / 9.129414 | — |
| M1 import-only child | — | — | 34.685617 / 39.532370 / 30.475183 | 32.606604 / 38.123678 / 29.140340 | — |
| M2 warm construction | 0.37 | 0.20 | 0.720346 / 2.036313 / 0.617593 | 0.749341 / 1.971181 / 0.625788 | Miss |
| M3a warm async exec | 0.35 | 0.07 | 0.077716 / 0.160903 / 0.071775 | 0.076875 / 0.162215 / 0.071384 | Miss |
| M3a warm sync exec | — | — | 0.078829 / 0.159400 / 0.071835 | 0.078107 / 0.147016 / 0.071064 | — |
| M_host warm custom call | — | — | 0.183666 / 0.439487 / 0.146055 | 0.178676 / 0.403559 / 0.145173 | — |
| M5 RSS delta, B | 35,500,000 | 10,000,000 | 13,119,488 | 12,820,480 | Miss |

### Repetition medians

| Metric | Unminified runs 1–3 | Shipping minified runs 1–3 |
|---|---:|---:|
| M1 cold total | 57.763401 / 57.426727 / 58.555311 | 53.979779 / 54.508194 / 54.184043 |
| M1 import (cold-full) | 34.607620 / 34.469370 / 34.678184 | 32.314830 / 33.079023 / 31.688741 |
| M1 construction | 12.462327 / 11.802430 / 12.047356 | 11.538458 / 11.596222 / 11.480424 |
| M1 first exec | 10.371167 / 10.103432 / 10.385979 | 9.927346 / 9.787653 / 9.919115 |
| M1 import-only child | 34.685617 / 33.691736 / 35.390224 | 33.128080 / 32.489123 / 32.606604 |
| M2 warm construction | 0.737078 / 0.694367 / 0.720346 | 0.725616 / 0.749341 / 0.790017 |
| M3a warm async exec | 0.078648 / 0.076514 / 0.077716 | 0.090350 / 0.076544 / 0.076875 |
| M3a warm sync exec | 0.079349 / 0.074591 / 0.078829 | 0.078317 / 0.078107 / 0.075553 |
| M_host warm custom call | 0.167786 / 0.254545 / 0.183666 | 0.178676 / 0.179112 / 0.174449 |
| M5 RSS delta, B | 13,119,488 / 13,510,656 / 12,937,216 | 12,656,640 / 13,056,000 / 12,820,480 |

## Artifact sizes

Each pair gives raw / gzip bytes. The driver counts every `.js` entry in the offline `npm pack --dry-run --json` manifest of each candidate tree. It applies gzip level 9 to each file separately. Both candidates share the same Wasm build.

| Wasm artifact | Raw / gzip bytes | Result |
|---|---:|---|
| Loader core (M6-wasm) | 229,108 / 83,382 | Pass |
| Linked core | 234,914 / 86,102 | — |
| Component | 259,369 / 91,341 | — |

| Shipped JS group | Unminified | Shipping minified |
|---|---:|---:|
| Generated glue | 565,157 / 63,043 | 273,336 / 45,167 |
| Registry | 54 / 74 | 54 / 74 |
| Other browser-reachable entry code | 69,100 / 22,669 | 69,100 / 22,669 |
| Node-only | 1,790 / 853 | 1,790 / 853 |
| All shipped JS (M6-js) | 636,101 / 86,639 | 344,280 / 68,763 |

The minified build stays selected. It reduced median cold start by 3.579358 ms in this series. The Annex H provisional browser-reachable budget is 45,000 B raw against 634,311 / 342,490 B unminified / minified. Node-only JS is within 40,000 B raw. Both whole-package gzip totals exceed 30,000 B.

### wasm-opt

**Not measured.** The owner decision on the Binaryen optimizer pin is pending. `--variant wasm-opt` emits this status and writes it to `tmp/bench-l2/wasm-opt/results.json`. Optimization flags, size deltas, and timing deltas are pending. The build leaves `wasmOpt` disabled.

### v1 size projection

The additions are planning estimates in bytes. Command rows count shared runners and parsers once.

| Kernel contribution | Raw bytes | Gzip bytes |
|---|---:|---:|
| L2 loader core, measured | 229,108 | 83,382 |
| L3–L6 language and VFS | 400,000 | 160,000 |
| Tier-1 commands | 280,000 | 112,000 |
| awk, sed and jq | 400,000 | 160,000 |
| yq formats | 230,000 | 92,000 |
| Archives and compression | 180,000 | 72,000 |
| Projected v1 loader core | 1,719,108 | 679,382 |

Recommend owner review of a Cargo-feature `slim` profile. The projected raw size exceeds 1,500,000 B.

## Verification

- `make check` passed G1–G8, K1, native Rust tests, bindings regeneration, the packed consumers, and the Node suite with 250 passes and 9 existing TODOs.
- The 30 committed L2 oracle cases passed. Child-process regressions enforce bounded wall time for the large assignment tree, the long PATH search, the large IFS split, and pipeline abort and deadline handling. The recursion diagnostic survived full and zero output budgets.
- `harness check scope` passed C2.
- Raw samples are in `tmp/bench-l2/committed/results.json` and `tmp/bench-l2/unminified/results.json`. The reference timings use the L2 specification. This lane replayed the committed oracle snapshots.
