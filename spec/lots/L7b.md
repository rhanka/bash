# Lot L7b — Host filesystem adapters (`OverlayFs`, `ReadWriteFs`, custom `IFileSystem`)

Lot spec for the build lane (Room 2). Sources of authority: `spec/SPEC_EVOL_BASH_SANDBOX.md` ("EVOL":
decisions D1, D2, D5, D6, D9, D10, D14, D15, D26; security invariants S1, S2, S5, S6, S7, S10; Annexes A, B,
C, D.1, D.3, E.7) and `plan/PLAN_BASH_SANDBOX_V1.md` (lot L7b, sections 5, 6 and 8). It builds directly on
`spec/lots/L0.md` (gates G1-G8) and `spec/lots/L1b.md` (WIT resources, `Vfs`, `Fiber`, driver loop, membrane,
`Bash`, `InMemoryFs`, `addHostPathHints`, `sanitizeMessage`). Because L7b runs in Wave W3 in parallel with L2,
it assumes **only** L1b's kernel and facade (not L2's shell parser or `echo`); the merge with L2 is section
9. Where this document and EVOL differ, stop and raise a spec issue, except for the one known EVOL erratum
recorded here:

> **EVOL Annex D.3 erratum (writeFile).** D.3 says `writeFile` "creates or replaces; parent must exist".
> The reference (`just-bash@3.6.0`) creates missing parent directories in `writeFile` and `appendFile` on
> all three of `InMemoryFs`, `OverlayFs` and `ReadWriteFs`; only `createExclusive` requires an existing
> parent. Under EVOL D1 the reference wins: this lot creates parents, and the D.3 row is to be corrected
> in the next EVOL revision.

- **Size.** 3 points. **Builder.** Gemini 4 Argon; fallback Sonnet 5.5. **Review.** Carbon, full review of
  every PR, dispatched on a green `make check`.
- **Branch.** `feat/l7b-host-fs`, worktree `tmp/l7b-host-fs`.
- **Outcome.** Ships the Node-only host filesystem adapters `OverlayFs` and `ReadWriteFs` with every EVOL D10
  and S7 path-security and inode-safety gate, completes the kernel `mount-host` / `unmount` mount table and
  the batched `fs-batch` host call across all 21 `fs-op` variants for `OverlayFs`, `ReadWriteFs` and
  caller-provided `IFileSystem` objects, and proves the EVOL D26 sentinel-file gates.

## 1. Lane rules

1. No reference source and no reference package in the worktree. No network.
2. Kernel: `no_std` on `wasm32-wasip2`, zero external crates, all heap allocations through `kernel::mem`,
   `unsafe` only in `kernel/src/mem/` and `kernel/src/bindings/`.
3. Host: plain ES modules with JSDoc and hand-written `.d.ts`. **Gate G2 placement rule:** only files inside
   `host/node/**` may import `node:*` built-ins (`node:fs`, `node:path`, `node:crypto`). `host/index.js`
   re-exports `./node/...` via relative specifiers, `host/core/**` uses zero `node:*` imports, and
   `host/browser.js` / `types/browser.d.ts` never export or reach `OverlayFs` or `ReadWriteFs`.
4. Never write to `/tmp`; all test directories, FIFOs, symlinks and sentinel fixtures go under the
   git-ignored `tmp/` of the worktree (run tests with `TMPDIR=<worktree>/tmp`).
5. Commits through `make commit`, no attribution trailer. English.

## 2. Scope

In scope:
- Node-only `OverlayFs` (`host/node/overlay-fs.js`) and `ReadWriteFs` (`host/node/read-write-fs.js`), sharing
  host-path validation (`host/node/host-paths.js`) and host-error rewriting (`host/node/fs-errors.js`),
  exported from `host/index.js` and typed in `types/index.d.ts`.
- Kernel `Vfs::mount_host` / `Vfs::unmount` (mount table in `kernel/src/fs.rs`), fiber suspension on
  host-mounted paths via `HostReq::FsBatch` across all 21 `fs-op` variants (`kernel/src/fiber.rs`,
  `kernel/src/bindings/adapt.rs`), and the test-only commands `__cat`, `__write`, `__walk`, `__fsbatch` in
  `kernel/src/fiber/testcmd.rs`, so kernel-to-host batching is tested end to end.
- Host driver `serveFsBatch` (`host/core/fs-batch.js`, called from `host/core/driver.js`) serving every
  `fs-op` through the revocable `Scope` membrane for `OverlayFs`, `ReadWriteFs` and any custom
  `IFileSystem`, with one directory listing per directory on `walk` / `read-dir-plus`.
- Facade: host mount registration and the default layout written into a host `fs` that has synchronous
  `mkdirSync` / `writeFileSync` (section 7.2).
- Adversarial D10/S7 suite, the 1 000-file batching gate, EVOL D26 sentinel gates, `tsc --noEmit`
  declaration checks, and the `test/pack.test.mjs` file list.

Out of scope: `MountableFs` and in-kernel symlinks/hard links/chmod/utimes/lazy files/journal/`compact()`
(L7a); `find`, `tar` and every other command (L9a-2, L16a); the shell language (L2 onward).

## 3. Allowed paths

