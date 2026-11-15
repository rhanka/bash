# PLAN — @sentropic/bash v1 (rung 3: lot plan, v1.2)

Status: executable plan for v1 parity, derived from `spec/SPEC_EVOL_BASH_SANDBOX.md` v1.3 ("EVOL") and
`spec/SPEC_STUDY_BASH_SANDBOX.md` v3.2 ("the study"). Decisions are in EVOL; this document schedules and
prices the work. v1.1 reconciles the adversarial review of v1 (section 13). Post-parity work is section 9.
v1.2 applies the owner answers of 15/11 on names and host disk access (EVOL D26, section 12).

Units: a point is the size unit of the study (S = 1, M = 3, L = 8, with intermediate values). A build unit
is one point at the Cheap tier. All figures are estimates; they are re-based after the two go/no-go gates.

## 1. Totals

| | Points | Build units | Share of points | Share of build cost |
|---|---:|---:|---:|---:|
| Strong | 96.5 | 482.5 | 52% | 73% |
| Mid | 60.5 | 151.25 | 32% | 23% |
| Cheap | 30 | 30 | 16% | 5% |
| **v1 parity** | **187** | **663.75** | | |

Design and review overhead (tier-weighted, section 2): 223.9 units (34% of build cost). **Total: 887.65 units.**
Routing every lot to the Strong tier would cost 935 build units; this routing saves 29%.

Against v1 of this plan (183.5 points, 684 build units, 959.25 in total): the lot list gains the package
skeleton in L0, the reference's minimal builtins and three hash commands, and loses the standalone facades
(moved to section 9); nine lots are re-routed (section 13); overhead is weighted by tier instead of flat.
v1.2 adds 2.5 points for EVOL D26: L17a +1.5 (single binary with both modes and the disk flags), L19 +0.5
(page-supplied filesystem), L1c +0.5 (granted-directory mount tests).

## 2. Model pool, routing and overhead

| Tier | Factor | Builder | Fallback | Overhead per point | Used for |
|---|---:|---|---|---:|---|
| Cheap | x1 | Haiku 5.5 | Engram Muse 1.3 once its quota is back (until then, Haiku 5.5) | 0.75 (0.4 for L10b and L12b) | Batched leaf commands and builtins, function libraries, simple formats, parity cases, the feature matrix |
| Mid | x2.5 | Codex Sol 6.1 at medium effort (never high: that quota is exhausted) | Sonnet 5.5 | 1.0 | Bounded modules with a complete spec: harness, VFS, leaf kernels, `sed`, hard commands, formats, bench, observability, fuzz harnesses, tier C |
| Strong | x5 | Sonnet 5.5 | Gemini 4 Argon | 1.5 | Contract, memory, VM, parser, expansion, semantics, state-coupled builtins, regex, `awk` and `jq` cores, YAML |
| Strong, security | x5 | Gemini 4 Argon | Sonnet 5.5 | 1.5 | Host adapters, network, archives, session drivers, security suites |

- The design lane (Fable 5.2) writes every lot spec and the oracle adapter; the review lane (Carbon)
  reviews; neither builds. Their cost is the overhead column. Carbon is dispatched on a PR only when
  `make check` is green, so that review units are not spent on work that fails its own gates.
- Escalation: a Mid lot that fails its exit gates twice moves to its fallback. A Strong lot is never
  downgraded mid-way. A Cheap PR that fails review twice is redone at the Mid tier and the family's sampling
  returns to full review.
- Overhead is re-based on the actual spend recorded in Track after the first batch of leaf commands; if
  leaf specs cost more than 1.0 unit per point, the Cheap rate is raised in the next plan revision.
- A lot is delivered as reviewable PRs of at most about 1 500 changed lines.

## 3. Waves

The machine runs at most **four agent lanes in total**, counting the design lane, the review lane and both
rooms. With the design lane writing the next specs and the review lane reviewing the previous PRs, a wave
therefore has **two slots** for building lots (Room 2) or reference-reading lots (Room 1). A slot runs its
lots one after the other. Slot A carries the critical path until W7.

| Wave | Slot A (critical lot first) | Slot B | Points | Build | Overhead | Wave total | Cumulative |
|---|---|---|---:|---:|---:|---:|---:|
| W0 | L0 | — | 3.5 | 17.5 | 5.25 | 22.75 | 22.75 |
| W1 | L1a-1 | L10a | 5.5 | 18.75 | 6.5 | 25.25 | 48 |
| W2 | L1b | L1a-2 | 10 | 50 | 15 | 65 | 113 |
| W3 | L2 | L7b | 11.5 | 57.5 | 17.25 | 74.75 | 187.75 |
| W4 | L3 | L8a | 16 | 80 | 24 | 104 | 291.75 |
| W5 | L4 | L8b then L1d | 13.5 | 55 | 17.75 | 72.75 | 364.5 |
| W6 | L5 | L7a then L12 then L1c | 17 | 72.5 | 23 | 95.5 | 460 |
| W7 | L6a | L16b | 6 | 30 | 9 | 39 | 499 |
| W8 | L13 | L9a-1 then L16d | 16.5 | 41.25 | 16.5 | 57.75 | 556.75 |
| W9 | L14a then L14b | L15a-1 then L15a-2 | 17 | 67.5 | 22.13 | 89.63 | 646.38 |
| W10 | L9b | L9a-2 then L6b-1 then L6b-2 | 17.5 | 28.75 | 15 | 43.75 | 690.13 |
| W11 | L15b-1 then L15b-3 | L16a-1 then L15b-2 | 18.5 | 68 | 23 | 91 | 781.13 |
| W12 | L10b | L16a-2 then L16c then L12b | 17.5 | 22 | 10.03 | 32.03 | 813.15 |
| W13 | L11a then L19 | L11b | 13.5 | 46.25 | 16 | 62.25 | 875.4 |
| W14 | L17a | — | 3.5 | 8.75 | 3.5 | 12.25 | 887.65 |

Wave rules:
- `active lanes <= 4` at all times. A third build lane may be opened only while the design lane or the
  review lane is idle (specs for the next two waves merged, or no PR waiting for review), and is closed
  when that lane resumes.
- A lot starts only when its dependencies are merged, its spec is merged and its snapshots are committed
  (section 6). "exit:" dependencies must be merged before the lot's final PR, not before it starts.
- A non-critical lot may be pulled forward into a free slot under the same conditions; it never delays
  the critical lot of the current wave.
- Parity gate between W13 and W14: parity report with 0 failing case outside the divergence list and every
  tier A/B row of EVOL Annex D covered, invariant table S1-S15 closed, benchmark gate green, feature matrix
  refreshed. The pre-release (`0.x`, tag `next`) is published at this gate, before tier C.

## 4. Critical path

`L0 (3.5) -> L1a-1 (2) -> L1b (6) -> L2 (8.5) -> L3 (8) -> L4 (8.5) -> L5 (8.5) -> L6a (3)`: 48 points.
L1a-2 (4) runs beside L1b and must be merged before L2; L8a (8) runs beside L3 and must be merged before
L4; L8b and L1d run beside L4 and must be merged before its final PR. Every lot of the path is at the
Strong tier. Everything else hangs off L2 or later and has slack.

## 5. Go/no-go gates

