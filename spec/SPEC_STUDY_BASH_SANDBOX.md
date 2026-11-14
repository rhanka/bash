# SPEC STUDY — @sentropic/bash sandbox (rung 1: study)

Status: study only. No decision is taken here; every choice marked **OWNER** is listed in section 6.
Scope: clean-room, MIT, zero third-party runtime dependency, WASM-first reimplementation with functional
parity against `vercel-labs/just-bash`, benchmarked against just-bash and MoonBash on this machine.

Conventions used below:
- `JB:` = path under `tmp/refs/just-bash/packages/just-bash/` (reference clone, git-ignored).
- `MB:` = path under `tmp/refs/MoonBash/` (reference clone, git-ignored).
- "measured" = measured on this machine during this study (AMD Ryzen AI Max+ 395, 32 threads, Linux 7.0,
  node v22.22.1, wabt 1.0.36). "extracted" = read from the reference sources. "estimate" = unverified.
- Measurement scripts are throwaway files under the git-ignored `tmp/bench/` of the worktree; they are not
  part of the repo. The real harness is specified in section 5.

---

## 1. Reference inventory

### 1.1 just-bash at a glance (extracted, v3.6.0)

| Item | Value | Source |
|---|---|---|
| License | Apache-2.0 (inbound is not MIT: nothing is copied or vendored) | `JB:package.json`, `JB:LICENSE` |
| Language / runtime | TypeScript, pure JS at runtime, Node >= 20.19 and browser bundle | `JB:package.json` |
| Runtime dependencies | 16 `dependencies` + 2 `optionalDependencies` (diff, fast-xml-parser, file-type, guarded-fetch, ini, modern-tar, papaparse, re2js, run, seek-bzip, smol-toml, sprintf-js, sql.js, turndown, typescript, yaml; zstd, liblzma) | `JB:package.json` |
| Source size | 1106 `.ts` files, 700 test files | `JB:src/` |
| Command sources | 82 command directories, about 67.6k non-test lines | `JB:src/commands/` |
| Interpreter | 97 files, about 34.4k lines (+73 test files) | `JB:src/interpreter/` |
| Parser + AST | 11 files / 9.5k lines + 1.1k lines of AST types | `JB:src/parser/`, `JB:src/ast/types.ts` |
| Filesystems | 17 files / 6.9k lines (+36 test files) | `JB:src/fs/` |
| Security layer | 27 files / 10.0k lines (+69 test files) | `JB:src/security/` |
| Network | 6 files / 1.75k lines | `JB:src/network/` |
| Build | `tsc` then esbuild (ESM split bundle, CJS bundle, browser bundle, CLI, shell, workers) | `JB:package.json` scripts |

### 1.2 Public API surface (extracted from `JB:src/index.ts`, `JB:src/Bash.ts`, `JB:src/types.ts`)

| Area | Symbols | Notes |
|---|---|---|
| Entry class | `Bash` | `new Bash(options)`; methods `exec(cmd, opts)`, `readFile`, `writeFile`, `getCwd`, `getEnv`, `registerCommand`, `registerTransformPlugin`, `transform` (`JB:src/Bash.ts:561-1066`) |
| `BashOptions` | `files`, `env`, `cwd`, `fs`, `executionLimits`, `executionLimitProfile`, `maxCallDepth`, `maxCommandCount`, `maxLoopIterations`, `fetch`, `network`, `python`, `javascript`, `commands`, `sleep`, `customCommands`, `logger`, `trace`, `defenseInDepth`, `coverage`, `processInfo {pid, ppid, uid, gid}` | `JB:src/Bash.ts:144` |
| `ExecOptions` | `env`, `replaceEnv`, `cwd`, `rawScript`, `stdin`, `stdinKind ("text"/"bytes")`, `signal` (AbortSignal), `args` | `JB:src/Bash.ts` |
| Result | `BashExecResult { stdout, stderr, exitCode, env, metadata?, stdoutKind? }` | `JB:src/types.ts:15-67` |
| State model | Filesystem persists across `exec` calls; shell state (env, cwd, functions) is isolated per `exec` | `JB:src/Bash.ts`, `JB:src/execution-scope.ts` |
| Filesystems | `InMemoryFs`, `OverlayFs` (host read, memory write), `ReadWriteFs` (host read/write under a root), `MountableFs` (mount table) | `JB:src/fs/*/` |
| FS contract | `IFileSystem`: 22 methods, **all I/O methods return Promises** (`readFile`, `readFileBuffer`, `writeFile`, `appendFile`, `exists`, `stat`, `lstat`, `mkdir`, `readdir`, `rm`, `cp`, `mv`, `chmod`, `symlink`, `link`, `readlink`, `realpath`, `utimes`, ...); `resolvePath` and `getAllPaths` are sync; lazy files via `LazyFileProvider` | `JB:src/fs/interface.ts:137-331` |
| Custom commands | `defineCommand(name, execute, {trusted})`, `LazyCommand { name, load }`, `createCommandContext`; context exposes `fs`, `cwd`, `env`, `stdin` (opaque `ByteString`), `limits`, `exec`, `fetch`, `sleep`, `signal` | `JB:src/custom-commands.ts`, `JB:src/types.ts:157-331` |
| Byte/text helpers | `ByteString`, `bytesOutput`, `textOutput`, `encodeUtf8ToBytes`, `decodeBytesToUtf8`, `latin1FromBytes`, ... | `JB:src/encoding.ts` |
| Network | `NetworkConfig { allowedUrlPrefixes, allowedMethods, dangerouslyAllowFullInternetAccess, maxRedirects, timeoutMs, maxResponseSize, denyPrivateRanges }`, request transforms (header injection per allowed URL), typed errors | `JB:src/network/types.ts:56-226` |
| Limits | `ExecutionLimits`: about 45 fields (source bytes, call depth, command count, loop iterations, awk/sed/jq iterations, parser tokens, traversal entries, live bytes, input bytes, FS bytes, archive bytes, wall time, string length, array elements, heredoc size, brace expansion, output size, ...) with profiles | `JB:src/limits.ts` |
| Parser / transform | `parse()`, AST node types, `BashTransformPipeline`, `TeePlugin`, `CommandCollectorPlugin`, `serialize()` | `JB:src/parser/parser.ts`, `JB:src/transform/` |
| Sandbox facade | `Sandbox`, `SandboxCommand` (Vercel Sandbox API shape: `runCommand`, `writeFiles`, ...) | `JB:src/sandbox/` |
| Security exports | `DefenseInDepthBox`, `SecurityViolationError`, `SecurityViolationLogger` | `JB:src/security/index.ts` |
| CLI | `just-bash`, `just-bash-shell` binaries | `JB:src/cli/` |

### 1.3 Command inventory and flag-coverage estimate

Registry: `JB:src/commands/registry.ts` (680 lines). 97 always-available names (`CommandName`), plus opt-in
`curl` (network), `python3`/`python`, `js-exec`/`node`. Every command is a lazy `import()` chunk.
Shell builtins live in the interpreter (`JB:src/interpreter/builtins/`): `cd`, `export`, `local`,
`declare`/`typeset`, `read`, `mapfile`/`readarray`, `set`, `shopt`, `unset`, `eval`, `source`/`.`, `exit`,
`return`, `break`, `continue`, `shift`, `getopts`, `let`, `hash`, `dirs`/`pushd`/`popd`, `compgen`,
`complete`, `compopt`, `help`, `type`, `test`/`[`.

