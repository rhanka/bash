# Lot L2c — Default snapshot, minified packages and Gate L2

Lot spec for the build lane (Room 2). Authorities are EVOL D2, D6, D8, D15–D18, D22, D24 and Annex H,
`spec/lots/L0.md` (release gates), `spec/lots/L2.md` sections 9–10 (measurement protocol), and
`spec/lots/L2b.md` (binding and resource ownership). Start after L2b on the integrated L2 build. Keep the
historical results in `plan/reports/GATE_L2.md` and append the new measurement and verdict. Raise any conflict
with EVOL as a spec issue.

- **Size.** 3 points, Strong. **Builder.** Codex Sol 6.2, xhigh. **Review.** Carbon, full PR review on a green
  `make check`.
- **Branch.** `feat/l2c-snapshot-gate`, worktree `tmp/l2c-snapshot-gate`.
- **Outcome.** Two independently installable tarballs select the binding by package name. The default
  constructor restores a build-matched default shell on Node and browser. The conductor can apply the Gate L2
  decision after 6 cumulative Strong points (L2b + L2c).

## 1. Lane rules

1. Work offline with verified tools. No reference source/package, dependencies at runtime, new Rust crates,
   Python or Docker. Use the worktree's ignored `tmp/` for stages, tarballs, installations and measurements.
2. Use the existing reproducible release pipeline, guard and gate scanners. Keep the WIT and public API,
   including Node synchronous construction and browser staging. No performance shortcut may alter a limit, grant
   or visible file.
3. Commits through `make commit`, English, without attribution trailers. The release lane publishes the packages
   and OCI artifact.

## 2. Scope

Implement the Wasm-internal restore path for the production default binding, share and minify the facade,
package Option D separately, and rerun the L2 gate against the artifacts that would ship. L7c owns shared-volume
isolation. L1c owns native composition and WIT freeze, L17a owns publication machinery, and L19 owns
browser-engine timings. A slim profile or a new threshold requires an owner decision.

## 3. Allowed paths and scope exceptions

```
host/**                       (default selection, shared facade, entry and loader)
kernel/src/glue/**            (internal snapshot/capture hooks if required)
kernel/src/mem/mod.rs         (crate-visible restore entry into the private allocator)
kernel/src/mem/ceiling.rs     (read-only freshness check on the private configuration and usage state)
kernel/src/mem/heap.rs        (allocator-owned checked page growth for restore only)
tools/**                      (snapshot, build stages, gate/pack walkers, measurement)
packages/bash-jco/**          (companion manifest and staging inputs)
test/**                       (snapshot, consumers, guard and decision fixtures)
plan/reports/GATE_L2.md
BRANCH.md
```

`wit/**`, the generated Rust bindings, parity corpora, exclusions and `Makefile` are read-only. Keep generated
artifacts under ignored `dist/` or `tmp/`. Apply the following exception only if the default manifest's
`exports` or `files` needs changing to pack the minified build. The second manifest declares its own package
name.

| Id | Path | Reason | Impact | Rollback |
|---|---|---|---|---|
| `BR<n>-EX1` | `package.json` | Route normal pack and consumers to the built entry graphs | Same specifiers, declarations and engine support, empty dependencies | Restore the preceding manifest and matching pack recipe |

## 4. Deterministic snapshot and restoration

1. **Capture the real default.** Build the exact production linked core with its release flags and command
   registry. Instantiate a fresh store, save its initialized memory (including nonzero data segments), page
   count, mutable globals and table state. Drive capture through L2b's first-party adapter, using its production
   lowering and allocation order for `configure(268435456n)`, `Vfs.create(134217728n)` and `Shell.create`
   with the validated, ordinary `contain` default config. Use its
   actual environment, grants, default process info, physical layout and command files. Retain both owned
   resources. Capture zero fibers or pending host calls. Host time, entropy, process IDs and build paths
   must never enter the image.