| Gate | After | Pass condition | On failure |
|---|---|---|---|
| Gate L0 | L0 | A `no_std` build for `wasm32-wasip2` links without `wasi-libc`; the component has zero imports (G1); the generated glue has no bare import (G2); two builds are identical (G3); the Annex A WIT validates | Work stops and the finding returns to the owner with the evidence, because the only known alternative conflicts with decision A1 (EVOL D24). WASI imports are not stubbed. Not a failure of this gate: generated bindings that cannot be made `no_std`; the fallback is hand-written glue (EVOL D16), decided inside L0 |
| Gate L2 | L2 | Measured against EVOL Annex H: Node cold start, core module size, RSS delta, boundary cost (kernel part); glue and facade size (JS part); `wasm-opt` gain | **Kernel part missed** (cold start, RSS, core size): every lot of W4 and later is paused; the numbers go to the owner, who owns the targets, with options (new target, `slim` build, scope change); only spec writing continues. **JS part only missed** (host above 60 KB): kernel lots continue; the split budget and the minified-glue fallback are put to the owner with the numbers. `wasm-opt` is kept only with a gain of at least 10% |
| Freeze | L1c | Composition and `wasmtime` run pass; boundary costs reported | The contract is amended as a draft and L1c is rerun before the freeze commit; lots that do not touch the contract continue |
| Parity gate | W13 | Section 3 | Failing areas go back to their lots; no pre-release |

## 6. Clean-room handoff

1. For each lot the design lane (Room 1) writes `spec/lots/<lot>.md`: behaviour tables (options, exit
   codes, messages, edge cases), the coverage flag table and the exact trigger sites where relevant, the
   list of oracle cases, the exit gates of this plan, the lot's allowed paths, and the interface it builds
   against as declarations written by the design lane: TypeScript signatures for facade lots (L1b, L7b,
   L16b, L17a), Rust trait headers for kernel interfaces shared between lots (memory module, command
   trait, value model, format interface, number-formatting thresholds). A lot spec carries no unresolved
   reference path and no reference code (EVOL Annex F).
2. Carbon reviews the lot spec; a spec that leaks reference structure is rejected at this point.
3. The oracle job (Room 1) records the lot's cases and commits `test/snapshots/<lot>/**`, together with
   any generated vectors or fixtures the lot's gates need (number vectors for L8b, the loopback fixture
   definition for L16b). The spec and the snapshots are merged to `main`.
4. Only then does the conductor create the build worktree `tmp/<slug>` from `main`. The lane bootstrap
   checks that the worktree contains neither `tmp/refs/` nor the oracle package, and the lot brief repeats
   the rule. A build lane needs no network and no registry credential.
5. The build lane works from the lot spec, EVOL, committed snapshots, and public POSIX or tool
   documentation. Questions go back to the design lane as spec issues; answers are spec amendments. A build
   lane that needs a new case writes the case; the oracle job records it.
6. Checks that need the oracle, the network or a registry are **conductor gates**, run outside the build
   lane: byte-for-byte reproduction of snapshots by the oracle, the just-bash benchmark column, the
   release-time digest comparison. They are listed under "Also" in the lots and never block a Room 2 PR's
   own `make check`.
7. Shared files are generated, not edited: the command registry and the coverage feature-id table are
   produced by `tools/build.mjs` from one file per command (L2), so parallel command lots have disjoint
   allowed paths.
8. A lane does not change rooms within a lot.

Scope exceptions: forbidden paths of `harness.profile.json` are touched by two lots only (the `bin` entry of L17a is `stpsh`). L0 declares three
exceptions (`Makefile`, `package.json`, `.github/workflows/**`) and moves all build and gate logic into
`tools/`; L17a declares one (`package.json`, for `bin`). The exception ids take the branch number assigned
by the conductor (`BR<n>-EX<k>`), each with its reason, impact and rollback in the branch's `BRANCH.md`.
No other lot needs one: later gates are added in `tools/check.mjs`.

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
- Activation: the harness exists from L12 (W6). The regression gate is switched on in `make check` when
  the gate L2 targets are confirmed and the first baseline is committed; each command lot with an M4
  workload must meet its target to exit and adds the workload to the baseline when it merges.

## 8. Lots

Every lot also satisfies: specs and snapshots only (section 6), no dependency added, no Python, no
attribution in commits, English, and `make check` green on a clean worktree. "0 failure" means every
committed snapshot of the lot replays identically, apart from cases on the divergence list.

### L0 — Component pipeline and package skeleton
- **Scope.** Toolchain pin (`rust-toolchain.toml`, `wasm32-wasip2`), tool pins with hashes under `tmp/tools/`, `tools/` manifest and lockfile (`jco-transpile`, `typescript`), `tools/build.mjs` and `tools/check.mjs` holding all build and gate logic (the `Makefile` only delegates to them, so later lots never touch it), an empty `no_std` kernel exporting one function through the WIT, gates G1-G8 (EVOL Annex C), validation of the Annex A WIT, `package.json` assembly (`exports` for `.` and `./browser`, `files`, no dependency), the published divergence list (EVOL Annex I) and the offline `npm pack` test run on every PR (EVOL D18). Go/no-go point 1 (EVOL D24). If the generated bindings cannot be made `no_std`, the fallback is hand-written glue (EVOL D16), which is not a stop.
- **Room.** Room 2 (build). **Wave.** W0, slot A. **Size.** 3.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 17.5 units + overhead 5.25 = 22.75 units.
- **Depends on.** —.
- **Entry.** Lot spec merged (section 6); EVOL v1.3 approved.
- **Exit gates.** G1-G8 pass on a clean worktree; `wasm-tools component wit` accepts Annex A; a `node:test` case instantiates the generated glue with an empty import object and calls the export; `npm pack` tarball installs in an empty project with no native tool and has no dependency and no install script (S13); written go/no-go report. `make check` green.
- **Also.** Scope exceptions to declare: `BR<n>-EX1` `Makefile` (delegation to `tools/`), `BR<n>-EX2` `package.json` (entries, `files`), `BR<n>-EX3` `.github/workflows/**` (run `make check`).
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l0-pipeline` (worktree `tmp/l0-pipeline`).

### L1a-1 — Memory module interface
- **Scope.** `kernel::mem` public interface (EVOL D6): the fallible container types and their signatures, a minimal bump allocator behind them, `configure` / `raise-ceiling`, the `cabi_realloc` hook, the G6 import gate with its two whitelisted modules. Lands before L1b branches so that both lots build against the same module.
- **Room.** Room 2 (build). **Wave.** W1, slot A. **Size.** 2 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 10 units + overhead 3 = 13 units.
- **Depends on.** L0.
- **Entry.** Lot spec merged (section 6); Gate L0 passed.
- **Exit gates.** Native `cargo test` of every container signature (success and error at budget); G6 passes with the interface in place; kernel still links with zero imports (G1). `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l1a1-mem-interface` (worktree `tmp/l1a1-mem-interface`).

### L10a — Parity harness
- **Scope.** Engine-agnostic replay runner and recorder with the engine-adapter interface, snapshot format including trace and coverage, handling of the divergence list (default profile V1: parity cases run under the `normal` profile unless a case states otherwise), API tier A/B test skeleton (EVOL Annex E). Self-tested on a mock adapter; the `@sentropic/bash` adapter is wired in L2; the just-bash adapter is written by the oracle job (Room 1).
- **Room.** Room 2 (build). **Wave.** W1, slot B. **Size.** 3.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 8.75 units + overhead 3.5 = 12.25 units.
- **Depends on.** L0.
- **Entry.** Lot spec merged (section 6); Gate L0 passed.
- **Exit gates.** Recorder and runner self-tests on the mock adapter (record then replay is byte-identical; a one-byte difference fails); `make check` replays a committed sample with no dependency and no network. `make check` green.
- **Also.** Conductor gate (Room 1): the oracle job reproduces a committed snapshot byte for byte with the just-bash adapter.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l10a-parity-harness` (worktree `tmp/l10a-parity-harness`).