Columns: "LOC" = non-test lines in the command directory; "flags" = short/long options declared in the
command's fuzz metadata (`flag: "-x"`, a lower bound of what is parsed); "tests" = `it`/`test` call sites in
the command directory; "coverage" = estimate of option coverage against the GNU/upstream tool
(H >= 70%, M 30-70%, L < 30%). The coverage column is an **estimate**, to be replaced by a per-flag audit
in the EVOL rung.

| Command(s) | LOC | flags | tests | coverage (estimate) | Notes |
|---|---:|---:|---:|---|---|
| echo, printf | 242 / 2031 | 3 / 1 | 22 / 78 | H / H | printf has `%q`, `-v`, own format engine |
| cat, tac, rev, nl | 303 / 73 / 122 / 388 | 7 / 0 / 0 / 6 | 60 / 7 / 19 / 32 | H / M / H / M | |
| head, tail | 357 / 64 | 4 / 4 | 20 / 24 | M / M | `-c -n -q -v`; no `tail -f` |
| wc, cut, paste, tr | 237 / 254 / 221 / 374 | 4 / 4 / 2 / 4 | 20 / 17 / 24 / 31 | H / M / M / M | |
| sort, uniq, comm, join | 736 / 143 / 191 / 472 | 14 / 4 / 3 / 8 | 57 / 15 / 16 / 26 | H / M / H / M | sort: `-b -c -d -f -h -k -M -n -o -r -s -t -u -V` |
| fold, expand, unexpand, column, split, strings, od | 295 / 636 / - / 361 / 711 / 382 / 214 | 3 / 3 / - / 5 / 5 / 4 / 4 | 26 / 53 / - / 25 / 42 / 43 / 14 | M | |
| grep, egrep, fgrep | 1162 | 21 | 330 | H | `-A -B -C -c -e -E -f -F -h -i -l -L -m -n -o -P -q -r -v -w -x` |
| rg | 3213 | 21 | 553 | M | large option surface upstream |
| sed | 4066 | 5 | 240 | H | `-e -E -i -n -r`; full command set incl. branches, hold space |
| awk | 6684 | 2 | 664 | H | own lexer/parser/AST interpreter; `-F -v` |
| jq, yq | 671 + 7929 (query-engine) / 1031 | 9 / 5 | 323 / 223 | H / M | shared query engine; yq also reads XML/TOML/INI/CSV via third-party parsers |
| xan | 6587 | 0 | 244 | M | CSV toolkit; depends on papaparse |
| find | 2479 | 8 | 217 | M | `-name -iname -type -maxdepth -mindepth -empty -print -print0` + expression tree, `-exec` |
| ls, tree, du, stat, file | 801 / 335 / 211 / 129 / 513 | 11 / 4 / 5 / 2 / 3 | 74 / 11 / 9 / 13 / 30 | M / M / M / L / M | ls: `-1 -a -A -d -F -h -l -r -R -S -t` |
| cp, mv, rm, mkdir, rmdir, touch, ln, chmod, readlink, mktemp | 227 / 215 / 95 / 78 / 226 / 202 / 163 / 296 / 136 / 450 | 5 / 3 / 4 / 2 / 2 / 4 / 4 / 2 / 2 / 6 | 15 / 21 / 15 / 11 / 1 / 8 / 20 / 13 / 14 / 58 | M | |
| basename, dirname, pwd, which, env, printenv, hostname, whoami | 30-200 each | 0-2 | 2-14 | H | trivial |
| true, false, yes, sleep, seq, expr, clear, help, history, alias, unalias, time, timeout | 30-460 each | 0-4 | 2-26 | M-H | `sleep` is virtual (host `sleep` hook) |
| xargs, tee | 456 / 84 | 6 / 1 | 35 / 9 | M / M | xargs: `-0 -d -I -n -r -t` |
| date | 234 | 4 | 70 | L | `-d -I -R -u` |
| diff | 553 | 6 | 93 | M | relies on the `diff` npm package |
| base64, md5sum, sha1sum, sha256sum | 312 / 303 | 2 / 1 | 35 / 31 | H | |
| gzip, gunzip, zcat, tar | 1108 / 3348 | 12 / 10 | 48 / 183 | M / M | tar relies on modern-tar; bzip2/xz/zstd via optional native deps |
| bash, sh | 208 | 1 | 52 | M | nested interpreter |
| html-to-markdown | 259 | 0 | 33 | n/a | wraps turndown (non-POSIX) |
| curl (opt-in) | 1180 | about 29 parsed options | 240 | M | goes through the allow-listed fetch |
| sqlite3 | 1933 | - | 167 | n/a | sql.js (WASM) in a worker |
| python3 / python (opt-in) | 2309 | - | 229 | n/a | CPython emscripten build in a worker (`JB:vendor/cpython-emscripten/`) |
| js-exec / node (opt-in) | 2967 | - | 317 | n/a | QuickJS-based runtime in a worker |

Totals (extracted by script): 82 command directories, 67 566 non-test lines, 5 682 test call sites.

### 1.4 Grammar and expansion coverage (what the spec tests cover)

| Corpus | Size | Source | Use for us |
|---|---|---|---|
| Bash spec cases | 137 files, 46.5k lines, 2 728 `####` cases | `JB:src/spec-tests/bash/cases/` (imported from the Oils project, Apache-2.0: `LICENSE-APACHE-2.0.txt` in that directory) | Intent only. Not vendored. |
| AWK spec cases | 317 files, 77.6k lines | `JB:src/spec-tests/awk/cases/` (onetrue-awk style) | Intent only |
| grep / sed / jq spec cases | 7 / 5 / 8 files; 2.1k / 4.2k / 4.1k lines | `JB:src/spec-tests/{grep,sed,jq}/cases/` | Intent only |
| Comparison tests | 62 test files, 57 recorded fixtures (recorded against real bash) | `JB:src/comparison-tests/` | Same method, our own cases |
| Known gaps | 484-line list | `JB:src/spec-tests/bash/KNOWN_LIMITATIONS.md` | Scope input for v1 |

Feature areas named by the bash spec files (one area per file, extracted from file names): alias; append;
arithmetic (`arith`, `arith-context`, `arith-dynamic`, `dparen`, `let`, `for-expr`); arrays (indexed,
associative, sparse, literal, compat); assignment (`assign`, `assign-extended`, `assign-deferred`, temp
bindings); brace expansion; builtins (`cd`, `dirs`, `echo`, `eval`/`source`, `getopts`, `printf`, `read`,
`set`, `trap`, `type`, `vars`, `special`, `meta`, `misc`, `history`, `fc`, `bind`, `completion`, `kill`,
`times`); `case`; command parsing and command substitution; comments; `[[ ]]` (`dbracket`) and `[`;
errexit and strict options; exit status; extglob (match and files); functions (`func-parsing`, `sh-func`);
glob, globstar, globignore; here-docs; `if`; loops; nameref; nocasematch; NUL bytes; parse errors;
pipelines and `PIPESTATUS`; process substitution; quoting; redirections (order, multi, command, numeric
fds); regex (`=~`); `set`/`shopt` options; subshells; tilde; unicode; variable operators (`var-op-*`:
test, strip, patsub, slice, length), `var-ref`, `var-sub`, special vars; word evaluation and word
splitting; whitespace; xtrace. Interactive areas (`prompt`, `interactive`, `background`, job control) are
present in the corpus but mostly skipped by just-bash.

