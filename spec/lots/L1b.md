# Lot L1b — WIT resources and JS facade

Lot spec for the build lane (Room 2). Sources of authority: `spec/SPEC_EVOL_BASH_SANDBOX.md` ("EVOL":
decisions D2-D6, D8, D9, D13-D16, D26; Annexes A, B, D, G, I) and `plan/PLAN_BASH_SANDBOX_V1.md` (lot
L1b). It builds on `spec/lots/L0.md` (pipeline, gates, package skeleton, placeholder entries) and on
`spec/lots/L1a-1.md` (the `kernel::mem` interface, used exactly as specified there). It assumes nothing
about their internals beyond those specs. Where this document and EVOL differ, stop and raise a spec
issue.

- **Size.** 6 points. **Builder.** Sonnet 5.5; fallback Gemini 4 Argon. **Review.** Carbon, full review of
  every PR, dispatched on a green `make check`.
- **Branch.** `feat/l1b-wit-facade`, worktree `tmp/l1b-wit-facade`.
- **Outcome.** The kernel exports the whole Annex A contract and the package exposes the real facade.
  There is no shell language yet (lot L2): scripts are run by a *bootstrap interpreter* (section 5) that
  is just enough to exercise fibers, host calls, limits and the membrane end to end.

## 1. Lane rules

1. No reference source and no reference package in the worktree. No network.
2. Kernel: `no_std` on wasm, no crate from a registry, all heap use through `kernel::mem`, `unsafe` only
   in `kernel/src/mem/` and in the binding module. Facade: plain ES modules with JSDoc, node built-ins
   only, and none at all in the browser entry.
3. Commits through `make commit`, no attribution trailer. English.

## 2. Scope

In scope:
- Kernel: every export of the `kernel` world (EVOL Annex A), the resources `vfs`, `shell`, `fiber`, the
  bootstrap interpreter, a first slice of the filesystem (section 6), limit plumbing for the limits
  reachable in this lot, the test-only commands.
- Bindings for the contract, by the approach Gate L0 retained (generated with the pinned `wit-bindgen`
  CLI and committed, or hand-written glue).
- JS: the driver loop, the capability membrane, the facade (`Bash`, `InMemoryFs`, custom commands), Node
  and browser loaders with the staging queue, `execSync`, `Bash.precompile()`.
- Components `kernel` and `kernel-test` in the build, replacing the placeholder entries of L0 (the
  pipeline probe and its tests stay).

Out of scope: the shell language, default filesystem layout, real commands (L2 and later); the rest of
the filesystem, `MountableFs`, lazy-file content, journal, `compact()` (L7a); host filesystem adapters
and `mount-host` behaviour (L7b); the network adapter (L16b); `logger`, trace and coverage delivery
(L1d); session drivers and `exec-oneshot` beyond a `bad-state`-free minimal path (L1c); tier C.

## 3. Allowed paths

```
wit/**                     (only if Gate L0 recorded syntax fixes still to apply)
kernel/src/**              (except kernel/src/mem/**, owned by L1a-1 and L1a-2)
kernel/Cargo.toml          (features only: `test-commands`)
host/**   types/**
test/**                    (node:test files and fixtures of this lot)
tools/components.json      (add the two components)
tools/**                   (only if a gate needs to learn the new components; say so in the PR)
BRANCH.md
```

No scope exception is needed: `package.json` already declares the entries (L0) and is not edited.
`kernel/src/mem/**` is read-only for this lot; a need for a new item there is a spec issue.

## 4. Kernel structure