```
kernel/src/fs.rs
kernel/src/fiber.rs
kernel/src/fiber/testcmd.rs
kernel/src/shell.rs        (only if the G_HOST_FS mount check needs a helper)
kernel/src/bindings/adapt.rs
host/index.js
host/core/facade.js
host/core/driver.js        (only the call into fs-batch.js; serveFsBatch moves out)
host/core/fs-batch.js
host/node/overlay-fs.js
host/node/read-write-fs.js
host/node/host-paths.js
host/node/fs-errors.js
types/index.d.ts
test/**                    (host-fs, D10 adversarial, batching and D26 tests; pack.test.mjs)
BRANCH.md
```

Read-only: `kernel/src/mem/**`, `wit/**` (it already declares `mount-host`, `unmount`, `fs-batch`, all 21
`fs-op` variants and all 15 `fs-error-code` values). No `BR<n>-EX<k>` scope exception is needed:
`package.json` already ships `host/` and `types/`.

## 4. Public exports and TypeScript surface (Node-only, EVOL Annex D.1 and D.3)

Export `OverlayFs` and `ReadWriteFs` from `host/index.js` **only** (never from `host/browser.js`), and add
their declarations to `types/index.d.ts` **only** (never `types/browser.d.ts`). Types `IFileSystem`,
`FileContent`, `FsStat` come from `types/core.d.ts` (L1b).

```ts
export interface OverlayFsOptions {
  /** Host directory shown inside the sandbox. */
  root: string;
  /** Where `root` appears in the sandbox. @default "/home/user/project" */
  mountPoint?: string;
  /** Reject every write with EROFS. @default false */
  readOnly?: boolean;
  /** Largest host file `readFile*` returns, in bytes; 0 = no cap. @default 10485760 */
  maxFileReadSize?: number;
  /** Bytes the in-memory layer may retain. @default 1073741824 */
  maxMemoryBytes?: number;
  /** Follow and create symlinks whose targets stay inside `root`. @default false */
  allowSymlinks?: boolean;
}

export declare class OverlayFs implements IFileSystem {
  constructor(options: OverlayFsOptions);
  getMountPoint(): string;
  /** Synchronous, memory layer only; used to seed the sandbox layout. */
  mkdirSync(path: string, options?: { recursive?: boolean }): void;
  /** Synchronous, memory layer only; parents are created. */
  writeFileSync(path: string, content: FileContent): void;
  /* + the 23 IFileSystem methods of types/core.d.ts, the 3 optional ones included */
}

export interface ReadWriteFsOptions {
  root: string;
  /** Largest file `readFile*` returns, in bytes; 0 = no cap. @default 10485760 */
  maxFileReadSize?: number;
  /** Largest multiply-linked file that append/chmod/utimes may copy to isolate it; 0 = no cap. @default 104857600 */
  maxCopyOnWriteSize?: number;
  /** Largest regular file `cp` copies; 0 = no cap. @default 0 */
  maxCopySize?: number;
  /** Follow and create symlinks whose targets stay inside `root`. @default false */
  allowSymlinks?: boolean;
}

export declare class ReadWriteFs implements IFileSystem {
  constructor(options: ReadWriteFsOptions);
  /* + the 23 IFileSystem methods of types/core.d.ts, the 3 optional ones included */
}
```

**Construction (both classes), synchronous, in this order:**
1. `OverlayFs` only: the mount point (default `/home/user/project`) loses trailing `/` characters unless it
   is `/`; if it does not then start with `/`, throw `Error("Mount point must be an absolute path: <given>")`
   (the option text is sandbox-side, not a host path). A `maxMemoryBytes` that is not a non-negative safe
   integer throws `Error("OverlayFs: invalid maxMemoryBytes")`.
2. Resolve `root` to an absolute host path. If it does not exist, throw
   `Error("<Class> root does not exist")`; if it is not a directory, throw
   `Error("<Class> root is not a directory")` (`<Class>` is `OverlayFs` or `ReadWriteFs`). The root path
   itself never appears in a message (S7).
3. Keep both the absolute root and its symlink-free form (the "canonical root"), and register both with
   `addHostPathHints` so the driver's `sanitizeMessage` also strips them.
4. `OverlayFs` only: the mount point and its ancestors exist as directories (`0o755`) in the memory layer.

## 5. Path-security and inode-safety gates (EVOL D10, S7, S10)

Every method of both classes applies these gates before it touches the host disk. "Host operation" below
means any `node:fs` call.

### 5.1 NUL characters and virtual paths
- `exists(path)` resolves `false` when `path` contains a NUL character, and never rejects for a missing path.
- Every other method rejects with `Error("ENOENT: path contains null byte, <op> '<path>'")` when any of its
  path arguments (both paths of two-path methods, and the target of `symlink`) contains a NUL character.
- Virtual paths are POSIX: split on `/`, drop empty and `.` segments, apply `..` without going above `/`
  (`/a/../../b` is `/b`). Every lookup table keyed by sandbox paths is a `Map` or a null-prototype object,
  so `__proto__`, `constructor`, `toString` are ordinary names (S10).

### 5.2 Containment on canonical paths
- `ReadWriteFs` maps virtual `/x` to `<root>/x`. `OverlayFs` maps a path at or under the mount point to the
  same suffix under `root`; any other path lives only in the memory layer and causes no host operation.
