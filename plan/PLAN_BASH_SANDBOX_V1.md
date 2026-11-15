# PLAN — @sentropic/bash v1 (rung 3: lot plan)

Status: executable plan for v1 parity, derived from `spec/SPEC_EVOL_BASH_SANDBOX.md` v1.3 ("EVOL") and
`spec/SPEC_STUDY_BASH_SANDBOX.md` v3.2 ("the study", section 7 as base). Decisions are in EVOL; this
document schedules and prices the work. Post-parity work is section 9.

Units: a point is the size unit of the study (S = 1, M = 3, L = 8, with intermediate values). A build unit
is one point at the Cheap tier. All figures are estimates; they are re-based after the two go/no-go gates.

## 1. Totals

| | Points | Build units | Share of points | Share of build cost |
|---|---:|---:|---:|---:|
| Strong | 106 | 530 | 58% | 77% |
| Mid | 51 | 127.5 | 28% | 19% |
| Cheap | 26.5 | 26.5 | 14% | 4% |
| **v1 parity** | **183.5** | **684** | | |

Design and review overhead: 1.5 units per point, 275.25 units (40% of build cost). **Total: 959.25 units.**
Routing every lot to the Strong tier would cost 917.5 build units; this routing saves 25%.

Against the study's v1 parity base (169 points; its 180 included the conditional L17b and the
post-parity L18), the plan adds 14.5 points: memory module (+2), contract additions (+1), observability
lot (+2), flag-coverage hook and tables, trace events and coverage instrumentation (+5.5), harness and
cases (+1.5), security lot (+1.5), browser lot (+0.5), `yq` hits (+0.5).

## 2. Model pool and routing rules

| Tier | Factor | Builder | Fallback | Used for |
|---|---:|---|---|---|
| Cheap | x1 | Haiku 5.5 | Engram Muse 1.3 once its quota is back (until then, a second Haiku 5.5 lane) | Leaf commands, leaf builtins, parity cases, tier C facades |
| Mid | x2.5 | Codex Sol 6.1 at medium effort (never high: that quota is exhausted) | Sonnet 5.5 | Bounded modules with a complete spec: VFS, leaf kernels, `sed`, hard commands, formats, bench, observability |
| Strong | x5 | Sonnet 5.5 | Gemini 4 Argon | Contract, memory, VM, parser, expansion, semantics, regex, `awk`, `jq`, YAML |
| Strong, security | x5 | Gemini 4 Argon | Sonnet 5.5 | Host adapters, network, archives, session drivers, security lot |

- The design lane (Fable 5.2) writes every lot spec and the oracle adapter; the review lane (Carbon)
  reviews; neither builds. Their cost is the overhead line.
- Escalation: a Mid lot that fails its exit gates twice moves to its fallback; a Mid lot on the critical
  path (L6a) moves on the first failure. A Strong lot is never downgraded mid-way. A Cheap PR that fails
  review twice is redone at the Mid tier and the family's sampling returns to full review.
- A lot is delivered as reviewable PRs of at most about 1 500 changed lines; large lots are split by the
  builder along the sub-items of their spec.

## 3. Waves

At most four build lanes run in parallel on the machine. Each wave holds at most four lots and exactly one
lot of the critical path (first in the list) until W6.

| Wave | Lots (critical lot first) | Lanes | Points | Build | Overhead | Wave total | Cumulative |
|---|---|---:|---:|---:|---:|---:|---:|
| W0 | L0 | 1 | 3 | 15 | 4.5 | 19.5 | 19.5 |
| W1 | L1a, L1b, L12b | 3 | 14 | 65 | 21 | 86 | 105.5 |
| W2 | L2, L7b, L16b | 3 | 14.5 | 72.5 | 21.75 | 94.25 | 199.75 |
| W3 | L3, L8a, L7a, L8b | 4 | 22 | 95 | 33 | 128 | 327.75 |
| W4 | L4, L10a, L12, L1c | 4 | 17 | 80 | 25.5 | 105.5 | 433.25 |
| W5 | L5, L9a-1, L13, L1d | 4 | 23.5 | 80 | 35.25 | 115.25 | 548.5 |
| W6 | L6a, L9b, L14, L15a | 4 | 28.5 | 101 | 42.75 | 143.75 | 692.25 |
| W7 | L9a-2, L6b, L10b, L16a-1 | 4 | 22 | 49.5 | 33 | 82.5 | 774.75 |
| W8 | L15b-1, L15b-2, L15b-3, L16a-2 | 4 | 16.5 | 55 | 24.75 | 79.75 | 854.5 |
| W9 | L16c, L16d, L19, L11 | 4 | 19.5 | 68 | 29.25 | 97.25 | 951.75 |
| W10 | L17a | 1 | 3 | 3 | 4.5 | 7.5 | 959.25 |

