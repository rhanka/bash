# SPEC EVOL — @sentropic/bash sandbox (rung 2: evolution spec)

Status: normative. Derived from `spec/SPEC_STUDY_BASH_SANDBOX.md` v3.2 (called "the study" below), which
holds the evidence, measurements and alternatives; this document holds the decisions. Where both speak,
this document wins. Per-lot cost and model routing belong to the plan rung (study section 7 is its input).

Conventions:
- MUST / MUST NOT / SHOULD are normative. "Gate" means a check that fails `make check` or the release.
- `JB:` = path under `tmp/refs/just-bash/packages/just-bash/` (reference clone, v3.6.0). Only
  reference-reading lanes open it (D20).
- "Posed by" is `Owner` or `Fable 5.2 + Carbon (consensus)`. Track ids are decision aggregate ids in
  `.track/events.jsonl`; "—" means the decision is recorded in the study and `BRANCH.md` only.
- Lot ids are those of study section 7.
- Items marked **new in EVOL** refine the study and are settled at the review of this document.

## 1. Decision index

| D | Title | Posed by |
|---|---|---|
| D1 | just-bash 3.6.0 is the behavioural authority | Owner |
| D2 | Kernel: Rust, `no_std`, WASI 0.2 component with zero imports | Owner |
| D3 | WIT contract `sentropic:bash@0.1.0-draft`, frozen after L2 and L1c | Consensus |
| D4 | Execution model: resumable fibers, inverted control, epochs | Consensus |
| D5 | VFS ownership and `bash.fs` bridging | Consensus |
| D6 | Memory: own allocator, budgeted fallible growth, chunked transfers | Consensus |
| D7 | Limit profiles; `contain` is the default | Owner |
| D8 | Trap contract, journal, compaction | Consensus |
| D9 | Capability model and revocable membrane | Consensus |
| D10 | Host filesystem adapters | Consensus |
| D11 | Network adapter | Consensus |
| D12 | Regex and glob engines | Consensus |
| D13 | Command scope, tiers and single kernel binary | Owner / Consensus |
| D14 | JS facade API surface and tiers | Consensus |
| D15 | Runtimes: Node, browser, wasmCloud | Owner / Consensus |
| D16 | Rust bindings from the pinned `wit-bindgen-cli` | Owner |
| D17 | Release pipeline, tools and gates | Owner / Consensus |
| D18 | Published artefacts | Owner |
| D19 | Oracle and parity protocol | Consensus |
| D20 | Clean-room lane rules | Owner |
| D21 | Security testing and fuzzing | Owner / Consensus |
| D22 | Benchmark protocol and budgets | Owner / Consensus |
| D23 | Divergence list | Owner |
| D24 | Go/no-go points | Consensus |

## 2. Decisions

### D1 — just-bash 3.6.0 is the behavioural authority
- **Decision.** Observable behaviour of `just-bash@3.6.0` is the truth: stdout, stderr, exit code, final
  environment and filesystem effects of a script, and the public API semantics. GNU bash is not a target.
  Arithmetic reproduces the reference's JavaScript-number results exactly: IEEE doubles for
  `+ - * / % **` (division truncated), signed 32-bit for bitwise and shifts, literals clamped to 2^53 - 1,
  and JavaScript number-to-string formatting (`$((2**70))` prints `1.1805916207174113e+21`). Scripts longer
  than 1 000 000 UTF-16 code units are rejected as the reference does.
- **Rationale.** The owner owns upstream features; one authority keeps the oracle free of exceptions.
- **Posed by.** Owner. **Track.** `01M7FTJAPDYBPMY9G0TFFSXBN6` (governance), `01M7FY8J77MKHWJ65HMYBKWC85`
  (arithmetic).
- **Acceptance.** Parity suite (D19) green; an arithmetic snapshot set covers values beyond 2^53, 32-bit
  shifts, division and modulo signs; the number formatter matches the oracle on a generated set of doubles.
- **Binds.** L3, L4, L5, L8b, L10a, L10b.

### D2 — Kernel: Rust, `no_std`, WASI 0.2 component with zero imports
- **Decision.** The kernel is one Rust workspace compiled to `wasm32-wasip2` as a Component Model
  component. It MUST import nothing (no `wasi:*`, no custom import). It is `no_std` + `alloc` on wasm and
  builds with `std` on the native target for tests. `Cargo.lock` MUST contain only workspace packages. No
  `wasm32-unknown-unknown`, no `wasm-bindgen`, no async runtime.
- **Rationale.** Owner constraints A1; zero imports gives default deny at the binary level and lets the
  generated JS have no external module (study 2.3).
- **Posed by.** Owner. **Track.** `01M7E9EB8FAAH8B4CX2V5VQ293`.
- **Acceptance.** Gates G1, G2, G6 of D17. `cargo test` passes natively.
- **Binds.** L0, L1a, L1b, every kernel lot.

### D3 — WIT contract `sentropic:bash@0.1.0-draft`, frozen after L2 and L1c
- **Decision.** Annex A is the contract. It stays `0.1.0-draft` until L2 and L1c have exercised the
  binding generator, the JS glue, a `wac` composition and the boundary costs; it is then frozen as `0.1.0`.
  After the freeze, additive changes bump the patch and breaking changes the minor, until 1.0. Payloads
  are `list<u8>`; `string` is used only for identifiers.
- **Rationale.** The toolchain is unproven on this contract; nothing is published before the freeze, and
  only L1b and L1c depend on it.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** `wasm-tools component wit` accepts the WIT; the component's embedded world equals the
  committed WIT; a freeze commit follows the L1c report.
- **Binds.** L0 (validation), L1b, L1c, L2.

