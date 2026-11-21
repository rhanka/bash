# Lot L7c — Execution isolation for a shared InMemoryFs

Lot spec for the build lane (Room 2). Authorities are EVOL D4–D10, D15, D27 and Annexes D, G and I,
`spec/lots/L1b.md` (driver, membrane and poison contract), `spec/lots/L2.md` (layout, PATH, command registry),
and `spec/lots/L7b.md` section 7 (mount-host, fs-batch and the host adapters). L7b is PR #12 at design time.
Start from its merged, L2-rebased contract. Do not depend on interim PR internals. Reconcile L2b/L2c loader and
pack paths on that base.

- **Size.** 2.5 points, Strong. **Builder.** Codex Sol 6.2, xhigh. **Review.** Carbon, full PR review on a green
  `make check`.
- **Branch.** `feat/l7c-shared-fs-isolation`, worktree `tmp/l7c-shared-fs-isolation`.
- **Outcome.** A shared volume has one authoritative tree. The caller selects the execution trap domain. Both
  modes preserve object identity, synchronous access when ready and all existing per-shell capability checks.
  This lot precedes L7a and does not gate W4.

## 1. Lane rules

1. No reference sources or packages, no network. Behaviour needed by Room 2 is stated below. Keep existing
   parity cases and snapshots read-only.
2. Keep the kernel zero-import and `no_std` on Wasm. No external crates or runtime packages. Plain ESM/JSDoc and
   hand-written declarations. No Python or Docker. Put temporary files under the worktree's ignored `tmp/`.
3. Commits through `make commit`, English, without attribution trailers.

## 2. Scope

Build on L7b's root mount and typed responses for all 21 filesystem operations. Add one constructor option,
split execution-store selection from volume ownership, initialize layout in the actual volume and serve branded,
synchronous root mounts in the existing exec loop. Keep the ordinary one-store constructor and L7b's async path
for host adapters. Implement the first-party and optional jco facade behavior from one source. `MountableFs`,
new in-memory op implementations, lazy content delivery, rebuilding a journal and `compact()` remain L7a
responsibilities.

**Reference behaviour.** just-bash 3.6.0 uses the supplied filesystem object directly. Two shells given the same
object see each other's file changes and expose that same object as `bash.fs`. Omitting it constructs a new
in-memory filesystem from the supplied files and resolved storage limit. If both `fs` and `files` are present,
the supplied FS wins. The file map is an initializer for a newly created volume only. Its presence still affects
the default cwd/layout choice. Each shell initializes its own environment, limits and command state. The
reference exposes no Wasm store, trap domain or isolation selector. Reviewer provenance only
(JB:src/Bash.ts:373). EVOL D27/Annex I documents the added mode switch, default separate execution stores and
opt-in shared failure/ceiling scope. Filesystem visibility and identity remain parity requirements in both
modes. This addition creates zero parity exclusions.

## 3. Allowed paths

```
host/core/facade.js  host/core/holder.js  host/core/instance.js
host/core/fs.js      host/core/driver.js  host/core/fs-batch.js
types/core.d.ts      types/index.d.ts     types/browser.d.ts
kernel/src/fs.rs     kernel/src/shell.rs  kernel/src/fiber/**
kernel/src/fiber.rs                       (only mounted-root dispatch/layout as needed)
test/**              (two-store and one-store suites, declarations, consumers)
DIVERGENCES.md
BRANCH.md
```

The L2 VM is `kernel/src/fiber/vm.rs`. Confirm the merged L7b paths when declaring the branch scope. Keep
`wit/**`, generated bindings, allocation code, Node host adapters, `package.json` and `Makefile` read-only. The
L7b `mount-host`, `unmount` and `fs-batch` signatures suffice. No BR exception needed.

## 4. Public interface and ownership

```ts
// In the existing BashOptions, shared by Node and ./browser
isolation?: "separate" | "shared";
```

Validate the value synchronously with other options, before side effects. Omitted/`undefined` means
`"separate"`. Reject any other value (including `null`) with `TypeError`. Recognize an in-memory volume by the
facade's private FS state. A prototype or an object implementing similarly named methods gives no synchronous
kernel capability.

| `Bash` construction | Execution and volume owners | Guest transport |
|---|---|---|
| No supplied FS, including either explicit mode | Fresh private store per construction, co-located shell and volume. No process singleton. L2c may restore an otherwise default configuration. | Local guest VFS |
| Branded `fs`, omitted mode or `"separate"` | Keep the volume in its original holder. Create one new execution holder and local root resource for each shell. | `mountHost(utf8("/"), 1)` on the local root and a private mount map to the volume |
| Branded `fs`, `"shared"` | Attach a new shell to that exact volume's holder, using its local Vfs handle. Several opt-in shells use one Wasm instance with distinct Shell/Fiber resources. | Local guest VFS |
| L7b host adapter/custom `IFileSystem` | Omitted or `"separate"` preserves L7b's fresh execution store and async membrane. Explicit `"shared"` requires a kernel-backed volume and throws `TypeError` on this path. | Existing async root mount |