- Before use, the host path is resolved to its symlink-free form: the longest existing ancestor is
  resolved by the OS, and the missing components are appended unchanged. The result must be the canonical
  root or lie under it (a boundary-aware test: `/data` does not contain `/datastore`); otherwise the path
  is refused (section 5.3 says how). Every later host operation of that call uses this canonical path,
  never the unvalidated one, so a symlink swapped in between validation and use cannot redirect it.
- A missing leaf that is in fact a dangling symlink is detected (the OS resolver reports it as missing):
  the leaf is examined without following it, and a symlink there is treated as in section 5.3.
- Operations that act on the final name itself (`lstat`, `readlink`, `rm`, `mv` source, `symlink` and
  `link` destinations, `mkdir`, `createExclusive`) validate the parent directory and keep the final name
  as given, so they never follow a symlink that occupies the name.

### 5.3 Symlink policy
**`allowSymlinks: false` (default).** A path whose resolution crosses a symlink anywhere (intermediate or
leaf, inside or outside `root`, dangling or not) is refused when the operation would follow it:
`ReadWriteFs` rejects with `EACCES: permission denied, '<path>' resolves outside sandbox` (reads, `stat`,
`readdir`, writes, `mkdir`, `createExclusive` alike) and `exists` resolves `false`; `OverlayFs` treats the
host entry as absent: `readFile` and `stat` reject with `ENOENT: no such file or directory, <op> '<path>'`,
`readdir` of a symlinked directory lists only memory-layer entries (none at first), and writes, `mkdir` and
`createExclusive` through it succeed in the memory layer. Operations that do not follow the leaf still work on a symlink:
`lstat` returns it as a symlink, and `readlink` returns its target as described below. `symlink()` always
rejects with `EPERM: operation not permitted, symlink '<linkPath>'` (also on a read-only `OverlayFs`). Files
are opened with `O_NOFOLLOW`; an `ELOOP` from that open (a symlink swapped in after validation) becomes
`EACCES: permission denied, '<path>' is a symlink` on `ReadWriteFs`.

**`allowSymlinks: true`.** A symlink is followed when its fully resolved target stays inside the canonical
root, including a dangling leaf whose target path stays inside; a target outside is refused as above
(`EACCES ... resolves outside sandbox`). A chain that loops or exceeds 40 hops rejects with `ELOOP`.
`ReadWriteFs.symlink(target, linkPath)` stores the target as given after checking that it resolves inside
the root (from the link's parent for a relative target; a target starting with `/` is read as a virtual
path), else `EACCES`.

**`readlink` (both settings, both classes).** A relative target is returned as stored; an absolute target
that resolves inside the canonical root is returned as the matching virtual path; any other absolute target
is reduced to its last component (`/etc/hostname` -> `hostname`; a dangling `/nonexistent/zz` -> `zz`), so
neither a host path nor its depth leaks.

**`ReadWriteFs.mv` and symlinks.** Moving a symlink checks first that its target still resolves inside the
root from the destination; if not, nothing moves and the call rejects with
`EACCES: permission denied, mv '<src>' -> '<dest>' would create symlink escaping sandbox`. Moving a directory
renames it, then checks every symlink in the moved tree; if one now escapes, the rename is undone and the
call rejects with `EACCES: permission denied, mv '<src>' -> '<dest>' would create symlinks escaping sandbox`.

### 5.4 Special files (FIFOs, sockets, character and block devices)
- A host entry that is neither a regular file nor a directory is never opened for reading or writing. Any
  open of a host file passes `O_NONBLOCK` (where the platform defines it) and checks the opened descriptor
  is a regular file before any read or write, which also catches a regular file replaced by a FIFO after
  validation.
- `stat`, `lstat`, `readdir` and `exists` report special files normally (`isFile: false`).
- Results (`<path>` is the virtual path):

  | Call on a host special file | `OverlayFs` | `ReadWriteFs` |
  |---|---|---|
  | `readFile`, `readFileBuffer`, `readFileBytes` | `EACCES: cannot read special file '<path>'` | same |
  | `appendFile` | `EACCES: cannot append special file '<path>'` | same |
  | `writeFile` | succeeds in the memory layer, host untouched | `EACCES: cannot write special file '<path>'` |
  | `cp` (source) | resolves, creates nothing | `EIO: cp '<path>'` |

- **Deliberate difference, `upstream-defect`.** The reference blocks forever on `readFile` of a host FIFO
  (both classes) and on `OverlayFs.appendFile` of one. EVOL D10 and S7 ("special files are never opened")
  forbid that, so the two read rows above use the reference's own `cannot ... special file` wording for a
  result the reference never returns. It is listed for the owner under EVOL Annex E.7 (section 9).

### 5.5 Atomic exclusive create (`createExclusive(path, { mode, directory })`)
- Never creates parents: a missing parent rejects with `ENOENT: no such file or directory, <open|mkdir> '<path>'`
  (`open` for a file, `mkdir` for a directory), a parent that is not a directory with `ENOTDIR`.
- An existing entry rejects with `EEXIST: file already exists, <open|mkdir> '<path>'`. Of N concurrent calls
  on one path, exactly one resolves.