### D4 — Execution model: resumable fibers, inverted control, epochs
- **Decision.** Each `exec` is a `fiber` resource with its own explicit-stack VM, shell state, fd table and
  budget. `run(fuel)` returns `done`, `out-of-fuel` or `host-call`. A host call is a *request returned to
  the driver*, which answers with `resume(epoch, outcome)`; the kernel never calls out. Concurrent execs on
  one shell are distinct fibers that interleave only at host-call and fuel boundaries. Re-entrant execution
  from a custom command creates a child fiber (`parent`), charged to the parent's aggregate budget.
  In-kernel nesting (`bash -c`, `eval`, `source`, `xargs`, `find -exec`, `timeout`, `time`) stays inside
  one fiber. A `resume` with a stale epoch MUST be rejected.
- **Rationale.** The reference API is asynchronous and WASI 0.2 is synchronous; this shape needs no JSPI
  and gives fuel, abort and stack-overflow immunity (study 2.1, 2.3).
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** Tests: two overlapping execs on one `Bash`; a custom command calling `ctx.exec` while
  suspended; a stale `resume` returns `stale-epoch`; recursion depth 10 000 in a script returns a limit
  result, not a trap.
- **Binds.** L1b, L2, L5, L9a-2.

### D5 — VFS ownership and `bash.fs` bridging
- **Decision.** An exec runs in the instance that owns its root VFS. `new Bash()` creates an instance with
  one `vfs`. A standalone `InMemoryFs` owns a `vfs` in its own instance; a `Bash` built on it attaches a
  `shell` there, so several `Bash` objects can share one `InMemoryFs`. Grants, command registry and budgets
  are per `shell` and per fiber, never per instance. `bash.fs` and `InMemoryFs` implement `IFileSystem`
  over the synchronous `vfs.op`, returning resolved promises. Any other `fs` is host-backed and reached by
  `fs-batch` host calls. `MountableFs` maps to `mount-vfs` / `mount-host`. Lazy files are stubs
  materialized by a `lazy-file` host call. The default layout of the reference (`/home/user`, `/bin`,
  `/usr/bin`, `/dev`, `/proc`, default env) is created by the kernel.
- **Rationale.** Zero-copy for the default case; exact reference semantics for shared filesystems.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** API tests for shared `InMemoryFs`, `bash.fs` mutation between execs, mounts, lazy
  files, default layout snapshot against the oracle.
- **Binds.** L1b, L2, L7a.

### D6 — Memory: own allocator, budgeted fallible growth, chunked transfers
- **Decision.**
  1. The kernel allocator is the only caller of `memory.grow` and enforces the instance ceiling
     (`configure`, `raise-ceiling`). No link-time memory maximum is set.
  2. Untrusted-sized data MUST grow through a budget charge followed by a fallible reservation. A breach
     unwinds the fiber and returns a limit result; the instance stays valid. Infallible growth
     (`push`, `format!`, `collect`, `to_string`, ...) on such data is banned by a source gate.
  3. Files, stdio and pipeline buffers use chunked storage.
  4. `cabi_realloc` is charged to the instance headroom, MUST trap rather than return null, and is kept
     unreachable-on-exhaustion by drivers: every driver checks sizes before lowering, and no single
     canonical-ABI value exceeds 1 MiB (stdin, file data, bodies and outputs are chunked).
  5. `exec-oneshot` inputs plus outputs are capped at half the profile's headroom.
- **Rationale.** Limits must be results, not traps (study 2.4); the canonical ABI has no failure channel.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** Allocator stress (10 000 alloc/free cycles, fragmentation bound reported); every byte
  limit of D7 has a test that ends in a limit result with the instance still usable; a driver test sends
  an oversized payload and gets `limit-exceeded` without a trap; memory grows mid-call without stale views.
- **Binds.** L1a, L1b, L2, all command lots.

### D7 — Limit profiles; `contain` is the default
- **Decision.** Annex B. Limit names and the `hardened`/`normal` values follow the reference;
  `contain` and `micro` are additions. `contain` is the default in every environment. Explicit overrides
  that do not fit the ceiling are rejected at construction. Kernel fuel is the deterministic CPU limit;
  wall time is checked by the driver between fuel slices.
- **Rationale.** Byte caps must be coherent with one ceiling that fits a 256 MiB host cap; hardened step
  limits break ordinary scripts.
- **Posed by.** Owner. **Track.** `01M7FTJ8W993KN9DJ3WNTKP0VY`.
- **Acceptance.** Per-profile tests of each cap; `normal` selectable by name and reproducing reference
  limit behaviour on the oracle's limit cases; latency of limit enforcement measured (bench M7).
- **Binds.** L1a, L1b, L2, L11.

### D8 — Trap contract, journal, compaction
- **Decision.** A trap is a kernel defect. After a trap the instance is poisoned: the exec returns a fatal
  result, every pending fiber fails, later calls reject with a typed error, and no VFS content is salvaged.
  An opt-in journal mirrors, at each exec completion, the files reported by `take-dirty`, and can rebuild a
  fresh instance. `bash.compact()` rebuilds an instance from the VFS to release retained memory. In a
  `wac` composition a kernel trap aborts the whole invocation.
- **Rationale.** Memory is untrusted after a trap; linear memory never shrinks.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** A test-only build with a trapping command proves poisoning, journal rebuild and
  compaction; any trap found by fuzzing blocks the release.
- **Binds.** L1b, L7a, L11.

### D9 — Capability model and revocable membrane
- **Decision.** Default deny. A shell holds `grants`; a host call for an ungranted capability is answered
  inside the kernel with a permission error and never reaches the driver. Capability table: study 4.2.
  Clock, timezone, entropy, pid/uid and hostname are virtual unless granted (UTC by default). The driver
  gives async extensions revocable proxies; on abort, deadline or limit it cancels the fiber, revokes the
  proxies and drops late resumes. `trusted: false` custom commands get a read-only copy of the env and
  budgeted output; the documentation states that custom commands are embedder code with ambient JS access.