Wave rules:
- A wave starts when its critical lot's dependencies are merged. A non-critical lot may be pulled forward
  into a free lane as soon as its own dependencies are merged and its spec is delivered; it never delays
  the critical lot of the current wave.
- L12b runs in a reference-reading lane and does not count against a build lane's worktree rules.
- L10b is continuous: cases are written as specs land (from W4); its bulk is scheduled in W7.
- The parity gate sits between W9 and W10: parity report with every tier A/B row of EVOL Annex D covered,
  security lot closed, benchmark gate green. The pre-release (`0.x`, `next`) follows W10.

## 4. Critical path

`L0 (3) -> L1a (6) -> L1b (6) -> L2 (8.5) -> L3 (8) -> L4 (8.5) -> L5 (8.5) -> L6a (3)`: 51.5 points
if L1a and L1b run one after the other. They are scheduled in parallel in W1 (L1b builds against the
allocator interface fixed in the L1a spec), which brings the path to 45.5 points. L4 also waits for L8a
(8 points, parallel to L3). Everything else hangs off L2 or later and has slack.

## 5. Go/no-go gates

| Gate | After | Pass condition | On failure |
|---|---|---|---|
| Gate L0 | L0 | A `no_std` build for `wasm32-wasip2` links without `wasi-libc`; the component has zero imports (G1); the generated glue has no bare import (G2); two builds are identical (G3); the Annex A WIT validates | Work stops. The finding returns to the owner with the evidence, because the only known alternative conflicts with decision A1 (EVOL D24). WASI imports are not stubbed. No further lot starts |
| Gate L2 | L2 | Measured against EVOL Annex H: Node cold start, core module size, glue size, RSS delta, boundary cost; `wasm-opt` gain measured | Budgets within target: they become gates (section 7). A budget missed: the numbers go to the owner, who owns the targets, with options (accept a new target, `slim` build, minified glue, scope change); W3 lots that do not depend on the answer (L8a, L7a, L8b) proceed, L3 waits for it. `wasm-opt` is kept only with a gain of at least 10%. The JS host target stays 60 KB unless the owner accepts the split |
| Freeze | L1c | Composition and `wasmtime` run pass; boundary costs reported | The contract is amended as a draft and L1c is rerun before the freeze commit; lots that do not touch the contract continue |

## 6. Clean-room handoff

1. For each lot the design lane (Room 1) writes `spec/lots/<lot>.md`: behaviour tables (options, exit
   codes, messages, edge cases), the coverage flag table where relevant, the list of oracle cases to
   record, and the exit gates of this plan. A lot spec carries no unresolved reference path and no
   reference code (EVOL Annex F).
2. Carbon reviews the lot spec before it is handed over; a spec that leaks reference structure is
   rejected at this point.
3. The spec is merged to `main`. The conductor then creates the build worktree `tmp/<slug>` from `main`.
   The lane bootstrap checks that the worktree contains neither `tmp/refs/` nor the oracle package, and the
   lot brief repeats the rule.
4. The build lane works from the lot spec, EVOL, committed snapshots, and public POSIX or tool
   documentation. Questions go back to the design lane as spec issues; answers are spec amendments, never
   pointers into the reference.
5. Oracle snapshots are recorded by the oracle job (Room 1) with the recorder built in L10a and committed
   under `test/snapshots/`; a build lane that needs a new case writes the case and asks for a recording.
6. A lane does not change rooms within a lot.

## 7. Benchmark protocol (summary of EVOL Annex H)

- Engines: `@sentropic/bash` and `just-bash@3.6.0` only; MoonBash appears in the feature matrix only.
- Node built-ins only; 30 fresh interleaved processes for cold metrics against an empty-ESM baseline; 20
  discarded warm-up iterations then at least 200 iterations for warm metrics; interpolated median and
  nearest-rank p95; `LC_ALL=C`, `TZ=UTC`; a workload is timed only if our output equals its snapshot.
- Targets (owner-owned, confirmed at gate L2): cold start <= 25 ms; `new Bash()` <= 0.20 ms; `echo hi`
  <= 0.07 ms; 1 000-iteration loop <= 1.5 ms; `grep`/`sed`/`awk`/`sort` on 10k lines <= 2/8/8/4 ms; RSS
  delta <= 10 MB; core module <= 1.5 MB raw and <= 700 KB gzip; JS host <= 60 KB.
- Noise floor: a metric fails only if the regression exceeds both `max(10%, floor)` (0.02 ms for
  sub-millisecond metrics, 0.5 ms otherwise, 1 MB for memory) and the baseline's p95 spread.
- Activation: the harness exists from L12 (W4). The regression gate is switched on in `make check` when
  the gate L2 targets are confirmed and the first baseline is committed; each command lot adds its M4 workload
  to the baseline when it merges.

## 8. Lots

Every lot also satisfies: specs and snapshots only (section 6), no dependency added, no Python, no
attribution in commits, English, and `make check` green on a clean worktree.

