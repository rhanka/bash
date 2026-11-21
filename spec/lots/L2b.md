# Lot L2b — First-party canonical-ABI binding

Lot spec for the build lane (Room 2). Sources of authority are `spec/SPEC_EVOL_BASH_SANDBOX.md` (EVOL D2–D6, D8,
D15–D18 and Annex G), `spec/lots/L0.md` (G1–G8), `spec/lots/L1b.md` (resource and facade contracts) and
`spec/lots/L2.md`. Start from the integrated L2 kernel including the Option D guard. The shipped WIT and the
generated Rust exports on that revision define the wire layout. Raise any discrepancy as a spec issue.

- **Size.** 3 points, Strong. **Builder.** Codex Sol 6.2, xhigh. **Review.** Carbon, full PR review on a green
  `make check`.
- **Branch.** `feat/l2b-canonical-abi`, worktree `tmp/l2b-canonical-abi`.
- **Outcome.** Both public entries select a first-party synchronous binding for all 29 engine functions. Pinned
  jco drives the same component independently in every full check. L2c follows this lot and owns the Gate L2
  verdict.

## 1. Lane rules

1. Work offline with the staged L0 tool pins. No reference source or reference package. Replay the committed
   cases without editing them.
2. Use plain ES modules, Node built-ins in tools and tests only, and relative imports in browser code. Both
   runtime dependency sets stay empty. No Python or Docker. Temporary files go under the worktree's ignored
   `tmp/`.
3. Commits through `make commit`, in English, without attribution trailers.

## 2. Scope

Implement the complete JS projection of `sentropic:bash/engine@0.1.0-draft`, including cold paths and exports
whose L2 implementation returns a typed refusal. Replace the production binding in both loaders. Keep the
kernel-test injection, L0 probe, Rust binding regeneration, L2 driver, limits and public declarations working.
Keep guarded jco available for the next lot's optional package. Snapshotting, facade minification, packaging
that second entry and performance acceptance belong to L2c. Filesystem transport belongs to L7b/L7c.

## 3. Allowed paths

```
host/core/instance.js       (binding selection and trap notification)
host/core/abi/**            (first-party codecs, descriptors and resources)
tools/build.mjs  tools/transpile.mjs  tools/bindings.mjs
tools/**                    (descriptor emission, signature check and oracle staging only)
test/**                     (ABI fixtures, differential tests, kernel-test and pack expectations)
BRANCH.md
```

The production build emits the adapter at the existing loader boundary `dist/js/kernel/kernel.js`. Keep oracle
output and fixtures in `dist/test/` or `tmp/`. No BR exception is needed. `Makefile`, manifests, `wit/**`, the
Rust guest, case/snapshot corpora and parity exclusions are read-only. `tools/run-tests.mjs` already discovers
nested `*.test.mjs` files.

## 4. Binding interface

Preserve synchronous `instantiate(getCoreModule, {}) -> { engine, _util }` and idempotent `_util.stop()` for
`host/core/instance.js`. Request one loader core. Instantiate it with six canonical resource intrinsics in
`[export]sentropic:bash/engine@0.1.0-draft`, a new/drop pair for each resource. The component has zero external
imports. Give each invocation fresh memory, handles, finalizers and stop state. Only immutable code, descriptors
and the compiled module may be shared. Keep the existing injection of `hooks.kernel`, `onEngine` and the
precompiled browser module.

| JS receiver | Methods (parameter order is the WIT order) | Count |
|---|---|---:|
| `engine` | `configure(memoryCeilingBytes)`, `raiseCeiling(memoryCeilingBytes)`, `usage()`, `coverageFeatures()`, `execOneshot(req)` | 5 |
| `engine.Vfs` / Vfs object | static `create(maxBytes)`, then `op(req)`, `mountVfs(path, source)`, `mountHost(path, mountId)`, `unmount(path)`, `takeDirty()`, `usedBytes()` | 7 |
| `engine.Shell` / Shell object | static `create(root, config)`, then `registerCommand(name)`, `env()`, `cwd()` | 4 |
| `engine.Fiber` / Fiber object | static `spawn(owner, parent, opts)`, then `writeStdin(chunk, last)`, `run(fuel)`, `resume(epoch, outcome)`, `readCallInput(epoch, fd, max)`, `pushCallData(epoch, channel, chunk)`, `assignVariable(epoch, name, value, subscript)`, `setCallEnv(epoch, env)`, `cancel(reason)`, `takeStdout(max)`, `takeStderr(max)`, `takeTrace(max)`, `takeCoverage(max)` | 13 |