- **Rationale.** Study 4.1-4.2; the reference's revocation discipline is kept, its global patching is not.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** For each capability: a denied test with a recording driver that sees no call; a late
  continuation after abort cannot mutate the VFS or resume the fiber.
- **Binds.** L1b, L7b, L11, L16b.

### D10 — Host filesystem adapters
- **Decision.** `OverlayFs` and `ReadWriteFs` are host-side JS (Node only). Every path goes through root
  containment on canonical paths; null bytes rejected; symlinks denied unless opted in; only regular files
  and directories are opened; atomic exclusive create; broken-symlink leaf check; hard-link copy-on-write;
  mutations on overlapping roots serialized; host paths stripped from errors. Calls are batched per
  directory.
- **Rationale.** Study 1.5, 4.2.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** Adversarial suite: traversal, symlink and special-file escapes, exclusive-create races,
  error-message leakage; a 1 000-file `find` costs at most one host round trip per directory.
- **Binds.** L7b, L11.

### D11 — Network adapter
- **Decision.** `curl` exists only when the network capability is granted. The Node adapter uses node
  built-ins only: URL-prefix and method allow-lists, every redirect re-validated, response size cap,
  resolution with `node:dns`, validation of the *resolved* addresses (private, loopback, link-local,
  metadata, IPv4-mapped IPv6, non-canonical literals), connection pinned to the validated address,
  credentials stripped on cross-origin redirects. It ships inside `@sentropic/bash` as a lazily imported
  module so that `new Bash({ network })` works without another install (**new in EVOL**; replaces the
  separate `@sentropic/net` package of the study). In wasmCloud, SSRF policy is the HTTP provider's.
- **Rationale.** The reference delegates this to a third-party package; drop-in use must not need one.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** Tests against a local server: redirect to a denied prefix, rebinding between check and
  connect, oversized body, method not allowed; reference `NetworkConfig` options accepted unchanged.
- **Binds.** L16b, L11.

### D12 — Regex and glob engines
- **Decision.** One linear-time regex engine (BRE, ERE, common Perl classes) with compile-time caps on
  program size and DFA cache; backreferences through a bounded backtracker charged in fuel; no host
  `RegExp`; lookaround under `grep -P` is unsupported with a clear error. One glob/extglob matcher.
- **Rationale.** ReDoS-free by construction, same class as the reference's engine.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** Blow-up patterns (matching and compilation) end within fuel; parity snapshots for
  `grep`, `sed`, `awk`, `[[ =~ ]]`.
- **Binds.** L8a, L9a-1, L13, L14.

### D13 — Command scope, tiers and single kernel binary
- **Decision.** One kernel component contains every command; companions are JS facades over it. Tier 1
  ships first, then `awk`/`jq`, then tier 3 (lists: study 3.1). `yq` supports all six reference formats
  (YAML, JSON, XML, TOML, INI, CSV). Not in v1: `python3`/`python`, `js-exec`/`node`, `sqlite3`, `xan`,
  `html-to-markdown`, `@just-bash/executor`; `python: true` or `javascript: true` MUST throw a typed
  "not supported" error at construction (**new in EVOL**). A `slim` build is decided after L2.
- **Rationale.** A second binary would copy data across memories; unused code costs only validation.
- **Posed by.** Owner (scope, `yq`) and Fable 5.2 + Carbon (single binary). **Track.**
  `01M7FTJ9RGM778N1T0NNKFP8EF` (`yq`), `01M7E4ZA5A1YDQD0N3459M3XN6` (packages).
- **Acceptance.** `getCommandNames()` equals the reference list minus the excluded commands; the parity
  table marks each command `covered`, `partial` or `not covered`; each `yq` format has read and write
  snapshots.
- **Binds.** L6a-L9b, L13-L16d, L18.

### D14 — JS facade API surface and tiers
- **Decision.** Annex D. The facade is hand-written ESM JavaScript with JSDoc and hand-written `.d.ts`,
  with no dependency; declarations are checked with `tsc --noEmit` on consumer fixtures. Tier A and B are
  v1. `exec` never throws for script errors (reference contract). `execSync` is an addition on Node that
  throws a typed error when an async capability is configured. `parse()`, AST types and the transform
  pipeline are not in v1.
- **Rationale.** Drop-in for `Bash.exec` users; no build step for the host.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** API tests per row of Annex D; `tsc --noEmit` on fixtures that import the reference's
  type names; the type check fails closed when the tool is absent.
- **Binds.** L1b, L7a, L10a, L17a.

### D15 — Runtimes: Node, browser, wasmCloud
- **Decision.**
  - **Node >= 22.** Core module read from disk and compiled synchronously once per process; generated
    `instantiate` called with an empty import object.
  - **Browser.** Streaming compile; a staging queue records operations issued before the instance exists
    and replays them at the first awaited call; synchronous reads before that need
    `await Bash.precompile()`. Every kernel command is exposed, a superset of the reference's browser
    bundle.
  - **wasmCloud.** Stateful sessions by static `wac` composition of an orchestrator with the kernel;
    stateless calls through `exec-oneshot`. Optional Rust session drivers with fixed capability sets
    (`session-pure`: no WASI import; `session-fs`; `session-net`) are shipped unplugged.
- **Rationale.** Owner constraints A1; study 2.5.
- **Posed by.** Owner (runtimes, topology, browser command set); Fable 5.2 + Carbon (mechanisms).
  **Track.** `01M7FTJ9HB55X3WBF2KP008SQW` (topology), `01M7FY8HXSPE3M3K68R90EQYHM` (browser commands).