- A file is created empty with `mode` exactly (`0o600` stays `0o600`); a directory with `mode`.
- **`OverlayFs`:** read-only rejects with `EROFS: read-only file system, <open|mkdir> '<path>'`. The final
  existence check (memory layer and host) and the insertion happen in the same synchronous step, with no
  `await` between them; the host is never written.
- **`ReadWriteFs`:** runs under the mutation queue (section 5.7). A file is created with one exclusive
  create-or-fail open (`O_CREAT | O_EXCL`, plus `O_NOFOLLOW` unless symlinks are allowed); a directory with
  `mkdir`, then opened without following symlinks. Then the adapter checks that the created entry is still
  the one it made: resolving the path again must give the same canonical path, and that path must still
  name the same inode (device and inode number) as the open descriptor. If not (a parent directory was
  swapped for a symlink during the call), it rejects with
  `EACCES: permission denied, '<path>' resolves outside sandbox during <open|mkdir>` and deletes nothing:
  every available name now points somewhere untrusted, so removing it could delete an attacker's target.

### 5.6 Hard links and copy-on-write (`ReadWriteFs`)
A host file inside `root` may share its inode with a name outside `root` (`nlink > 1`). Changing that inode
in place would change the outside file, so:
- `writeFile` on an existing single-link regular file truncates and writes it in place; on a multiply linked
  file, or a missing one, it writes the new content to a fresh sibling file and renames it over the name.
- `appendFile`, `chmod`, `utimes` on a multiply linked file first copy it to a fresh sibling, apply the
  change there, and rename it over the name. If the bytes to copy exceed a non-zero `maxCopyOnWriteSize`,
  the call rejects with `EFBIG: file too large for copy-on-write <op> '<path>' (<size> bytes, max <max>)`
  (`<op>` is `append`, `chmod` or `utimes`); `writeFile` copies nothing and has no such cap.
- The sibling is created in the same directory with an exclusive create, under a name the sandbox cannot
  predict (`.sentropic-write-` followed by 32 random hex digits), with mode `0o666` (process umask applies)
  for a new file and the old mode for a replaced one (then the requested mode for `chmod`, the requested
  times for `utimes`). Before the rename the adapter checks the sibling is still the regular file it created
  (same device and inode); on any failure the sibling is removed and the target name is left unchanged.
- After the rename the name has a new inode with `nlink == 1`; the outside name keeps its bytes, mode and
  mtime.
- `cp` onto an existing multiply linked destination follows the `writeFile` rule. `cp` of a regular file
  larger than a non-zero `maxCopySize` rejects with
  `EFBIG: file too large to copy '<path>' (<size> bytes, max <max>)`.

### 5.7 Serialized mutations on overlapping roots (`ReadWriteFs`)
All `ReadWriteFs` instances of the process share one queue. Each mutating method (`writeFile`, `appendFile`,
`mkdir`, `createExclusive`, `rm`, `cp`, `mv`, `chmod`, `symlink`, `link`, `utimes`) waits until no mutation
is running on a canonical root that equals, contains or lies inside its own, then runs; mutations on
disjoint roots run concurrently. Waiting calls start in arrival order.

### 5.8 Host errors (`host/node/fs-errors.js`, S7)
- Messages this lot writes itself (all the messages quoted in sections 4-6) already carry only virtual
  paths and are thrown as written.
- Any other error from a host operation is replaced by `Error("<CODE>: <op> '<virtualPath>'")`, where
  `<CODE>` is the error's `code` property as Node reports it (`EIO` when there is none) and `<op>` the
  operation name of the call; the new error carries no `path`, `dest`, `syscall` or host stack. This is
  reference behaviour even when `<CODE>` is a Node code rather than a POSIX one: removing a non-empty
  directory without `recursive` on `ReadWriteFs` gives `ERR_FS_EISDIR: rm '<path>'`.
- As a second line, every message leaving either class passes `sanitizeMessage` (L1b), which removes the
  registered root prefixes and generic host path shapes.

## 6. Behaviour of the `IFileSystem` methods

Messages use the virtual path as given by the caller. "Both" means `OverlayFs` and `ReadWriteFs`. On
`OverlayFs`, every write goes to the memory layer only, removal records a tombstone, and the host is never
modified; with `readOnly: true` every write rejects with `EROFS: read-only file system, <op> '<path>'`
(`<path>` is the destination for `cp`, `mv`, `link`; `<op>` is `write`, `append`, `mkdir`, `rm`, `chmod`,
`utimes`, `cp`, `mv`, `link`, or `open`/`mkdir` for `createExclusive`), except `symlink`, which keeps its
`EPERM` (section 5.3). The synchronous `mkdirSync` / `writeFileSync` always write the memory layer, even
when read-only (they seed the layout, section 7.2).

