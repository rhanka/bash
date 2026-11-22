# Gate L2

**Superseded (L2b baseline and L2c No-go, kept for history).** Earlier shipping builds exceeded kernel or JS budgets. L2b owns the first-party glue. The build-lot pause under PLAN section 5 remains in effect until the current verdict allows W4.

The current verdict is the L2c fix rerun at the end of this report. Exactly one verdict is current.

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

## L2c rerun on the packed default release (superseded)

**No-go (kernel miss, superseded).** The packed default release missed the M1 cold-start and M5 RSS kernel budgets. Superseded by the fix rerun below.

`node tools/measure-l2.mjs --variant committed` ran the full Gate L2 protocol against the installed packed default release (`@sentropic/bash` as packed, minified, installed offline), with the installed `@sentropic/bash-jco` package as the matching Option D control. `make check` is green (G1–G8, K1, native Rust tests, bindings regeneration) including the 30/30 L2 parity replay on the first-party binding and on each packed package, and `harness check scope` passes. Raw data: `tmp/bench-l2/committed/results.json`.

### Revision and protocol (L2c)

| Field | Measured value |
|---|---|
| OS kernel | Linux 7.0.0-43-generic |
| CPU | AMD RYZEN AI MAX+ 395 w/ Radeon 8060S |
| Node | v22.22.1 |
| Rust | rustc 1.96.0 (ac68faa20 2026-05-25) |
| wasm-tools | wasm-tools 1.263.0 (7a2fa422c 2026-10-30) |
| Binaryen | 132.0.0 in the transpiler tool tree, unpinned for core optimization |
| Git | `2db1506` plus the `tmp/`-creation fix (now `a8890d6`), `dirty = true` (uncommitted scratch outputs only; artifacts byte-identical to the committed tree) |
| Loader Wasm SHA-256 | `35e3753f8070ccec72baa3a981eb7898adf426bb8ebc85312db147861138d96d` |
| Component SHA-256 | `308ec3d7b2d6414fe30dd60c9e8cb94f9ee28b8a8a96bd611b3d0e55e5b34e0d` |
| WIT | identical bytes in both packages (`wit/sentropic-bash.wit`) |
| Child environment | `LC_ALL=C`, `TZ=UTC`, worktree-local `TMPDIR` and npm cache, offline npm |

Same seeds (`0x6c320001`, `0x6c320002`, `0x6c320003`), 30 shuffled rounds per seed, 20 discarded warmups, then at least 1,000 samples and 2,000 ms per warm workload. Cold excludes process launch and includes all lazy I/O and compilation. M5 is median full RSS minus median empty RSS per seed.

### Snapshot invariants (L2c section 4 and 8)

The default shell restores from a build-matched passive image via the private core export `__restore_default() -> i64` (invisible to the WIT). Capture ran through the first-party adapter with production lowering and allocation order (`configure(268435456)`, `Vfs.create(134217728)`, `contain` default shell), twice, byte-identical.

- Stack pointer back at its initial value 1,048,576; the single mutable global is the stack pointer. No `table.set/grow/fill/copy/init` in the core. Quiescence held at capture: zero live borrows/fibers/pending host calls/deferred frees, `headroom().used() == 0`, reserve closed, `LOWERING == [None; 3]`.
- Initial page count 17, final page count 19 (both within the 268,435,456 B cap). Changed ranges: 2,172, sorted, bounded, disjoint, including nonzero-to-zero words; image 4,564 B in one passive segment. No shipped JS pointer constants, `refs` array, memory image or pointer-bearing manifest.
- Observed handles: Vfs id 1 and Shell id 2 (distinct, right kinds, claimed exactly once). Resource count 2; zero spare stores across two restores. Allocator usage at capture: used 8,202 B, high-water 8,847 B, ceiling 268,435,456 B.
- Snapshot size delta: plain linked core 242,534 B raw to augmented 276,760 B raw (+34,226 B); shipped loader core 270,954 B raw / 97,765 B gzip.

### Timing and memory (L2c)

Median / p95 / minimum per metric; reference values from the L2 specification. The verdict compares unrounded median-of-run medians against the inclusive kernel thresholds.