- **Acceptance.** Node API suite; browser suite in headless Chrome for Testing driven over the DevTools
  protocol; `wasmtime` smoke test of kernel + `session-pure` composed with `wac`, under a 256 MiB limiter
  and an epoch deadline; `exec-oneshot` round trip.
- **Binds.** L1b, L1c, L19.

### D16 — Rust bindings from the pinned `wit-bindgen-cli`
- **Decision.** Export glue is generated by `wit-bindgen-cli` (run with `--std-feature`,
  `--runtime-path`, `--bitflags-path` pointing at own modules, `--disable-custom-section-link-helpers`),
  committed, and regenerated only when the WIT changes. The `wit-bindgen` crate MUST NOT be a dependency.
  Fallback if the output cannot be made `no_std` without a runtime crate: hand-written glue.
- **Rationale.** The crate links runtime code; the generator adds no crate.
- **Posed by.** Owner. **Track.** `01M7FTJ9ACHNZ45G8YNMN43C01`.
- **Acceptance.** Gate G6; a regeneration check proves the committed file matches the WIT.
- **Binds.** L0, L1b.

### D17 — Release pipeline, tools and gates
- **Decision.** Annex C. Rust and all build tools are needed only to build this repository; consumers need
  none. `wasm-opt` runs only on the core module before componentization and only if L2 proves a gain of at
  least 10%. Tools are pinned by exact version and SHA-256 under git-ignored `tmp/tools/`; npm tools come
  from a committed `tools/` manifest and lockfile, installed with `--ignore-scripts`.
- **Rationale.** Owner constraints A1; supply-chain exposure limited to the build lane.
- **Posed by.** Owner (constraints); Fable 5.2 + Carbon (gates and pins). **Track.**
  `01M7E9EB8FAAH8B4CX2V5VQ293`.
- **Acceptance.** Gates G1-G8 pass on a clean worktree; a missing or mismatching tool hash fails the build.
- **Binds.** L0.

### D18 — Published artefacts
- **Decision.** One npm package `@sentropic/bash` with empty `dependencies` and no install script,
  containing: `kernel.component.wasm`, `wit/`, the generated `js/` (core module, glue, types), the facade,
  and the unplugged session drivers. The same component bytes are pushed to an OCI registry. `*.wasm` is
  never committed; consumers use published packages. Pre-release `0.x` under the `next` tag once tier A/B
  parity passes.
- **Posed by.** Owner. **Track.** `01M7FTJ933E69W3QN9THHCWDDY`, `01M7DXY6KTXZZCRJ70ZG0GNT75`.
- **Rationale.** TS consumers get everything from npm; wasmCloud deployments pull from OCI.
- **Acceptance.** Package-content test; npm component digest equals the OCI digest; install in an empty
  project needs no native tool and runs `exec('echo hi')`.
- **Binds.** L0, L17a, release.

### D19 — Oracle and parity protocol
- **Decision.** Annex E. The oracle is the published `just-bash@3.6.0`, run as a black box.
- **Rationale.** D1; no host command is ever executed, so generated scripts are safe to compare.
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Acceptance.** `make check` replays snapshots with no dependency and no network; the oracle job
  reproduces every committed snapshot byte for byte.
- **Binds.** L10a, L10b, L12.

### D20 — Clean-room lane rules
- **Decision.** Annex F.
- **Posed by.** Owner. **Track.** `01M7FTJA7EB2SQNHXG0WQVY1A1`.
- **Rationale.** References are Apache-2.0, the output is MIT.
- **Acceptance.** Build worktrees contain neither `tmp/refs/` nor the oracle package (checked by the lane
  bootstrap); every lot brief states the rule; review rejects a PR that cites a reference path as source.
- **Binds.** All lots; L12b runs in a reference-reading lane.

### D21 — Security testing and fuzzing
- **Decision.** Annex G lists the invariants. Tests: adversarial suites per threat class (study 4.4),
  seeded JS and native fuzzers in `make check` (short slice), mutation fuzzing against our engine only,
  and a coverage-guided `cargo-fuzz` campaign run from git-ignored `tmp/fuzz/`, out of every manifest,
  required before 1.0 on: parser, expansion, regex compiler, inflate/tar, path canonicalization, the six
  `yq` readers. Every finding becomes a seeded regression test.
- **Posed by.** Owner (campaign); Fable 5.2 + Carbon (the rest). **Track.** `01M7FTJAERMSZW1EGR0DZH6ST5`.
- **Rationale.** Hand-written parsers and codecs need branch-guided coverage.
- **Acceptance.** Campaign report (targets, duration, corpus size, findings) attached to the 1.0 gate; zero
  open trap.
- **Binds.** L11.

### D22 — Benchmark protocol and budgets
- **Decision.** Annex H. The benchmark runs against just-bash only.
- **Posed by.** Owner (targets, scope); Fable 5.2 + Carbon (protocol, host-size split). **Track.**
  `01M7E9EB1WERJ0F7CPJ9QZQR1W`, `01M7FTJ9ZWD7NT9ABFGAX0WF9C`.
- **Rationale.** Performance targets are owner-owned; they become gates only after L2 confirms them.
- **Acceptance.** `make bench` produces the result table and JSON; the regression gate is active after L2.
- **Binds.** L2, L12, L12b.

### D23 — Divergence list
- **Decision.** Annex I is the complete list of intended differences from the reference. Any other
  observable difference is a defect, or an `upstream-defect` entry submitted to the owner.
- **Posed by.** Owner. **Track.** `01M7FTJ8W993KN9DJ3WNTKP0VY`, `01M7FY8HXSPE3M3K68R90EQYHM`.
- **Rationale.** D1.
- **Acceptance.** The list is published with the package; the oracle harness carries exactly these
  exclusions.