Use the committed `wit/sentropic-bash.wit` for every field, case and declaration order. Emit or maintain plain
JS arrays/objects for the descriptor graph. The build compares it mechanically with normalized
`wasm-tools component wit --json` output and the exact loader-core function signatures, resource namespace,
destructor names, memory, `cabi_realloc` and component post-return mappings. A new function, a reordered
variant, a changed parameter or an unknown import fails the build. No compressed descriptor string, base64,
LZSS, runtime WIT/JSON parser or dynamic code generation.

JS record keys and grant flags are camelCase, case labels are the WIT spellings. An option projects to
`undefined` or the contained value. A variant is `{tag}` or `{tag,val}`. `bytes` is `Uint8Array`, `list<u16>`
lifts to an owned `Uint16Array`. Other lists lift to arrays and 64-bit integers to BigInt. Preserve the pinned
projection's distinction between named and inline results. `vfs.op` and nested `fs-result` return tagged
`ok`/`err` values. The inline `result` functions unwrap success (unit becomes `undefined`) and throw the lifted
error record, including `{code,detail}` or `{code,path}`, after post-return. Do not wrap those records in an
Error or discard fields.

## 5. Validation and call lifetime

1. **Lowering.** Validate the entire typed argument tree, including `this` and optional parent borrows, before
   the first guest allocation or call. Stage validated values once so getters, mutation or detached views cannot
   change what is written after validation. Reject invalid input synchronously with `TypeError` for
   type/shape/brand and `RangeError` for numeric bounds. For u8/u16/u32 accept integral Numbers in [0, 2^width −
   1], for s32 in [−2^31, 2^31 − 1]. For s64/u64 require BigInt in the corresponding signed/unsigned 64-bit
   range. Require actual booleans, numeric f64, known enum/variant tags and the correct payload, records and
   list element types. Required fields must exist. Omitted option fields mean none and omitted known grant bits
   mean false. Reject unknown grant bits/names. No silent integer wrapping, truthiness coercion or
   stringification. Encode WIT strings with UTF-8 (`TextEncoder` on a JS string, including replacement of lone
   UTF-16 surrogates). Preserve arbitrary bytes, byte offsets and lengths in `list<u8>`.
2. **Layout.** Use wasm32 little-endian canonical sizes, alignments, record padding, variant payload alignment,
   joined flat slots and canonical bitcasts. Lower up to 16 flat parameter slots directly. For a larger tuple,
   allocate its aligned parameter buffer with `cabi_realloc` and pass the pointer. On guest entry the callee
   owns and frees that buffer. JS must never free it a second time. The single flat-result limit determines
   direct versus indirect returns. Check alignment and non-wrapping `ptr + count * stride` against current
   memory before every read/write, including nested arrays
   and zero lengths. Preserve direct unsigned i32/i64 bit patterns. Use checked arithmetic in JS before
   converting offsets or counts to wasm32. A zero-length list consumes zero data bytes. Obtain views again after
   each allocation and guest entry that can grow memory. Copy lifted lists/strings before cleanup. Validate
   lengths/work against the applicable transfer or aggregate budget before allocating host containers.
3. **Lifting.** Validate bool encodings (0/1), option/result discriminants (0/1), variant/enum ranges, reserved
   flag bits, UTF-8 strings (fatal decoding preserving a leading BOM), resource kind and ownership, every
   pointer pair and nested record. Malformed guest output is a binding trap and poisons only that store. A typed
   WIT error is an ordinary completed call. Preserve L2's public preflights and limit results, including 1 MiB
   stream chunks, the 3 MiB script exemption and non-streamed string/half-headroom checks.
4. **Own/borrow.** Resource-new records an opaque representation in a private per-store table and returns a
   nonzero u32 handle. Fail closed on exhaustion or a duplicate live ID. Lift an owned handle once into a
   branded prototype-linked wrapper. Check store identity, resource kind, live ownership and receiver on every
   call. Lower a borrow of a locally exported resource to its live representation for the dynamic extent of the
   call. Keep borrowed wrappers alive through return. Never transfer or drop them as a side effect of borrowing.
   Block disposal/re-entry that would invalidate a live call. Rust may retain its own `KRc`. Numeric handles,
   prototypes, stale/finalized wrappers and foreign stores cannot establish ownership.
   When the guest drops an owned resource, its imported `[resource-drop]<kind>(handle)` looks up the handle
   in that store's table for `vfs`, `shell` or `fiber`. Require a live, guest-owned entry with no active borrow.
   An invalid drop traps and poisons the calling store. On success remove the entry, unregister any finalizer,
   and invoke the matching `[dtor]<kind>(rep)` exactly once with the saved representation. Retire ownership
   before entering the destructor so a trap or a stale finalizer cannot destroy it again.