Declared just-bash limitations relevant to parity (`JB:src/spec-tests/bash/KNOWN_LIMITATIONS.md`,
`JB:THREAT_MODEL.md` section 3.2): no real job control or signals; arithmetic is not 64-bit (clamped to
`MAX_SAFE_INTEGER`); `errexit` in compound commands pending; `read -N`, some IFS and here-doc edge cases
pending; extglob and history low priority.

### 1.5 Security model of the references

just-bash (`tmp/refs/just-bash/THREAT_MODEL.md`, 400 lines):

| Layer | Mechanism | Files |
|---|---|---|
| Architecture | No `child_process` import; only registered JS commands run | `JB:src/commands/registry.ts` |
| Filesystem | Virtual FS by default; `OverlayFs`/`ReadWriteFs` gate every path through root containment, null-byte rejection, symlinks denied by default, hard-link copy-on-write, error sanitization | `JB:src/fs/real-fs-utils.ts`, `JB:src/fs/overlay-fs/`, `JB:src/fs/sanitize-error.ts` |
| Network | Off by default; URL-prefix allow-list, method allow-list, each redirect re-validated, response size cap, private-range denial | `JB:src/network/` |
| Parser limits | `MAX_INPUT_SIZE` 1 MB, `MAX_TOKENS` 100k, `MAX_PARSER_DEPTH` 200, `MAX_PARSE_ITERATIONS` 1M | `JB:src/parser/types.ts:10-13` |
| Execution limits | About 45 counters (section 1.2), checked at every loop, call and expansion | `JB:src/limits.ts` |
| ReDoS | User regexes go through `re2js` (linear time) | `JB:src/regex/` |
| Prototype pollution | Null-prototype maps and guards, enforced by a banned-pattern linter | `JB:src/security/prototype-pollution/`, `JB:scripts/check-banned-patterns.js` |
| Defense in depth | `DefenseInDepthBox`: AsyncLocalStorage-scoped monkey-patching that blocks `Function`, `eval`, `setTimeout`, `process.*`, `WebAssembly`, `Proxy`, module loading while sandboxed code runs | `JB:src/security/defense-in-depth-box.ts`, `JB:src/security/blocked-globals.ts` |
| Info disclosure | Virtual pid/uid, host paths stripped from errors | `JB:src/Bash.ts` |

Residual risks stated by the reference (threat model section 4): dynamic `import()` defence depends on the
runtime; pre-captured references bypass the global patches; `globalThis` reassignment; signal/job control
not modelled; python surface when enabled. The whole "code execution escape" class exists because the
interpreter and the untrusted data share one JS heap.

MoonBash (`MB:docs/SECURITY.md`): same four layers (architecture, parser limits, execution limits with
lower defaults of 10 000 commands/iterations, defense-in-depth globals), a VM-based linear-time regex
(`moonbitlang/regexp`) **with a fallback to JS `RegExp` under a timeout wrapper** for backreferences and
lookahead, and the same network allow-list. Its test snapshot (`MB:docs/TEST_STATUS_2026-02-19.md`) shows
partial parity: grep spec 227 pass / 54 fail, jq 598 / 170, bash spec chunks mostly skipped or failing, one
security attack file failing.

Takeaway for the design: a WASM kernel removes the shared-heap class (prototype pollution, constructor
chains, global patching) for everything that runs inside linear memory. The remaining JS surface is the
host glue, which must stay small enough to audit line by line.

### 1.6 Test suite structure of just-bash

| Suite | Runner | Content |
|---|---|---|
| Unit / functional | vitest (`test:run`, `test:unit`) | co-located `*.test.ts`: 73 interpreter, 36 fs, 12 network, about 330 command files |
| Spec | vitest (`src/spec-tests/*/…spec.test.ts`) | parses upstream `.test.sh`/awk/sed/jq case files, per-suite `skips.ts` |
| Comparison | vitest (`test:comparison`, `RECORD_FIXTURES=1` to record) | runs the same script in real bash and replays recorded fixtures |
| Security | vitest | `attacks/`, `sandbox/`, `limits/`, `prototype-pollution/`, `*.security.test.ts` |
| Fuzz | vitest + fast-check (`test:fuzz`, `FUZZ_RUNS`) | grammar-based generators, oracles, corpus, coverage (`JB:src/security/fuzzing/`) |
| Dist | vitest (`test:dist`) | bundle and package-content checks |
| Syntax | 19 files | parser-level cases (`JB:src/syntax/`) |

Our equivalent uses `node:test` + `node:assert` only, with our own cases (section 4.4 and lot L10).

### 1.7 Startup path and size of just-bash (measured, published package 3.6.0)

Installed for measurement only under the git-ignored `tmp/bench/` (`npm install --ignore-scripts`).

| Metric | Value | Method |
|---|---:|---|
| npm package | 21.6 MB unpacked, 970 files | `npm view just-bash dist` |
| Install footprint | 102 MB `node_modules`, 73 packages | `du -sh` |
| ESM entry `dist/bundle/index.js` | 504 KB | file size |
| ESM entry + 199 lazy chunks | 2.02 MB raw, 557 KB gzip -9 | `cat … | wc -c` |
| CJS bundle / browser bundle | 1.7 MB / 1.2 MB | file size |
| `import('just-bash')` | 60.5 ms median (p95 69.5) | 30 fresh processes |
| `new Bash()` (first) | 6.2 ms median | same |
| first `exec('echo hi')` | 44.1 ms median (p95 53.2) | same |
| import + ctor + first exec | **111 ms median** (p95 128) | same |
| Process wall time | 179 ms vs 54 ms for bare `node -e 0` (delta 125 ms) | same |
| RSS after first exec | 93.5 MB vs 58.0 MB bare node (delta 35.5 MB); heap used 16.7 MB vs 3.9 MB | `process.memoryUsage()` |

What is loaded when (extracted): `JB:src/Bash.ts` statically imports the parser, the interpreter, the
in-memory and mountable filesystems, the limits, the security box and the transform serializer; commands
are lazy chunks resolved by `import()` on first use (`JB:src/commands/registry.ts`), which is why the first
`exec` costs 44 ms even for `echo`. Heavy optional runtimes (python, sqlite3, js-exec) live in workers.

Warm costs, same process, median of 50-200 runs after warm-up (measured):