- **Binds.** L10a, L10b.

### D24 — Go/no-go points
- **Decision.**
  - **L0.** A `no_std` build for `wasm32-wasip2` links without `wasi-libc`, yields a zero-import component
    and a glue without bare import. If not, work stops and the question returns to the owner; WASI imports
    are not stubbed.
  - **L2.** Measured: core module size with the Cargo profile, `wasm-opt` gain, component size, glue size
    and init time, Node cold start, per-call boundary cost. Outcome: budgets of Annex H confirmed or
    returned to the owner with numbers; `wasm-opt` kept or dropped; `slim` build decided.
  - **L1c.** Composed-component measurements under `wasmtime`; then the WIT freeze (D3).
- **Posed by.** Fable 5.2 + Carbon (consensus). **Track.** —.
- **Rationale.** The two largest unverified assumptions are tested by the three smallest sequential lots.
- **Acceptance.** A written report per point, reviewed, before the next lots start.
- **Binds.** L0, L2, L1c.

---

## Annex A — WIT contract (`0.1.0-draft`)

Not yet validated by a tool (none is installed in the design lane); validation is the first task of L0.

```wit
package sentropic:bash@0.1.0-draft;

interface types {
  type bytes = list<u8>;

  enum profile { micro, contain, hardened, normal }
  record limit-override { name: string, value: u64 }
  record limits { profile: profile, overrides: list<limit-override> }
  flags grants { host-fs, network, real-clock, real-sleep, entropy, custom-commands }

  record env-var { name: bytes, value: bytes }
  record process-info { pid: u32, ppid: u32, uid: u32, gid: u32 }
  record shell-config {
    limits: limits,
    grants: grants,
    commands: option<list<string>>,       // none = every kernel command
    custom-commands: list<string>,
    env: list<env-var>,
    cwd: option<bytes>,
    process-info: option<process-info>,
    default-layout: bool,
  }
  record exec-opts {
    script: bytes,
    args: list<bytes>,
    env: list<env-var>,
    replace-env: bool,
    cwd: option<bytes>,
    raw-script: bool,
  }
  enum exec-status { completed, limit-exceeded, aborted, deadline }
  record exec-result {
    exit-code: s32,
    status: exec-status,
    limit-name: option<string>,
    env: list<env-var>,
  }

  enum node-kind { file, directory, symlink }
  record stat { kind: node-kind, size: u64, mode: u32, mtime-ms: s64, ino: u64 }
  record dir-entry { name: bytes, kind: node-kind }
  record walk-entry { path: bytes, info: stat }
  enum fs-error-code { not-found, exists, not-directory, is-directory, not-empty, permission,
                       invalid, link-loop, too-large, no-space, unsupported, io }
  record fs-error { code: fs-error-code, path: bytes }

  record read-req { path: bytes, offset: u64, len: u32 }
  record write-req { path: bytes, data: bytes, append: bool }
  record create-req { path: bytes, mode: u32, directory: bool }
  record mkdir-req { path: bytes, recursive: bool, mode: option<u32> }
  record remove-req { path: bytes, recursive: bool, force: bool }
  record pair-req { source: bytes, target: bytes, recursive: bool }
  record chmod-req { path: bytes, mode: u32 }
  record utimes-req { path: bytes, atime-ms: s64, mtime-ms: s64 }
  record walk-req { root: bytes, max-depth: u32, max-entries: u32 }
  variant fs-op {
    stat(bytes), lstat(bytes), exists(bytes), read-link(bytes), real-path(bytes),
    read-dir(bytes), read-dir-plus(bytes), walk(walk-req),
    read-file(read-req), write-file(write-req), create-exclusive(create-req),
    mkdir(mkdir-req), remove(remove-req), copy(pair-req), rename(pair-req),
    symlink(pair-req), hard-link(pair-req), chmod(chmod-req), utimes(utimes-req),
  }
  variant fs-ok {
    none, flag(bool), info(stat), path(bytes), data(bytes),
    entries(list<dir-entry>), entries-plus(list<walk-entry>),
  }
  type fs-result = result<fs-ok, fs-error>;
  record fs-batch-req { mount-id: u32, ops: list<fs-op> }
  record changed-file { path: bytes, removed: bool }

  record header { name: string, value: bytes }
  record command-call { name: string, args: list<bytes>, cwd: bytes, env: list<env-var>,
                        input-len: u64 }
  record fetch-req { method: string, url: string, headers: list<header>, input-len: u64,
                     timeout-ms: u32, max-response-bytes: u64 }
  record fetch-resp { status: u16, status-text: string, url: string, headers: list<header> }
  record clock-reading { wall-ms: s64, monotonic-ns: u64 }
  variant host-req {
    fs-batch(fs-batch-req), lazy-file(bytes), custom-command(command-call),
    fetch(fetch-req), sleep(u64), clock, entropy(u32),
  }
  variant host-outcome {
    fs(list<fs-result>), data(bytes), command-exit(s32), fetched(fetch-resp),
    clock(clock-reading), completed, denied, limit-exceeded(string), failed(bytes),
  }
  record pending-call { epoch: u64, req: host-req }
  variant step { done(exec-result), out-of-fuel, host-call(pending-call) }
  enum cancel-reason { aborted, deadline, limit }
  enum out-channel { stdout, stderr, body }

  enum kernel-error-code { not-configured, already-configured, ceiling, invalid-config,
                           stale-epoch, bad-state, too-large }
  record kernel-error { code: kernel-error-code, detail: string }
  record memory-usage { used: u64, ceiling: u64, high-water: u64 }

  record oneshot-file { path: bytes, data: bytes, mode: u32 }
  record oneshot-req { files: list<oneshot-file>, config: shell-config, opts: exec-opts,
                       stdin: bytes, fuel: u64 }
  record oneshot-result { outcome: exec-result, stdout: bytes, stderr: bytes,
                          changed: list<oneshot-file>, removed: list<bytes> }
}

interface engine {
  use types.{bytes, shell-config, exec-opts, step, host-outcome, cancel-reason, out-channel,
             fs-op, fs-result, fs-error, changed-file, kernel-error, memory-usage,
             oneshot-req, oneshot-result};

  configure: func(memory-ceiling-bytes: u64) -> result<_, kernel-error>;     // once, first call
  raise-ceiling: func(memory-ceiling-bytes: u64) -> result<_, kernel-error>; // monotonic
  usage: func() -> memory-usage;

  resource vfs {
    create: static func(max-bytes: u64) -> result<vfs, kernel-error>;
    op: func(req: fs-op) -> fs-result;
    mount-vfs: func(path: bytes, source: borrow<vfs>) -> result<_, fs-error>;
    mount-host: func(path: bytes, mount-id: u32) -> result<_, fs-error>;
    unmount: func(path: bytes) -> result<_, fs-error>;
    take-dirty: func() -> list<changed-file>;
    used-bytes: func() -> u64;
  }

  resource shell {
    create: static func(root: borrow<vfs>, config: shell-config) -> result<shell, kernel-error>;
    register-command: func(name: string) -> result<_, kernel-error>;
  }

  resource fiber {
    spawn: static func(owner: borrow<shell>, parent: option<borrow<fiber>>, opts: exec-opts)
        -> result<fiber, kernel-error>;
    write-stdin: func(chunk: bytes, last: bool) -> result<_, kernel-error>;
    run: func(fuel: u64) -> step;
    resume: func(epoch: u64, outcome: host-outcome) -> result<step, kernel-error>;
    read-call-input: func(epoch: u64, max: u32) -> result<bytes, kernel-error>;
    push-call-data: func(epoch: u64, channel: out-channel, chunk: bytes)
        -> result<_, kernel-error>;
    cancel: func(reason: cancel-reason);
    take-stdout: func(max: u32) -> bytes;
    take-stderr: func(max: u32) -> bytes;
  }

  exec-oneshot: func(req: oneshot-req) -> result<oneshot-result, kernel-error>;
}

world kernel {
  export engine;
}
```