### L0 — Component pipeline
- **Scope.** Toolchain pin (`rust-toolchain.toml`, `wasm32-wasip2`), tool pins with hashes under `tmp/tools/`, `tools/` manifest and lockfile for `jco-transpile` and `typescript`, build script, `make build` and `make test` wiring (Makefile scope exception), an empty `no_std` kernel exporting one function through the WIT, gates G1-G8 of EVOL Annex C. Validates the Annex A WIT with `wasm-tools`. This lot is go/no-go point 1 (EVOL D24).
- **Room.** Room 2 (build). **Wave.** W0. **Size.** 3 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** A1 (EVOL D2, D16, D17).
- **Entry.** Lot spec delivered by the design lane (section 6); EVOL v1.3 approved; scope exception for `Makefile` and `.gitignore` declared.
- **Exit gates.** G1-G8 pass on a clean worktree; `wasm-tools component wit` accepts Annex A; a `node:test` case instantiates the generated glue with an empty import object and calls the export; written go/no-go report reviewed. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l0-pipeline` (worktree `tmp/l0-pipeline`).

### L1a — Kernel memory module
- **Scope.** `kernel::mem` (EVOL D6): free-list allocator over `memory.grow` with ceiling, `configure` / `raise-ceiling`, budget accounting, the fallible containers (vector, string, map, box, shared pointer), `cabi_realloc` on the headroom, unwinding reserve; G6 import gate.
- **Room.** Room 2 (build). **Wave.** W1. **Size.** 6 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 30 units + overhead 9 = 39 units.
- **Depends on.** L0.
- **Entry.** Lot spec delivered by the design lane (section 6); L0 report accepted.
- **Exit gates.** Native `cargo test`: stress of 10 000 alloc/free cycles with high-water <= 1.25x peak live bytes and full return on drop-all; every container returns an error at budget and at ceiling; G6 passes; memory grows mid-call without stale views (`node:test`). `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l1a-kernel-mem` (worktree `tmp/l1a-kernel-mem`).

### L1b — WIT resources and JS facade
- **Scope.** Kernel skeleton for the full Annex A contract (`vfs`, `shell`, `fiber`, epochs, chunked calls, `cancel`), generated bindings (D16), JS facade over the glue (D14: `Bash`, `InMemoryFs`, command context), capability membrane (D9), Node and browser loaders with the staging queue (D15), result contract of Annex D.2 for the cases reachable without a parser.
- **Room.** Room 2 (build). **Wave.** W1. **Size.** 6 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 30 units + overhead 9 = 39 units.
- **Depends on.** L0; allocator interface of L1a (fixed in its spec).
- **Entry.** Lot spec delivered by the design lane (section 6); L0 report accepted; L1a container interface published.
- **Exit gates.** `node:test`: stale `resume` returns `stale-epoch`; denied host calls never reach a recording driver; late continuation after abort cannot resume; two shells on one `InMemoryFs` are isolated (S14); regeneration check of the bindings; `tsc --noEmit` on facade declarations. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l1b-wit-facade` (worktree `tmp/l1b-wit-facade`).

### L12b — Feature matrix
- **Scope.** `docs/FEATURE_MATRIX.md`: commands, options and API of just-bash 3.6.0, MoonBash 0.1.0 and `@sentropic/bash` (target and status columns). Reference-reading, documentation-only lane (EVOL Annex F.1).
- **Room.** Room 1 (reference-reading, documentation only). **Wave.** W1. **Size.** 2 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 5 units + overhead 3 = 8 units.
- **Depends on.** O6, O7.
- **Entry.** Lot spec delivered by the design lane (section 6); Room 1 worktree with `tmp/refs/`.
- **Exit gates.** Matrix covers the 85 + 5 command names and the API tiers of Annex D.1; no code copied; reviewed against the study inventory. Refreshed at the parity gate. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `docs/l12b-feature-matrix` (worktree `tmp/l12b-feature-matrix`).

### L2 — Vertical slice and measurements
- **Scope.** Lexer and parser subset, resumable VM (D4), chunked buffers, VFS core with default layout (D5), command trait with one file per command and a generated registry, the generic flag-coverage hook over per-command static tables, `echo`. Measurements of EVOL D24: core module size with the Cargo profile, `wasm-opt` gain, component size, glue size and init, Node cold start, boundary cost. Go/no-go point 2.
- **Room.** Room 2 (build). **Wave.** W2. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L1a, L1b.
- **Entry.** Lot spec delivered by the design lane (section 6); L1a and L1b merged.
- **Exit gates.** Oracle snapshots for `echo`, variables and pipelines pass; recursion depth 10 000 gives a limit result; limit tests for the byte caps of the default profile; measurement report with the numbers against EVOL Annex H; decision record for `wasm-opt` and the `slim` build. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l2-vertical-slice` (worktree `tmp/l2-vertical-slice`).

### L7b — Host filesystem adapters
- **Scope.** `OverlayFs`, `ReadWriteFs` and custom `IFileSystem` bridging on Node (EVOL D10, Annex D.3), batched `fs-batch`, `mount-host`.
- **Room.** Room 2 (build). **Wave.** W2. **Size.** 3 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** L1b.
- **Entry.** Lot spec delivered by the design lane (section 6); L1b merged.
- **Exit gates.** Adversarial suite (traversal, symlink, special files, exclusive-create race, error-message leakage) in `node:test`; a 1 000-file listing costs at most one host round trip per directory; S7 tests. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l7b-host-fs` (worktree `tmp/l7b-host-fs`).