### L1b — WIT resources and JS facade
- **Scope.** Kernel skeleton for the full Annex A contract (`vfs`, `shell`, `fiber`, epochs, chunked calls, `cancel`), generated bindings (D16), JS facade over the glue (D14: `Bash`, `InMemoryFs`, command context), capability membrane (D9), Node and browser loaders with the staging queue (D15), result contract of Annex D.2 for the cases reachable without a parser, option behaviour of D13 and V5 (`python`, `javascript`, `defenseInDepth`), trap poisoning (D8) with a test-only trapping export.
- **Room.** Room 2 (build). **Wave.** W2, slot A. **Size.** 6 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 30 units + overhead 9 = 39 units.
- **Depends on.** L1a-1.
- **Entry.** Lot spec merged (section 6); L1a-1 merged.
- **Exit gates.** `node:test`: stale `resume` returns `stale-epoch`; denied host calls never reach a recording driver; late continuation after abort cannot resume; two shells on one `InMemoryFs` are isolated (S14); an oversized payload gets `limit-exceeded` from the driver pre-check without a trap (D6); `python: true` and `javascript: {}` construct, the four commands exit 127 and the name helpers return the names; `defenseInDepth` has no effect; after the test-only trap every call rejects with the typed error (S12); a default `new Bash()` cannot read a host sentinel file (EVOL D26); binding regeneration check; `tsc --noEmit` on facade declarations. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l1b-wit-facade` (worktree `tmp/l1b-wit-facade`).

### L1a-2 — Memory module completion
- **Scope.** Free-list allocator over `memory.grow` with ceiling, full budget accounting, unwinding reserve, headroom accounting for `cabi_realloc` (EVOL D6).
- **Room.** Room 2 (build). **Wave.** W2, slot B. **Size.** 4 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 20 units + overhead 6 = 26 units.
- **Depends on.** L1a-1.
- **Entry.** Lot spec merged (section 6); L1a-1 merged.
- **Exit gates.** Native stress: 10 000 mixed alloc/free cycles with high-water <= 1.25x peak live bytes and the whole heap returned on drop-all; every container returns an error at budget and at ceiling; `raise-ceiling` is monotonic; memory grows mid-call without stale views (`node:test`). `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l1a2-mem-allocator` (worktree `tmp/l1a2-mem-allocator`).

### L2 — Vertical slice and measurements
- **Scope.** Lexer and parser subset, resumable VM (D4), chunked buffers, VFS core with default layout (D5), command trait with one file per command and a generated registry and feature-id table (so parallel command lots never edit a shared file), the generic flag-coverage hook over per-command static tables, `echo`, the four limit profiles with override validation (D7, Annex B), the `@sentropic/bash` adapter for the harness. Measurements of EVOL D24. Go/no-go point 2.
- **Room.** Room 2 (build). **Wave.** W3, slot A. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L1a-2, L1b, L10a.
- **Entry.** Lot spec merged (section 6); L1a-2, L1b and L10a merged; snapshots of this lot committed.
- **Exit gates.** Snapshots for `echo`, variables and pipelines pass with 0 failure; recursion depth 10 000 gives a limit result; each byte cap of each of the four profiles ends in exit 126 with the instance usable; an override that does not fit the ceiling is rejected at construction; measurement report with numbers against EVOL Annex H; decision record for `wasm-opt` (kept only with a gain >= 10%) and the `slim` build. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l2-vertical-slice` (worktree `tmp/l2-vertical-slice`).

### L7b — Host filesystem adapters
- **Scope.** `OverlayFs`, `ReadWriteFs` and custom `IFileSystem` bridging on Node (EVOL D10, Annex D.3), batched `fs-batch`, `mount-host`.
- **Room.** Room 2 (build). **Wave.** W3, slot B. **Size.** 3 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** L1b.
- **Entry.** Lot spec merged (section 6); L1b merged.
- **Exit gates.** Adversarial suite in `node:test` with one case per item of D10 (traversal, symlink, FIFO/socket/device, exclusive-create race, broken-symlink leaf, hard link, error-message leakage); a 1 000-file listing costs at most one host round trip per directory; S7 holds; with `OverlayFs` on a directory the sentinel file is readable and the host file is unchanged after a write (EVOL D26). `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l7b-host-fs` (worktree `tmp/l7b-host-fs`).

### L3 — Full lexer and parser
- **Scope.** Complete grammar (study 1.4): compound commands, `[[ ]]`, `$(( ))`, here-documents, ANSI-C quoting, `normalizeScript`, parse errors with the reference messages, the two script-size checks of EVOL D1.
- **Room.** Room 2 (build). **Wave.** W4, slot A. **Size.** 8 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 40 units + overhead 12 = 52 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); Gate L2 passed (kernel part); snapshots committed.
- **Exit gates.** Parser snapshot set passes with 0 failure (messages and exit 2); script over `maxSourceBytes` gives exit 126 and over 1 000 000 UTF-16 units exit 2; seeded parser fuzz slice in `make check` with no trap; token and depth limits give the specified exit codes. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l3-parser` (worktree `tmp/l3-parser`).

### L8a — Regex and glob engines
- **Scope.** Linear-time regex engine with compile caps and bounded backreferences, glob and extglob matcher (EVOL D12).
- **Room.** Room 2 (build). **Wave.** W4, slot B. **Size.** 8 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 40 units + overhead 12 = 52 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); Gate L2 passed (kernel part); snapshots committed.
- **Exit gates.** Native unit tests; the listed blow-up patterns for matching and for compilation end within fuel (S9); pattern snapshots (BRE, ERE, classes) pass with 0 failure. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l8a-regex-glob` (worktree `tmp/l8a-regex-glob`).

### L4 — Expansion engine
- **Scope.** Parameter operations including `${var@op}`, command, arithmetic and process substitution, brace, tilde, glob/extglob/globstar, word splitting, reference arithmetic (EVOL D1), `bash:expansion:*` coverage hits (Annex D.7).
- **Room.** Room 2 (build). **Wave.** W5, slot A. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L3, L8a; exit: L8b, L1d.
- **Entry.** Lot spec merged (section 6); L3 and L8a merged; snapshots committed. L8b and L1d must be merged before the final PR.
- **Exit gates.** Expansion and arithmetic snapshot sets pass with 0 failure (values beyond 2^53, 32-bit shifts, division and modulo signs); expansion limits give exit 126; every `bash:expansion:*` identifier is hit by at least one case and hit sequences equal the snapshots. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l4-expansion` (worktree `tmp/l4-expansion`).

### L8b — Shared leaf kernels
- **Scope.** printf and strftime engine, number parsing, JavaScript-compatible number formatting (EVOL D1), md5/sha1/sha256, base64.
- **Room.** Room 2 (build). **Wave.** W5, slot B. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 3 = 10.5 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); L2 merged; pre-generated vector file of doubles and their expected strings committed by Room 1.
- **Exit gates.** Number formatter equals the committed vectors (0 mismatch); hash and base64 standard vectors; printf snapshots pass with 0 failure. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l8b-leaf-kernels` (worktree `tmp/l8b-leaf-kernels`).