| Script | just-bash 3.6.0 |
|---|---:|
| `:` | 0.17 ms |
| `echo hi` | 0.35 ms |
| `new Bash()` (warm) | 0.37 ms |
| `for i in $(seq 1 1000); do :; done` | 7.4 ms |
| `wc -l` on 10k lines (about 250 KB) | 0.74 ms |
| `grep -c` fixed pattern on 10k lines | 11.0 ms |
| `sort -r … | head -1` on 10k lines | 9.2 ms |
| `awk '{s+=$2} END{print s}'` on 10k lines | 61.0 ms |
| `sed -n 's/foo/baz/p' … | wc -l` on 10k lines | 73.7 ms |

### 1.8 How MoonBash compiles and bundles (extracted)

| Item | Value | Source |
|---|---|---|
| Language | MoonBit, 98 `.mbt` files, 47.8k lines | `MB:src/lib/` |
| Target | `moon build --target js --release`: **pure JavaScript, not WASM** (`"supported-targets": "js"`) | `MB:src/moon.mod.json`, `MB:package.json` |
| Architecture | "fat kernel, thin shell": lexer, parser, interpreter, 87 commands, awk/sed/jq VMs and the in-memory FS in MoonBit; TS wrapper for the just-bash-compatible API; FFI reduced to fetch, timers, heavy VMs, disk adapter | `MB:docs/ARCHITECTURE.md` section 2 |
| Bundling | `vp pack` (Vite+/Rolldown) over `src/wrapper/*.ts` -> `dist/index.mjs` | `MB:docs/ARCHITECTURE.md` section 6 |
| Declared size | 4.56 MB readable, 1.65 MB minified, 434 KB gzip | `MB:docs/ARCHITECTURE.md` section 6.1 |
| Dependencies | 17 MoonBit registry packages (regexp, md5, sha, gzip, flate, yaml, jq, glob, ...) plus npm `pyodide`, `sql.js` | `MB:src/moon.mod.json`, `MB:package.json` |
| Availability | `moon-bash` is **not published on npm** (registry 404 on 2026-11-14); a bench needs a source build with the `moon` toolchain, which is not installed here | `npm view moon-bash` |

MoonBash's stated reasons for JS over WASM (`MB:docs/ARCHITECTURE.md` section 6.1): string copies at the
boundary, poor gzip ratio of `.wasm`, and "async instantiate prevents synchronous cold start". Section 2.1
tests these claims on this machine: the third one does not hold for Node (synchronous
`new WebAssembly.Module` costs about 2.5 ms per MB), and the first one is bounded by a design where the
data stays inside linear memory.

---

## 2. Architecture options for a WASM-first engine

### 2.1 Cost primitives measured on this machine

Synthetic modules generated with `wat2wasm`, 25 fresh node processes each, median:

| Primitive | Value |
|---|---:|
| `new WebAssembly.Module` (sync), 0.9 KB | 0.18 ms |
| `new WebAssembly.Module` (sync), 394 KB / 5 000 functions | 1.11 ms |
| `new WebAssembly.Module` (sync), 956 KB / 12 000 functions | 2.48 ms |
| `new WebAssembly.Instance` (any of the above) | 0.06 ms |
| First exported call | 0.06 ms |
| JS -> WASM call (i32 args) | 1.7 ns |
| JS -> WASM -> JS import -> return | 5.3 ns |
| `TextEncoder.encodeInto`, 80-byte string into linear memory | 39 ns |
| `TextDecoder.decode`, 80 bytes out of linear memory | 56 ns |
| Copy 60 KB into linear memory (`Uint8Array.set`) | 0.57 us |
| Decode 60 KB of UTF-8 out of linear memory | 2.8 us |

Reading: module creation is dominated by validation (V8 compiles function bodies lazily with its baseline
tier, so part of the compile cost moves to the first call of each function; the benchmark harness must
measure the first `exec`, not only instantiation). Instantiation is negligible, so a **fresh instance per
`Bash` object, or even per `exec`, is affordable** (0.06 ms) and is a strong isolation primitive. Boundary
calls cost nanoseconds; what costs is string transcoding, about 0.5-0.7 ns per byte, so the design rule is
"cross the boundary per `exec` and per host capability call, never per line or per character".

Structural constraint from the just-bash API: `IFileSystem` is asynchronous and custom commands are async
JS functions. A WASM engine cannot `await`. Three ways to bridge:
1. JSPI (WebAssembly JS Promise Integration): behind a flag in node 22, not usable as a baseline.
2. Asyncify-style code transformation: needs binaryen (third-party tool), inflates the binary.
3. **Resumable engine**: the interpreter is an explicit-stack VM whose `run(fuel)` export returns a status
   (`DONE`, `OUT_OF_FUEL`, `HOST_CALL`); on `HOST_CALL` the JS host performs the async operation, writes the
   result into linear memory and calls `resume()`. No toolchain support needed, and the same mechanism
   provides fuel metering, cooperative time slicing, `AbortSignal` support and immunity to native stack
   overflow.
Option 3 is assumed by every option below. With the default in-memory VFS living inside linear memory, a
script that touches no host capability completes in a single boundary crossing, which also enables a
synchronous `execSync` (not offered by just-bash).

### 2.2 Options

**(a) Rust -> `wasm32-unknown-unknown`, zero crates.** Engine written in Rust using only `core`/`alloc`/
`std`, no `wasm-bindgen`, no `wasm-pack`, hand-written `extern "C"` import/export ABI, hand-written JS glue.
`panic = "abort"`, `opt-level = "z"` or `"s"`, LTO, one codegen unit. `Cargo.lock` holds a single package.

**(b) Hand-written WAT.** Engine written directly in WebAssembly text, assembled with `wat2wasm` (already
installed) or shipped with a small own assembler.

**(c) AssemblyScript / MoonBit / Zig.** AssemblyScript is an npm package (third-party JS toolchain with
binaryen). MoonBit needs the `moon` toolchain (binary distribution, young ecosystem, registry packages for
anything beyond core). Zig needs a tarball install (single static binary, std only, good wasm output).

**(d) Hybrid: TS host shell + WASM kernels for hot builtins only.** Parser and interpreter in TS/JS; grep,
sed, awk, jq, sort, regex as WASM kernels called with byte buffers.