### L16b — Network adapter and `curl`
- **Scope.** Lazily imported network adapter inside `@sentropic/bash` (EVOL D11, Annex D.4): synchronous `NetworkConfig` validation, allow-lists, hop-by-hop redirects with transforms, DNS validation and pinning, typed errors; kernel `curl` with its exit-code map from the lot spec.
- **Room.** Room 2 (build). **Wave.** W2. **Size.** 3 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** L1b.
- **Entry.** Lot spec delivered by the design lane (section 6); L1b merged; `curl` lot spec with the exit-code table.
- **Exit gates.** Local-server tests: denied prefix, redirect to a denied prefix, rebinding between check and connect, oversized body, method not allowed; `curl` oracle snapshots recorded against a local server; S8 tests. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l16b-net-curl` (worktree `tmp/l16b-net-curl`).

### L3 — Full lexer and parser
- **Scope.** Complete grammar (study 1.4): compound commands, `[[ ]]`, `$(( ))`, here-documents, ANSI-C quoting, `normalizeScript`, parse errors with the reference messages, the two script-size checks of EVOL D1.
- **Room.** Room 2 (build). **Wave.** W3. **Size.** 8 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 40 units + overhead 12 = 52 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 go decision.
- **Exit gates.** Parser snapshot set (parse-error messages and exit 2) passes; seeded parser fuzz slice in `make check` with no trap; parser limits (tokens, depth) give exit 2 or 126 as specified. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l3-parser` (worktree `tmp/l3-parser`).

### L8a — Regex and glob engines
- **Scope.** Linear-time regex engine with compile caps and bounded backreferences, glob and extglob matcher (EVOL D12).
- **Room.** Room 2 (build). **Wave.** W3. **Size.** 8 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 40 units + overhead 12 = 52 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 go decision.
- **Exit gates.** Native unit tests; blow-up patterns for matching and compilation end within fuel; pattern snapshots (BRE, ERE, classes) pass; S9 tests. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l8a-regex-glob` (worktree `tmp/l8a-regex-glob`).

### L7a — VFS completion
- **Scope.** `bash.fs` and `InMemoryFs` complete over `vfs.op` (Annex D.3), `MountableFs` with `mount-vfs`, lazy files, hard links and stat identity, `take-dirty` journal and `bash.compact()` (EVOL D5, D8).
- **Room.** Room 2 (build). **Wave.** W3. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 4.5 = 12 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 merged.
- **Exit gates.** `IFileSystem` contract tests per method of Annex D.3; journal rebuild and compaction tests; lazy-file materialization through chunks. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l7a-vfs` (worktree `tmp/l7a-vfs`).

### L8b — Shared leaf kernels
- **Scope.** printf and strftime engine, number parsing, JavaScript-compatible number formatting (EVOL D1), md5/sha1/sha256, base64.
- **Room.** Room 2 (build). **Wave.** W3. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 4.5 = 12 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 merged.
- **Exit gates.** Number formatter matches the oracle on a generated set of doubles; hash and base64 vectors; printf snapshots. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l8b-leaf-kernels` (worktree `tmp/l8b-leaf-kernels`).

### L4 — Expansion engine
- **Scope.** Parameter operations including `${var@op}`, command, arithmetic and process substitution, brace, tilde, glob/extglob/globstar, word splitting, reference arithmetic (EVOL D1), `bash:expansion:*` coverage hits (Annex D.7).
- **Room.** Room 2 (build). **Wave.** W4. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L3, L8a.
- **Entry.** Lot spec delivered by the design lane (section 6); L3 and L8a merged.
- **Exit gates.** Expansion and arithmetic snapshot sets pass (values beyond 2^53, 32-bit shifts); expansion limits give exit 126; coverage hit sequences for expansions match snapshots. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l4-expansion` (worktree `tmp/l4-expansion`).

