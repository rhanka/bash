# Gate L0 report

Verdict: **Go with fallback** (hand-written canonical-ABI glue for the probe; `jco-transpile` alone was sufficient).
Criteria 5 (CI green on the pull request) and 6 (report reviewed) are open: they belong to the conductor and the reviewer.

## 1. Toolchain and tools actually used

| Item | Version | Hash / source |
|---|---|---|
| rustc / cargo | 1.96.0 (ac68faa20 / 30a34c682, 2026-05-25), target `wasm32-wasip2` | rustup, `rust-toolchain.toml` |
| `wasm-component-ld` (rustup, present, unused by the final recipe) | 0.5.22 | toolchain |
| `wasm-tools` | 1.263.0 | archive SHA-256 `21c036c1740a0552124c79c6e2170cb61c5b016005671a6d5bc06cf642165431` (`.verified` marker matches the pin) |
| `wit-bindgen` CLI | 0.63.0 | archive SHA-256 `bf4efe883ab70d34a4740253911dda20caf895d58a003af58369506b04684cbe` (marker matches) |
| `@bytecodealliance/jco-transpile` | 0.24.0 | lockfile integrity `sha512-ISCXvZc+...puBSA==` (matches the spec) |
| `typescript` | 7.0.2 | lockfile integrity `sha512-8FYau96o...RVUNA==` (matches the spec) |
| `tools/package-lock.json` | lockfileVersion 3 | SHA-256 `c2140a2b703074f1d8f54ce5cfa6944731deaa5ed9f3b873dcc40f996c1324f6` |
| node / npm | 22.22.1 / 11.21.1 | machine |

`fetch-tools.mjs --offline` verifies all three groups against `SENTROPIC_TOOLS_DIR=/home/antoinefa/src/bash/tmp/tools`. No download was made in this lane.

## 2. Link proof (spec 6.2)

Attempt 1 (the first and only one needed; no variation was tried):

```
RUSTFLAGS="-C link-self-contained=no -C link-arg=--no-entry -C link-arg=--skip-wit-component --remap-path-prefix=<root>=."
CARGO_TARGET_DIR=<out>/target cargo build --release --offline --target wasm32-wasip2 -p sentropic-bash-probe
wasm-tools component embed probe/wit --world probe <core> -o <tmp>
wasm-tools component new <tmp> -o <out>/probe.component.wasm
wasm-tools validate <out>/probe.component.wasm
```

Result: **linked**. The core module has magic `00 61 73 6d 01 00 00 00`, no import section entry (`wasm-tools print | grep -c '(import'` prints 0), and exports `memory`, `cabi_realloc`, `cabi_post_echo-bytes`, `echo-bytes`, `grow`, `ping`. The component has magic `00 61 73 6d 0d 00 01 00`. Printed world, no `import` line:

```
package root:component;

world root {
  export ping: func(x: u32) -> u32;
  export echo-bytes: func(data: list<u8>) -> list<u8>;
  export grow: func(pages: u32) -> u32;
}
```

Retained recipe: the one above, implemented in `tools/build.mjs`. The printed package is `root:component` (the embed step does not carry the probe package name); G4 therefore compares the world's import and export name sets, and runs `wasm-tools component targets`.

Extra check (spec section 12): the same build **without** `-C link-self-contained=no` yields a byte-identical component. For a `no_std` crate no C library is linked in either case, so the flag has no effect on the probe. It is kept as specified; its effect on a crate that links `std` is unknown (the kernel is `no_std`).

## 3. Bindings: generated or hand-written

**Hand-written (EVOL D16 fallback).** `wit-bindgen rust` 0.63.0 with the spec's options (`--std-feature --runtime-path crate::rt --bitflags-path crate::rt::bitflags --disable-custom-section-link-helpers`; all five option names exist) generates glue that does `extern crate alloc`, builds a `Vec<u8>` with `Vec::from_raw_parts` and frees with `alloc::dealloc`. That needs the `alloc` crate and a `#[global_allocator]`, which the probe must not use (G6 checks it). A runtime crate is not needed, but the heap is. L1b must decide where the heap lives: the generated glue is acceptable under `kernel/src/bindings/` (G6 exempts it) provided the kernel supplies a global allocator. `--check-bindings` is implemented and reports "none committed" for the probe; it activates when a component lists `bindings` in `tools/components.json`.

## 4. `jco-transpile` alone: sufficient

Yes; the full `jco` CLI is not needed. Options matched against `transpile.d.ts` (0.24.0): `instantiation: "sync"`, `base64Cutoff: 0`, `validLiftingOptimization: true`, `tlaCompat: false`, `minify: false`, `wasiShim: false`, `nodejsCompat: false`, `optimize: false`; no import map. Files produced: `probe.js`, `probe.d.ts`, `probe.core.wasm` (exactly one `.wasm`). `instantiate(getCoreModule, imports, instantiateCore, options)` is exported and synchronous; no top-level `await` (all 20 `await` occurrences are inside async functions); no static or dynamic import in the generated file. The runtime is embedded: the generated module is large for a 1 KB component (section 5).

## 5. Sizes

| Artefact | Bytes |
|---|---|
| probe core module (`probe.core.wasm`) | 510 |
| probe component (`probe.component.wasm`) | 1 004 |
| core module copy in `js/probe/` (rewritten by jco) | 559 |
| `probe.js` raw / gzip -9 | 103 718 / 20 949 |
| `probe.d.ts` raw / gzip -9 | 2 268 / 1 041 |