| Concept | Rule |
|---|---|
| Instance state | One global state: ceiling configured or not, the feature-id table (empty until L1d), counters for handles |
| `vfs` | Owns a tree of nodes and a `Budget` with limit `maxFileSystemBytes`. Shared through `KRc`; a `shell` keeps a strong reference to its root `vfs` |
| `shell` | Immutable configuration (limits resolved to a profile plus overrides, grants, command registry, custom-command names, process info), plus the persistent state read by `env()` and `cwd()`. A root `Budget` per shell for live bytes |
| `fiber` | VM state of one exec: bootstrap program, output buffers, pending host call, epoch counter, status; a child `Budget` of the shell's (or of the parent fiber's when `parent` is given) so that nested execs charge their ancestors |
| Handles | Resource handles are the canonical-ABI ones; the kernel keeps no global table of fibers. A borrowed handle is never stored: `create`/`spawn` clone the `KRc` behind it |

No weak reference is needed: a child fiber holds its parent's `Budget`, not the parent; a `vfs` mounted
into another is held by a strong reference and cycles are rejected. `KWeak` is therefore **not** added to
the memory module.

Profiles: implement the table of EVOL Annex B (four profiles, every limit) as constant data; resolve
`limits.overrides` by name; reject at `shell.create` with `invalid-config` an unknown limit name or a set
of byte caps whose sum `maxFileSystemBytes + maxLiveBytes + maxOutputSize + headroom` exceeds the
configured ceiling. Assign `LimitId` values (from 2 upward) in one table that also holds each limit's
name; `exec-result.limit-name` uses that name.

Epochs: each fiber numbers its host calls from 1. `resume`, `read-call-input`, `push-call-data`,
`assign-variable` and `set-call-env` return `stale-epoch` unless the epoch is the pending one. After
`cancel`, every one of them returns `bad-state`, `run` returns `done` with the cancellation result, and the
output buffers remain drainable.

## 5. Bootstrap interpreter

Until L2 replaces it, a script is processed as follows, after the size checks of 5.1:

- Split into lines on `\n`; trim ASCII spaces and tabs; skip empty lines.
- Split each line into words on runs of spaces and tabs. No quoting, no expansion, no operators.
- The first word is the command name; execute lines in order; the exit code of the script is that of the
  last command executed.
- Command lookup order: a registered custom command (a `custom-command` host call, section 7.3); a
  test-only command when the kernel is built with the `test-commands` feature; otherwise the command is
  unknown: append `bash: <name>: command not found\n` to stderr, exit code 127, continue with the next
  line.
- An empty or whitespace-only script produces exit code 0 and no output.
- `exec-opts.env`, `replace-env` and `cwd` form the exec's environment and working directory; the result's
  `env` is that environment plus the entry `?` holding the last exit code as text. The shell's persistent
  `env()` and `cwd()` are **not** modified by an exec.
- Fuel: each command costs one unit plus what it declares; when the slice is exhausted `run` returns
  `out-of-fuel` and the next `run` continues where it stopped.

### 5.1 Size checks (EVOL D1), in this order

1. If `script-overflow` is set, or the script exceeds `maxSourceBytes` in UTF-8 bytes or in UTF-16 code
   units: result `limit-exceeded`, exit code 126, stderr
   `bash: script input size limit exceeded (<maxSourceBytes> bytes)\n`.
2. Else if the script is longer than 1 000 000 UTF-16 code units: exit code 2, stderr
   `bash: syntax error: Parse error at 1:1: Input too large: <n> bytes exceeds limit of 1000000\n`, where
   `<n>` is the length in UTF-16 code units.

The driver computes both lengths from the JavaScript string before lowering; it sets `script-overflow`
(and sends an empty script) whenever the UTF-8 form would exceed 3 MiB.

### 5.2 Other results reachable in this lot

| Situation | Result |
|---|---|
| `cancel(aborted)`, or the driver's abort signal already set before the first `run` | status `aborted`, exit 124, stderr `bash: execution aborted\n` |
| `cancel(deadline)` | status `deadline`, exit 124, stderr `bash: execution aborted\n` |
| stdin beyond `maxInputBytes` | status `limit-exceeded`, exit 126, limit name `maxInputBytes` |
| output beyond `maxOutputSize` | status `limit-exceeded`, exit 126, stderr `bash: pipeline: total output size exceeded (><cap> bytes), increase executionLimits.maxOutputSize\n` |
| a `kernel::mem` charge refused with `Limit(id)` | status `limit-exceeded`, exit 126, limit name of `id`; stderr `bash: <limit name> exceeded\n` (placeholder wording until the lot that owns the limit) |
| a `kernel::mem` operation refused with `Ceiling` | status `limit-exceeded`, exit 126, limit name `memory-ceiling` |