Rules attached to the contract:
- `create`/`spawn` are static functions because they can fail; borrowed handles are not retained past the
  call (the kernel keeps its own shared reference to the underlying object).
- A custom command's stdin and a fetch body are pulled with `read-call-input`; a custom command's output
  and a fetch response body are pushed with `push-call-data` before `resume`. All chunks are at most 1 MiB.
- `mount-id` values are allocated by the driver and identify a host-backed filesystem in `fs-batch`.
- `exec-oneshot` returns `bad-state` if the script needs a host call.
- The session drivers live in a second package, `sentropic:bash-session@0.1.0-draft`: each world imports
  `sentropic:bash/engine` and exports one `session` resource (`create`, `exec`, `read-file`, `write-file`).
  `session-pure` imports nothing else; `session-fs` adds `wasi:filesystem/{preopens,types}` and
  `wasi:clocks/monotonic-clock`; `session-net` adds `wasi:http/outgoing-handler` and
  `wasi:clocks/wall-clock`. Their WIT is written in L1c.

## Annex B — Limit profiles

In every profile `VFS + live + output + headroom = ceiling`. Script and stdin bytes are charged to `live`;
stdout and stderr share the output cap; canonical-ABI buffers are charged to the headroom. With several
fibers or shells on one instance the ceiling can be reached first; the requesting fiber gets the same
limit result.

| Profile | VFS | Live / input | Output / string | Commands / loop / work units | Wall time | Headroom | Ceiling | `exec-oneshot` cap |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `micro` | 32 MiB | 16 / 16 MiB | 8 / 4 MiB | 25k / 25k / 1M | 10 s | 8 MiB | 64 MiB | 4 MiB |
| `contain` (default) | 128 MiB | 64 / 64 MiB | 32 / 16 MiB | 100k / 100k / 100M | 60 s | 32 MiB | 256 MiB | 16 MiB |
| `hardened` | 128 MiB | 64 / 32 MiB | 10 / 10 MiB | 10k / 10k / 100k | 30 s | 54 MiB | 256 MiB | 27 MiB |
| `normal` | 1 024 MiB | 512 / 512 MiB | 256 / 64 MiB | 100k / 100k / 100M | 1 h | 64 MiB | 1 856 MiB | 32 MiB |

All other limits (depths, parser tokens, glob operations, brace results, array elements, file
descriptors, traversal, archive and query limits) take the reference values of the matching reference
profile (`JB:src/limits.ts:164-262`; `contain` and `micro` take the `normal` and `hardened` values
respectively). Additions: regex program size and DFA cache caps, archive expansion ratio, YAML alias count.
Headroom figures are estimates to confirm at L2.

## Annex C — Release pipeline and gates

Order (commands: study 2.6):
1. `cargo build --release --target wasm32-wasip2`, profile `opt-level = "z"`, `lto = "fat"`,
   `codegen-units = 1`, `panic = "abort"`, `strip = true`; flags `link-self-contained=no`, `--no-entry`,
   `--skip-wit-component`, path remapping. Output: core module.
2. Optional `wasm-opt -Oz` on the core module (only if kept at L2).
3. `wasm-tools component embed` + `component new` from `wit/`, no adapter. Output: component.
4. JS glue and types from the component with `jco-transpile` (sync instantiation, no WASI shim, wasm kept
   as a file, no top-level await).