A standalone `new InMemoryFs()` initially owns a VFS-only store. Separate-mode attachments leave it VFS-only.
Explicit shared attachments turn that same store into an execution domain. A shell-created `a.fs` remains in `a`'s
store. A trap in `new Bash({fs: a.fs})` leaves `a` healthy. If `a` itself traps, its owned volume
becomes unavailable under D8. Never inspect poisoned memory, salvage files, silently substitute an empty volume
or migrate existing handles. This also applies to a mixture of separate and shared attachments. A live separate
store can continue computation independent of the lost volume. Access to that volume must fail. Expose
`KernelPoisonedError` from direct access to the lost FS and an ordinary FS `io` result for an otherwise healthy
guest's mounted request.

Grants, command names, custom callbacks, env/cwd, path cache and live/output/fiber budgets always belong to
their shell. Separate-mode ceiling and headroom derive from its own profile/overrides. Never raise the volume
owner's or a sibling's ceiling to fit this exec store. In shared mode use the existing monotonic `raiseCeiling`
and largest headroom rules. The allocator, high-water mark and poison flag belong to the common store. The volume's
own byte quota applies to all writers once, including writes made by the layout and guest. File sharing grants
the supplied volume only. A visible stub or another shell's `G_HOST_FS` must never confer access to a command,
network, host disk or another mount map.

## 5. Mount, construction and synchronous execution

1. **Attach.** Keep separate references to the execution holder, local root and public FS. Store mount id 1 in
   the owning shell's private map with a driver-private tag identifying a kernel-backed volume. Borrow handles
   only into their own store. Hold the volume wrapper strongly for as long as the mount needs it.
   `bash.fs === fs` and `bash.fs instanceof InMemoryFs` hold for both modes. The no-FS case makes zero guest
   `fs-batch` calls.
2. **Set up once.** Resolve/validate limits, filters, paths and config before publishing anything. In separate
   mode create/configure a fresh execution store, create its local root and register `mount-host` at `/` before
   executing. L7b gives longest component-boundary match, duplicate path `exists`, unknown unmount `not-found`,
   and `unsupported` for direct `vfs.op` on a mounted path. Drive actual setup against the volume owner, never
   against the covered local tree. Shell creation must understand a mounted root. Keep per-shell initialization
   (limits, env, registry) in its own store.
3. **Seed the public volume.** For a freshly created volume apply its initial files first. For a supplied volume
   keep its existing contents as the starting tree, even when `files` is also supplied. Apply the L2 section
   5.2/L7b section 7.2 sequence synchronously on ready stores: system directories, default home/tmp when
   eligible, device and proc entries (including this shell's process info), cwd, then physical stubs in both
   `/bin` and `/usr/bin` for active registry and custom names. Preserve L2 modes, bytes, overwrite order,
   directory collision errors, quota accounting and best-effort stub writes. Late `registerCommand` must update
   both public stubs and only the registering shell's registry. Share the layout writer/ordering with ordinary
   construction wherever possible. Existing volume files outside this initialization survive, including two
   attachments in sequence. On failure release unpublished resources/mounts and retain pre-existing owners. A
   retry follows ordinary construction semantics.
4. **Browser order.** Before compilation, stage against the volume's single ordered queue and preserve call
   order across attachments, initial files, layout and subsequent writes. Ensure both holders are ready before a
   separate shell enters its driver. After `await Bash.precompile()`, ready built-in mounts must support
   immediate synchronous construction and use. Synchronous calls made too early still give the documented
   compile-first error. A pending activation or a failing attachment cannot replay writes twice.
5. **Dispatch inside `execLoop`.** On `step.tag === "host-call"`, preserve drain, cancel/deadline and epoch
   logic. The kernel checks `G_HOST_FS` before emitting a mounted request. Set that shell-config grant for the internal transport.
   Its private map restricts the authority to the explicitly supplied volume. For `fs-batch {mountId,ops}` on a live, branded
   in-memory entry, run a synchronous helper returning `{tag:"fs", val: results}`. Process each op in order
   through the *volume owner's* `Instance.call(() => vfs.op(op))`. Paths/offsets/data/mode/time and tagged
   success/error records pass through in the existing WIT forms. Emit exactly one result per op, including a
   failure in the middle. Continue subsequent operations in order, with completed mutations visible. Preserve `unsupported` for operations L7a has yet to implement. Call
   `fiber.resume(epoch,outcome)` on the **execution** store and process its returned step immediately. Zero
   promises, public FS callbacks, yields for FS I/O or recursive execs in this branch. Keep scheduler fuel ticks
   and abort checks. Reject an unknown mount id with `denied` before touching any owner.
6. **Bounds and poison.** Apply both sides' existing transfer, traversal and headroom checks, including 1 MiB
   file chunks and response size at most half the requesting shell's headroom. Bound the response before
   constructing/lowering an oversized list. Preserve exact bytes and typed refusals. Private internal dispatch
   must not invoke methods overridden on the public FS object. A trap in a volume operation poisons its own
   store. A trap in an execution store revokes that store's scopes, fails its fibers and prevents a pending
   resume without disposing another holder's resources. On a dead shell, shell methods (including convenience
   reads/writes) observe D8. Independent access via the original standalone `fs` remains available while its own
   holder is healthy.
