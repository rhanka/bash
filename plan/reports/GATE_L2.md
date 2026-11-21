# Gate L2

**No-go (kernel miss).** M1, M2, M3a, M5 exceeded the kernel targets. Pause W4 and later build lots under PLAN section 5. Spec writing can continue while the owner reviews the targets, a slim build, and scope options. Shipped JS also exceeded the 60,000 B target.

## Verification

- G1–G8, K1, native Rust tests, binding regeneration, and packed Node and browser consumers passed. Node reported 228 passes, zero failures, and 9 existing TODOs.
- Oracle replay passed all 30 committed cases with zero failures, stale snapshots, exclusions, or uncovered cases. All four profile suites and the 10,000-deep recursion check passed.
- The hardened ceiling test holds VFS bytes in two volumes, retains output, and allocates live buffers. It reaches the instance ceiling, returns exit 126 with the memory-ceiling diagnostic, and executes echo ok on the same shell.
- The Rust include! registry builds on Rust 1.96. G3 reproduced the core, component, and JS in separate source trees.

## Environment and protocol

| Field | Value |
|---|---|
| OS kernel | Linux 7.0.0-43-generic |
| CPU | AMD RYZEN AI MAX+ 395 w/ Radeon 8060S |
| Node | v22.22.1 |
| Rust | rustc 1.96.0 (ac68faa20 2026-05-25) |
| wasm-tools | wasm-tools 1.263.0 (7a2fa422c 2026-10-30) |
| Binaryen | 132.0.0 in the transpiler tool tree, unpinned for core optimization |
| Git | f3d319328e20f916056973d2c3d47d2e949599b6 with the L2 working tree, dirty = true |
| Child environment | LC_ALL=C, TZ=UTC, TMPDIR=$PWD/tmp/bench-l2/tmp, npm_config_cache=$PWD/tmp/npmcache, offline npm |

Each candidate passed an echo hi assertion in a fresh child. The boundary workload asserted the noop result. Each of three runs scheduled 30 rounds of fresh Node ESM children. Each round shuffled an empty baseline, an import-only child, and an import-plus-construction-plus-first-exec child. Seeds were 0x6c320001, 0x6c320002, 0x6c320003. Cold timings exclude starting Node. Each run subtracts the median baseline RSS from the median RSS after first exec.

Each warm run used a separate child, 20 discarded iterations per workload, and at least 1,000 timed iterations over at least 2,000 ms. Async and sync echo hi each reuse one warm shell. The noop command incurs one host-call/resume crossing. Timings use performance.now(). The median interpolates the middle values and p95 uses nearest rank. Each summary statistic is the median of its three run values.

Size measurements use gzip level 9 separately for each file. JS file names come from offline npm pack --dry-run --json. Reference values come from the spec benchmark table. This lane replayed the committed oracle snapshots.

## Timing and memory

Millisecond cells show median / p95 / minimum. The minified candidate ships.

| Metric | just-bash 3.6.0 | Target | Unminified | Minified |
|---|---:|---:|---:|---:|
| M1 cold total | 118.8 | 25.0 (stretch 15.0) | 123.002678 / 132.620836 / 119.008115 | 103.522521 / 111.511246 / 98.454853 |
| M1 import (cold-full) | — | — | 32.133271 / 35.542798 / 30.231221 | 30.885244 / 35.488102 / 27.904810 |
| M1 construction | — | — | 81.831046 / 90.913666 / 79.068791 | 62.938322 / 68.072621 / 59.739877 |
| M1 first exec | — | — | 9.380706 / 10.736478 / 9.090379 | 9.388020 / 11.727602 / 8.664407 |
| M1 import-only child | — | — | 31.733769 / 36.139140 / 30.030914 | 30.882163 / 33.942954 / 27.979721 |
| M2 warm construction | 0.37 | 0.20 | 72.162789 / 80.507219 / 68.588987 | 51.941174 / 58.356323 / 48.818452 |
| M3a warm async exec | 0.35 | 0.07 | 0.094779 / 0.167325 / 0.089929 | 0.096682 / 0.176713 / 0.090881 |
| M3a warm sync exec | — | — | 0.093015 / 0.162095 / 0.089428 | 0.093897 / 0.162346 / 0.090230 |
| M_host warm custom call | — | — | 0.220150 / 0.435550 / 0.178727 | 0.221667 / 0.420121 / 0.179418 |

| Metric | just-bash 3.6.0 | Target | Unminified | Minified |
|---|---:|---:|---:|---:|
| M5 RSS delta, B | 35,500,000 | 10,000,000 | 13,348,864 | 12,888,064 |

### Three runs

Timing cells give the three run medians in seed order. Raw JSON retains every child result and iteration, sample counts, and each run's p95 and minimum.

| Metric | Unminified runs 1–3 | Minified runs 1–3 |
|---|---:|---:|
| M1 cold total | 123.002678, 122.912459, 123.877920 | 103.900963, 102.032892, 103.522521 |
| M1 import (cold-full) | 31.496747, 32.133271, 32.930175 | 31.000887, 30.270979, 30.885244 |
| M1 construction | 82.000259, 81.831046, 81.781508 | 63.206761, 62.313165, 62.938322 |
| M1 first exec | 9.384648, 9.338227, 9.380706 | 9.388020, 9.013665, 9.606866 |
| M1 import-only child | 31.733769, 31.323080, 32.545902 | 31.483510, 29.255527, 30.882163 |
| M2 warm construction | 70.667373, 72.481318, 72.162789 | 52.111669, 50.625082, 51.941174 |
| M3a warm async exec | 0.095686, 0.094779, 0.094017 | 0.097564, 0.096612, 0.096682 |
| M3a warm sync exec | 0.093747, 0.093015, 0.092193 | 0.095740, 0.093897, 0.093406 |
| M_host warm custom call | 0.206569, 0.220150, 0.224568 | 0.219178, 0.230363, 0.221667 |
| M5, B | 13,541,376 / 13,348,864 / 13,324,288 | 13,203,456 / 12,888,064 / 12,613,632 |