5. Package assembly, then OCI push of the component.

| Gate | Check | Fails when |
|---|---|---|
| G1 zero import | `wasm-tools component wit` on the component | any import is listed |
| G2 no bare import | scan of generated and facade JS | an import specifier is neither relative nor `node:` (and `node:` appears only in Node adapters) |
| G3 two-build identity | two clean builds in different directories | component or core module bytes differ |
| G4 component validity | `wasm-tools validate`; embedded world equals `wit/` | mismatch |
| G5 one core module | file count of the glue output | more than one core module, or wasm inlined as base64 |
| G6 zero crate, no banned growth | `Cargo.lock` inspection; source scan (D6.2) | a non-workspace package; infallible growth on untrusted data |
| G7 manifests | `check-no-deps`, no Python, no attribution | existing repo gates |
| G8 tool pins | SHA-256 of every tool in `tmp/tools/` | missing or different hash |

Tools (all build- or test-time, outside the package): rustc/cargo 1.96.0 with the `wasm32-wasip2`
standard library; `wit-bindgen-cli` 0.63.0; `wasm-tools` 1.263.0; `@bytecodealliance/jco-transpile` 0.24.0;
`typescript`; optional Binaryen `version_132`; test-only `wac-cli` 0.12.0, `wasmtime` 50.0.1, Chrome for
Testing; oracle-only `just-bash` 3.6.0; fuzz-only `cargo-fuzz` on nightly. Justifications: study 2.6.
This repository's `make check` builds the kernel (`*.wasm` is git-ignored), so `make test` depends on
`make build`.

## Annex D — Facade API mapped to just-bash

| Tier | just-bash surface (`JB:src/index.ts`, `JB:src/Bash.ts`) | Ours |
|---|---|---|
| A | `new Bash({files, env, cwd, fs, executionLimits, executionLimitProfile, maxCallDepth, maxCommandCount, maxLoopIterations, commands, customCommands, sleep, network, fetch, processInfo})` | Same; profile names extended with `contain`, `micro` |
| A | `bash.exec(cmd, {env, replaceEnv, cwd, rawScript, stdin, stdinKind, signal, args})` -> `{stdout, stderr, exitCode, env, stdoutKind?}`; non-throwing; `normalizeScript`; binary-preserving decoding | Same |
| A | `bash.fs`, `readFile`, `writeFile`, `getCwd`, `getEnv`, `registerCommand` | Same |
| A | `defineCommand`, `LazyCommand`, `createCommandContext`; context `fs`, `cwd`, `env`, `exportedEnv`, `assignShellVariable`, `stdin`, `limits`, `exec`, `origCommand`, `execWithInheritedStdin`, `fetch`, `sleep`, `signal`, `fileDescriptors`, `getRegisteredCommands` | Same, behind the membrane |
| A | `InMemoryFs` and the `IFileSystem` contract (23 methods, `createExclusive`, stat identity, lazy files, sync `mkdirSync`/`writeFileSync`/`writeFileLazy`) | Same |
| A | `getCommandNames`, `getNetworkCommandNames`; byte helpers (`ByteString`, `bytesOutput`, `textOutput`, `encodeUtf8ToBytes`, `decodeBytesToUtf8`, `latin1FromBytes`, `stdoutAsBytes`, ...) | Same |
| B | `OverlayFs`, `ReadWriteFs`, `MountableFs` | Same options, own implementation (D10) |
| B | `NetworkConfig`, `NetworkAccessDeniedError`, `RedirectNotAllowedError`, `TooManyRedirectsError`, `ResponseTooLargeError` | Same (D11) |
| B | ESM and CJS entries, `./browser` entry | Same; browser exposes every kernel command |
| C (after parity) | `Sandbox`, `SandboxCommand`; CLI binaries; `logger`, `trace`, `coverage` options | Accepted and ignored until implemented (**new in EVOL**: `logger`, `trace`, `coverage` do not throw) |
| Not in v1 | `parse`, AST types, `BashTransformPipeline`, plugins, `serialize`, `registerTransformPlugin`, `transform` | Absent; calling throws "not supported" |
| Not reproduced | `DefenseInDepthBox` and security exports; `defenseInDepth` option | Option accepted and ignored (**new in EVOL**); exports absent |
| Not in v1 | `python`, `javascript` options; `getPythonCommandNames`, `getJavaScriptCommandNames` | Options throw; helpers return empty lists |
| Additions | — | `execSync`, `Bash.precompile()`, `bash.compact()`, journal option, `executionLimitProfile: "contain" \| "micro"` |

## Annex E — Oracle and parity protocol

1. **Oracle.** `just-bash@3.6.0` from npm, installed with `npm ci --ignore-scripts` from a committed
   lockfile into git-ignored `tmp/bench-engines/just-bash/`. Run in a child process, in-memory filesystem,
   network, python and javascript disabled, `LC_ALL=C`, `TZ=UTC`, fixed virtual clock where the reference
   allows it.
2. **Where it runs.** Only in the oracle job and in reference-reading lanes (Annex F). Never in a build or
   test-writing worktree.
3. **Case format.** A case is our own script plus initial files, env, stdin and options, written from the
   behavioural spec. No case is copied or adapted from a reference test file.
4. **Snapshot.** The recorder runs each case on the oracle and commits
   `{stdout, stderr, exitCode, env, changed files}` with the oracle version. `make check` replays cases on
   our engine against snapshots: no dependency, no network.
5. **Generated differential.** The oracle job also compares grammar-generated scripts live. A mismatch is
   reported to build lanes as `(script, expected, actual)` and added as a case once minimized.
6. **Nondeterminism.** Cases depending on time, randomness or pids use the virtual values; a case whose
   oracle output is unstable over three runs is rejected.