### L1d — Observability
- **Scope.** `logger`, trace and coverage buffers, `take-trace`, `take-coverage`, `coverage-features`, drain order (on `out-of-fuel`, before host callbacks, on `cancel`), facade delivery, browser filter for `cmd:flag` (EVOL Annex A rules, D.1, D.6, D.7).
- **Room.** Room 2 (build). **Wave.** W5, slot B. **Size.** 2 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 5 units + overhead 2 = 7 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); L2 merged.
- **Exit gates.** `node:test`: ordered delivery before `exec` resolves; drain before a custom command callback; logger call sequence equals Annex D.1 and an empty or whitespace-only script produces no logger call; a script cannot distinguish two trace-clock sequences (S11); `exec-oneshot` rejects `trace` and `coverage`. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l1d-observability` (worktree `tmp/l1d-observability`).

### L5 — Interpreter semantics
- **Scope.** Redirections and fd table, functions, subshells, `set`/`shopt`, errexit, traps subset, arrays, namerefs, special-builtin precedence, builtin dispatch with `bash:cmd:*` and `bash:builtin:*` hits.
- **Room.** Room 2 (build). **Wave.** W6, slot A. **Size.** 8.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 42.5 units + overhead 12.75 = 55.25 units.
- **Depends on.** L4.
- **Entry.** Lot spec merged (section 6); L4 merged; snapshots committed.
- **Exit gates.** Semantics snapshot sets per area pass with 0 failure; two overlapping execs and a re-entrant `ctx.exec` test (D4); every `bash:cmd:*` identifier hit by at least one case; hit sequences equal the snapshots. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l5-interpreter` (worktree `tmp/l5-interpreter`).

### L7a — VFS completion
- **Scope.** `bash.fs` and `InMemoryFs` complete over `vfs.op` (Annex D.3), `MountableFs` with `mount-vfs`, lazy files, hard links and stat identity, `take-dirty` journal and `bash.compact()` (EVOL D5, D8).
- **Room.** Room 2 (build). **Wave.** W6, slot B. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 3 = 10.5 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); L2 merged.
- **Exit gates.** One contract test per method of Annex D.3 (23 methods); journal rebuild after the test-only trap restores every committed file; compaction lowers `usage().used`; lazy-file materialization through chunks. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l7a-vfs` (worktree `tmp/l7a-vfs`).

### L12 — Benchmark harness
- **Scope.** `bench/run.mjs` per EVOL Annex H, wired into `tools/check.mjs` (no `Makefile` change): engines, cold and warm protocols, statistics, baseline file, noise-floored regression gate.
- **Room.** Room 2 (build). **Wave.** W6, slot B. **Size.** 2 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 5 units + overhead 2 = 7 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); Gate L2 measurement report.
- **Exit gates.** `make bench` produces the table and JSON; gate unit tests on synthetic series (a regression above both thresholds fails, one below either passes); baseline committed. `make check` green.
- **Also.** Conductor gate (Room 1): the just-bash column is produced by the oracle job.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l12-bench` (worktree `tmp/l12-bench`).

### L1c — Session drivers and composition
- **Scope.** Session driver components (`session-pure`, `session-fs`, `session-net`) and their WIT, `wac` composition, `wasmtime` smoke test under a 256 MiB limiter and an epoch deadline, `exec-oneshot` (EVOL D15), the wasmCloud and Kubernetes disk rule of EVOL D26 (`session-pure` has no filesystem import; `session-fs` mounts the granted directory as root). Its report triggers the WIT freeze (D3).
- **Room.** Room 2 (build). **Wave.** W6, slot B. **Size.** 3.5 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 17.5 units + overhead 5.25 = 22.75 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); L2 merged; `wac` and `wasmtime` pinned.
- **Exit gates.** Composed component runs the L2 snapshot subset under `wasmtime` with 0 failure; import lists equal the driver worlds and `session-pure` lists no filesystem import; `session-fs` with one preopened directory sees it as `/` and cannot read its parent, and with no preopen sees no host file; `exec-oneshot` round trip; composed-component measurements reported; freeze commit of `sentropic:bash@0.1.0`. `make check` green.
- **Also.** Conductor gate: smoke run of the composed component on the wasmCloud host of the owner, with a granted volume.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l1c-session-wac` (worktree `tmp/l1c-session-wac`).

### L6a — State-coupled builtins
- **Scope.** `declare`/`typeset`, `local`, `readonly`, `export`, `unset`, `eval`, `source`, `exec`, `command`, `builtin`, `read`, `mapfile`.
- **Room.** Room 2 (build). **Wave.** W7, slot A. **Size.** 3 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** L5.
- **Entry.** Lot spec merged (section 6); L5 merged; snapshots committed.
- **Exit gates.** Per-builtin snapshots pass with 0 failure; every option and every error row of the builtin specs has a snapshot. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l6a-builtins-state` (worktree `tmp/l6a-builtins-state`).

### L16b — Network adapter and `curl`
- **Scope.** Lazily imported network adapter inside `@sentropic/bash` (EVOL D11, Annex D.4): synchronous `NetworkConfig` validation, allow-lists, hop-by-hop redirects with transforms, DNS validation and pinning, typed errors; kernel `curl` with its exit-code map from the lot spec.
- **Room.** Room 2 (build). **Wave.** W7, slot B. **Size.** 3 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 15 units + overhead 4.5 = 19.5 units.
- **Depends on.** L2, L10a.
- **Entry.** Lot spec merged (section 6); L2 and L10a merged; `curl` snapshots recorded by Room 1 against the loopback fixture server and committed with the fixture definition.
- **Exit gates.** Loopback-server tests: denied prefix, redirect to a denied prefix, rebinding between check and connect, oversized body, method not allowed (S8); `curl` snapshots replay with 0 failure against the same fixture. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l16b-net-curl` (worktree `tmp/l16b-net-curl`).

### L13 — `sed`
- **Scope.** Parser, addresses, pattern and hold space cycle, branches with fuel, `-i`, `sed:cmd:*` hits and flag table.
- **Room.** Room 2 (build). **Wave.** W8, slot A. **Size.** 8.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 21.25 units + overhead 8.5 = 29.75 units.
- **Depends on.** L8a, L7a, L1d, L12.
- **Entry.** Lot spec merged (section 6); Listed lots merged; snapshots committed.
- **Exit gates.** `sed` snapshots pass with 0 failure; every flag of the flag table and every `sed:cmd:*` identifier is covered; branch loops end in exit 126; M4 `sed` workload meets the Annex H target as confirmed at gate L2. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l13-sed` (worktree `tmp/l13-sed`).