| Criterion | (a) Rust zero-crate | (b) hand WAT | (c) AS / MoonBit / Zig | (d) hybrid TS + kernels |
|---|---|---|---|---|
| Startup model | One module, est. 0.6-1.2 MB for core: 1.5-3 ms sync compile + 0.06 ms instantiate; `Module` cached per process, instance per `Bash` | Smallest binary (est. 100-300 KB): < 1 ms | Zig similar to (a); AS adds its runtime/GC; MoonBit wasm-gc target needs recent engines | JS parse/compile of the interpreter dominates (the cost just-bash pays today: 60 ms import); kernels add 0.2-1 ms each on first use |
| JS<->WASM boundary for the VFS | VFS in linear memory: zero crossings for in-memory work; host FS via `HOST_CALL` | Same as (a) | Same as (a) | VFS stays in JS: every kernel call copies file content in and out (0.5-0.7 ns/byte each way), pipelines cross per stage |
| Binary size | Medium; `std` formatting and panics need care (`panic=abort`, no `fmt` in hot paths); no `wasm-opt` unless binaryen is accepted | Smallest | Zig small; AS medium; MoonBit wasm-gc small | Small WASM, large JS |
| Memory isolation | Full: untrusted data never becomes JS objects; Rust memory safety inside the module (no `unsafe` outside the ABI and allocator) | Full isolation from JS, but **no memory safety inside** the module: every bounds check is manual | Zig: isolation, manual memory safety (safe-mode checks in debug only); AS: GC-managed | Interpreter state on the JS heap: prototype-pollution and escape classes remain, as in just-bash |
| Fuel / step limits | VM loop decrements fuel; hard memory cap via the module's declared max pages; fresh instance after a trap | Same, by hand | Same | Two limit systems (JS + each kernel) |
| Build reproducibility | `rustup` pin via `rust-toolchain.toml` + `rustup target add wasm32-unknown-unknown` (native, official, no Docker); deterministic output with fixed flags; committed `.wasm` checked by a rebuild-and-compare gate | `wat2wasm` pinned; trivially reproducible | New toolchain installs: zig tarball, `moon` installer, or npm packages (rejected by the zero-third-party rule for AS) | Needs a TS build step (`tsc`/bundler are third-party dev tools) or JS with JSDoc |
| Supply chain | rustc/cargo already installed (1.96); adds one official target component; zero crates | wabt already installed | Zig: one new binary; MoonBit: new vendor toolchain + registry; AS: npm tree | Same as the kernel language, plus the JS tooling |
| Contributor cost | Mainstream language, strong tooling and tests (`cargo test` natively on x86_64 for kernel unit tests) | Very high: about 100k lines of equivalent logic in WAT is not reviewable; error-prone | Zig medium; MoonBit high (small community); AS medium | Low entry cost, but two languages on every hot path and duplicated semantics |
| Cheap-model friendliness (section 7) | Good: precise specs map to Rust modules with native unit tests | Poor | Mixed | Good for TS parts |
| Performance ceiling | High: byte-oriented, no GC, predictable | Highest in theory, impractical | Zig comparable to (a) | Bounded by JS interpreter and boundary copies |

### 2.3 Recommendation (OWNER gate Q1)

**Option (a): a Rust zero-crate fat kernel compiled to `wasm32-unknown-unknown`, with a thin dependency-free
JS host, and the resumable-VM execution model.** Hand-written WAT (b) is kept only for micro-kernels where
it pays for itself and stays reviewable (for example a `memchr`/newline scanner), if a benchmark proves a
gain. Hybrid (d) is rejected as the target architecture because it keeps the two costs this project exists
to remove (JS startup of a large interpreter, shared-heap security surface); it remains a valid fallback for
individual companion commands during migration.

Rationale:
1. Startup: measured budget is about 2.5 ms per MB of module plus 0.06 ms per instance, against 111 ms for
   just-bash import + first exec. Even a 1.2 MB kernel leaves an order of magnitude of margin.
2. Security: linear-memory isolation removes the JS escape and prototype-pollution classes for the kernel;
   the JS host can be kept to a few hundred auditable lines; a trap poisons only a disposable instance.
3. Zero-dependency rule: Rust `std`/`core` gives collections, UTF-8 and formatting without any crate; the
   toolchain is already on the machine except for one official target component.
4. Testability: kernel logic compiles natively for fast unit tests and fuzzing loops, while the shipped
   artefact is the WASM build exercised through `node:test`.
5. Cheapest delivery: per-command work is separable and specifiable (section 7).

Known costs to accept: we write our own regex engine, glob matcher, printf/format engine, hashing (md5,
sha1, sha256), deflate/inflate, and JSON/YAML/CSV parsers, since no crate is allowed. Without `wasm-opt`
the binary is larger than the theoretical minimum; a size gate (section 5.5) keeps it bounded.

### 2.4 Kernel sketch (to be detailed in the EVOL rung)

```
JS host (no dependency)                         WASM kernel (Rust, zero crates)
  Bash class / options / capability table         lexer -> parser -> compact AST/bytecode
  module cache (one WebAssembly.Module)            resumable VM (explicit stack, fuel counter)
  instance per Bash object                         expansion engine, arithmetic (i64)
  exec(): write script+stdin into memory           VFS (in-memory tree, byte files, lazy-file stubs)
          loop { status = run(fuel) }              builtins + core commands (tier 1)
            HOST_CALL -> fs/fetch/sleep/custom     regex (linear-time), glob, printf engine
            OUT_OF_FUEL -> yield / abort check     limits: steps, output, depth, memory pages
          read stdout/stderr/exit code             imports: host_call(), clock, random (capability-gated)
```

ABI principles: a single `host_call(op, ptr, len)` import family; all strings and buffers are (pointer,
length) pairs in linear memory; no JS object ever enters the kernel; exports limited to `alloc`, `free`,
`init`, `exec_begin`, `run`, `resume`, result accessors. Bytes are the native type (bash semantics are
byte-oriented), UTF-8 decoding happens once on the way out.

---

## 3. Packaging

### 3.1 Core versus companion packages (proposal, OWNER gate Q4)

Constraint: every extra WASM module carries its own allocator and runtime support, and data shared with the
core VFS must be copied or reached through host calls. So a companion is justified by size or by standalone
value, not by tidiness.

| Package | Content | Rationale |
|---|---|---|
| `@sentropic/bash` (core) | JS host + kernel: parser, interpreter, all shell builtins, VFS, limits, regex/glob engines, and tier-1 commands: `echo printf cat head tail wc tee true false pwd cd ls mkdir rmdir rm cp mv ln touch chmod stat readlink mktemp basename dirname env printenv which seq sleep date test expr sort uniq cut tr paste rev tac nl grep egrep fgrep sed find xargs base64 yes` | What agents call in almost every session; must be available with zero lazy-load cost |
| `@sentropic/awk` | awk | Large (6.7k lines in the reference), own language VM |
| `@sentropic/jq` | jq, later yq | Large query engine (8.6k lines in the reference) |
| `@sentropic/grep` | standalone JS API over the core regex/search kernel, plus `rg` | Standalone value outside the shell; `grep` itself stays in core |
| `@sentropic/ls` | standalone listing/formatting API (`ls`, `tree`, `du`) | Named by the owner as a companion candidate; thin, low priority |
| `@sentropic/archive` | tar, gzip/gunzip/zcat (own inflate/deflate) | Sizeable codecs, rarely on the hot path |
| `@sentropic/text` | diff, comm, join, column, fold, expand/unexpand, split, strings, od, md5sum/sha1sum/sha256sum, file | Long tail of text tools |
| `@sentropic/net` | curl over a host-provided fetch capability | Keeps the network surface out of the default install |
| `@sentropic/make` | make (not in just-bash; owner-named) | New value, after parity |
| Not planned for v1 | `python3`, `js-exec`/`node`, `sqlite3`, `xan`, `html-to-markdown` | Each needs a large third-party runtime or library (CPython, QuickJS, SQLite, CSV/HTML stacks): incompatible with the zero-dependency rule unless reimplemented; left to custom commands |

Loading model: companions register through the same lazy-command mechanism as custom commands; the core
resolves an unknown command name to a registered companion, instantiates its module on first use and
streams stdin/stdout through linear-memory buffers.

### 3.2 API compatibility surface with just-bash