### L10a — Parity harness
- **Scope.** Engine-agnostic replay runner and recorder with the engine-adapter interface, snapshot format including trace and coverage, API tier A/B test skeleton (EVOL Annex E). The just-bash adapter itself is written by the oracle job (Room 1).
- **Room.** Room 2 (build). **Wave.** W4. **Size.** 3.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 17.5 units + overhead 5.25 = 22.75 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 merged.
- **Exit gates.** Recorder self-test on our engine; `make check` replays committed snapshots with no dependency and no network; the oracle job reproduces a committed snapshot byte for byte. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l10a-parity-harness` (worktree `tmp/l10a-parity-harness`).

### L12 — Benchmark harness
- **Scope.** `bench/run.mjs` per EVOL Annex H: engines `@sentropic/bash` and just-bash, cold and warm protocols, statistics, baseline file, noise-floored regression gate.
- **Room.** Room 2 (build). **Wave.** W4. **Size.** 2 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 5 units + overhead 3 = 8 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 measurement report.
- **Exit gates.** `make bench` produces the table and JSON; gate unit tests on synthetic series; baseline committed. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l12-bench` (worktree `tmp/l12-bench`).

### L1c — Session drivers and composition
- **Scope.** Session driver components (`session-pure`, `session-fs`, `session-net`) and their WIT, `wac` composition, `wasmtime` smoke test under a 256 MiB limiter and an epoch deadline, `exec-oneshot` (EVOL D15). Its report triggers the WIT freeze (D3).
- **Room.** Room 2 (build). **Wave.** W4. **Size.** 3 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** L2, O4.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 go decision; `wac` and `wasmtime` pinned.
- **Exit gates.** Composed component runs a snapshot subset under `wasmtime`; import lists equal the driver worlds; `exec-oneshot` round trip; composed-component measurements reported; freeze commit of `sentropic:bash@0.1.0`. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l1c-session-wac` (worktree `tmp/l1c-session-wac`).

### L5 — Interpreter semantics
- **Scope.** Redirections and fd table, functions, subshells, `set`/`shopt`, errexit, traps subset, arrays, namerefs, special-builtin precedence, builtin dispatch with `bash:cmd:*` and `bash:builtin:*` hits.
- **Room.** Room 2 (build). **Wave.** W5. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L4.
- **Entry.** Lot spec delivered by the design lane (section 6); L4 merged.
- **Exit gates.** Semantics snapshot sets per area pass; two overlapping execs and a re-entrant `ctx.exec` test (D4); coverage hit sequences match snapshots. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l5-interpreter` (worktree `tmp/l5-interpreter`).

### L9a-1 — Hard stream commands
- **Scope.** `grep`/`egrep`/`fgrep`, `sort`, `tr`, `expr`, with their coverage flag tables.
- **Room.** Room 2 (build). **Wave.** W5. **Size.** 4.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 11.25 units + overhead 6.75 = 18 units.
- **Depends on.** L2, L8a.
- **Entry.** Lot spec delivered by the design lane (section 6); L8a merged; command specs with flag tables.
- **Exit gates.** Per-command oracle snapshots pass; `cmd:flag` hits match; M4 microbench recorded. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l9a1-stream-commands` (worktree `tmp/l9a1-stream-commands`).

### L13 — `sed`
- **Scope.** Parser, addresses, pattern and hold space cycle, branches with fuel, `-i`, `sed:cmd:*` hits and flag table.
- **Room.** Room 2 (build). **Wave.** W5. **Size.** 8.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 21.25 units + overhead 12.75 = 34 units.
- **Depends on.** L8a.
- **Entry.** Lot spec delivered by the design lane (section 6); L8a merged; `sed` spec.
- **Exit gates.** `sed` snapshot set passes; branch loops end in a limit result; coverage sequences match. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l13-sed` (worktree `tmp/l13-sed`).

### L1d — Observability
- **Scope.** `logger`, trace and coverage buffers, `take-trace`, `take-coverage`, `coverage-features`, drain order (on `out-of-fuel`, before host callbacks, on `cancel`), facade delivery, browser filter for `cmd:flag` (EVOL Annex A rules, D.1, D.6, D.7).
- **Room.** Room 2 (build). **Wave.** W5. **Size.** 2 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 5 units + overhead 3 = 8 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 merged.
- **Exit gates.** `node:test`: ordered delivery before `exec` resolves; drain before a custom command callback; no logger call on an empty script; a script cannot observe the trace clock (S11). `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l1d-observability` (worktree `tmp/l1d-observability`).

### L6a — State-coupled builtins
- **Scope.** `declare`/`typeset`, `local`, `readonly`, `export`, `unset`, `eval`, `source`, `exec`, `command`, `builtin`, `read`, `mapfile`.
- **Room.** Room 2 (build). **Wave.** W6. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 4.5 = 12 units.
- **Depends on.** L5.
- **Entry.** Lot spec delivered by the design lane (section 6); L5 merged; builtin specs.
- **Exit gates.** Per-builtin snapshots pass. Escalates to the fallback on the first failed gate (critical path). `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l6a-builtins-state` (worktree `tmp/l6a-builtins-state`).

