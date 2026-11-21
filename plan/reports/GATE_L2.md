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
| Git | `ec1f82a0e00d13cfbb821bbe6455542e23f7b827` plus the PR #11 remediation changes, `dirty = true` |
| Loader Wasm SHA-256 | `d8a597b704fab7fdf9f86ba31006f9b4c59fb1ad95a285fcc30b2922736c601e` |
| Child environment | `LC_ALL=C`, `TZ=UTC`, worktree-local `TMPDIR` and npm cache, offline npm |

Each candidate passes `echo hi` and custom `noop` smoke checks in a separate process before sampling. Each of three repetitions uses 30 seeded rounds. Each round shuffles fresh baseline, import-only, and import-plus-construction-plus-first-exec processes. The seeds are `0x6c320001`, `0x6c320002`, and `0x6c320003`. Cold timings exclude Node process startup. M5 subtracts the median baseline RSS from the median first-exec RSS in each repetition.

M2, M3a, M3aSync, and M_host each use their own process per repetition. Each process runs 20 warmup iterations in a separate function, collects garbage once, and yields to the event loop for queued finalizers before starting the sample clock. It measures at least 1,000 iterations over at least 2,000 ms with normal garbage collection. Async echo, sync echo, and the custom call each reuse one shell within their own process. Timings use `performance.now()`, an interpolated median, and nearest-rank p95. Each reported statistic is the median of the three repetition values.

Measurements use the worktree changes on top of the listed revision because this lane has read-only Git metadata.

## Timing and memory

Timing cells show median / p95 / minimum in milliseconds. The reference values come from the L2 specification.

| Metric | just-bash 3.6.0 | Target | Unminified | Shipping minified | Shipping result |
|---|---:|---:|---:|---:|---|
| M1 cold total | 118.8 | 25.0 (stretch 15.0) | 57.322936 / 64.731737 / 51.026860 | 50.362844 / 58.759024 / 48.024959 | Miss |
| M1 import (cold-full) | — | — | 33.726374 / 39.498820 / 30.604167 | 29.479551 / 32.381563 / 27.902402 | — |
| M1 construction | — | — | 12.127804 / 15.836329 / 10.953909 | 11.166169 / 13.099467 / 10.564025 | — |
| M1 first exec | — | — | 10.298926 / 13.179379 / 9.510753 | 9.592210 / 11.499796 / 9.053482 | — |
| M1 import-only child | — | — | 33.722417 / 37.581211 / 30.627090 | 29.491853 / 32.215180 / 28.027627 | — |
| M2 warm construction | 0.37 | 0.20 | 0.718011 / 1.966021 / 0.615659 | 0.700569 / 1.722783 / 0.623063 | Miss |
| M3a warm async exec | 0.35 | 0.07 | 0.078678 / 0.156555 / 0.073258 | 0.077646 / 0.149872 / 0.072767 | Miss |
| M3a warm sync exec | — | — | 0.080201 / 0.155893 / 0.073108 | 0.076243 / 0.142609 / 0.072456 | — |
| M_host warm custom call | — | — | 0.189858 / 0.411675 / 0.148139 | 0.172590 / 0.373864 / 0.146115 | — |
| M5 RSS delta, B | 35,500,000 | 10,000,000 | 13,488,128 | 12,875,776 | Miss |

### Repetition medians