| Tier | Surface | v1 stance |
|---|---|---|
| A — drop-in | `new Bash({files, env, cwd, fs, executionLimits, commands, customCommands, sleep, network/fetch, processInfo})`, `exec(cmd, {env, replaceEnv, cwd, stdin, signal, args, rawScript})`, result `{stdout, stderr, exitCode, env}`, `readFile`, `writeFile`, `getCwd`, `getEnv`, `defineCommand`, `InMemoryFs` with the `IFileSystem` contract | Same names and semantics; proven by our own API tests |
| B — compatible behaviour, own implementation | `OverlayFs`, `ReadWriteFs`, `MountableFs`, `NetworkConfig` allow-list, limit names and defaults, byte/text helpers (`stdinKind`, `stdoutKind`) | Same options; host-side JS adapters behind explicit capabilities |
| C — later | `parse()` + AST types, transform pipeline and plugins, `Sandbox` facade, CLI binaries, `trace`, `logger`, `coverage` | After parity of tiers A and B; AST shape compatibility is an owner question (Q7) |
| D — not reproduced | `DefenseInDepthBox` and related exports, `python`, `javascript`, sqlite3 | The global-patching layer is specific to a shared-heap design; documented as N-A |
| Additions | `execSync` (when no async capability is involved), `Bash.precompile()`/module cache control, fuel and memory limits, per-instance reset | Opt-in, never required for drop-in use |

Types: `.d.ts` files are hand-written (no `tsc` in the build); whether the host is authored in TypeScript
(node 22 type stripping, no emit) or in JS with JSDoc is owner question Q6.

---

## 4. Security model draft

### 4.1 Principles

1. Default deny. A `Bash` created without options can compute and use its in-memory VFS; it cannot read a
   host file, open a socket, spawn a process, read the host clock precisely, or read host environment.
2. Capabilities are explicit objects passed at construction, each mapped to a `host_call` operation code.
   An operation without a granted capability returns `EPERM`-style errors inside the sandbox; it never
   reaches host code.
3. The kernel is the only code that sees untrusted bytes. The JS host validates only lengths and operation
   codes, and never evaluates, never builds property names from sandbox data, and never passes sandbox
   strings to `import()`, `require`, `Function` or `RegExp`.
4. An instance is disposable: any trap (unreachable, out-of-bounds, stack exhaustion, memory cap) ends the
   `exec` with a fatal status and the instance is discarded and recreated (0.06 ms).

### 4.2 Capability table (default: all denied)

| Capability | Grants | Default | Notes |
|---|---|---|---|
| `fs: memory` | In-kernel VFS seeded by `files` | granted | Size-capped (`maxFileSystemBytes`) |
| `fs: host-read` (`OverlayFs`) | Read under a root, writes stay in memory | denied | Root containment on canonical paths, symlinks denied unless opted in, null bytes rejected, host paths stripped from errors |
| `fs: host-write` (`ReadWriteFs`) | Read/write under a root | denied | Same gates, plus hard-link copy-on-write semantics |
| `fs: custom` (`IFileSystem`) | Caller-provided async FS | denied | Reached through `HOST_CALL`, results copied into linear memory |
| `network` | `fetch` through an allow-list (URL prefixes, methods, redirects re-validated, size cap, private ranges denied) | denied | `curl` is not even registered without it |
| `process` | Nothing in v1 | denied, not grantable | No host process execution; `bash`/`sh` are nested interpreters |
| `time: clock` | Wall clock for `date`, `$SECONDS`, `$EPOCHSECONDS` | virtual clock (fixed epoch or caller-provided) | Real clock is opt-in; avoids timing side channels and makes runs deterministic |
| `time: sleep` | `sleep`, `timeout` | virtual (advances the virtual clock, no real wait) | Real sleep via the `sleep` hook, as in just-bash |
| `random` | `$RANDOM`, `mktemp`, `shuf` | deterministic seeded PRNG | Host entropy opt-in |
| `env` | Host environment | denied | Only the `env` option is visible; virtual pid/uid/hostname |
| `custom commands` | Caller JS functions | per command | `trusted` flag semantics kept; arguments and stdin copied out as bytes |

### 4.3 Resource limits

| Limit | Enforcement point | Proposed default |
|---|---|---|
| Fuel (VM steps) | Decremented in the VM dispatch loop and in every builtin loop (regex steps, sort comparisons, awk/sed/jq ops) | derived from the just-bash counters: 100k commands, 100k loop iterations per loop |
| Wall time | Checked at each fuel slice boundary by the host (`OUT_OF_FUEL` -> check deadline and `AbortSignal`) | 1 h hard cap like the reference, configurable |
| Memory | `maximum` pages declared at instantiation: the engine refuses `memory.grow` beyond it; kernel allocator returns ENOMEM cleanly before the cap | 256 MiB default, configurable |
| Output size | Counter on stdout+stderr appends | 256 MiB reference default; a lower default is an owner question (Q3) |
| Recursion | Explicit VM stack with depth counters: function calls (100), command substitution (50), parser nesting (200), eval/source depth (64) | reference values |
| Input | Script bytes, heredoc bytes, token count | 64 MiB / 64 MiB / 100k (reference values) |
| Expansion | Brace results, glob operations, string length, array elements | reference values (`JB:src/limits.ts`) |
| VFS | Total bytes, entry count, path depth, traversal entries | reference values |
| Regex | Linear-time engine by construction (Pike VM / lazy DFA); backreferences (`grep` BRE, `sed`) go through a bounded backtracker charged in fuel; **no fallback to JS `RegExp`** | n/a |

Limit names and option keys stay compatible with `ExecutionLimits` so existing callers keep working; the
two new knobs (fuel, memory pages) are additions.

### 4.4 Fuzz and adversarial strategy (node built-ins only)

| Technique | Implementation without third-party packages |
|---|---|
| Grammar-based script generation | Own generator in JS using a seeded PRNG (xorshift) so every failure is reproducible from a seed printed by `node:test` |
| Mutation fuzzing | Byte-level mutations of a corpus of our own scripts (insert, delete, duplicate, flip, splice), run against `parse` and `exec` with tight limits |
| Oracles | (1) no trap and no host exception, (2) every run ends within fuel and memory limits, (3) no host capability call when none is granted (the host records calls), (4) determinism: same seed, same output, (5) differential: same script in the system `bash` for the deterministic subset, compared on stdout/exit code |
| Adversarial suites | Own cases per threat class from section 1.5: parser bombs, expansion bombs, path traversal and symlink escapes on the host FS adapters, redirect chains on the network allow-list, output floods, deep recursion, regex blowups, NUL bytes and invalid UTF-8, option-key names such as `__proto__` and `constructor` in env/array keys |
| Native-side fuzzing | Kernel compiled natively (x86_64) and driven by a loop harness in a `cargo test` binary with the same seeded generator: catches panics and overflow with debug assertions on |
| Regression corpus | Every failing seed becomes a named test under `test/security/` |
| Budget | A short deterministic fuzz slice in `make check`; a long run via an env var (`FUZZ_RUNS`) outside the gate |

---

## 5. Benchmark protocol draft

### 5.1 Metrics