| Metric | just-bash 3.6.0 | Target | Default (restore) | Option D control | Result |
|---|---:|---:|---:|---:|---|
| M1 cold total | 118.8 | 25.0 (15.0 stretch) | 26.304 / 32.077 / 23.846 | 37.689 / 44.412 / 35.477 | Miss |
| M1 import (cold-full) | — | — | 12.709 / 16.156 / 11.671 | 15.896 / 19.679 / 14.488 | — |
| M1 construction | — | — | 5.545 / 7.469 / 5.063 | 11.818 / 14.786 / 11.130 | — |
| M1 first exec | — | — | 7.587 / 9.894 / 6.955 | 9.717 / 11.528 / 9.045 | — |
| M1 import-only child | — | — | 13.098 / 15.284 / 11.400 | 15.720 / 17.640 / 14.595 | — |
| M2 warm construction | 0.37 | 0.20 | 0.19677 / 0.72126 / 0.14276 | 0.73575 / 1.72580 / 0.63312 | Pass |
| M3a warm async exec | 0.35 | 0.07 | 0.04964 / 0.08832 / 0.04695 | 0.07813 / 0.14947 / 0.07278 | Pass |
| M3a warm sync exec | — | — | 0.04905 / 0.08597 / 0.04661 | 0.07710 / 0.15123 / 0.07221 | — |
| M_host warm custom call | — | — | 0.11704 / 0.22689 / 0.10690 | 0.17511 / 0.39949 / 0.14567 | — |
| M5 RSS delta, B | 35,500,000 | 10,000,000 | 10,696,704 | 5,468,160 | Miss |
| M6-wasm loader core | — | 1,500,000 raw and 700,000 gzip | 270,954 / 97,765 | 270,954 / 97,765 | Pass |

Per-seed M1 medians: 26.304 / 26.508 / 26.178. Per-seed M2 medians: 0.25360 / 0.18101 / 0.19677. Per-seed M3a medians: 0.04928 / 0.04964 / 0.04975. Per-seed M5: 10,504,192 / 10,696,704 / 10,821,632.

The restore fixed M2 (0.70 to 0.197 against the 0.20 budget) and M3a (0.078 to 0.050), and cut M1 from 50.36 to 26.30 and M5 from 12,875,776 to 10,696,704 B against the guarded L2 release. M1 still misses by 1.30 ms (compile-dominated import at 12.7 ms) and M5 by 696,704 B.

### Package split and JS budgets (L2c section 5)

Per-file raw/gzip bytes of the shipped JS (decimal). The default package alone carries the Annex H gate. Both candidate browser caps are reported (the owner choice is pending); the higher cap is not silently substituted.

| Shipped JS file | Raw | Gzip (level 9) |
|---|---:|---:|
| `dist/js/shared.js` | 63,469 | 21,264 |
| `dist/js/browser.js` | 1,692 | 796 |
| `dist/js/index.js` | 1,031 | 443 |
| `dist/js/node/load.js` | 739 | 429 |

- B (browser-reachable: `browser.js` + `shared.js`) = 65,161 B. Against B <= 45,000: miss, headroom -20,161. Against B <= 58,000: miss, headroom -7,161.
- N (remaining shipped JS) = 1,770 B against N <= 40,000: pass, headroom +38,230.
- T (B + N) = 66,931 B against T <= 60,000: miss, headroom -6,931.
- Z (per-file gzip sum) = 22,932 B against Z <= 30,000: pass, headroom +7,068.

The Option D control carries no JS gate; for the record its split is B = 313,566 (guarded jco glue 272,057 B), N = 1,770, T = 315,336, Z = 60,443.

The facades are deterministic minified ESM from one source graph (lockfile `oxc-minify`), factored into one browser-safe shared module per package with a binding-specific entry. `host/node/` keeps the synchronous deferred `process.getBuiltinModule` lookup after bundling; the browser entry and shared chunk have no transitive path to it. Declarations for both tarballs are staged directly from the root `types/**` tree; the companion keeps no separately maintained `.d.ts` files. Both packages install alone offline with empty dependencies and no install script, and each excludes the alternative binding. Pack manifests, G2/G5 on shipped paths, identical component/WIT/loader-core hashes and the local OCI payload input (the component bytes above; remote OCI verification stays at the release gate) are asserted by the pack tests. Real-browser timing stays with L19.

### wasm-opt

**Not measured.** Binaryen 132.0.0 is present only in the transpiler tool tree and is not pinned for offline core optimization, so `wasmOpt` stays `false` per the unapproved arm. `--variant wasm-opt` records this status. Optimization flags, size deltas and timing deltas are pending the owner pin; the build ships the verified no-opt release.