7. **Preserve dispatch parity.** Paths inspected by L2 command lookup (explicit paths, PATH search/cache, cwd
   checks) read the visible volume. Registering a command in A never allows B to execute its callback. Do not
   shortcut lookup using an unmounted copy of the default stubs. For non-branded mounts preserve L7b's
   `serveExtension`, revocable scope and ordered FS error mapping. `execSync` accepts a ready purely in-memory
   bridge. It continues to reject async custom FS, custom commands, fetch/network and sleep with
   `AsyncCapabilityError`.

## 6. Executable exit gates

Run `npm_config_cache=$PWD/tmp/npmcache make check`. Exercise the real test kernel, with observers on each
store's calls/epochs and without exposing memory through the public facade.

1. **Types, identity, topology.** `tsc --noEmit` accepts both literals and rejects a third. JS rejects invalid
   values before any write. Verify one fresh store per unshared shell, three distinct memories for two separate
   shells on one standalone volume, and one memory for two shared shells on one volume. Both modes expose the
   original FS object. A mixed pair has two memories. Two different shared volumes never collapse into one
   instance.
2. **Visibility and layout.** Test initial files, a supplied volume plus a conflicting `files` map (only the
   supplied volume's sentinel is present, default cwd is `/`), proc bytes, stub bytes/modes, filtered registry
   and late registration against a local control. A guest write by A is immediately visible through `fs` and a
   sync guest read by B, and vice versa, including binary data and a removal. Compare exit/status and actual
   shared contents for `__write`/`__cat`, `/bin/echo hi` and a missing path. Verify independent custom dispatch
   (B returns 127 for A's private name). Include `fs: a.fs` and browser staged writes/reads before and after
   precompile.
3. **Fast path and capability.** One six-op `__fsbatch 1` yields exactly 1 request, 6 ordered results, 1
   same-epoch resume, 0 async waits for that batch, and no lost/repeated mutation. Check all 21 op tags against
   the volume's direct results in equivalent initial states, including its current unsupported cases. Unshared
   and shared-mode local echo produce 0 FS crossings. An unknown id reaches 0 volumes. Missing `G_HOST_FS`
   produces 0 host calls. The custom-FS case still throws `AsyncCapabilityError` in sync mode, and L7b's
   one-batch 1,000-file walk (at most 11 host listings) and sentinels stay green.
4. **Budgets and concurrency.** Two shells with different profiles and `maxLiveBytes` on one volume:
   `__alloc 200000` with a 100,000 B live cap returns 126 while the larger shell succeeds. Measure independent
   ceilings in separate mode and a common monotonic ceiling in shared mode. Fill the common volume to its
   configured quota. Both see the same retained bytes and next-write refusal. Interleave at least 20 writes to
   distinct paths between the two shells and verify all 20 contents. Ordered writes within one batch produce the
   last writer's bytes.
5. **Trap domains.** In each mode leave an exec pending at a custom call. Synchronize on its arrival, then run
   `__trap` in the same store. Both affected execs return exit 1, non-enumerable `fatal: true` and the L1b fatal
   diagnostic. Subsequent execution on that store rejects `KernelPoisonedError` and no stale resume reaches the
   guest. In separate mode the sibling/standalone volume can read a pre-trap sentinel, write and run `echo ok`.
   In shared mode the sibling and common volume reject. A third, independent shell succeeds. Exercise both trap
   directions for `fs: a.fs` and a mixed pair, asserting zero guest calls to the failed owner.
6. **Integration.** G1–G8/K1 green, 30/30 L2 oracle cases, 4/4 profile suites, no new exclusions. Packed Node
   and browser consumers from both packages retain their declarations and `execSync` with an in-memory root.
   Record both Annex H browser-cap results on the updated default pack.

## 7. Points the builder must confirm and report

Report the instance counts, identities, per-store ceiling/quota observations, batch and resume counts, browser
staging order and both trap matrices. Identify the L7b merge base and re-run its mount/sentinel tests. Record
where L7a can reconnect a rebuilt separate execution store to its live volume. Replacing a shared holder must
account for every attached shell. Document the D27/V9 mode and trap behavior in the shipped divergence list
without adding an oracle exclusion.