| Id | Metric | Definition |
|---|---|---|
| M1 | Cold start | Fresh node process: `import` + construct + first `exec('echo hi')`, measured in-process with `performance.now()`, plus process wall time minus the bare `node -e 0` baseline |
| M2 | Warm construct | `new Bash()` in a warm process |
| M3 | Warm exec | Median time of `exec` for a fixed script set (`:`, `echo hi`, a 1 000-iteration loop, a 20-line agent-style script) |
| M4 | Per-builtin microbench | One script per command over fixed inputs (1 KB, 250 KB, 10 MB), reported as ms and MB/s |
| M5 | Memory | RSS and `heapUsed` deltas against bare node after M1, and after a 10 MB workload; WASM linear-memory high-water mark for our engine |
| M6 | Size | Published file sizes: JS host, `.wasm` raw and gzip -9, total install footprint (`node_modules` bytes, package count) |
| M7 | Throughput under limits | Time for a script that hits the fuel limit to be stopped (limit enforcement latency) |

### 5.2 Harness (node built-ins only)

- `bench/run.mjs` (the `make bench` target already expects it): orchestrator using `node:child_process`
  (`execFileSync` of `process.execPath`) for cold runs, `performance.now()` and `process.hrtime.bigint()`
  for timing, `node:fs` for sizes, `node:zlib` for gzip sizes, `node:os` for the machine fingerprint.
- One adapter module per engine (`bench/engines/<name>.mjs`) exporting `create()` and `exec()`; the
  workload list is shared, so every engine runs byte-identical scripts and inputs.
- Output: JSON lines under git-ignored `tmp/bench-results/` plus a Markdown table printed to stdout; the
  JSON carries node version, CPU model, engine versions and commit sha.
- Result equality is asserted before timing (stdout and exit code must match across engines for a workload
  to be reported), so a faster wrong answer cannot win.

### 5.3 Reference engines (bench only, never a repo dependency)

| Engine | Install | Location |
|---|---|---|
| just-bash | `npm install --ignore-scripts --no-audit --no-fund just-bash@<pinned>` with a local `--cache` | `tmp/bench-engines/just-bash/` (git-ignored); version recorded in the result JSON |
| MoonBash | Not on npm. Option 1: build from the reference clone, which needs the `moon` toolchain (owner question Q8). Option 2: bench against just-bash only and report MoonBash as `source-gap` | `tmp/bench-engines/moonbash/` |
| System bash | `/bin/bash -c`, as an external reference point for M3/M4 (process spawn included, labelled as such) | n/a |

`make check-no-deps` keeps guarding the manifests: bench engines never appear in any `package.json` of the
repo.

### 5.4 Statistics

- Cold metrics: 30 fresh processes per engine, interleaved (A, B, A, B, ...) to spread thermal and cache
  effects; report median, p95, min.
- Warm metrics: 20 warm-up iterations discarded, then at least 200 iterations or 2 s; report median, p95.
- A comparison is reported only when the two medians differ by more than the larger p95-to-median spread;
  otherwise it is labelled "within noise".
- Regression gate: compare against a committed baseline JSON (`bench/baseline.json`) produced on the same
  machine class; the gate fails when a gated metric regresses by more than the agreed budget.

### 5.5 Proposed performance budgets (OWNER gate Q2)

Baselines are the section 1.7 measurements of just-bash 3.6.0 on this machine.

| Metric | just-bash (measured) | Proposed v1 budget | Ratio |
|---|---:|---:|---:|
| M1 cold start (import + ctor + first exec) | 111 ms | <= 10 ms | >= 11x |
| M2 warm `new Bash()` | 0.37 ms | <= 0.15 ms | >= 2.5x |
| M3 `echo hi` | 0.35 ms | <= 0.05 ms | >= 7x |
| M3 1 000-iteration loop | 7.4 ms | <= 1.5 ms | >= 5x |
| M4 `grep -c` on 10k lines | 11.0 ms | <= 2 ms | >= 5x |
| M4 `sed s///p` on 10k lines | 73.7 ms | <= 8 ms | >= 9x |
| M4 `awk` sum on 10k lines | 61.0 ms | <= 8 ms | >= 7x |
| M4 `sort -r` on 10k lines | 9.2 ms | <= 4 ms | >= 2x |
| M5 RSS delta after first exec | 35.5 MB | <= 8 MB | >= 4x |
| M6 shipped code, core | 2.02 MB JS (557 KB gzip), 102 MB installed with 73 packages | <= 1.2 MB total (`.wasm` + JS), <= 400 KB gzip, 0 dependency | n/a |
| Regression budget in the gate | n/a | fail on > 10% regression of a gated median against the baseline | n/a |

These are targets to validate with the first vertical slice (lot L2), not commitments.

---

## 6. Open OWNER questions

**Q1 — WASM toolchain.**
Context: only the x86_64 Rust target is installed; wabt is installed; no zig, no moon.
Options: (a) Rust zero-crate + `rustup target add wasm32-unknown-unknown`; (b) hand WAT only; (c) Zig;
(d) hybrid TS + kernels.
Recommendation: (a), with WAT allowed for benchmark-proven micro-kernels.
Consequence: one native rustup step pinned by `rust-toolchain.toml`; we own regex, codecs and parsers;
without binaryen there is no `wasm-opt` pass (sub-question: accept binaryen as a pinned native build tool,
yes/no; recommendation: no for v1).

**Q2 — Performance budgets.**
Context: section 5.5 table, based on measured just-bash numbers.
Options: accept the table; tighten (for example cold start <= 5 ms); or gate only on "never slower than
just-bash" for v1.
Recommendation: accept the table as targets, confirm after lot L2, gate regressions at 10%.
Consequence: budgets drive the single-module decision and forbid lazy JS chunks on the hot path.

**Q3 — Parity target when just-bash differs from GNU bash.**
Context: just-bash clamps arithmetic to 53 bits, skips some errexit and IFS cases, and has lower/higher
default limits than MoonBash.
Options: (a) bug-for-bug just-bash; (b) GNU bash 5.x behaviour as the truth, just-bash API as the shape;
(c) just-bash by default with a `strict-bash` switch.
Recommendation: (b), with a documented divergence list.
Consequence: differential tests run against the system bash; a few just-bash users may see different
(more correct) output; default limits (output 256 MiB, 100k commands) kept equal to just-bash unless the
owner asks for stricter defaults.

**Q4 — Scope of v1 (which commands first) and companion split.**
Context: section 3.1; 97 command names plus opt-in runtimes in the reference.
Options: (a) tier-1 list of section 3.1 in core, then awk and jq, then the long tail; (b) all 97 before any
release; (c) shell-only core, every command a companion.
Recommendation: (a); python3, js-exec, sqlite3, xan and html-to-markdown declared out of v1.
Consequence: "full functional parity" is reached in stages; the parity report lists each command as
`covered`, `partial` or `not covered`.

**Q5 — Shipping the `.wasm` artefact.**
Context: consumers must not need Rust.
Options: (a) commit the built `.wasm` and verify it by a rebuild-and-compare gate; (b) build only at
publish time; (c) embed as base64 in a JS file for bundler friendliness.
Recommendation: (b) for the repo plus a reproducibility check, and (c) as an additional published entry
point for browser bundlers.
Consequence: defines the release procedure and whether binary files live in git.