### L9b — Leaf tier-1 commands
- **Scope.** About 44 leaf commands of tier 1 (study 3.1), one file each, with flag tables. Split into PRs of at most six commands.
- **Room.** Room 2 (build). **Wave.** W6. **Size.** 8.5 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 8.5 units + overhead 12.75 = 21.25 units.
- **Depends on.** L2, L7a, L8b.
- **Entry.** Lot spec delivered by the design lane (section 6); L7a and L8b merged; one spec per command.
- **Exit gates.** Per-command snapshots and flag hits pass; one command in five reviewed in full after the first three PRs pass unchanged. `make check` green.
- **Review.** Carbon samples one PR in five after the first three pass unchanged; full review of the first three.
- **Branch.** `feat/l9b-leaf-commands` (worktree `tmp/l9b-leaf-commands`).

### L14 — `awk`
- **Scope.** Lexer, parser, VM, built-in functions, `awk:stmt:*` and `awk:expr:*` hits, flag table. Built-in functions may be delegated to the Cheap tier from the spec.
- **Room.** Room 2 (build). **Wave.** W6. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L8a, L8b.
- **Entry.** Lot spec delivered by the design lane (section 6); L8a and L8b merged; `awk` spec.
- **Exit gates.** `awk` snapshot set passes; iteration limits give exit 126; M4 budget for `awk` measured. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l14-awk` (worktree `tmp/l14-awk`).

### L15a — `jq`
- **Scope.** JSON parser, shared value model (first PR), generator evaluator, builtins, `jq:node:*` hits, flag table.
- **Room.** Room 2 (build). **Wave.** W6. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L8a.
- **Entry.** Lot spec delivered by the design lane (section 6); L8a merged; `jq` spec.
- **Exit gates.** `jq` snapshot set passes; value model published for L15b-2 and L15b-3; query limits give exit 126. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l15a-jq` (worktree `tmp/l15a-jq`).