### v1 size projection (L2c section 6, snapshot counted once)

| Kernel contribution | Raw bytes | Gzip bytes |
|---|---:|---:|
| L2c loader core, measured | 270,954 | 97,765 |
| L3–L6 language and VFS | 400,000 | 160,000 |
| Tier-1 commands | 280,000 | 112,000 |
| awk, sed and jq | 400,000 | 160,000 |
| yq formats | 230,000 | 92,000 |
| Archives and compression | 180,000 | 72,000 |
| Projected v1 loader core | 1,760,954 | 693,765 |

Recommend owner review of a Cargo-feature `slim` profile. The projected raw size exceeds 1,500,000 B; the projected gzip size fits 700,000 B.

### Verification (L2c)

- `make check` green: G1–G8, K1, native Rust tests, bindings regeneration, both packed entry smoke tests, and the Node suite with 332 passes and 9 existing TODOs (0 failures). The L2 parity replay passes 30/30 on the first-party binding, the guarded jco binding, and each installed package. All four limit profiles pass on each installed package, and the L2b differential suite passes.
- Gate logic and exit gates 1–4 are covered by `test/snapshot.test.mjs` (15 tests: predicate, equivalence, isolation, refusals, growth, disposal, trap, staged browser restore, precompile provenance, jco ordinary path, logger delivery, image shape) and `test/pack.test.mjs` (10 tests: manifests, offline install, ESM/CJS/browser, declarations, hashes, G2/G5, both replay totals, all profiles).
- `harness check scope` passed C2; `harness check branch` passed C1.
- Raw samples are in `tmp/bench-l2/committed/results.json` (default install and Option D control).

## L2c fix rerun on the packed default release (current)

**Go on kernel, owner review of the JS budget (JS-only miss).** All kernel thresholds pass; browser JS exceeds 45,000 B (and 58,000 B) and total JS exceeds 60,000 B. W4 can proceed. This is the exactly one current verdict.

`node tools/measure-l2.mjs --variant committed` ran the full Gate L2 protocol on the installed packed default release with the installed `@sentropic/bash-jco` control, after the PR #16 fix (TOCTOU single-read, ceiling, holder cleanup, coalesced snapshot, flattened bundle, lazy shellConfig, pooled codec, no-freeze descriptors). `make check` is green (G1–G8, K1, Rust tests, bindings regen, 30/30 parity on both bindings and both packages). Raw data: `tmp/bench-l2/committed/results.json`.

### Revision and protocol (fix)

| Field | Measured value |
|---|---|
| OS kernel | Linux 7.0.0-43-generic |
| CPU | AMD RYZEN AI MAX+ 395 w/ Radeon 8060S |
| Node | v22.22.1 |
| Rust | rustc 1.96.0 (ac68faa20 2026-05-25) |
| wasm-tools | wasm-tools 1.263.0 (7a2fa422c 2026-10-30) |
| Binaryen | 132.0.0 in the transpiler tool tree, unpinned for core optimization |
| Git | `04b8a1b` plus the fix changes, `dirty = true` (uncommitted fix outputs only) |
| Loader Wasm SHA-256 | `d78254338163a0a4…` (full digest in `tmp/bench-l2/committed/results.json`) |
| Child environment | `LC_ALL=C`, `TZ=UTC`, worktree-local `TMPDIR` and npm cache, offline npm |

Same seeds (`0x6c320001`, `0x6c320002`, `0x6c320003`), 30 shuffled rounds per seed, 20 discarded warmups, then at least 1,000 samples and 2,000 ms per warm workload.

### Snapshot invariants (fix)

- Stack pointer back at 1,048,576; single mutable global is the stack pointer. No table mutation. Quiescence held at capture.
- Initial page count 17, final page count 19 (within 268,435,456 B). Changed ranges: 22, sorted, bounded, disjoint, including nonzero-to-zero words; image 15,477 B in one passive segment. No shipped JS pointer constants.
- Observed handles: Vfs id 1 and Shell id 2 (distinct, right kinds, claimed once). Resource count 2; zero spare stores. Allocator usage: used 8,202 B, high-water 8,847 B, ceiling 268,435,456 B.
- Snapshot size delta: shipped loader core 252,632 B raw / 88,621 B gzip (was 270,954 / 97,765; −18,322 B raw from 2,172 to 22 `memory.init` ops).