**Q6 — Host language and types.**
Context: zero third-party build tooling is preferred; `tsc` is a third-party package.
Options: (a) plain ESM JavaScript with JSDoc + hand-written `.d.ts`; (b) TypeScript sources run with node's
built-in type stripping and shipped with a hand-written or stripped output; (c) accept `typescript` as a
justified dev tool.
Recommendation: (a) for the host (a few hundred lines), tests in `.mjs` with `node:test`.
Consequence: no TS build step at all; type declarations are maintained by hand and checked by API tests.

**Q7 — AST and transform API compatibility.**
Context: just-bash exports `parse()`, AST node types and a transform/serialize pipeline used by plugins.
Options: (a) not in v1; (b) expose an AST as JSON produced by the kernel, shape-compatible with just-bash;
(c) own AST shape.
Recommendation: (a) for v1, (b) afterwards if a consumer needs it.
Consequence: v1 is a drop-in for `Bash.exec` users, not for transform-plugin users.

**Q8 — MoonBash in the benchmark.**
Context: not published on npm; building it needs the `moon` toolchain (new vendor binary).
Options: (a) install `moon` under `tmp/` for bench only; (b) report MoonBash as `source-gap` and bench
against just-bash and system bash; (c) use its published size numbers only.
Recommendation: (b) now, (a) only if the owner wants the three-way table.
Consequence: with (b) the "benchmarked against MoonBash" objective is `partial`.

**Q9 — Regex semantics.**
Context: no third-party engine allowed; agents use ERE/PCRE-like syntax in grep/sed/awk.
Options: (a) linear-time engine for BRE/ERE + common Perl classes, bounded backtracking only for
backreferences; (b) full backtracking engine with fuel; (c) host `RegExp` fallback.
Recommendation: (a); `grep -P` lookaround reported as unsupported in v1.
Consequence: ReDoS-free by construction; a small set of PCRE-only patterns fails with a clear error.

**Q10 — Execution model exposure.**
Context: the resumable VM makes a synchronous `execSync` and a fresh-instance-per-exec mode possible.
Options: expose both as additions; or keep the API strictly identical to just-bash.
Recommendation: expose `execSync` and an `isolation: "instance" | "exec"` option as additions.
Consequence: slightly larger API to document and test; no impact on drop-in users.

---

## 7. First rough lot breakdown (sketch, cheapest-delivery oriented)

Sizes are relative (S = 1, M = 3, L = 8, XL = 20). "Strong" = needs a strong model for design or subtle
semantics; "Cheap" = a fast/cheap build model can deliver from a precise spec with a test list.

| Lot | Content | Size | Model | Depends on |
|---|---|---:|---|---|
| L0 | Toolchain pin (`rust-toolchain.toml`, wasm target), build script in node, `make` targets via a scope exception, reproducibility check | S | Cheap | Q1, Q5 |
| L1 | ABI and host: memory layout, `host_call` protocol, resumable `run/resume`, JS host with module cache, capability table, limits plumbing | M | **Strong** | L0 |
| L2 | Vertical slice: lexer + parser subset, VM, `echo`/`:`/variables/pipelines, in-kernel VFS minimal, first benchmark against just-bash (validates Q2 budgets) | L | **Strong** | L1 |
| L3 | Full parser (grammar areas of section 1.4) + parse-error behaviour + parser fuzz | L | **Strong** | L2 |
| L4 | Expansion engine: parameter ops, command/arithmetic/process substitution, brace, tilde, glob/extglob, word splitting, quoting | L | **Strong** | L3 |
| L5 | Interpreter semantics: redirections and fd table, functions, subshells, `set`/`shopt`, errexit, traps subset, arrays, namerefs | L | **Strong** | L4 |
| L6 | Shell builtins (`cd export local declare read mapfile unset eval source getopts printf test [[ …`) | M | Cheap, per-builtin spec | L5 |
| L7 | VFS complete + host adapters (`OverlayFs`, `ReadWriteFs`, `MountableFs`, custom `IFileSystem`) with path-security tests | M | Strong for the security gates, cheap for the rest | L1 |
| L8 | Shared kernels: linear-time regex, glob matcher, printf/format, number parsing, hashing | M | **Strong** (regex), cheap (hashing, format) | L2 |
| L9 | Tier-1 commands (about 45), one spec + test list per command | XL | **Cheap**, highly parallel | L6, L8 |
| L10 | Own functional parity suite (`node:test`): API tier A/B tests, per-area grammar cases, differential recorder against system bash | L | Cheap for case writing, strong for the recorder design | L2 onward, continuous |
| L11 | Security: adversarial suites, seeded fuzzers (JS and native), limit-latency tests | M | **Strong** | L5, L7 |
| L12 | Benchmark harness + baseline + regression gate (`bench/run.mjs`) | M | Cheap from section 5 | L2 |
| L13 | `sed` (in core) | M | Cheap with a precise command table | L8 |
| L14 | `@sentropic/awk` | L | Strong for the language core, cheap for built-in functions | L8 |
| L15 | `@sentropic/jq` (+ yq later) | L | Strong for the evaluator, cheap for builtins | L8 |
| L16 | Long tail companions (`text`, `archive`, `net`, `ls`, `grep` API, `make`) | XL | **Cheap**, parallel | L9 |
| L17 | API tier C (parse/AST export, transform, `Sandbox` facade, CLI) | M | Cheap | Q7 |

Cheapest-delivery reading:
- The strong-model budget concentrates on L1-L5, L8 (regex) and L11: about 40% of the total size, mostly
  sequential, and it produces the specs that make everything else mechanical.
- L6, L9, L12, L13, L16 and most of L10 are spec-driven, test-first and parallelisable across cheap lanes:
  one command = one spec file (options table, exit codes, error strings, edge cases) + one test list + one
  Rust module with native unit tests. This is about 55% of the total size.
- The order L0 -> L1 -> L2 gives a measured go/no-go on the architecture (Q1, Q2) after the three smallest
  strong lots, before any volume work starts.
- Risk hot spots to review adversarially before the plan rung: the async host-call bridge (L1), the binary
  size without `wasm-opt` (L2 measurement), and regex semantics coverage (L8, Q9).

---

## 8. Sources and reproducibility of this study

- Reference trees: `tmp/refs/just-bash` (v3.6.0 sources), `tmp/refs/MoonBash` (v0.1.0 sources).
- Counts in sections 1.1-1.4 come from one-off node scripts over those trees (files, lines, `flag:`
  declarations, `it(`/`test(` call sites); they are approximations of surface, not of quality.
- Measurements in sections 1.7 and 2.1 come from one-off node scripts under the git-ignored `tmp/bench/`
  (just-bash 3.6.0 from npm, synthetic WAT modules assembled with wabt 1.0.36), 25-30 fresh processes per
  cold metric, median reported. They are a single-machine snapshot and must be re-measured by the lot L12
  harness before any budget becomes a gate.
- Items labelled "estimate" (flag coverage classes, kernel size, lot sizes) are unverified.