7. **Upstream defects.** When the oracle fails with an internal, non-shell error, the case is tagged
   `upstream-defect`, excluded from parity and listed for the owner with the reference source path.
8. **Parity report.** Per command and per grammar area: cases, pass, fail, `not covered`; published with
   each pre-release. The system bash is never an oracle.

## Annex F — Clean-room lane rules

1. **Room 1 (reference-reading):** the design lane, the oracle job, and documentation-only lots such as the
   feature matrix. May read `tmp/refs/` and run the oracle. Produces behavioural specs (options, exit
   codes, messages, edge cases as tables and prose), snapshots and reports. Never writes kernel, facade or
   test code.
2. **Room 2 (build and test-writing):** works from specs, snapshots, POSIX and tool documentation. Its
   worktree MUST NOT contain `tmp/refs/` or the oracle package; its brief states this rule.
3. Specs describe behaviour, not implementation: no reference code, no reference identifiers beyond public
   API names, no translated test bodies.
4. Reference test suites are not vendored. Error-message strings required for parity are behaviour and may
   appear in specs and snapshots.
5. A lane does not change rooms within a lot. Review rejects any Room 2 change that cites a reference path.

## Annex G — Security invariants

| # | Invariant | Verified by |
|---|---|---|
| S1 | The kernel component has no import | G1 |
| S2 | Without grants, no host call reaches the driver | recording-driver tests (D9) |
| S3 | No untrusted input causes a trap; limits are results | fuzzing oracles, limit tests (D6, D7) |
| S4 | Byte budgets bind before the memory ceiling for one fiber; ceiling exhaustion is a limit result | D6 tests |
| S5 | A stale or late resume cannot change kernel state | epoch tests (D4) |
| S6 | After abort, deadline or limit, extension proxies are revoked | membrane tests (D9) |
| S7 | Host paths never leave the root; special files are never opened; host paths never appear in errors | adapter suite (D10) |
| S8 | Network requests go only to validated, pinned addresses within the allow-list | adapter suite (D11) |
| S9 | Regex matching and compilation are bounded | D12 tests |
| S10 | The driver never evaluates sandbox data, never builds property names from it, never passes it to `import()`, `require`, `Function` or `RegExp` | review checklist + source scan |
| S11 | Clock, timezone, entropy, pid/uid, hostname and host env are virtual unless granted | capability tests |
| S12 | A trap poisons the instance; no state is read from it afterwards | D8 tests |
| S13 | The published package has no dependency and no install script | package-content test |

## Annex H — Benchmark protocol and budgets

Protocol (details: study section 5): engines `@sentropic/bash` and `just-bash@3.6.0` only; node built-ins
only; cold metrics on 30 fresh interleaved processes against an empty-ESM baseline; warm metrics after 20
discarded iterations; interpolated median and nearest-rank p95; `LC_ALL=C`, `TZ=UTC`; a workload is timed
only if our output equals the oracle snapshot. The regression gate fails a metric only if the regression
exceeds both `max(10%, floor)` (0.02 ms sub-millisecond, 0.5 ms otherwise, 1 MB memory) and the baseline's
p95 spread. MoonBash is compared through `docs/FEATURE_MATRIX.md` only.

| Metric | just-bash 3.6.0 (measured) | Target (confirmed at L2) |
|---|---:|---:|
| M1 Node cold start (import + ctor + first exec) | 118.8 ms | <= 25 ms (stretch 15 ms) |
| M1b browser cold start | not measured | reported, no target before L19 |
| M2 warm `new Bash()` | 0.37 ms | <= 0.20 ms |
| M3 `echo hi` / 1 000-iteration loop | 0.35 ms / 7.4 ms | <= 0.07 ms / <= 1.5 ms |
| M4 `grep -c`, `sed`, `awk`, `sort` on 10k lines | 11.0 / 73.7 / 61.0 / 9.2 ms | <= 2 / 8 / 8 / 4 ms |
| M5 RSS delta after first exec | 35.5 MB | <= 10 MB |
| M6 kernel core module | 2.02 MB of JS (557 KB gzip) | <= 1.5 MB raw, <= 700 KB gzip (all tiers) |
| M6 JS host | — | core entry <= 45 KB raw; lazy Node adapters <= 40 KB raw; <= 30 KB gzip in total; no minification |
| M7 limit latency; M8 host-backed FS; M9 agent session; M10 heavy pipeline | not measured | not slower than just-bash until measured |

The JS host line replaces the earlier "JS host <= 60 KB" wording; it is reported to the owner with the L2
measurement, since performance targets are owner-owned.

## Annex I — Divergence list

| # | Difference from just-bash 3.6.0 | Source of the decision |
|---|---|---|
| V1 | Default limit profile is `contain` (reference: `normal`); `contain` and `micro` exist | Owner, O1 |
| V2 | Not available in v1: `python3`/`python`, `js-exec`/`node`, `sqlite3`, `xan`, `html-to-markdown`, `@just-bash/executor` | Owner, A4 |
| V3 | Not available in v1: `parse`, AST types, transform pipeline and plugins | Consensus, B3 |
| V4 | The browser entry exposes `tar` and `yq`, which the reference's browser bundle excludes | Owner, P1 |
| V5 | `defenseInDepth` is accepted and has no effect; its exports are absent | Consensus (study 3.2, tier D) |
| V6 | Additions that do not alter existing behaviour: `execSync`, `precompile`, `compact`, journal, the component and its WIT | Consensus |

## 3. Questions for the owner

None. The three items marked **new in EVOL** (network adapter inside the core package; `python` /
`javascript` options throwing; `logger`, `trace`, `coverage`, `defenseInDepth` accepted and ignored) are
technical refinements for the review of this document.