### L9a-1 — Hard stream commands
- **Scope.** `grep`/`egrep`/`fgrep`, `sort`, `tr`, `expr`, with their coverage flag tables.
- **Room.** Room 2 (build). **Wave.** W8, slot B. **Size.** 4.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 11.25 units + overhead 4.5 = 15.75 units.
- **Depends on.** L8a, L1d, L12.
- **Entry.** Lot spec merged (section 6); Listed lots merged; snapshots committed.
- **Exit gates.** Per-command snapshots pass with 0 failure; every flag of each flag table and every row of each error table has a snapshot; M4 `grep` and `sort` workloads meet their targets. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l9a1-stream-commands` (worktree `tmp/l9a1-stream-commands`).

### L16d — `rg`
- **Scope.** `rg` on the grep kernel, with its flag table.
- **Room.** Room 2 (build). **Wave.** W8, slot B. **Size.** 3.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 8.75 units + overhead 3.5 = 12.25 units.
- **Depends on.** L9a-1.
- **Entry.** Lot spec merged (section 6); L9a-1 merged; snapshots committed.
- **Exit gates.** Snapshots pass with 0 failure; every flag of the flag table covered. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l16d-rg` (worktree `tmp/l16d-rg`).

### L14a — `awk` core
- **Scope.** Lexer, parser, VM, `awk:stmt:*` and `awk:expr:*` hits, flag table, the built-in function interface.
- **Room.** Room 2 (build). **Wave.** W9, slot A. **Size.** 6 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 30 units + overhead 9 = 39 units.
- **Depends on.** L8a, L8b, L1d, L12.
- **Entry.** Lot spec merged (section 6); Listed lots merged; snapshots committed.
- **Exit gates.** Core `awk` snapshots pass with 0 failure; every `awk:stmt:*` and `awk:expr:*` identifier covered; iteration limits give exit 126; M4 `awk` workload meets its target. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l14a-awk-core` (worktree `tmp/l14a-awk-core`).

### L14b — `awk` built-in functions
- **Scope.** String, numeric and I/O built-in functions on the interface of L14a.
- **Room.** Room 2 (build). **Wave.** W9, slot A. **Size.** 2.5 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 2.5 units + overhead 1.88 = 4.38 units.
- **Depends on.** L14a.
- **Entry.** Lot spec merged (section 6); L14a merged; function table spec and snapshots committed.
- **Exit gates.** One snapshot per function and per error row, 0 failure. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `feat/l14b-awk-builtins` (worktree `tmp/l14b-awk-builtins`).

### L15a-1 — `jq` core
- **Scope.** JSON parser, shared value model (first PR), generator evaluator, `jq:node:*` hits, flag table, the builtin interface.
- **Room.** Room 2 (build). **Wave.** W9, slot B. **Size.** 5.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 27.5 units + overhead 8.25 = 35.75 units.
- **Depends on.** L8a, L8b, L1d.
- **Entry.** Lot spec merged (section 6); Listed lots merged; snapshots committed.
- **Exit gates.** Core `jq` snapshots pass with 0 failure; every `jq:node:*` identifier covered; query limits give exit 126; value model published. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l15a1-jq-core` (worktree `tmp/l15a1-jq-core`).

### L15a-2 — `jq` builtins
- **Scope.** The builtin function library on the interface of L15a-1.
- **Room.** Room 2 (build). **Wave.** W9, slot B. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 3 = 10.5 units.
- **Depends on.** L15a-1.
- **Entry.** Lot spec merged (section 6); L15a-1 merged; builtin table spec and snapshots committed.
- **Exit gates.** One snapshot per builtin and per error row, 0 failure. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l15a2-jq-builtins` (worktree `tmp/l15a2-jq-builtins`).

### L9b — Leaf tier-1 commands
- **Scope.** About 41 leaf commands of tier 1 (study 3.1, without the stateful `alias`, `unalias`, `history`), one file each, with flag tables. PRs of at most six commands.
- **Room.** Room 2 (build). **Wave.** W10, slot A. **Size.** 8.5 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 8.5 units + overhead 6.38 = 14.88 units.
- **Depends on.** L7a, L8b, L1d.
- **Entry.** Lot spec merged (section 6); Listed lots merged; one spec per command and its snapshots committed.
- **Exit gates.** Per command: snapshots pass with 0 failure, every flag of its table and every row of its error table has a snapshot. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `feat/l9b-leaf-commands` (worktree `tmp/l9b-leaf-commands`).

### L9a-2 — Re-entrant commands
- **Scope.** `find` with `-exec` and its trace events (Annex D.6, batch size 500), `xargs`, `bash`/`sh`, `timeout`, `time`, with flag tables.
- **Room.** Room 2 (build). **Wave.** W10, slot B. **Size.** 5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 12.5 units + overhead 5 = 17.5 units.
- **Depends on.** L5, L1d.
- **Entry.** Lot spec merged (section 6); L5 and L1d merged; snapshots committed.
- **Exit gates.** Snapshots pass with 0 failure; `find` trace names, keys and counters equal the snapshots (default path, missing path, trailing slash cases included); nested execs charge the parent budget; flag tables covered. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l9a2-reentrant-commands` (worktree `tmp/l9a2-reentrant-commands`).

### L6b-1 — Builtins with their own grammar or state
- **Scope.** `compgen`/`complete`/`compopt`, `test`/`[`, `getopts`, `let`, and the stateful `alias`, `unalias`, `history`.
- **Room.** Room 2 (build). **Wave.** W10, slot B. **Size.** 2.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 6.25 units + overhead 2.5 = 8.75 units.
- **Depends on.** L5.
- **Entry.** Lot spec merged (section 6); L5 merged; snapshots committed.
- **Exit gates.** Per-builtin snapshots pass with 0 failure; every operator of `test` and every option of the completion builtins has a snapshot. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l6b1-builtins-mid` (worktree `tmp/l6b1-builtins-mid`).

### L6b-2 — Simple builtins
- **Scope.** `:`, `true`, `false`, `cd`, `pwd`, `shift`, `dirs`/`pushd`/`popd`, `hash`, `help`, `type`, `wait`, and the builtins the reference accepts with minimal behaviour (`ulimit`, `umask`, `times`, `caller`, `enable`, `fc`, `jobs`, `fg`, `bg`, `disown`, `suspend`, `kill`), each as recorded by the oracle.
- **Room.** Room 2 (build). **Wave.** W10, slot B. **Size.** 1.5 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 1.5 units + overhead 1.13 = 2.63 units.
- **Depends on.** L5.
- **Entry.** Lot spec merged (section 6); L5 merged; snapshots committed.
- **Exit gates.** Per-builtin snapshots pass with 0 failure. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `feat/l6b2-builtins-leaf` (worktree `tmp/l6b2-builtins-leaf`).

### L15b-1 — `yq` command and YAML
- **Scope.** `yq` command (flags, in-place, front matter, slurp) and the format reader/writer interface (first PR), YAML reader/writer with alias and depth caps, JSON wiring, `jq:node:*` hits from `yq`.
- **Room.** Room 2 (build). **Wave.** W11, slot A. **Size.** 5.5 points, Strong.
- **Builder.** Sonnet 5.5; fallback Gemini 4 Argon.
- **Cost.** Build 27.5 units + overhead 8.25 = 35.75 units.
- **Depends on.** L15a-1.
- **Entry.** Lot spec merged (section 6); L15a-1 merged; snapshots committed.
- **Exit gates.** YAML read and write snapshots pass with 0 failure; alias-bomb test ends in exit 126; flag table covered. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l15b1-yq-yaml` (worktree `tmp/l15b1-yq-yaml`).