5. **Return and destruction.** On normal guest return and successful lifting call the declared `cabi_post_*`
   hook exactly once with the original raw result before exposing success or throwing a typed error. Multiple
   export names can share one compiled function. A guest/realloc/lift/post-return/destructor trap stops the
   store immediately. Never re-enter it in a cleanup/finalizer or issue a second post-return. Release JS
   bookkeeping on every path. `[Symbol.dispose]()` and finalization collectively invoke at most one matching
   `[dtor]` per owned representation. Protect against stale finalizer tokens and handle reuse. No wrapper or
   finalizer can use a stopped store.
6. **Trap boundary.** Reuse `Instance.call/poison` and its waiters so the triggering exec and affected pending
   fibers complete with the L1b fatal result (exit 1), scopes are revoked, and later calls reject with
   `KernelPoisonedError`. Healthy stores continue. Host input errors and structured guest refusals leave the
   store usable.

## 6. Executable exit gates

`npm_config_cache=$PWD/tmp/npmcache make check` runs all gates offline, including
`node --test test/abi/*.test.mjs` through the normal runner.

1. **Surface.** Assert 29/29 mapped and exercised on both adapters, with 3 resource kinds and 6 core imports.
   Check signature/layout drift with one changed field order and one changed export signature fixture. G1–G8,
   K1, binding regeneration and both packed public entry smoke tests pass.
2. **Differential.** Build the oracle with pinned `@bytecodealliance/jco-transpile@0.24.0` and its Option D
   guard from the production/test components used by the first-party adapter. For each component, assert
   that both bindings load the same core SHA-256. In separate fresh stores replay identical configure/raise, create/drop, VFS, shell, fiber, oneshot and
   host-call/epoch transcripts. Compare every tag, field, byte, BigInt, error payload and subsequent observable
   state. Identify resources by kind and creation order. Include all 21 fs-op cases, the 3 step cases, tagged FS
   errors, cancel and stale resume, trace/coverage drains, optional parent and zero-length final stdin. Exercise
   unimplemented L2 cases through their specified refusal or a typed fixture. Cover all 8 host request and
   10 host outcome cases in the codec corpus, both result arms and present/absent options. Zero valid-input
   mismatches.
3. **Codec boundaries.** Exercise each descriptor kind in both directions, each scalar minimum and maximum,
   empty/nested lists, non-ASCII strings, bytes `00 7f 80 ff` and a sliced byte buffer. For each rejected
   lowering family (range, wrong type, malformed nested shape, ownership), count guest allocations and calls. Both
   counts stay zero and the next valid call succeeds. The pinned oracle defines valid projections. Independent
   negative fixtures enforce strict rejection where jco coerces. Provide controlled return fixtures for bad
   bool/tag/flags, UTF-8, misalignment, out-of-bounds length and invalid owned handle. Each produces exactly one
   poison notification.
4. **Lifetime.** Instrument guest entry and post-return on ok and err paths. Assert exactly one post per
   declared successful lift and zero after a trap. Retain returned buffers across at least one real memory
   growth (verify buffer identity changed), another call and disposal. Assert exact bytes. For all 3 resource
   kinds exercise wrong-kind, foreign, forged, disposed and repeated cleanup paths. On a live store assert one
   destructor per disposed owner and zero destructors caused by borrows. Use deterministic finalizer-token tests
   without depending on GC scheduling. For each kind have a core fixture create and drop a guest-owned handle
   through the two imports. Assert one destructor with the original representation and no second call from a
   stale finalizer. In fresh stores a repeated drop, wrong-kind drop and drop during a borrow must each trap,
   issue one poison notification and leave the destructor count unchanged. For an indirect call (e.g.
   `Shell.create`), count one allocation of the outer tuple and one guest free on both success and typed error.
5. **Integration.** Run the real test-kernel trapping command with a second pending fiber, then run `echo ok` on
   an independent store. Replay 30/30 committed L2 oracle cases through each binding with zero exclusions, stale
   snapshots or failures. All 4 limit profiles pass. The default tarball contains zero generated jco runtime
   files.

## 7. Points the builder must confirm and report

Record the 29-name coverage table, descriptor/WIT and loader-core hashes, the jco pin and guard check, observed
post-return mappings, memory-growth and destructor counters, and the two replay totals in the PR. Report a drift
or shape mismatch with its function, input category and first differing field. L2b produces conformance
evidence. Only the L2c packed measurement can change Gate L2.