In every limit case the fiber ends, its buffers stay drainable, and the instance and the `vfs` remain
usable by other fibers.

### 5.3 Test-only commands (feature `test-commands`)

Present only in the `kernel-test` component; the release `kernel` component must not contain them (a
test checks that none of these names appears in its bytes).

| Command | Effect |
|---|---|
| `__out <n>` | Appends `n` bytes (`0x61`) to stdout through the budgeted path |
| `__err <n>` | Same on stderr |
| `__stdin` | Copies the fiber's stdin to stdout |
| `__alloc <n>` | Allocates and keeps `n` bytes charged to the fiber's live budget until the fiber ends |
| `__spin` | Loops forever, one fuel unit per iteration |
| `__exit <n>` | Ends the script with exit code `n` |
| `__setenv <name> <value>` | Sets a variable in the exec's environment |
| `__write <path> <text>` | Writes a file in the shell's `vfs` |
| `__cat <path>` | Appends a file to stdout; exit 1 with `cat: <path>: No such file or directory\n` if missing |
| `__hostfs <mount-id> <path>` | Issues `fs-batch` with one `stat`; prints `ok` or the error code |
| `__lazy <path>` | Issues `lazy-file`; appends the pushed `body` to stdout |
| `__fetch <url>` | Issues `fetch` (method GET); prints the status then the pushed `body` |
| `__sleep <ms>` | Issues `sleep` |
| `__clock` | Issues `clock`; prints `wall-ms` |
| `__entropy <n>` | Issues `entropy`; prints the byte count received |
| `__trap` | Executes the `unreachable` instruction |

A host call for a capability absent from the shell's grants (`host-fs`, `network`, `real-sleep`,
`real-clock`, `entropy`, `custom-commands`) is **not issued**. For `__hostfs`, `__lazy` and `__fetch` the command prints `denied`
on stderr and returns exit code 126. `__sleep`, `__clock` and `__entropy` instead fall back to the
virtual behaviour of EVOL D9 (no wait, a virtual clock starting at 0 and advanced by sleeps, a fixed-seed
generator) and succeed.

## 6. Filesystem slice

`vfs.op` implements, on an in-memory tree with byte paths (absolute, `/`-separated): `stat`, `lstat`,
`exists`, `read-dir` (names in ascending byte order), `read-file` (offset and length), `write-file`
(offset, truncate, mode, mtime; parent must exist), `mkdir` (with `recursive`), `remove` (file or empty
directory; `recursive` and `force`), `all-paths`. Every other operation, `mount-vfs`, `mount-host` and
`unmount` return the error code `unsupported`. File content is charged to the `vfs` budget; exceeding it
returns `no-space`. `take-dirty` returns an empty list and `used-bytes` the budget's usage. Path
resolution collapses `.` and `..` and never leaves `/`.

`vfs.create` makes an empty tree containing only `/`. The default layout of the reference is lot L2's.

## 7. JavaScript side