### L15b-3 — INI and CSV for `yq`
- **Scope.** INI and CSV/TSV readers/writers on the format interface.
- **Room.** Room 2 (build). **Wave.** W11, slot A. **Size.** 3 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 3 units + overhead 2.25 = 5.25 units.
- **Depends on.** first PR of L15b-1.
- **Entry.** Lot spec merged (section 6); First PR of L15b-1 merged; format specs and snapshots committed.
- **Exit gates.** Read and write snapshots per format, 0 failure; CSV row and cell limits give exit 126. First PR reviewed in full. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `feat/l15b3-ini-csv` (worktree `tmp/l15b3-ini-csv`).

### L16a-1 — Compression and archives
- **Scope.** inflate/deflate, `gzip`/`gunzip`/`zcat`, `tar`, with archive limits and extraction containment (EVOL S15).
- **Room.** Room 2 (build). **Wave.** W11, slot B. **Size.** 5 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 25 units + overhead 7.5 = 32.5 units.
- **Depends on.** L2, L7a, L7b.
- **Entry.** Lot spec merged (section 6); Listed lots merged; codec and `tar` specs and snapshots committed.
- **Exit gates.** Round-trip tests and snapshots with 0 failure; archive-bomb, symlink-parent and special-file cases end in limit or error results, including on a host mount. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l16a1-archive` (worktree `tmp/l16a1-archive`).

### L15b-2 — XML and TOML for `yq`
- **Scope.** XML and TOML readers/writers on the format interface; no DOCTYPE or external entity (S15).
- **Room.** Room 2 (build). **Wave.** W11, slot B. **Size.** 5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 12.5 units + overhead 5 = 17.5 units.
- **Depends on.** first PR of L15b-1.
- **Entry.** Lot spec merged (section 6); First PR of L15b-1 merged; format specs and snapshots committed.
- **Exit gates.** Read and write snapshots per format, 0 failure; entity and depth adversarial cases end in errors. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l15b2-xml-toml` (worktree `tmp/l15b2-xml-toml`).

### L10b — Cross-cutting parity cases
- **Scope.** Our own cases beyond those of the lot specs: grammar areas across features, API tiers A and B, limit behaviour under the `normal` profile against the oracle, trace and coverage streams. Snapshots are recorded by the oracle job.
- **Room.** Room 2 (build). **Wave.** W12, slot A. **Size.** 9 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 9 units + overhead 3.6 = 12.6 units.
- **Depends on.** L10a and the lots under test.
- **Entry.** Lot spec merged (section 6); Listed lots merged; case list reviewed.
- **Exit gates.** Parity report: 0 failing case outside the divergence list; every Annex D.7 identifier that is not excluded is hit by at least one case; every row of Annex D.1 tiers A and B has a case; no case derived from a reference test file. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `feat/l10b-parity-cases` (worktree `tmp/l10b-parity-cases`).

### L16a-2 — `diff`
- **Scope.** `diff` with normal, unified and context output.
- **Room.** Room 2 (build). **Wave.** W12, slot B. **Size.** 3 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 7.5 units + overhead 3 = 10.5 units.
- **Depends on.** L2.
- **Entry.** Lot spec merged (section 6); Snapshots committed.
- **Exit gates.** Snapshots pass with 0 failure; flag table covered. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l16a2-diff` (worktree `tmp/l16a2-diff`).

### L16c — Leaf tier-3 commands
- **Scope.** `comm`, `join`, `column`, `fold`, `expand`, `unexpand`, `split`, `strings`, `od`, `file`, `tree`, `du`, and `md5sum`, `sha1sum`, `sha256sum` (commands over the hash kernels of L8b, including `-c`).
- **Room.** Room 2 (build). **Wave.** W12, slot B. **Size.** 3.5 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 3.5 units + overhead 2.63 = 6.13 units.
- **Depends on.** L7a, L8b, L1d.
- **Entry.** Lot spec merged (section 6); Command specs and snapshots committed.
- **Exit gates.** Per command: snapshots pass with 0 failure, flag and error tables covered. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `feat/l16c-leaf-tier3` (worktree `tmp/l16c-leaf-tier3`).

### L12b — Feature matrix
- **Scope.** `docs/FEATURE_MATRIX.md`: commands, options and API of just-bash 3.6.0, MoonBash 0.1.0 and `@sentropic/bash` (target and status columns). Reference-reading, documentation-only lane (EVOL Annex F.1).
- **Room.** Room 1 (reference-reading, documentation only). **Wave.** W12, slot B. **Size.** 2 points, Cheap.
- **Builder.** Haiku 5.5; fallback Engram Muse 1.3.
- **Cost.** Build 2 units + overhead 0.8 = 2.8 units.
- **Depends on.** —.
- **Entry.** Lot spec merged (section 6); Room 1 worktree with `tmp/refs/`.
- **Exit gates.** Matrix has one row per name of the 85 + 5 command names and per row of Annex D.1; status column filled from the parity report; no code copied. Refreshed at the parity gate. `make check` green.
- **Review.** Carbon reviews the first three PRs in full, then one in five; dispatched only on a green `make check`.
- **Branch.** `docs/l12b-feature-matrix` (worktree `tmp/l12b-feature-matrix`).

### L11a — Fuzz harnesses and campaigns
- **Scope.** Seeded JS and native fuzzers in `make check`, mutation fuzzing on our engine, and the coverage-guided campaign of EVOL D21 from `tmp/fuzz/` on its seven targets (parser, expansion, regex compiler, inflate/tar, path canonicalization, the `yq` readers, the URL and IP parser).
- **Room.** Room 2 (build). **Wave.** W13, slot A. **Size.** 4.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 11.25 units + overhead 4.5 = 15.75 units.
- **Depends on.** L5, L8a, L16a-1, L16b, L15a-1, L15b-1, L15b-2, L15b-3.
- **Entry.** Lot spec merged (section 6); Listed lots merged.
- **Exit gates.** Each of the seven targets ran at least one hour and until 15 minutes without new coverage; campaign report (targets, duration, corpus size, findings); every finding committed as a seeded regression test. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l11a-fuzz` (worktree `tmp/l11a-fuzz`).