2. **Prove quiescence.** Finish all canonical post-returns. Require zero live borrows, fibers, pending host
   calls and deferred frees, `headroom().used() == 0`, reserve `OPEN == false`, `LOWERING == [None; 3]`. The
   instance-local kernel state in linear memory includes the allocator bins, accounting/budget links,
   ceiling, high-water and shell/VFS references. Assert the stack pointer is back at its initial value
   (1,048,576 on the measured L2 core), mutable globals outside memory equal their initial values and the
   indirect table has exactly its initial entries. Check via build-only inspection of the same layout. Unmodeled
   mutable state or a changed table fails capture. Do not fix an invariant by clearing live state.
3. **Encode the difference inside this core.** Compare post-setup bytes to their initial values, treating new
   pages as zero-filled. Emit sorted, bounded, disjoint changed ranges including nonzero-to-zero changes, their
   destination offsets and required growth as passive Wasm data/code. Bind all metadata and the two resource
   representations to this exact build inside the Wasm. Capture/replay must preserve allocator and Rust
   reference ownership. No shipped JS pointer constants, `refs` array, memory image, auxiliary Wasm download or
   pointer-bearing manifest. The private canonical resource table may hold the opaque representation received
   from a core intrinsic, as it does for normal creation.
4. **Private core ABI.** `__restore_default() -> i64` is a one-shot export of the loader core, invisible to the
   WIT. Only enter on a fresh, unused store before `configure`, allocation or any other mutating guest call.
   Before changing state, require `!RESTORE_ENTERED`, `!CONFIGURED`, `USED == 0`, pristine allocation
   high-water and the exact initial page count. With non-wrapping arithmetic, check that the build's final
   page count fits the 268,435,456 B default cap and that all image ranges fit those pages. A failed
   preflight returns `0i64` with memory, handles and latch untouched.
   Keep `RESTORE_ENTERED` in a per-store mutable global outside the captured image. Set it once after preflight
   and before `memory.grow` or `memory.init`. From glue, call a restore-only `pub(crate)` helper exposed by
   `crate::mem`. Its allocator-owned page primitive checks the final linear-memory byte size against the
   explicit 268,435,456 B default cap and grows exactly the required pages. Ordinary pre-`configure`
   allocations retain the 1 MiB `BOOTSTRAP_CEILING`. A refused growth returns `0i64` with the latch set,
   before any image write or handle registration. After successful growth apply the ranges with `memory.init`
   and drop the passive segment. Call the existing `[resource-new]vfs` and `[resource-new]shell` imports,
   in that order, with this image's own live representations. Pack the nonzero u32 handles in the i64
   (Vfs low 32, Shell high 32). JS checks for `0n` before unpacking both halves as unsigned. On success it
   verifies two distinct live handles of the right kinds, claims each exactly once with L2b's ownership/finalizer
   machinery, and populates fresh Holder/FS/Bash state. Initial handle IDs are local to the table.
   Never manufacture a wrapper from an address or reuse a template wrapper. A
   failed constructor discards its fresh store as one unit. A `0i64` refusal is a normal return, so a late
   probe on a live store leaves it usable. A trap during image installation or handle registration follows D8.
5. **Select on validated options.** Internal pure `isDefaultSnapshotConfig(normalized) -> boolean` uses only the
   already validated, single-read configuration. It requires `contain`, the standard ceiling, zero overrides
   including aliases, default layout/cwd, no supplied FS/files/env/commands/customCommands/processInfo, no
   sleep/fetch/network/grant changes, no trace/coverage and ordinary default runtime options. A validated
   host-only logger may be present. Omitted options, `{}`, explicit `contain` with empty overrides and
   `{logger}` qualify. `{commands: []}`, `{files: {}}`, explicit `fs`, caller env/cwd, non-default runtime
   flags, profiles other than `contain` and any override disqualify. Unknown/invalid options retain their
   synchronous constructor errors. Use ordinary setup for any unclassified option or internal override.
   Predicate has no I/O, callbacks, allocation in the guest or test of constructor arity.