| Call | Result (both, unless a column is named) |
|---|---|
| `readFile`, `readFileBuffer`, `readFileBytes` on a file over a non-zero `maxFileReadSize` | `EFBIG: file too large, read '<path>' (<size> bytes, max <max>)` |
| same, missing | `ENOENT: no such file or directory, open '<path>'` |
| same, on a directory | `OverlayFs`: `EIO: open '<path>'`; `ReadWriteFs`: `EISDIR: illegal operation on a directory, read '<path>'` |
| `readFileBytes` | the bytes as a one-character-per-byte string (`latin1FromBytes`) |
| `writeFile` / `appendFile` with a missing parent | parents are created (`0o755`; D.3 erratum above) |
| `writeFile` on a directory | `OverlayFs`: succeeds (the memory file shadows the directory); `ReadWriteFs`: `EISDIR: write '<path>'` |
| `OverlayFs` memory growth over `maxMemoryBytes` | `ENOSPC: overlay memory byte limit exceeded (<max> bytes)`, nothing changed |
| `mkdir` existing, without `recursive` | `EEXIST: file already exists, mkdir '<path>'` |
| `mkdir` with a missing parent, without `recursive` | `ENOENT: no such file or directory, mkdir '<path>'` |
| `mkdir` `recursive` on an existing regular file | `OverlayFs`: succeeds; `ReadWriteFs`: `EEXIST: file already exists, mkdir '<path>'` |
| `rm` missing | `ENOENT: no such file or directory, rm '<path>'`; with `force: true` resolves |
| `rm` non-empty directory without `recursive` | `OverlayFs`: `ENOTEMPTY: directory not empty, rm '<path>'`; `ReadWriteFs`: `ERR_FS_EISDIR: rm '<path>'` |
| `rm("/")` | `OverlayFs`: `ENOTEMPTY: directory not empty, rm '/'`; `ReadWriteFs`: `EACCES: permission denied, '/' resolves outside sandbox` |
| `OverlayFs.rm` of host entries | tombstones the path and every host descendant; `exists`, `stat`, `readdir` then report them absent |
| `cp` of a directory without `recursive` | `OverlayFs`: `EISDIR: is a directory, cp '<src>'`; `ReadWriteFs`: `EIO: cp '<src>'` |
| `mv` of a directory into itself or a descendant | `OverlayFs`: `EINVAL: cannot copy '<src>' into itself, '<dest>'`; `ReadWriteFs`: `EINVAL: cannot move '<src>' into itself, '<dest>'` |
| `readdir`, `readdirWithFileTypes` | names without `.` and `..`, sorted by UTF-16 code unit (`B`, `_`, `a`); one host directory listing per call; `OverlayFs` merges memory entries over host entries (memory wins on a name) and drops tombstoned names |
| `link` on a directory | `EPERM: operation not permitted, link '<src>'` |
| `readlink` on a non-symlink | `EINVAL: invalid argument, readlink '<path>'` |
| `realpath` | the normalized virtual path with symlinks resolved; missing: `ENOENT: no such file or directory, realpath '<path>'`; a loop: `ELOOP` |
| `stat`, `lstat` | `{ isFile, isDirectory, isSymbolicLink, mode, size, mtime, dev, ino }`; `ReadWriteFs` adds `identity: "real:<dev>:<ino>"`; an `OverlayFs` memory entry has `identity: "overlay:<n>"` (n counts from 1 per instance) and no `dev`/`ino` |
| `OverlayFs.link` | copies the file's bytes to the new name in memory |
| `OverlayFs.chmod`, `utimes` on a host file | copies it into memory with the new metadata |
| `getAllPaths` (synchronous) | `ReadWriteFs`: every path under the root, found by a synchronous walk, without the root itself; `OverlayFs`: the memory-layer paths minus tombstones, plus host paths found by a synchronous walk that starts at virtual `/`, which reaches the host only when the mount point is `/` (with the default mount point, no host path is listed) |
| `resolvePath(base, path)` | synchronous POSIX resolution of `path` against `base` |

## 7. Kernel `mount-host` and host `fs-batch` bridging

### 7.1 Kernel (`kernel/src/fs.rs`, `kernel/src/fiber.rs`, `kernel/src/bindings/adapt.rs`)

1. **Error codes.** Extend L1b's `fs::Code` (9 values) with the six WIT `fs-error-code` values it lacks:
   `LinkLoop`, `TooLarge`, `Busy`, `CrossDevice`, `ReadOnly`, `Io`, and map all 15 both ways in `adapt.rs`.
2. **Mount table on `Vfs`.** A list of `{ path: KBytes, mount_id: u32 }`:
   - `mount_host(path, mount_id)`: the normalized path (`/` allowed); an empty or relative path is
     `Code::Invalid`, an already mounted path `Code::Exists`.
   - `unmount(path)`: removes that entry, else `Code::NotFound`.
   - `resolve_host_mount(&self, normalized_path: &[u8]) -> Option<(u32, &[u8])>`: the longest mount path
     that equals the path, is `/`, or is followed in the path by `/`; returns its id and the rest of the
     path.
   - A synchronous `vfs.op` on a host-mounted path returns `Code::Unsupported` (host mounts are reached only
     through fibers).
   - Keep this table apart from the in-memory node storage, which L2 changes (section 9).
3. **`HostReq::FsBatch`.** Replace L1b's `HostReq::FsStat` with `HostReq::FsBatch { mount_id: u32, ops }`
   covering the 21 `fs-op` cases (`stat`, `lstat`, `exists`, `read-link`, `real-path`, `read-dir`,
   `read-dir-plus`, `walk`, `all-paths`, `read-file`, `write-file`, `write-lazy`, `create-exclusive`,
   `mkdir`, `remove`, `copy`, `rename`, `symlink`, `hard-link`, `chmod`, `utimes`), and L1b's
   `Outcome::Fs(Option<&'static str>)` with one result per op over the 8 `fs-ok` cases (`none`, `flag`,
   `info`, `path`, `data`, `paths`, `entries`, `entries-plus`) or an `fs-error`.