File layout (suggested; entry file names are fixed by L0's `package.json`):

```
host/index.js            Node entry: Bash, InMemoryFs, defineCommand, helpers, error classes
host/browser.js          browser entry: same surface, no node: import, staging queue
host/node/load.js        synchronous module load from disk (the only node: imports of the core)
host/core/driver.js      fiber loop, chunking, pre-checks
host/core/membrane.js    revocable capability scopes
host/core/facade.js      Bash, exec, execSync, option validation
host/core/fs.js          InMemoryFs over vfs.op
host/core/bytes.js       byte helpers of EVOL Annex D.5
types/index.d.ts  types/browser.d.ts
```

### 7.1 Public declarations

The facade implements these declarations, which the builder writes into `types/` (they are this lot's
interface; `IFileSystem`, `NetworkConfig` and the byte helpers are as EVOL Annex D.3-D.5 states them):

```ts
export type ExecutionLimitProfile = "normal" | "hardened" | "contain" | "micro";
export interface BashLogger { info(message: string, data?: Record<string, unknown>): void;
                              debug(message: string, data?: Record<string, unknown>): void; }
export interface BashOptions {
  files?: InitialFiles; env?: Record<string, string>; cwd?: string; fs?: IFileSystem;
  executionLimits?: ExecutionLimits; executionLimitProfile?: ExecutionLimitProfile;
  maxCallDepth?: number; maxCommandCount?: number; maxLoopIterations?: number;
  fetch?: SecureFetch; network?: NetworkConfig;
  python?: boolean; javascript?: boolean | JavaScriptConfig;
  commands?: string[]; sleep?: (ms: number) => Promise<void>; customCommands?: CustomCommand[];
  logger?: BashLogger; trace?: (event: TraceEvent) => void; coverage?: { hit(feature: string): void };
  defenseInDepth?: unknown;
  processInfo?: { pid?: number; ppid?: number; uid?: number; gid?: number };
}
export interface ExecOptions { env?: Record<string, string>; replaceEnv?: boolean; cwd?: string;
  rawScript?: boolean; stdin?: string; stdinKind?: "text" | "bytes"; signal?: AbortSignal; args?: string[]; }
export interface BashExecResult { stdout: string; stderr: string; exitCode: number;
  env: Record<string, string>; stdoutKind?: "text" | "bytes"; }
export declare class Bash {
  constructor(options?: BashOptions);
  readonly fs: IFileSystem;
  exec(commandLine: string, options?: ExecOptions): Promise<BashExecResult>;
  execSync(commandLine: string, options?: Omit<ExecOptions, "signal">): BashExecResult;
  readFile(path: string): Promise<string>;
  writeFile(path: string, content: string): Promise<void>;
  getCwd(): string;
  getEnv(): Record<string, string>;
  registerCommand(command: Command): void;
  static precompile(source?: unknown): Promise<void>;
}
export interface ExecResult { stdout: string; stderr: string; exitCode: number; stdoutKind?: "text" | "bytes"; }
export interface Command { name: string; trusted?: boolean;
  execute(args: string[], ctx: CommandContext): Promise<ExecResult>; }
export interface LazyCommand { name: string; trusted?: boolean; load: () => Promise<Command>; }
export type CustomCommand = Command | LazyCommand;
export declare function defineCommand(name: string,
  execute: (args: string[], ctx: CommandContext) => Promise<ExecResult>,
  options?: { trusted?: boolean }): Command;
export interface CommandContext {
  fs: IFileSystem; cwd: string; env: Map<string, string>; exportedEnv?: Record<string, string>;
  stdin: ByteString; limits: Required<ExecutionLimits>; signal?: AbortSignal;
  exec?(command: string, options: { cwd: string; env?: Record<string, string>; replaceEnv?: boolean;
        stdin?: string; stdinKind?: "text" | "bytes"; args?: string[]; signal?: AbortSignal }): Promise<ExecResult>;
  origCommand?(args: string[]): Promise<ExecResult>;
  fetch?: SecureFetch; sleep?(ms: number): Promise<void>;
  assignShellVariable?(name: string, value: string, subscript?: string): Promise<void>;
  fileDescriptors?: Map<number, string>; getRegisteredCommands?(): string[];
  trace?(event: TraceEvent): void; coverage?: { hit(feature: string): void }; xpgEcho?: boolean;
}
export declare class KernelPoisonedError extends Error {}     // any call after a trap
export declare class AsyncCapabilityError extends Error {}    // execSync with an async capability
export declare class NotSupportedError extends Error {}       // transform(), registerTransformPlugin()
export declare function getCommandNames(): string[];
export declare function getNetworkCommandNames(): string[];
export declare function getPythonCommandNames(): string[];     // ["python3", "python"]
export declare function getJavaScriptCommandNames(): string[]; // ["js-exec", "node"]
```

`getCommandNames()` returns the empty list in this lot (no command yet) and `getNetworkCommandNames()`
returns `["curl"]`. `types/browser.d.ts` re-exports the same surface.

### 7.2 Construction and options

- Each `Bash` owns one kernel instance unless it is given an `InMemoryFs`, in which case it attaches a
  `shell` to the instance that owns that filesystem (EVOL D5). Instance creation calls `configure` with
  the ceiling of the profile (EVOL Annex B); a `Bash` attaching to an existing instance calls
  `raise-ceiling` when its profile needs more.
- Default profile: `contain`. `executionLimits` entries become `limits.overrides`; the legacy top-level
  `maxCallDepth`, `maxCommandCount`, `maxLoopIterations` are aliases.
- Initial working directory and environment, in this key order: `HOME`, `PATH`=`/usr/bin:/bin`,
  `IFS`=space-tab-newline, `OSTYPE`=`linux-gnu`, `MACHTYPE`=`x86_64-pc-linux-gnu`, `HOSTTYPE`=`x86_64`,
  `HOSTNAME`=`localhost`, `PWD`, `OLDPWD`, `OPTIND`=`1`, then the caller's `env` entries, then
  `SHELLOPTS`=`braceexpand:hashall:interactive-comments`, `BASHOPTS`=`globskipdots`. When neither `cwd`
  nor `files` is given: cwd `/home/user` and `HOME`=`/home/user`; otherwise cwd is the given `cwd` or `/`,
  and `HOME`=`/`. `PWD` and `OLDPWD` equal the cwd. `getEnv()` returns this object and `getCwd()` the cwd;
  neither changes after an `exec`.
- `files`: written into the `vfs` before the first exec (content as string or bytes, or
  `{content, mode, mtime}`); a function-valued entry (lazy file) is accepted and stored for L7a, reading it
  rejects with `NotSupportedError` in this lot.
- Options of other lots are validated for type and stored without effect: `network`, `fetch`, `logger`,
  `trace`, `coverage`, `commands`, `processInfo`. A non-`InMemoryFs` `fs` is accepted; operations on it
  work through the membrane for custom commands and `bash.fs` returns the given object.
- `python` and `javascript` (any truthy value, including a configuration object): accepted; construction
  never throws because of them; the commands `python3`, `python`, `js-exec`, `node` are absent, so running
  them gives the unknown-command result (exit 127). The two name helpers return the names regardless.
- `defenseInDepth`: any value accepted, no effect. No security-violation export exists.
- Invalid option types (for example `cwd: 3`, `executionLimitProfile: "x"`, an override that does not fit
  the ceiling) throw a `TypeError` or `RangeError` at construction.
- `transform()` and `registerTransformPlugin()` exist and throw `NotSupportedError`.

### 7.3 Driver loop

For one `exec` (asynchronous form):
1. Pre-checks before lowering anything (EVOL Annex A rules): script lengths (5.1); every non-streamed
   value (env values, args, cwd) against `maxStringLength` and against half the profile's headroom in
   total. A failed pre-check produces the limit result (exit 126) without calling the kernel.
2. `fiber.spawn`; send stdin with `write-stdin` in chunks of at most 1 MiB, the last with `last = true`.
3. Loop `run(fuel)` with a fuel slice of 100 000:
   - `out-of-fuel`: drain stdout and stderr (`take-*` with max 1 MiB until empty); if the abort signal is
     set call `cancel(aborted)`; if the wall-time limit of the profile has passed call `cancel(deadline)`;
     yield to the event loop at least every 10 ms of wall time; continue.
   - `host-call`: drain outputs, then serve the request through the membrane (7.4) and call `resume` with
     the outcome; a request for which no handler is configured is answered `denied`.
   - `done`: drain outputs, build the result.
4. Result: stdout and stderr converted by the rule of EVOL Annex D.2 (valid UTF-8 decoded, other bytes one
   character each), `exitCode`, `env` as an object in kernel order, `stdoutKind` only when the kernel
   reports bytes.
5. `exec` never rejects for a script outcome. It rejects with `KernelPoisonedError` if the instance is
   poisoned (7.6), and with `TypeError` for invalid options.

Concurrent `exec` calls on one `Bash` each run their own fiber and interleave at `await` points; the
driver holds no lock.

`execSync` runs the same loop without yielding. It throws `AsyncCapabilityError` before running anything
if the instance was built with an `fs` other than `InMemoryFs`, a `network` or `fetch` option, a `sleep`
function, or any custom command; it is absent from the browser entry's guarantees until the module is
compiled (it throws `AsyncCapabilityError` if called while the staging queue is non-empty).

Views over linear memory are those of the generated glue; the driver keeps no typed-array view across a
kernel call.

### 7.4 Membrane

Every host call is served inside a *scope* tied to `(fiber, epoch)`:
- The scope hands the extension (custom command, custom `IFileSystem`, `fetch`, `sleep`) wrapper objects,
  never the caller's originals nor kernel handles. Each wrapper method first checks that the scope is
  live.
- The scope is revoked when the call settles, when the fiber is cancelled, or when the abort signal
  fires. After revocation every wrapper method throws (or rejects with) an error named
  `ExecutionAbortedError`, and a `resume`, `push-call-data`, `read-call-input`, `assign-variable` or
  `set-call-env` attempted for that scope is dropped by the driver without reaching the kernel.
- On cancellation the driver waits at most `maxExtensionCleanupTimeMs` (profile value) for the pending
  extension promise, then abandons it and ends the exec with the cancellation result.
- Custom command call: arguments decoded as text; `ctx.stdin` is fetched lazily with `read-call-input`
  (fd 0) on first access; `ctx.env` is a copy (`Map`), written back with `set-call-env` after the command
  resolves only when the command is trusted; `ctx.exportedEnv` holds the exported entries; `ctx.exec`
  spawns a child fiber (`parent` = the calling fiber) and runs the same driver loop; `ctx.origCommand`
  spawns a child fiber with `bypass-custom`; `ctx.assignShellVariable` maps to `assign-variable`;
  `ctx.getRegisteredCommands` returns the names known to the facade; `ctx.fileDescriptors` is built from
  the call's `fds` with contents pulled lazily. The command's result is pushed with `push-call-data` in
  1 MiB chunks, then `resume(command-exit)`. A command that throws or rejects gives exit code 1 with its
  message on stderr, host paths replaced by `<path>`.
- `trusted: false` commands (`trusted` defaults to true for commands passed in `customCommands`, as in the
  reference): `set-call-env` is never sent and their `ctx.env` is frozen against mutation.
- Sandbox data never becomes a property name on a plain object: environment maps and file tables in the
  host use `Map` or null-prototype objects (EVOL S10).
- Lazily loaded commands (`LazyCommand`): `load()` is awaited on first use inside the scope.

### 7.5 Loaders

- Node (`host/index.js`): the core module file is read with `readFileSync(new URL(...))` and compiled
  with `new WebAssembly.Module` once per process (module cache); `new Bash()` is fully synchronous.
- Browser (`host/browser.js`): no `node:` import, directly or transitively. The module is obtained by
  `Bash.precompile(source)`, where `source` is a `Response`, a promise of one, a URL, an `ArrayBuffer` or
  a `WebAssembly.Module`; without a prior `precompile`, the first awaited operation compiles from the
  default URL next to the entry file. Until the module exists, a **staging queue** records, in order:
  construction options, `files`, `registerCommand`, and `InMemoryFs` synchronous writes; it is replayed
  when the first awaited call (`exec`, `readFile`, `writeFile`, any promise-returning `fs` method)
  instantiates the kernel. Before that, `getCwd()` and `getEnv()` return the staged initial values,
  synchronous reads (`getAllPaths`, `resolvePath` excepted, which is pure) throw a `TypeError` asking for
  `await Bash.precompile()`.
- The staging queue is unit-tested in node by loading the browser entry with a module provider that
  resolves later; verification in a real browser is lot L19.

### 7.6 Trap poisoning (EVOL D8)

If any kernel call throws a `WebAssembly.RuntimeError`, the driver marks the instance poisoned: the
current exec resolves with exit code 1, stderr `bash: internal error: sandbox instance failed\n`, and a
non-enumerable property `fatal: true`; every other pending exec of that instance resolves the same way;
every later call on a `Bash` or `InMemoryFs` of that instance rejects (or throws, for synchronous
methods) with `KernelPoisonedError`. Nothing is read from the instance afterwards.

## 8. Build changes

`tools/components.json` gains: `kernel` (crate `sentropic-bash-kernel`, world `kernel`, release) and
`kernel-test` (same crate with `--features test-commands`). `host/` loads `kernel`; tests load
`kernel-test` through an internal option of the loader, not part of the public declarations. The packed
tarball contains `kernel` only.

## 9. Tests and exit gates

`node:test` files under `test/`, run by `make check`; native kernel tests by `cargo test`.

1. **Contract.** G1 and G4 pass for both components: zero imports, and the component's world equals
   `wit/`. Binding regeneration check passes (or the PR states the hand-written path retained at Gate
   L0).
2. **Result contract.** Empty script: exit 0, empty outputs. `nosuch arg`: exit 127, stderr
   `bash: nosuch: command not found\n`. Script of 1 000 006 code units: exit 2 with the message of 5.1.
   `maxSourceBytes: 100` and a 205-byte script: exit 126 with the message of 5.1. Abort signal already
   set: exit 124, `bash: execution aborted\n`. `maxOutputSize: 10` with `__out 16`: exit 126 and the
   message of 5.2. Six cases, exact strings.
3. **Environment.** `new Bash().getCwd()` is `/home/user`; `getEnv()` has exactly the twelve keys of 7.2
   in order with the stated values; with `{cwd: "/x", env: {A: "1"}}`: cwd `/x`, `HOME` `/`, `A` placed
   after `OPTIND`; after an `exec` that runs `__setenv B 2`, the result's `env` contains `B` and `?`, and
   `getEnv()` is unchanged.
4. **Chunking.** `__stdin` with 3 MiB + 1 byte of stdin returns the same bytes; `__out 5000000` under the
   `normal` profile returns 5 000 000 bytes; no single canonical-ABI `list<u8>` passed by the driver
   exceeds 1 048 576 bytes (asserted by instrumenting the driver in the test).
5. **Pre-check.** An env value larger than `maxStringLength`, and args totalling more than half the
   headroom, each give exit 126 with zero kernel calls made for that exec (counted).
6. **Limits are results.** `__alloc` beyond `maxLiveBytes`, stdin beyond `maxInputBytes`, and an
   allocation beyond the ceiling each give exit 126 with the right limit name; a following
   `exec('__out 3')` on the same `Bash` succeeds. Overrides that do not fit the ceiling throw at
   construction.
7. **Fuel and cancel.** `__spin` with an abort after 20 ms resolves with exit 124 within 200 ms; with
   `executionLimits: {maxExecutionTimeMs: 200}` and no abort it ends by the wall-time limit with exit 124
   within 400 ms; outputs produced before
   cancellation are returned.
8. **Epochs.** Driving the kernel directly: `resume` with a wrong epoch returns `stale-epoch`; after
   `cancel`, `resume` returns `bad-state` and `take-stdout` still drains.
9. **Concurrency and re-entry.** Two overlapping `exec` calls, each running a custom command that awaits
   a timer, both complete with their own output. A custom command calling `ctx.exec('__out 3')` gets
   `aaa` back while its parent is suspended, and the child's allocation is charged to the parent's
   budget (a child `__alloc` beyond the parent's remaining live budget gives 126).
10. **Default deny (S2).** With no grant, `__hostfs`, `__lazy`, `__fetch` each end with `denied` and a
    recording driver sees zero host calls; `__sleep 5000` returns immediately and `__clock` after it
    prints `5000`; `__entropy 16` prints `16` and two instances give the same bytes.
11. **Membrane (S5, S6).** A custom command that keeps `ctx.fs` and calls it after it resolved gets
    `ExecutionAbortedError`; one that ignores abort and resolves 500 ms later does not delay the exec
    beyond `maxExtensionCleanupTimeMs` + 50 ms and cannot write a file afterwards; a late `resume` is
    not delivered (kernel call count unchanged).
12. **Trust.** An untrusted command's change to `ctx.env` is not visible to the rest of the script; a
    trusted one's is. A command that throws gives exit 1 and a message without the worktree's absolute
    path.
13. **Prototype pollution (S10).** Env names, file names, command names and args equal to `__proto__`,
    `constructor`, `prototype`, `toString` round-trip as data; `({}).polluted` is undefined afterwards.
14. **Shared filesystem (S14).** Two `Bash` objects on one `InMemoryFs` with different grants and
    profiles: a file written by one is read by the other; a custom command of one is unknown (127) in the
    other; a limit hit in one leaves the other usable.
15. **Runtime options (EVOL D13, V5).** `new Bash({python: true})`, `({javascript: true})` and
    `({javascript: {}})` construct; `python3`, `python`, `js-exec`, `node` each give exit 127 and the
    unknown-command message; `getPythonCommandNames()` is `["python3","python"]` and
    `getJavaScriptCommandNames()` is `["js-exec","node"]`. `new Bash({defenseInDepth: true})` and
    `({defenseInDepth: {enabled: false}})` behave exactly like `new Bash()` on the six cases of gate 2;
    the package exports no name containing `DefenseInDepth` or `SecurityViolation`.
16. **Trap poisoning (S12).** On `kernel-test`: `__trap` resolves with exit 1, the fatal message and
    `fatal === true`; a concurrent exec on the same instance resolves the same way; the next `exec`,
    `readFile` and `fs.exists` reject with `KernelPoisonedError`; a second `Bash` on its own instance is
    unaffected. The release `kernel` bytes contain none of the test-only command names.
17. **Host disk (EVOL D26).** A sentinel file is created in the worktree's `tmp/`; a default `new Bash()`:
    `fs.exists(<its absolute path>)` is false, `readFile` rejects, and during the whole test no `node:fs`
    function is called by the package except the single read of the core module (asserted by wrapping
    `node:fs` in the test process before import).
18. **Filesystem slice.** One test per operation of section 6, including `no-space` at the budget and
    `unsupported` for the others; `InMemoryFs` methods not backed yet reject with an error whose message
    starts with `ENOSYS`.
19. **execSync.** Returns the same result as `exec` for the six cases of gate 2; throws
    `AsyncCapabilityError` with a custom command configured.
20. **Staging queue.** With the browser entry and a module provider resolving after 50 ms: construction,
    `registerCommand` and `fs.writeFileSync` before the first `await` are all effective in the first
    `exec`; `getCwd()` before compilation returns the staged value; G2's browser-graph check passes.
21. **Types.** `tsc --noEmit` passes on `types/` and on a consumer fixture that uses every declaration of
    7.1.
22. `make check` green on a clean worktree, G1-G8 `OK`; the `npm pack` test of L0 updated to run
    `new Bash().exec('')` from the installed tarball and get exit code 0.

## 10. Handover notes

The PR description states: the binding approach used; the `LimitId` table; the module tree; every
placeholder left for later lots (bootstrap interpreter and limit wording for L2, filesystem operations
for L7a, `mount-host` for L7b, trace and coverage for L1d); deviations from this spec, each with its
reason.