### L19 — Browser entry verification
- **Scope.** Headless Chrome for Testing rig over the DevTools protocol, `precompile`, streaming compile, staging queue, trace and coverage in the browser (no `cmd:flag` hit), the commands of divergence V4, a page-supplied asynchronous `IFileSystem` backed by the origin-private file system as the explicit disk grant (EVOL D26), metric M1b.
- **Room.** Room 2 (build). **Wave.** W13, slot A. **Size.** 4 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 10 units + overhead 4 = 14 units.
- **Depends on.** L1d, L9a-2, L16a-1, L15b-1, L15b-2, L15b-3.
- **Entry.** Lot spec merged (section 6); Chrome for Testing pinned with its hash.
- **Exit gates.** Browser suite passes with 0 failure, including `tar`, `gzip`, `yq` cases and a run over the page-supplied `IFileSystem` (files written by a script are read back through the adapter; without it no page storage is touched); G2 browser-graph check; M1b reported. Rerun at the parity gate. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l19-browser` (worktree `tmp/l19-browser`).

### L11b — Security suites and triage
- **Scope.** Adversarial suites completing the invariant table S1-S15 (EVOL Annex G) where earlier lots do not already prove a row, limit-latency tests (M7), triage and fixes of campaign findings.
- **Room.** Room 2 (build). **Wave.** W13, slot B. **Size.** 5 points, Strong.
- **Builder.** Gemini 4 Argon; fallback Sonnet 5.5.
- **Cost.** Build 25 units + overhead 7.5 = 32.5 units.
- **Depends on.** L1d, L7a, L7b, L16b; findings of L11a.
- **Entry.** Lot spec merged (section 6); Listed lots merged.
- **Exit gates.** Invariant table with a passing suite named for each of S1-S15; M7 limit latency not above just-bash on the same cases; zero open trap. `make check` green.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l11b-security` (worktree `tmp/l11b-security`).

### L17a — Tier C: `Sandbox` facade and the `stpsh` binary
- **Scope.** `Sandbox` facade and `SandboxCommand` (argument quoting included), the single binary `stpsh` (EVOL D26): script mode (`stpsh file`, `-c`, stdin), interactive shell on a terminal, empty filesystem by default, `--root`, `--allow-write`, `--just-bash`, and the reference option set. Standalone facades (`grep`, `ls`, `awk`, `jq`) are post-parity (section 9).
- **Room.** Room 2 (build). **Wave.** W14, slot A. **Size.** 3.5 points, Mid.
- **Builder.** Codex Sol 6.1 medium; fallback Sonnet 5.5.
- **Cost.** Build 8.75 units + overhead 3.5 = 12.25 units.
- **Depends on.** parity gate.
- **Entry.** Lot spec merged (section 6); Parity gate passed; snapshots of the reference CLIs recorded by Room 1 for the `--just-bash` cases.
- **Exit gates.** API snapshots for `Sandbox` pass with 0 failure, including arguments with quotes and spaces; `stpsh` tests: with no flag a script cannot read a sentinel file of the current directory; with `--root` it reads it and a write fails; with `--allow-write` the write succeeds as on the reference; with `--just-bash` outputs equal the recorded snapshots of both reference CLIs; a bare `stpsh` with a terminal enters the interactive shell and with a pipe runs the script; one test per documented option. `make check` green.
- **Also.** Scope exception to declare: `BR<n>-EX1` `package.json` (`bin`). Release gate (conductor): npm component digest equals the OCI digest.
- **Review.** Carbon, full review of every PR, dispatched only on a green `make check`.
- **Branch.** `feat/l17a-tier-c` (worktree `tmp/l17a-tier-c`).

## 9. Post-parity

Not part of the v1 totals. Each item gets its own plan entry after the parity gate.