4. **Capability gate (EVOL D9, S2).** Before emitting `HostReq::FsBatch`, the fiber checks
   `shell.granted(G_HOST_FS)`; without it, the op fails inside the kernel with `Code::Permission` and no
   host call reaches the driver.
5. **Test commands (`kernel/src/fiber/testcmd.rs`, feature `test-commands`).**
   - `__cat <path>` and `__write <path> <text>`: when `shell.root.resolve_host_mount(path)` matches, issue
     `read-file` / `write-file` through `HostReq::FsBatch` (stdout gets the bytes; an error gives exit 1 and
     L1b's stderr line `cat: <path>: <text>\n`, its text table extended with `Permission denied` for
     `permission`), else keep L1b's in-memory behaviour.
   - `__walk <path> <max_depth> <max_entries>`: one `HostReq::FsBatch` with one `walk` op; prints
     `<count>\n`, the number of `walk-entry` records returned.
   - `__fsbatch <mount_id>`: one batch of `mkdir /fb`, `create-exclusive /fb/x`, `write-file /fb/x`,
     `read-dir /fb`, `stat /fb/x`, `remove /fb` (recursive), printing one line per result (`ok <case>` or
     `err <code>`).

### 7.2 Facade and driver (`host/core/facade.js`, `host/core/fs-batch.js`)