Unminified baseline RSS medians were 57,376,768 / 57,577,472 / 57,499,648 B. Full-process RSS medians were 70,918,144 / 70,926,336 / 70,823,936 B. Warm iteration counts were M2 warm construction 1,000/1,000/1,000, M3a warm async exec 18,706/18,347/18,456, M3a warm sync exec 19,967/18,882/19,497, M_host warm custom call 7,941/7,288/7,240.

Minified baseline RSS medians were 57,176,064 / 57,473,024 / 57,618,432 B. Full-process RSS medians were 70,379,520 / 70,361,088 / 70,232,064 B. Warm iteration counts were M2 warm construction 1,000/1,000/1,000, M3a warm async exec 18,049/17,357/17,696, M3a warm sync exec 19,072/19,156/19,131, M_host warm custom call 7,632/6,860/7,560.

### Cost profile

Cold import took 30.885244 ms, construction 62.938322 ms, and first exec 9.388020 ms. In a separate warm construction probe on the unminified build, medians were 72.437651 ms in generated initialization, 0.201769 ms in core Wasm instantiation, and 0.771322 ms calling configure, VFS creation and shell layout. A Node CPU profile concentrated samples in the generated _initGenerator. L2 classifies the constructor and cold-start thresholds as kernel metrics even when generated JS drives the cost.

A warm exec probe measured 0.104807 ms end to end and 0.090409 ms in measured engine calls, including marshalling and kernel execution. The gate timings use the uninstrumented driver.

The fresh-process RSS probe for the selected glue build gave phase medians of 57,544,704 B before import, 67,471,360 B after import, 69,568,512 B after construction, and 70,070,272 B after first exec. Gate M5 uses separate baseline children.

## Artifact sizes

Pairs give raw / gzip bytes. Both glue variants share the same Wasm build. The measured minified host and generated kernel directory matched the shipping build byte for byte.

| Wasm artifact | Both candidates |
|---|---:|
| Loader core (M6-wasm) | 219,953 / 79,176 |
| Linked core | 225,759 / 81,878 |
| Component | 250,214 / 87,088 |

M6-wasm targets 1,500,000 B raw and 700,000 B gzip.

| Shipped JS group | Unminified | Minified |
|---|---:|---:|
| Generated glue | 565,129 / 63,041 | 273,308 / 45,163 |
| Registry | 54 / 74 | 54 / 74 |
| Other browser-reachable entry code | 68,414 / 22,529 | 68,414 / 22,529 |
| Node-only | 1,790 / 853 | 1,790 / 853 |
| All shipped JS (M6-js) | 635,387 / 86,497 | 343,566 / 68,619 |

Retain minification. It lowered M1 by 19.480157 ms, and the minified release passed G1–G8 and the full test suite.

The provisional Annex H browser-reachable entry including glue and registry is 633,597 / 341,776 B raw against 45,000 B. Node-only JS is 1,790 B against 40,000 B. Total gzip is 86,497 / 68,619 B against 30,000 B. The owner retains the JS budget decision.

### wasm-opt

Gain unmeasured. Binaryen 132.0.0 is present as a transpiler dependency. The offline lane has no approved optimizer pin. Feature flags, raw and gzip deltas, and timing deltas are unmeasured. The build keeps wasmOpt disabled.

### v1 size projection

The additions are planning estimates in bytes. The shell and VFS row covers the remaining interpreter, expansion, and filesystem work. Command rows count shared runners and their parsers once.

| Kernel contribution | Raw bytes | Gzip bytes |
|---|---:|---:|
| L2 loader core, measured | 219,953 | 79,176 |
| L3–L6 language and VFS | 400,000 | 160,000 |
| Tier-1 commands | 280,000 | 112,000 |
| awk, sed and jq | 400,000 | 160,000 |
| yq formats | 230,000 | 92,000 |
| Archives and compression | 180,000 | 72,000 |
| Projected v1 loader core | 1,709,953 | 675,176 |

Recommend owner review of a Cargo-feature slim profile. The projected raw size exceeds 1,500,000 B. Measure generated instantiation independently when evaluating a smaller command set.

## Handover

- The contain and normal command-count tests use a bounded call tree to execute over 100,000 stages with fewer than 100,000 tokens. The flat-list recipe reaches the parser cap first. A separate test checks that cap.
- Deadline tests advance the driver clock across each profile's unchanged deadline. Profile values range from 10 seconds to one hour.
- Candidate upstream defects are filesystem quota exceptions, lone surrogates from echo -e Unicode escapes, and whole-output byte decoding when any sequence is invalid. L2 returns a quota result (exit 126), emits U+FFFD for lone surrogates, and decodes each valid UTF-8 sequence. For echo -e with bytes C3 A9 FF, the reference gives Ã©ÿ and L2 gives éÿ.
- The reference's first lazy command can fail with coverage and default defenseInDepth, which blocks the dynamic import of node:module. The coverage oracle uses defenseInDepth: false.
- L2 merges before L7b. L7b owns the rebase touchpoints in section 3.1 and uses COMMAND_NAMES for its host-backed stubs.
- Raw samples are in tmp/bench-l2/unminified/results.json and tmp/bench-l2/minified/results.json. The constructor, exec and RSS diagnostic is in tmp/l2-diagnostic.json.
- Rebuild with npm_config_cache=$PWD/tmp/npmcache make build. Use node tools/measure-l2.mjs --variant committed to measure the package tree, or --variant unminified and --variant minified to regenerate either candidate.