Risk: the embedded jco runtime is about 100 KB raw, a fixed cost for every component (async tasks, resources and so on, unused here). This bears on the startup budget.

## 6. Annex A validation

`wasm-tools component wit wit/` accepts `wit/sentropic-bash.wit` (package `sentropic:bash@0.1.0-draft`, interfaces `types` and `engine`, world `kernel`) with no error. No syntax fix was needed; the contract is copied verbatim from EVOL Annex A.

## 7. Gates and artefact hashes

```
G1 zero-import: OK
G2 no bare import: OK
G3 two-build identity: OK
G4 component validity: OK
G5 one core module: OK
G6 zero crate, heap discipline: OK
G7 manifests: OK
G8 tool pins: OK
```

G3 builds from two copies under different paths (`tmp/g3-a/copy-one`, `tmp/g3-b/nested/copy-two`); both produced identical hashes, equal to those of the build in the worktree itself:

```
7d9dda28b82467dbbf54c59be0c0b9747de53647f7d58c7113fdc82acdb1afcb  js/probe/probe.core.wasm
fe6cb9c803e158574e38077d2902a0a9edbef2bd24cf9ae1ce1293577f5f8079  js/probe/probe.d.ts
4a9d709e72e78c4c22fadb65c7252c400794cd36ba5ba48079cc5ad16f3679f3  js/probe/probe.js
fb0a28aa22da243731bbd0cedcc4af7dd27e3a3b91fa4d8d5725b9cf89f645fa  probe.component.wasm
588c05fcab3ea12f6ccbe46b50bfb1cbafcd94a7364909b54897334ccb28c04b  probe.core.wasm
```

`make check` exits 0 and runs 47 tests in 9 files, all passing (probe, entries with `import` and `require`, gates on fixtures, pack with offline install, fetch-tools).

## 8. Section 12 confirmations

- `wit-bindgen rust` options: confirmed with `--help` and a run (section 3).
- `transpile` options: confirmed against `transpile.d.ts` 0.24.0 (section 4). Note `transpile(path, opts)` takes a path; `transpileBytes` takes bytes.
- C library: see section 2; no libc is linked for a `no_std` crate, with or without the flag.
- Synchronous instantiation on node 22: `pipelineProbe()` instantiates synchronously on node 22.22.1, including the 70 000-byte round trip and a memory growth between two calls. Only 22.22.1 was available; the lower bound 22.12 is unverified.
- `require` of the ES module works on 22.22.1 with no CommonJS wrapper (the `require` test passes). Across the range it relies on `require(esm)` being unflagged from 22.12; unverified below 22.22.1.

## 9. Deviations from the spec

1. `npm ci` replaced by `npm install --ignore-scripts --no-audit --no-fund` in `<tools>/npm/` (conductor instruction: `npm ci` fails with TypeScript 7 in that layout). Before install, the root `devDependencies` of the lockfile must equal `tools/package.json`, and every package must carry a `sha512-` integrity and an https `resolved` URL. After install, the staged lockfile is compared by resolution (package path, version, resolved URL, integrity), not by bytes, because npm versions rewrite other metadata; gate G8 repeats the comparison.
2. `node --test test/` does not work on node 22.22.1 (the directory is run as a module). `tools/run-tests.mjs` walks `test/**/*.test.mjs` recursively and is the single runner for `make test` and `tools/check.mjs`.
3. G3 copies `git ls-files --cached --others --exclude-standard` (tracked plus untracked, not ignored) instead of tracked files only, so it also covers a tree before commit; on a clean checkout the two sets are equal.
4. `fetch-tools.mjs` accepts `file:` URLs and the `SENTROPIC_PINS` and `--skip-npm` test hooks, used by `test/fetch-tools.test.mjs` (no network in the test).
5. Hand-written probe glue (section 3).
6. `.github/workflows/check.yml` pins every third-party action to a full commit SHA (checkout, setup-node, cache), with the release tag in a comment. Checkout uses `fetch-depth: 0` and `persist-credentials: false` so the attribution check sees the PR commits; the toolchain step runs `rustup toolchain install && rustup target add wasm32-wasip2`.
7. Branch id: the conductor gave no branch number, so the exceptions are `BR1-EX1` to `BR1-EX3`.
8. `plan/reports/GATE_L0.md` was added to the allowed paths, as section 11 says.

## 10. Open risks

- L1a-1: the probe's bump allocator never frees and never reuses; `kernel::mem` needs a real allocator. The 17-page initial memory (1 MiB default stack plus data) is a size baseline to review; stack size is a linker argument.
- L1b: generated glue needs `alloc` and a global allocator (section 3); decide the glue strategy before the facade. The jco runtime size (section 5). The generated `d.ts` names the world `root:component/root`; the facade should alias types. Package name of the embedded world is `root:component` unless the embed step is given a package.
- L2: `wasm-opt` is a no-op hook (`wasmOpt: false`; setting it to true throws until the tool is pinned). The pinned jco-transpile has an `optimize` option that was left off.
- Reproducibility is proven for a `no_std` crate only; a kernel with `core::fmt` panics paths may embed paths (the remap and `strip = true` are in place, to be re-proven by G3 at each lot).
- CI is unproven (not run on a GitHub runner yet). Local simulation: tests run with `SENTROPIC_TOOLS_DIR` unset and `tmp/tools` linked to a verified tools directory. The `.verified` marker now holds the archive hash and the hash of the extracted executable; the executable is re-hashed on every check, so a restored cache is not trusted on the marker alone.