1. **Mount registration.** When `new Bash({ fs })` gets an `fs` that is not a kernel-backed `InMemoryFs`
   (L1b's `userFs !== null`): `G_HOST_FS` is granted (as in L1b), the facade keeps
   `B.mounts = new Map([[1, userFs]])`, and the staged shell creation first calls `mountHost(utf8("/"), 1)`
   on the instance's root `vfs`.
2. **Layout in a host `fs`.** The kernel layout (L2 section 5.2) cannot live in the kernel `vfs` when a host
   mount covers `/`. When `userFs` has synchronous `mkdirSync` and `writeFileSync` (as `OverlayFs` has),
   the constructor writes the same layout into it with those two methods, with the same bytes and order:
   `/bin`, `/usr/bin`, and with the default layout `/home/user`, `/tmp`; `/dev`, `/dev/fd`, the five
   `/dev` files; `/proc/self/fd` and the `/proc` files; then the command stubs. A thrown error whose message
   starts with `ENOSYS` is ignored; any other propagates. An `fs` without those two methods (`ReadWriteFs`,
   most custom objects) gets no layout, and commands then run through the registry fallback of L2 section
   7.3, item 7 (no `/usr/bin` in that `fs`).
3. **cwd and `HOME`** follow L2 section 5.1, item 1: the `fs` option does not change them, so
   `new Bash({ fs: overlay })` starts in `/home/user` (the mount point `/home/user/project` is below it).
4. **Stubs.** In L7b the facade writes stubs for the custom command names; the stubs for kernel commands
   iterate L2's `COMMAND_NAMES` and are added when L7b rebases on L2 (section 9).
5. **`serveFsBatch`** (moved from `host/core/driver.js`, which keeps a one-line call):
   - `fs = B.mounts.get(batch.mountId)`; if absent, answer `{ outcome: { tag: "denied" } }`.
   - Run inside L1b's `serveExtension` with `const g = scope.wrap(fs, FS_METHODS)`, so `execSync` with a host
     `fs` keeps throwing `AsyncCapabilityError`, and abort or deadline revokes the scope (S5, S6).
   - Each op maps to `g` (paths decoded from UTF-8; `<s>`, `<t>` = source, target):

     | `fs-op` | Call | `fs-ok` |
     |---|---|---|
     | `stat` / `lstat` | `g.stat(s)` / `g.lstat(s)` | `info` |
     | `exists` | `g.exists(s)` | `flag` |
     | `read-link` / `real-path` | `g.readlink(s)` / `g.realpath(s)` | `path` |
     | `read-dir` | `g.readdirWithFileTypes(s)` when present, else `g.readdir(s)` + `g.lstat` per entry | `entries` |
     | `read-dir-plus` | same listing + `g.lstat` per entry, all within this one host turn | `entries-plus` |
     | `walk` | section 7.3 | `entries-plus` |
     | `all-paths` | `g.getAllPaths()` | `paths` |
     | `read-file { offset, len }` | `g.readFileBuffer(s)` sliced to `[offset, offset + len)`, at most `floor(headroom / 2)` bytes | `data` |
     | `write-file` | no offset: `g.appendFile`; offset 0 with `truncate`: `g.writeFile`; else read, patch, `g.writeFile`; then `g.chmod` / `g.utimes` when `mode` / `mtime-ms` is given | `none` |
     | `write-lazy` | none: `err(unsupported)` | — |
     | `create-exclusive` | `g.createExclusive` when present, else `exists` check + `mkdir`/`writeFile` + `chmod` | `none` |
     | `mkdir` | `g.mkdir(s, { recursive })`, then `g.chmod` when `mode` is given | `none` |
     | `remove` | `g.rm(s, { recursive, force })` | `none` |
     | `copy` / `rename` | `g.cp(s, t, { recursive })` / `g.mv(s, t)` | `none` |
     | `symlink` / `hard-link` | `g.symlink(s, t)` / `g.link(s, t)` | `none` |
     | `chmod` | `g.chmod(s, mode)` | `none` |
     | `utimes` | `g.utimes(s, new Date(Number(atimeMs)), new Date(Number(mtimeMs)))` | `none` |

   - **Error mapping:** from the leading `CODE:` of the message (or `err.code`): `ENOENT` not-found,
     `EEXIST` exists, `ENOTDIR` not-directory, `EISDIR` and `ERR_FS_EISDIR` is-directory, `ENOTEMPTY`
     not-empty, `EACCES`/`EPERM` permission, `EINVAL`/`ENAMETOOLONG` invalid, `ELOOP` link-loop, `EFBIG`
     too-large, `ENOSPC` no-space, `EBUSY` busy, `EXDEV` cross-device, `EROFS` read-only,
     `ENOSYS`/`ENOTSUP` unsupported, anything else io. The `path` of the `fs-error` is the virtual path; any
     text that reaches stderr passes `sanitizeMessage`.

### 7.3 Directory-batched `walk` (EVOL D10: at most one host round trip per directory)
`walk { root, max-depth, max-entries }` is served entirely inside **one** `fs-batch` turn: from `root`,
breadth first, one `g.readdirWithFileTypes` (one host directory listing) per directory, plus the `lstat`
needed for each entry's `stat` record, bounded by `min(max-depth, maxTraversalDepth)` and
`min(max-entries, maxTraversalEntries)`, each listed entry counting one unit against `maxTraversalWork`, and
the response capped at `floor(headroom / 2)` bytes; reaching a bound ends the walk with what was collected.
`OverlayFs` and `ReadWriteFs` may expose an internal helper that lists a directory with its entries'
`lstat` under the already validated canonical directory, so that a walk does not re-validate every entry;
the bound stays one directory listing per directory.

## 8. Tests and executable exit gates

`make check` runs `node:test` suites covering every gate below; fixtures live under
`<worktree>/tmp/test-host-fs-*` and are removed in `finally`.

1. **G1-G8 and browser isolation.** `make check` passes G1-G8; `import("@sentropic/bash/browser")` exports
   neither `OverlayFs` nor `ReadWriteFs` and its import graph reaches no `node:` module (G2);
   `tsc --noEmit` passes on a consumer fixture importing `OverlayFs`, `OverlayFsOptions`, `ReadWriteFs`,
   `ReadWriteFsOptions` and `IFileSystem` from `@sentropic/bash`.
2. **EVOL D26 sentinels.**
   - `OverlayFs`: a host file `sentinel.txt` holding `"SENTINEL-" + randomUUID()`; with
     `bash = new Bash({ fs: new OverlayFs({ root: dir }) })`, both `bash.readFile("/home/user/project/sentinel.txt")`
     and `__cat /home/user/project/sentinel.txt` (on `kernel-test`) return the token exactly. Then
     `bash.writeFile`, `__write`, `appendFile`, `chmod`, `rm` through the overlay change what the sandbox
     sees, and afterwards the host file has the original bytes, size, mtime and inode.
   - `ReadWriteFs`: a second sentinel is read through `bash2.readFile("/sentinel2.txt")` and
     `__cat /sentinel2.txt`; `bash2.writeFile("/written.txt", "W")` creates `<dir2>/written.txt` with `W`.
   - Custom `IFileSystem`: a plain object with the 20 required methods over a `Map` holding a third token:
     `bash3.readFile` and `__cat` read it, and `bash3.execSync("__cat /t")` throws `AsyncCapabilityError`.
   - A default `new Bash()` cannot read any of the three host files (L1b gate kept).
3. **Traversal, NUL and symlinks (D10).**
   - `../..` traversal, `%2e%2e` (a literal name, not decoded), `"a\0b"` (`exists` false; every other
     method rejects with the NUL message of section 5.1), names `__proto__` and `constructor`.
   - Host symlinks inside `root`: one to an outside file `<worktree>/tmp/outside-secret.txt`, one directory
     symlink to an outside directory, one dangling link to an outside path, one relative link to a file
     inside `root`. With `allowSymlinks: false`, on both classes: `readFile`, `stat`, `readdir` through the
     links, `writeFile`, `appendFile`, `mkdir`, `createExclusive` through the directory link all give the
     section 5.3 result (`ReadWriteFs` EACCES `resolves outside sandbox`, also for the inside link;
     `OverlayFs` ENOENT for `readFile`/`stat`, an empty `readdir`, memory-only success for the writes);
     `lstat` reports each link as a
     symlink; `readlink` gives `outside-secret.txt`, the relative target, and the last component of the
     dangling target; `symlink()` rejects with EPERM; the outside file is never read, created or changed.
   - With `allowSymlinks: true`: the inside link reads its target; the outside and dangling-outside links
     still reject; `a -> b -> a` rejects with `ELOOP`; `ReadWriteFs.mv` of a relative link to a place where
     it would escape rejects with the section 5.3 message and moves nothing, and `mv` of a directory
     holding such a link is undone.
4. **Special files (D10).** A FIFO made with `mkfifo` (`/usr/bin/mkfifo`; the test fails, not skips, if it is
   missing on Linux) inside `root`: every row of the section 5.4 table gives its result on both classes, each
   call settling in under 500 ms, and `stat` reports `isFile: false`.
5. **Exclusive create and hard links (D10).**
   - 20 concurrent `createExclusive("/lock", { mode: 0o600 })` on `OverlayFs` and on `ReadWriteFs`: exactly 1
     resolves, 19 reject with `EEXIST: file already exists, open '/lock'`, and the mode is `0o600`.
   - `createExclusive("/nodir/x", ...)` rejects with `ENOENT: no such file or directory, open '/nodir/x'` on
     both.
   - Revalidation: a test hook between the create and the identity check swaps the parent directory for a
     symlink to an outside directory; the call rejects with the section 5.5 message and nothing is deleted.
   - Hard links: `<worktree>/tmp/outside-hardlink.txt` (`"ORIGINAL"`) linked to `<root>/linked.txt`
     (`nlink === 2`); `appendFile`, `chmod`, `utimes`, `writeFile("/linked.txt", "CHANGED")` on `ReadWriteFs`
     each succeed and leave the outside file with `"ORIGINAL"`, its mode and its mtime; `linked.txt` ends
     with `nlink === 1`. With `maxCopyOnWriteSize: 16`, `appendFile` on a 19-byte hard-linked file rejects
     with `EFBIG: file too large for copy-on-write append '/l2.txt' (19 bytes, max 16)`, while `writeFile`
     on it succeeds.
6. **Overlapping roots.** Two `ReadWriteFs` on `root` and `join(root, "sub")` and a third on a disjoint
   directory; a test hook records start and end times of each mutation; 20 parallel `writeFile`/`mv` calls
   show no overlap between the first two instances and at least one overlap between an overlapping-root
   instance and the disjoint one.
7. **Host paths in errors (S7).** Trigger `ENOENT`, `EACCES`, `EEXIST`, `EISDIR`, `ENOTDIR`, `ENOTEMPTY`,
   `ELOOP`, `EFBIG`, `EROFS`, `ERR_FS_EISDIR` and both constructor errors: no `Error.message`, no `stack`
   line, and no `exec` stderr contains the absolute fixture directory, the worktree path, or `os.homedir()`.
8. **Directory batching (D10).** A host tree of 1 000 files in 10 subdirectories of 100 files. On
   `kernel-test` with `hooks.onHostCall` counting calls, `__walk /home/user/project 10 2000` on `OverlayFs`
   and `__walk / 10 2000` on `ReadWriteFs` print a count `>= 1010`, make exactly 1 `fs-batch` host call, and
   make at most 11 host directory listings (counted by a spy on the adapters' listing helper).
9. **Capability and membrane (S2, S6).** A kernel-test shell without `G_HOST_FS` on a host-mounted `vfs`
   fails `__cat` with `permission` and `hooks.onHostCall` sees 0 calls. An `exec` against a custom `fs` whose
   `readFileBuffer` resolves after 2 000 ms, aborted at 20 ms, resolves with exit 124, and the late
   continuation cannot call any wrapped method (`ExecutionAbortedError`).
10. **Tarball.** `EXPECTED` in `test/pack.test.mjs` gains `host/core/fs-batch.js`, `host/node/overlay-fs.js`,
    `host/node/read-write-fs.js`, `host/node/host-paths.js`, `host/node/fs-errors.js`; the offline consumer
    `import { OverlayFs, ReadWriteFs } from "@sentropic/bash"` works.

## 9. Handover notes, merge with L2, and points to confirm

Record in `BRANCH.md`:

1. **Merge with L2 (W3).** L2 merges first; this lot then rebases on `main` and resolves every conflict in
   its own PR, following the ownership table of L2 section 3.1 (`kernel/src/fs.rs`, `kernel/src/fiber.rs`,
   `kernel/src/fiber/testcmd.rs`, `kernel/src/shell.rs`, `kernel/src/bindings/adapt.rs`,
   `host/core/facade.js`, `host/core/driver.js`, `host/index.js`, `types/index.d.ts`, `test/pack.test.mjs`).
   At that rebase: (a) the stubs of section 7.2, item 4 iterate `COMMAND_NAMES`; (b) `__cat`/`__write` are
   dispatched by L2's VM instead of L1b's bootstrap interpreter; (c) gates 2 and 8 run again on the merged
   tree. Keep the mount table separate from node storage and `serveFsBatch` in its own module so that the
   `driver.js` diff stays one line.
2. **Node 22 on Linux:** the `errno` seen for `O_NOFOLLOW` on a symlink (`ELOOP`) and for the directory
   open of `createExclusive` (`O_RDONLY | O_DIRECTORY | O_NOFOLLOW`), and that both map as specified.
3. **Walk numbers:** `fs-batch` count, directory listing count and wall time of gate 8 for both classes.
4. **`upstream-defect` for the owner listing (EVOL Annex E.7):** the reference hangs on `readFile` of a host
   FIFO (both classes) and on `OverlayFs.appendFile` of one; this lot rejects per D10/S7 (section 5.4).