### Timing and memory (fix)

Median / p95 / minimum per metric; the verdict compares unrounded median-of-run medians against the inclusive kernel thresholds.

| Metric | just-bash 3.6.0 | Target | Default (restore) | Option D control | Result |
|---|---:|---:|---:|---:|---|
| M1 cold total | 118.8 | 25.0 (15.0 stretch) | 21.534 / 24.877 / 20.349 | 42.418 / 50.549 / 37.232 | Pass |
| M1 import (cold-full) | — | — | 10.488 / 13.407 / 9.849 | — | — |
| M1 construction | — | — | 3.502 / 4.825 / 3.334 | — | — |
| M1 first exec | — | — | 7.301 / 8.674 / 7.007 | — | — |
| M1 import-only child | — | — | — | — | — |
| M2 warm construction | 0.37 | 0.20 | 0.12034 / 0.59936 / 0.09810 | — | Pass |
| M3a warm async exec | 0.35 | 0.07 | 0.04524 / 0.07761 / 0.04327 | — | Pass |
| M3a warm sync exec | — | — | 0.11173 / 0.23601 / 0.07258 | — | — |
| M_host warm custom call | — | — | 0.30182 / 0.54377 / 0.15749 | — | — |
| M5 RSS delta, B | 35,500,000 | 10,000,000 | 3,950,592 | 5,378,048 | Pass |
| M6-wasm loader core | — | 1,500,000 raw and 700,000 gzip | 252,632 / 88,621 | 252,632 / 88,621 | Pass |

Per-seed M1 medians: 22.258 / 21.534 / 20.965. Per-seed M2 medians: 0.12542 / 0.12034 / 0.11903. Per-seed M3a medians: 0.04570 / 0.04524 / 0.04523. Per-seed M5: 4,206,592 / 3,887,104 / 3,950,592.

### Package split and JS budgets (fix)

| Shipped JS file | Raw | Gzip (level 9) |
|---|---:|---|
| `dist/js/shared.js` | 61,210 | 20,440 |
| `dist/js/browser.js` | 1,692 | 796 |
| `dist/js/index.js` | 1,031 | 443 |
| `dist/js/node/load.js` | 739 | 429 |

- B (browser-reachable: `browser.js` + `shared.js`) = 62,902 B. Against B <= 45,000: miss, headroom −17,902. Against B <= 58,000: miss, headroom −4,902.
- N (remaining shipped JS) = 1,770 B against N <= 40,000: pass, headroom +38,230.
- T (B + N) = 64,672 B against T <= 60,000: miss, headroom −4,672.
- Z (per-file gzip sum) = 22,108 B against Z <= 30,000: pass, headroom +7,892.

### wasm-opt

**Not measured.** Binaryen 132.0.0 is present only in the transpiler tool tree and is not pinned for offline core optimization, so `wasmOpt` stays `false` per the unapproved arm.

### v1 size projection (fix, snapshot counted once)

| Kernel contribution | Raw bytes | Gzip bytes |
|---|---|---:|
| L2c fix loader core, measured | 252,632 | 88,621 |
| L3–L6 language and VFS | 400,000 | 160,000 |
| Tier-1 commands | 280,000 | 112,000 |
| awk, sed and jq | 400,000 | 160,000 |
| yq formats | 230,000 | 92,000 |
| Archives and compression | 180,000 | 72,000 |
| Projected v1 loader core | 1,742,632 | 684,621 |

Recommend owner review of a Cargo-feature `slim` profile. The projected raw size exceeds 1,500,000 B; the projected gzip size fits 700,000 B.

### Verification (fix)

- `make check` green: G1–G8, K1, native Rust tests, bindings regeneration, both packed entry smoke tests, and the Node suite with 339 passes and 9 existing TODOs (0 failures). The L2 parity replay passes 30/30 on the first-party binding, the guarded jco binding, and each installed package. All four limit profiles pass on each installed package.
- Exit gates 1–4 now covered by `test/snapshot.test.mjs` (20 tests: prior 15 plus single-read predicate, hostile-grant ordinary setup, ceiling liftBudget, holder failure cleanup, coalesced image) and `test/pack.test.mjs` (10 tests).
- Raw samples are in `tmp/bench-l2/committed/results.json` (default install and Option D control).