### L9a-2 — Re-entrant commands
- **Scope.** `find` with `-exec` and its trace events (Annex D.6, batch size 500), `xargs`, `bash`/`sh`, `timeout`, `time`, with flag tables.
- **Room.** Room 2 (build). **Wave.** W7. **Size.** 5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 12.5 units + overhead 7.5 = 20 units.
- **Depends on.** L5.
- **Entry.** Lot spec delivered by the design lane (section 6); L5 merged; command specs.
- **Exit gates.** Snapshots pass; `find` trace names, keys and counters match snapshots; nested execs charge the parent budget. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l9a2-reentrant-commands` (worktree `tmp/l9a2-reentrant-commands`).

### L6b — Leaf builtins
- **Scope.** `:`, `true`, `false`, `cd`, `pwd`, `shift`, `dirs`/`pushd`/`popd`, `getopts`, `let`, `hash`, `help`, `type`, `test`/`[`, `wait`, `compgen`/`complete`/`compopt`.
- **Room.** Room 2 (build). **Wave.** W7. **Size.** 3 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 3 units + overhead 4.5 = 7.5 units.
- **Depends on.** L5.
- **Entry.** Lot spec delivered by the design lane (section 6); L5 merged; builtin specs.
- **Exit gates.** Per-builtin snapshots pass. `make check` green.
- **Review.** Carbon samples one PR in five after the first three pass unchanged; full review of the first three.
- **Branch.** `feat/l6b-builtins-leaf` (worktree `tmp/l6b-builtins-leaf`).

### L10b — Parity cases
- **Scope.** Our own cases per grammar area and per command, written from the specs, including trace and coverage cases; snapshots recorded by the oracle job.
- **Room.** Room 2 (build). **Wave.** W7. **Size.** 9 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 9 units + overhead 13.5 = 22.5 units.
- **Depends on.** L10a; continuous from wave 4.
- **Entry.** Lot spec delivered by the design lane (section 6); L10a merged; specs available.
- **Exit gates.** Parity report per area and command (cases, pass, fail, not covered); no case derived from a reference test file. `make check` green.
- **Review.** Carbon samples one PR in five after the first three pass unchanged; full review of the first three.
- **Branch.** `feat/l10b-parity-cases` (worktree `tmp/l10b-parity-cases`).

### L16a-1 — Compression and archives
- **Scope.** inflate/deflate, `gzip`/`gunzip`/`zcat`, `tar`, with archive limits and extraction containment (EVOL S15).
- **Room.** Room 2 (build). **Wave.** W7. **Size.** 5 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 25 units + overhead 7.5 = 32.5 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 merged; codec and `tar` specs.
- **Exit gates.** Round-trip and oracle snapshots; archive-bomb and symlink-parent tests end in limit or error results. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l16a1-archive` (worktree `tmp/l16a1-archive`).

### L15b-1 — `yq` command and YAML
- **Scope.** `yq` command (flags, in-place, front matter, slurp), YAML reader/writer with alias and depth caps, JSON wiring, `jq:node:*` hits from `yq`.
- **Room.** Room 2 (build). **Wave.** W8. **Size.** 5.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 27.5 units + overhead 8.25 = 35.75 units.
- **Depends on.** L15a.
- **Entry.** Lot spec delivered by the design lane (section 6); L15a merged; `yq` and YAML specs.
- **Exit gates.** YAML read and write snapshots; alias-bomb test ends in a limit result. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l15b1-yq-yaml` (worktree `tmp/l15b1-yq-yaml`).

### L15b-2 — XML and TOML for `yq`
- **Scope.** XML and TOML readers/writers on the value model; no DOCTYPE or external entity (S15).
- **Room.** Room 2 (build). **Wave.** W8. **Size.** 5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 12.5 units + overhead 7.5 = 20 units.
- **Depends on.** value model of L15a.
- **Entry.** Lot spec delivered by the design lane (section 6); Value model merged; format specs.
- **Exit gates.** Read and write snapshots per format; entity and depth adversarial tests. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l15b2-xml-toml` (worktree `tmp/l15b2-xml-toml`).

### L15b-3 — INI and CSV for `yq`
- **Scope.** INI and CSV/TSV readers/writers.
- **Room.** Room 2 (build). **Wave.** W8. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 4.5 = 12 units.
- **Depends on.** value model of L15a.
- **Entry.** Lot spec delivered by the design lane (section 6); Value model merged; format specs.
- **Exit gates.** Read and write snapshots per format; CSV row and cell limits give exit 126. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l15b3-ini-csv` (worktree `tmp/l15b3-ini-csv`).

### L16a-2 — `diff`
- **Scope.** `diff` with normal, unified and context output.
- **Room.** Room 2 (build). **Wave.** W8. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 4.5 = 12 units.
- **Depends on.** L2.
- **Entry.** Lot spec delivered by the design lane (section 6); L2 merged; `diff` spec.
- **Exit gates.** Snapshots pass. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l16a2-diff` (worktree `tmp/l16a2-diff`).

### L16c — Leaf tier-3 commands
- **Scope.** `comm`, `join`, `column`, `fold`, `expand`, `unexpand`, `split`, `strings`, `od`, `file`, `tree`, `du`.
- **Room.** Room 2 (build). **Wave.** W9. **Size.** 3 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 3 units + overhead 4.5 = 7.5 units.
- **Depends on.** L2, L7a.
- **Entry.** Lot spec delivered by the design lane (section 6); Command specs.
- **Exit gates.** Per-command snapshots pass; sampling review. `make check` green.
- **Review.** Carbon samples one PR in five after the first three pass unchanged; full review of the first three.
- **Branch.** `feat/l16c-leaf-tier3` (worktree `tmp/l16c-leaf-tier3`).

### L16d — `rg`
- **Scope.** `rg` on the grep kernel, with its flag table.
- **Room.** Room 2 (build). **Wave.** W9. **Size.** 3.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 8.75 units + overhead 5.25 = 14 units.
- **Depends on.** L9a-1.
- **Entry.** Lot spec delivered by the design lane (section 6); L9a-1 merged; `rg` spec.
- **Exit gates.** Snapshots and flag hits pass. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l16d-rg` (worktree `tmp/l16d-rg`).

### L19 — Browser entry verification
- **Scope.** Headless Chrome for Testing rig over the DevTools protocol, `precompile`, streaming compile, staging queue, trace and coverage in the browser, the commands of divergence V4, metric M1b.
- **Room.** Room 2 (build). **Wave.** W9. **Size.** 3.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 8.75 units + overhead 5.25 = 14 units.
- **Depends on.** L2, L1d, B9.
- **Entry.** Lot spec delivered by the design lane (section 6); Chrome for Testing pinned.
- **Exit gates.** Browser suite passes; G2 browser-graph check; M1b reported. Rerun at the parity gate. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l19-browser` (worktree `tmp/l19-browser`).

### L11 — Security lot
- **Scope.** Adversarial suites per invariant S1-S15 (EVOL Annex G), seeded JS and native fuzzers, mutation fuzzing on our engine, limit-latency tests (M7), the coverage-guided campaign of D21 from `tmp/fuzz/`.
- **Room.** Room 2 (build). **Wave.** W9. **Size.** 9.5 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 47.5 units + overhead 14.25 = 61.75 units.
- **Depends on.** L5, L7b, L8a, L16a-1, L16b, L15a, L15b-1..3.
- **Entry.** Lot spec delivered by the design lane (section 6); Listed lots merged.
- **Exit gates.** Invariant table with one passing suite per row; campaign report (targets, duration, corpus, findings); zero open trap; every finding added as a seeded regression test. `make check` green.
- **Review.** Carbon, full review of every PR.
- **Branch.** `feat/l11-security` (worktree `tmp/l11-security`).

### L17a — Tier C facades and release
- **Scope.** `Sandbox` facade, CLI binaries, standalone facades (`grep`, `ls`, `awk`, `jq`), package assembly and the PR-time `npm pack` test (EVOL D18).
- **Room.** Room 2 (build). **Wave.** W10. **Size.** 3 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 3 units + overhead 4.5 = 7.5 units.
- **Depends on.** parity of tiers A and B.
- **Entry.** Lot spec delivered by the design lane (section 6); Parity gate passed.
- **Exit gates.** API tests; install of the tarball in an empty project runs `exec('echo hi')`; npm and OCI digests equal at release. `make check` green.
- **Review.** Carbon samples one PR in five after the first three pass unchanged; full review of the first three.
- **Branch.** `feat/l17a-tier-c` (worktree `tmp/l17a-tier-c`).

## 9. Post-parity

Not part of the v1 totals. Each item gets its own plan entry after the parity gate.

| Item | Scope | Size (estimate) | Tier |
|---|---|---:|---|
| Companion study | Build form of the runtime companions (EVOL D25): WASI component without host-JS glue (design lane's preference) or Emscripten glue; VFS bridge; security exports; supply-chain pins | 3 | design and review lanes |
| `@sentropic/python` | `python3`/`python` on an embedded CPython component, composed on demand; own release lane; zero-dependency exception limited to this package | 8 | Strong |
| `@sentropic/js` | `js-exec`/`node` on an embedded QuickJS component; same rules | 8 | Strong |
| L17b | AST export, transform pipeline, `serialize`, only if decision B3 is reversed | 3 | Strong |
| L18 | `@sentropic/make` | 8 | Strong |

## 10. Track mapping

One Track item per lot under the root item `01M7DVAD9N3Z3GS7BKYD3DC63J`. To be created by the conductor.

| Lot | Track item title | Wave | Depends on |
|---|---|---|---|
| L0 | L0 — Component pipeline | W0 | A1 (EVOL D2, D16, D17) |
| L1a | L1a — Kernel memory module | W1 | L0 |
| L1b | L1b — WIT resources and JS facade | W1 | L0; allocator interface of L1a (fixed in its spec) |
| L12b | L12b — Feature matrix | W1 | O6, O7 |
| L2 | L2 — Vertical slice and measurements | W2 | L1a, L1b |
| L7b | L7b — Host filesystem adapters | W2 | L1b |
| L16b | L16b — Network adapter and curl | W2 | L1b |
| L3 | L3 — Full lexer and parser | W3 | L2 |
| L8a | L8a — Regex and glob engines | W3 | L2 |
| L7a | L7a — VFS completion | W3 | L2 |
| L8b | L8b — Shared leaf kernels | W3 | L2 |
| L4 | L4 — Expansion engine | W4 | L3, L8a |
| L10a | L10a — Parity harness | W4 | L2 |
| L12 | L12 — Benchmark harness | W4 | L2 |
| L1c | L1c — Session drivers and composition | W4 | L2, O4 |
| L5 | L5 — Interpreter semantics | W5 | L4 |
| L9a-1 | L9a-1 — Hard stream commands | W5 | L2, L8a |
| L13 | L13 — sed | W5 | L8a |
| L1d | L1d — Observability | W5 | L2 |
| L6a | L6a — State-coupled builtins | W6 | L5 |
| L9b | L9b — Leaf tier-1 commands | W6 | L2, L7a, L8b |
| L14 | L14 — awk | W6 | L8a, L8b |
| L15a | L15a — jq | W6 | L8a |
| L9a-2 | L9a-2 — Re-entrant commands | W7 | L5 |
| L6b | L6b — Leaf builtins | W7 | L5 |
| L10b | L10b — Parity cases | W7 | L10a; continuous from wave 4 |
| L16a-1 | L16a-1 — Compression and archives | W7 | L2 |
| L15b-1 | L15b-1 — yq command and YAML | W8 | L15a |
| L15b-2 | L15b-2 — XML and TOML for yq | W8 | value model of L15a |
| L15b-3 | L15b-3 — INI and CSV for yq | W8 | value model of L15a |
| L16a-2 | L16a-2 — diff | W8 | L2 |
| L16c | L16c — Leaf tier-3 commands | W9 | L2, L7a |
| L16d | L16d — rg | W9 | L9a-1 |
| L19 | L19 — Browser entry verification | W9 | L2, L1d, B9 |
| L11 | L11 — Security lot | W9 | L5, L7b, L8a, L16a-1, L16b, L15a, L15b-1..3 |
| L17a | L17a — Tier C facades and release | W10 | parity of tiers A and B |

## 11. Assumptions to re-check

- Tier factors (1 / 2.5 / 5) and the overhead rate are estimates; actual spend per lot is recorded in Track
  and the plan is re-based after gates L0 and L2.
- The overhead is allocated per point, not per build unit, because spec and review effort follows scope:
  on Cheap lots it exceeds the build cost.
- Headroom, glue size and cold start are unmeasured until L2; the WIT is unvalidated until L0.