| Item | Scope | Size (estimate) | Tier |
|---|---|---:|---|
| Companion study | Build form of the runtime companions (EVOL D25): WASI component without host-JS glue (design lane's preference) or Emscripten glue; VFS bridge; supply-chain pins | 3 | design and review lanes |
| `@sentropic/python` | `python3`/`python` on an embedded CPython component, composed on demand | 8 | Strong |
| `@sentropic/js` | `js-exec`/`node` on an embedded QuickJS component | 8 | Strong |
| Standalone facades | `@sentropic/grep`, `ls`, `awk`, `jq` as JS facades over the kernel (not part of the reference's API) | 1 | Mid |
| L17b | AST export, transform pipeline, `serialize`, only if decision B3 is reversed | 3 | Strong |
| L18 | `@sentropic/make` | 8 | Strong |

Invariants for the companions (EVOL D25): they reach the kernel through the existing host-call path only,
so kernel gates G1 and G6 are unchanged with a companion installed; each has its own EVOL, oracle snapshots
and release lane, and the zero-dependency exception is limited to those packages; they register with
`@sentropic/bash` through an option object passed by the caller (the core never imports a companion by a
bare specifier, which G2 forbids); their JS entry exports the reference's security symbols in either build
form.

## 10. Track mapping

Items to create under the root item `01M7DVAD9N3Z3GS7BKYD3DC63J`, by the conductor. Each lot item:
kind `task`, responsible = the builder lane, accountable = the conductor, links to its `spec/lots/<lot>.md`
and branch, planned spend = the "Units" column, actual spend recorded at merge. Dependencies are given as
lot ids; they become Track item ids at creation. Decisions referenced by the lots: wasmCloud topology
`01M7FTJ9HB55X3WBF2KP008SQW`, MoonBash `01M7FTJ9ZWD7NT9ABFGAX0WF9C`, clean room
`01M7FTJA7EB2SQNHXG0WQVY1A1`, fuzzing `01M7FTJAERMSZW1EGR0DZH6ST5`, runtime companions
`01M7G0VX1KEG93PCF546Q159RV`.

| Lot | Track item title | Wave | Depends on | Units |
|---|---|---|---|---:|
| L0 | L0 — Component pipeline and package skeleton | W0 | — | 22.75 |
| L1a-1 | L1a-1 — Memory module interface | W1 | L0 | 13 |
| L10a | L10a — Parity harness | W1 | L0 | 12.25 |
| L1b | L1b — WIT resources and JS facade | W2 | L1a-1 | 39 |
| L1a-2 | L1a-2 — Memory module completion | W2 | L1a-1 | 26 |
| L2 | L2 — Vertical slice and measurements | W3 | L1a-2, L1b, L10a | 55.25 |
| L7b | L7b — Host filesystem adapters | W3 | L1b | 19.5 |
| L3 | L3 — Full lexer and parser | W4 | L2 | 52 |
| L8a | L8a — Regex and glob engines | W4 | L2 | 52 |
| L4 | L4 — Expansion engine | W5 | L3, L8a; exit: L8b, L1d | 55.25 |
| L8b | L8b — Shared leaf kernels | W5 | L2 | 10.5 |
| L1d | L1d — Observability | W5 | L2 | 7 |
| L5 | L5 — Interpreter semantics | W6 | L4 | 55.25 |
| L7a | L7a — VFS completion | W6 | L2 | 10.5 |
| L12 | L12 — Benchmark harness | W6 | L2 | 7 |
| L1c | L1c — Session drivers and composition | W6 | L2 | 22.75 |
| L6a | L6a — State-coupled builtins | W7 | L5 | 19.5 |
| L16b | L16b — Network adapter and curl | W7 | L2, L10a | 19.5 |
| L13 | L13 — sed | W8 | L8a, L7a, L1d, L12 | 29.75 |
| L9a-1 | L9a-1 — Hard stream commands | W8 | L8a, L1d, L12 | 15.75 |
| L16d | L16d — rg | W8 | L9a-1 | 12.25 |
| L14a | L14a — awk core | W9 | L8a, L8b, L1d, L12 | 39 |
| L14b | L14b — awk built-in functions | W9 | L14a | 4.38 |
| L15a-1 | L15a-1 — jq core | W9 | L8a, L8b, L1d | 35.75 |
| L15a-2 | L15a-2 — jq builtins | W9 | L15a-1 | 10.5 |
| L9b | L9b — Leaf tier-1 commands | W10 | L7a, L8b, L1d | 14.88 |
| L9a-2 | L9a-2 — Re-entrant commands | W10 | L5, L1d | 17.5 |
| L6b-1 | L6b-1 — Builtins with their own grammar or state | W10 | L5 | 8.75 |
| L6b-2 | L6b-2 — Simple builtins | W10 | L5 | 2.63 |
| L15b-1 | L15b-1 — yq command and YAML | W11 | L15a-1 | 35.75 |
| L15b-3 | L15b-3 — INI and CSV for yq | W11 | first PR of L15b-1 | 5.25 |
| L16a-1 | L16a-1 — Compression and archives | W11 | L2, L7a, L7b | 32.5 |
| L15b-2 | L15b-2 — XML and TOML for yq | W11 | first PR of L15b-1 | 17.5 |
| L10b | L10b — Cross-cutting parity cases | W12 | L10a and the lots under test | 12.6 |
| L16a-2 | L16a-2 — diff | W12 | L2 | 10.5 |
| L16c | L16c — Leaf tier-3 commands | W12 | L7a, L8b, L1d | 6.13 |
| L12b | L12b — Feature matrix | W12 | — | 2.8 |
| L11a | L11a — Fuzz harnesses and campaigns | W13 | L5, L8a, L16a-1, L16b, L15a-1, L15b-1, L15b-2, L15b-3 | 15.75 |
| L19 | L19 — Browser entry verification | W13 | L1d, L9a-2, L16a-1, L15b-1, L15b-2, L15b-3 | 14 |
| L11b | L11b — Security suites and triage | W13 | L1d, L7a, L7b, L16b; findings of L11a | 32.5 |
| L17a | L17a — Tier C: Sandbox facade and the stpsh binary | W14 | parity gate | 12.25 |

Milestone items (kind `milestone`): `Gate L0` (after L0), `Gate L2` (after L2), `WIT freeze` (after L1c),
`Parity gate and 0.x pre-release` (after W13). Deferred items (kind `task`, status deferred): the six rows
of section 9.

## 11. Assumptions to re-check

- Tier factors (1 / 2.5 / 5) and overhead rates are estimates; actual spend per lot is recorded in Track and
  the plan is re-based after gates L0 and L2 and after the first leaf batch.
- Headroom, glue size and cold start are unmeasured until L2; the WIT is unvalidated until L0.
- The two-slot waves follow from the four-lane cap; if the cap is raised, slot B lots can run in parallel
  without changing dependencies.

## 12. Owner answer on names and host disk access (ANSWERED, 15/11)

Decision EVOL D26 (Track `01M7G4KVMH4GVS2JBBYNJ67841`, `01M7G4KVWXWGS3WA2EBETGZFH8`): one binary `stpsh`; npm
library `@sentropic/bash`; OCI artefact `sentropic-bash`; WIT package `sentropic:bash`; host disk access
denied by default on every target, the reference's behaviour one flag or option away. Owning lot and
observable gate per target:

| Target | Default and grant | Owning lot | Exit gate |
|---|---|---|---|
| Local CLI | Empty filesystem; `--root`, `--allow-write`, `--just-bash` | L17a | Sentinel-file tests for each flag; `--just-bash` equals snapshots of the reference CLIs; terminal versus pipe mode |
| Node library | `InMemoryFs`; `fs` adapters | L1b (default), L7b (adapters) | Default instance cannot read a host sentinel; `OverlayFs` reads it and leaves the host file unchanged |
| Browser | `InMemoryFs`; page-supplied `IFileSystem` | L19 | Suite run over an adapter backed by the origin-private file system |
| wasmCloud, Kubernetes | `session-pure` without disk; `session-fs` mounting the granted directory as root | L1c | Import lists; preopened-directory tests under `wasmtime`; conductor smoke run on the owner's wasmCloud host |
| Artefact names | npm, OCI, WIT names | L0 (package), release | Package-content test; OCI push under `sentropic-bash` with the npm component's digest |

## 13. Reconciliation of the review of v1

| Finding | Outcome | Final position posed by | Where |
|---|---|---|---|
| B1.1 L10a after its consumers | accepted | Fable 5.2 + Carbon (consensus) | L10a in W1 on a mock adapter; our adapter wired in L2 |
| B1.2 L16b before L2 | accepted | consensus | L16b in W7, depends on L2 and L10a |
| B1.3 L1d after coverage consumers | accepted | consensus | L1d in W5; exit dependency of L4; entry dependency of the command lots |
| B1.4 missing dependency edges, `yq` race | accepted | consensus | Dependencies of L4, L13, L14a, L15a-1, L16a-1, L19, L11a, L11b; format interface in the first PR of L15b-1 |
| B2.1 L1a and L1b in parallel under G6 | accepted, amended | consensus | L1a split: L1a-1 (interface, minimal allocator) merges before L1b branches; L1a-2 completes beside L1b |
| B2.2 Room 2 gates needing the oracle; snapshots before the worktree | accepted | consensus | Section 6 steps 3, 4 and 6; conductor gates under "Also". One line of EVOL Annex E is amended: `curl` cases are recorded against a loopback fixture server |
| B2.3 scope exceptions | accepted | consensus | Section 6: three exceptions in L0, one in L17a; gate logic in `tools/` |
| M1.1 orphaned commands and builtins | accepted | consensus | Hash commands in L16c; `alias`/`unalias`/`history` in L6b-1; the twelve minimal builtins in L6b-2 |
| M1.2 profiles and trap poisoning | accepted | consensus | Four profiles and override rejection in L2; poisoning in L1b; journal rebuild in L7a; `normal`-profile limit parity in L10b; V1 handling in L10a |
| M1.3 option tests, divergence list, `npm pack` | accepted, amended | consensus | Option tests in L1b; package skeleton, divergence list and PR-time `npm pack` in L0 rather than L2, to keep the critical path unchanged; pre-release at the parity gate |
| M2 routing: L12b Cheap, L10a Mid, L6a Strong, L14 and L15a split, L15b-3 Cheap, L11 split, L17a Mid | accepted | consensus | Section 8 |
| M2 routing: L6b to Mid | amended | consensus | Split instead: L6b-1 at Mid for the builtins with their own grammar or state, L6b-2 at Cheap for the simple ones |
| M2 overhead weights | accepted, amended | consensus | Section 2: 1.5 / 1.0 / 0.75 / 0.4, review dispatched on green; a re-base checkpoint after the first leaf batch is added |
| M2 standalone facades | accepted | consensus | Section 9 |
| M3.1 gate L2 split, D16 at gate L0 | accepted | consensus | Section 5 |
| M3.2 four lanes machine-wide | accepted | consensus | Section 3: two slots per wave |
| M3.3 numeric exit gates | accepted | consensus | Section 8: 0-failure, full flag and identifier coverage, M4 targets, fuzz floors, M7 bound |
| m1 lot spec contents, matrix refresh | accepted | consensus | Section 6 steps 1 and 7; parity gate |
| m2 companion invariants | accepted | consensus | Section 9 |
| m3 Track mapping | accepted | consensus | Section 10 |
| Owner question Q1 | answered by the owner (15/11) | Owner | EVOL D26 and V8; section 12; L17a, L19, L1c, L1b, L7b |