| Metric | Unminified runs 1–3 | Shipping minified runs 1–3 |
|---|---:|---:|
| M1 cold total | 53.399501 / 58.154155 / 57.322936 | 53.064470 / 50.074618 / 50.362844 |
| M1 import (cold-full) | 31.777385 / 34.368347 / 33.726374 | 30.925147 / 29.269340 / 29.479551 |
| M1 construction | 11.604343 / 12.310838 / 12.127804 | 11.464259 / 10.970405 / 11.166169 |
| M1 first exec | 9.907334 / 10.497115 / 10.298926 | 9.784483 / 9.507952 / 9.592210 |
| M1 import-only child | 32.439717 / 33.863974 / 33.722417 | 30.373643 / 29.491853 / 29.411953 |
| M2 warm construction | 0.718011 / 0.714735 / 0.727640 | 0.708294 / 0.700418 / 0.700569 |
| M3a warm async exec | 0.078247 / 0.078678 / 0.079139 | 0.077646 / 0.076554 / 0.078428 |
| M3a warm sync exec | 0.078047 / 0.080201 / 0.081614 | 0.077015 / 0.076243 / 0.076143 |
| M_host warm custom call | 0.198053 / 0.189858 / 0.178966 | 0.174358 / 0.172590 / 0.165612 |
| M5 RSS delta, B | 13,555,712 / 13,363,200 / 13,488,128 | 12,972,032 / 12,875,776 / 12,382,208 |

## Artifact sizes

Each pair gives raw / gzip bytes. The unminified tree starts from the shipping package's offline npm pack manifest and receives regenerated glue from the same component. The driver counts every `.js` entry in each candidate's offline `npm pack --dry-run --json` manifest and applies gzip level 9 to each file separately. Both candidates share the same Wasm build.

| Wasm artifact | Raw / gzip bytes | Result |
|---|---:|---|
| Loader core (M6-wasm) | 236,280 / 85,530 | Pass |
| Linked core | 242,086 / 88,211 | — |
| Component | 266,541 / 93,474 | — |

| Shipped JS group | Unminified | Shipping minified |
|---|---:|---:|
| Generated glue | 565,157 / 63,043 | 273,336 / 45,167 |
| Registry | 54 / 74 | 54 / 74 |
| Other browser-reachable entry code | 69,100 / 22,669 | 69,100 / 22,669 |
| Node-only | 1,790 / 853 | 1,790 / 853 |
| All shipped JS (M6-js) | 636,101 / 86,639 | 344,280 / 68,763 |

The minified build stays selected. It reduced median cold start by 6.960092 ms in this series. The Annex H provisional browser-reachable budget is 45,000 B raw against 634,311 / 342,490 B unminified / minified. Node-only JS is within 40,000 B raw. Both whole-package gzip totals exceed 30,000 B.

### wasm-opt

**Not measured.** The owner decision on the Binaryen optimizer pin is pending. `--variant wasm-opt` emits this status and writes it to `tmp/bench-l2/wasm-opt/results.json`. Optimization flags, size deltas, and timing deltas are pending. The build leaves `wasmOpt` disabled.

### v1 size projection

The additions are planning estimates in bytes. Command rows count shared runners and parsers once.

| Kernel contribution | Raw bytes | Gzip bytes |
|---|---:|---:|
| L2 loader core, measured | 236,280 | 85,530 |
| L3–L6 language and VFS | 400,000 | 160,000 |
| Tier-1 commands | 280,000 | 112,000 |
| awk, sed and jq | 400,000 | 160,000 |
| yq formats | 230,000 | 92,000 |
| Archives and compression | 180,000 | 72,000 |
| Projected v1 loader core | 1,726,280 | 681,530 |

Recommend owner review of a Cargo-feature `slim` profile. The projected raw size exceeds 1,500,000 B.

## Verification

- `make check` passed G1–G8, K1, native Rust tests, bindings regeneration, the packed consumers, and the Node suite with 256 passes and 9 existing TODOs.
- The 30 committed L2 oracle cases passed. Child-process regressions enforce bounded wall time for shared-prefix variable lookup and restoration, relative commands under a large cwd, long PATH and IFS processing, and pipeline abort and deadline handling. The stdin tests cover an oversized input following a live-byte failure and an oversized input with a blank script. The recursion diagnostic survived full and zero output budgets.
- `harness check scope` passed C2.
- Raw samples are in `tmp/bench-l2/committed/results.json` and `tmp/bench-l2/unminified/results.json`. The reference timings use the L2 specification. This lane replayed the committed oracle snapshots.