6. **Select on module provenance independently.** Only the default binding with the package's own production
   loader core may restore. Record provenance at load/compile time, including browser compile cache and
   outstanding promise. `precompile(custom)` (bytes, URL, Response or Module), `hooks.kernel`, custom module
   providers or ceilings use the ordinary path even if the export exists. Preserve browser streaming/fallback,
   retry on compilation failure, staged getters, and ordered queued writes/registration. A staged default shell
   may restore at attach, before replaying later queued mutations exactly once. Node attaches eagerly. Precompiled browser shells support synchronous access. A standalone `InMemoryFs` takes the ordinary VFS-only path. The
   optional jco package uses ordinary initialization over the same final component.
7. **Pipeline order.** Produce the image and register its internal export before final component embed/new.
   Capture from the exact layout being restored. Appending a passive segment/helper must leave the captured
   memory layout and existing table indexes valid. Verify the loader core extracted from the final component has
   the helper. Any layout-changing optimization precedes capture or requires regeneration and equivalence proof.
   Generate the component once per build variant, then derive both bindings' loader cores from it. Changes to
   WIT, registries, allocator, release flags or core must regenerate the image. G3 compares two independent
   builds from different paths.

## 5. Package interface and size accounting

- `import { Bash } from "@sentropic/bash"` and `"@sentropic/bash/browser"` select first-party by default.
  `"@sentropic/bash-jco"` and `"@sentropic/bash-jco/browser"` select Option D explicitly. Match the
  corresponding public runtime exports, TS declarations and supported Node `require` behavior. Stage declarations
  for both tarballs directly from the root `types/**` tree at pack time. The companion has no separately
  maintained `.d.ts` files. Each package installs alone offline. Build the facade from one source graph with
  a binding-specific entry at build time. Exclude the alternative binding from each tarball. No runtime
  selector/dependency or postinstall build.
- Stage readable sources as deterministic, minified shipped ESM using the lockfile's `oxc-minify`. Factor shared
  code into one browser-safe module per package. Preserve externally observable names/types, error classes and
  symbol methods. Keep the actual synchronous deferred lookup of `node:fs` / `node:url` via
  `process.getBuiltinModule` inside `host/node/`, including **after bundling**. `host/index.js` imports that
  adapter. `host/browser.js` and the shared chunk have no transitive path to it. Preserve host-path redaction on first use. The first Wasm read occurs on construction.
- Keep Option D's pinned jco 0.24.0 synchronous glue and small generated resource shims. Guard exactly the
  existing 14 JSPI probes (13 post-return probe sites and 1 realloc site) in both minified and readable
  generator shapes. With JSPI present preserve invocation and catch behavior. Fail the build on generator drift.
  G5 requires one loader core per kernel glue directory and zero base64 literals matching the gate. The default binding uses plain
  descriptors and needs no shim. Keep raw Rust sources, test/probe kernels, jco toolchain and scratch images out of
  both packages.
- Use the normal build/pack path for each deliverable. Compare the installed trees from
  `npm pack --offline --json --ignore-scripts` with their manifests. Run G2/G5 on shipped paths as well as
  source/build gates. Each contains the same `dist/kernel.component.wasm` and `wit/sentropic-bash.wit` byte for
  byte. Assert SHA-256 equality with the local OCI *payload* input and normalized embedded world equality
  (G1/G4). Compare both loader cores with the main core extracted from that component. At release compare the
  downloaded OCI layer payload digest with the npm component, and check WIT content. Leave registry publication
  to the release gate.
- Compute `B` = raw bytes in the browser-reachable shipped JS graph including shared code, glue and registry. Let
  `N` be the raw bytes of all remaining shipped JS. Resolve literal imports, re-exports and lazy chunks, count each
  shipped path once, and reconcile to the complete pack manifest. `T = B + N`, `Z` = sum of gzip level-9 byte
  lengths **per shipped file**. Use decimal bytes. Report B, N, T, Z and per-file raw/gzip for both packages.
  The default package alone carries the Annex H gate. Fixed bounds are `N <= 40000`, `T <= 60000`, `Z <= 30000`.
  Always print separate pass/fail and signed headroom against `B <= 45000` and `B <= 58000`. Never aggregate the
  two tarballs or silently choose the higher cap.

## 6. Binaryen measurement and Gate L2 rerun

**Owner pin pending.** In the unapproved/declined arm keep `wasmOpt: false` and report the no-opt core,
component, timing and v1 projection. Mark the optimized numbers unmeasured. A transitive Binaryen executable
gives no authority to run/enable it. In the approved arm verify the owner's exact Binaryen 132 version and
digest/integrity offline. Use `wasm-opt -Oz` on the linked core before componentization. Derive the smallest
validating feature set (L2 section 9.3), record exact flags/pin, rebuild the build-matched snapshot, component,
loaders and both packages. For both matched variants report raw/gzip loader core and
component, deltas, M1 and M3a. Retain only if `raw_after <= 0.90 * raw_before` on the shipped loader core, the
full suite and G1–G8 pass and neither M1 nor M3a changes from passing to missing. Otherwise ship the verified
no-opt variant. Reproject v1 with the snapshot counted once and the remaining code itemized in L2 section 9.4.
Recommend owner review of `slim` if either projected bound is exceeded.

Run `node tools/measure-l2.mjs --variant committed` on the **installed default minified release** produced
above. Extend the driver to stage/install it offline. Record a matching Option D control. Use `--variant wasm-opt`
only in the approved arm. Report OS, CPU/scheduler, Node, rustc and pins, revision/dirty flag, binary and JS
hashes and per-variant manifest. `LC_ALL=C`, `TZ=UTC`, local `TMPDIR`, normal OS scheduler, no overlapping
diagnostic jobs. Keep raw data in `tmp/bench-l2/<variant>/results.json`.

1. Use seeds `0x6c320001`, `0x6c320002`, `0x6c320003`, 30 shuffled rounds per seed of fresh empty ESM,
   import-only and import/new/first async `echo hi` children. Time from import start, exclude process launch,
   include all lazy I/O and compilation. Verify stdout `hi\n`, stderr empty and exit 0. Report cold sub-times
   and `M5 = median(full RSS) - median(empty RSS)` per seed.
2. In independent warm children discard 20 iterations per workload, then collect at least 1,000 samples **and**
   at least 2,000 ms each. Shuffle/balance workload order across seeds. Measure eager M2, directly timed
   `new Bash()` **through its own first verified** async echo, reused async M3a and sync echo, and a real noop
   custom-command call/resume. Give each seed's interpolated median, nearest-rank p95, minimum and sample count,
   then the median of each statistic over the three seeds. Report constructor retention/GC policy. The paired
   interval includes deferred work. Report it directly as a diagnostic.
3. The guarded L2 release measured M1 53.368 ms, M2 0.724 ms, M3a 0.09554 ms, M5 12,677,120 B, T
   343,594 B. The L2 one-command physical-image prototype measured 18.70 / 0.097 / 0.031 ms, 3.77 MB, paired
   0.151 ms. Its B/N/T/Z byte counts were 44,150 / 962 / 45,112 / 18,568 with compact prototype descriptors.
   Plain descriptors and both repository scans are mandatory on the release build. At 85 synthetic registrations, B was 44,831 B and M2/paired were 0.112/0.166 ms. The published
   reference measured 0.573/1.283 ms at 85 commands and 0.059/0.680 ms with echo alone. Identify the catalog for
   each comparison. Only the packed production registry enters the verdict. The synthetic catalog measures
   registration cost, and the browser smoke gives functionality only.

**Decision.** Require `make check` green, 30/30 parity cases and all four profile suites before issuing a new
verdict. A correctness or conformance failure blocks lot exit. Compare unrounded median-of-run medians against
the following inclusive kernel thresholds.

| M1 cold | M2 warm construction | M3a reused async echo | M5 RSS delta | M6-wasm (loader core) |
|---:|---:|---:|---:|---:|
| 25.0 ms (15.0 stretch) | 0.20 ms | 0.070 ms | 10,000,000 B | 1,500,000 B raw **and** 700,000 B gzip |

- **No-go (kernel miss).** Any kernel threshold misses, whatever caused it. Keep W4 and later builds paused.
  Report the missed metric, profile and measured choices to the owner. Spec writing can proceed.
- **Go.** All kernel and fixed JS thresholds pass, and the browser graph passes the selected cap, or both
  candidate caps while the choice is pending. W4 (L3 and L8a) can start.
- **Go on kernel, owner review of the JS budget (JS-only miss).** Kernel passes, but a fixed JS bound or the
  selected browser cap misses. While the choice is pending, use this verdict whenever browser JS exceeds
  45,000 B, including a miss at both caps. W4 proceeds. Report both Annex H outcomes. At exactly 45,000 B both pass, at 58,000 B only the higher passes,
  above 58,000 B both fail. An unresolved pin uses the verified no-opt release. L7c is outside the Gate L2
  critical path.

## 7. Executable exit gates

`npm_config_cache=$PWD/tmp/npmcache make check` runs all functional gates. The builder runs the benchmark
separately on the green release artifacts.

1. **Snapshot.** Two independent G3 builds give identical snapshot core, component, binding and packaged JS
   bytes. Compare restored versus ordinary setup through the same first-party binding for env/export order,
   cwd, command names, physical bytes/stat/mode/inodes in both stub directories, dirty state, quota,
   allocator usage/ceiling/high-water, first write/delete and first echo. A synthetic initialized nonzero word
   reset to zero survives restoration.
   Exercise post-return and explicit disposal of both owners. No raw representation/offset sidecar in either
   pack.
2. **Isolation.** Two restores produce two distinct memories and branded handle tables, with zero spare stores.
   Mutation or real test-instrumented trap in one leaves the other usable. Inject the trap in a restore-capable
   test build without enabling the production fast path for arbitrary test hooks. Verify a fresh restore can
   grow past the 1 MiB bootstrap ceiling. A second restore and a restore after ordinary initialization both
   return `0n` with identical page count, memory bytes and handle table before and after. The next ordinary
   call succeeds. Also check refusal immediately after `configure`, while `USED` is still zero. Force a page
   growth refusal in a fresh fixture. Assert `0n`, zero image writes and zero resource registrations. A retry
   on that core attempts no growth. Failing construction exposes zero usable half-created shells.
3. **Predicate and loaders.** Count 1 restore and 0 ordinary setup calls for each eligible configuration on Node
   and the packaged default browser URL after precompile. For each disqualifier in section 4, count 0 restores
   and confirm normal semantics or the constructor error. Include logger delivery, filtered commands, a small
   ceiling, invalid option, staged writes before compilation and caller-supplied modules with and without the
   helper. After precompile `execSync("echo hi")` returns exit 0 through either entry.
4. **Packages and guard.** Install each of the two packed files independently in an empty consumer, exercise
   Node ESM/CJS, browser precompile/async/sync, and `tsc --noEmit`. Compare each installed Node/browser
   declaration and its relative imports byte for byte with the corresponding root `types/**` files.
   Zero third-party dependencies or install scripts, identical component and WIT hashes, G1–G8/K1 pass.
   In both guard shapes test JSPI absent, present and throwing, and a changed count/shape fixture that fails
   generation. Replay 30/30 cases through each
   package. All four limit profiles and the L2b differential suite pass.
5. **Gate logic and measurements.** Test the verdict function at equality and just over each bound, including
   each browser-cap interval, a simultaneous kernel/JS miss, and a red correctness suite. Attach the three run
   values, paired workload, package split and optimizer comparison/status. Record exactly one current verdict
   with its W4 consequence in `plan/reports/GATE_L2.md`.

## 8. Points the builder must confirm and report

Report stack/table invariants, initial/final page counts, changed range count, snapshot raw/gzip delta, observed
handles/kinds and resource counts. Record pack manifests/digests, G2/G5 results and the release payload used for
the local OCI comparison. Give both browser headrooms and pin status even on a GO. Carry the v1 size projection,
real-browser timing and remote OCI verification to their named gates.
