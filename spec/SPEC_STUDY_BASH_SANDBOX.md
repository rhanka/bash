# SPEC STUDY — @sentropic/bash sandbox (rung 1: study, v3)

Status: study only, v3. v2 reconciled the adversarial review (verdict `approve` on v2). v3 reworks the
architecture for the owner's decision A1: Rust to `wasm32-wasip2`, Component Model, dedicated WIT contract,
running in wasmCloud, Node and the browser. Section 6 lists the answered decisions and the questions still
open. Appendix A maps each review finding to its resolution; Appendix B records the v3 changes.
Scope: clean-room, MIT, zero third-party runtime dependency, WASM-first reimplementation with functional
parity against `vercel-labs/just-bash`, benchmarked against just-bash and MoonBash on this machine.

Conventions used below:
- `JB:` = path under `tmp/refs/just-bash/packages/just-bash/` (reference clone, git-ignored).
  `JBR:` = path under the reference repo root `tmp/refs/just-bash/`.
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
| Language / runtime | TypeScript, pure JS at runtime, Node >= 20.19, browser bundle (`./browser` export), CJS and ESM entries | `JB:package.json` |
| Runtime dependencies | 16 `dependencies` (diff, fast-xml-parser, file-type, guarded-fetch, ini, modern-tar, papaparse, re2js, run, seek-bzip, smol-toml, sprintf-js, sql.js, turndown, typescript, yaml) + 2 `optionalDependencies` (`@mongodb-js/zstd`, `node-liblzma`) | `JB:package.json` |
| Second package | `@just-bash/executor` v7.0.0 (tool-invocation bridge used by `JavaScriptConfig.invokeTool`) | `JBR:packages/just-bash-executor/package.json` |
| Source size | 1106 `.ts` files, 700 test files | `JB:src/` |
| Command sources | 82 directories under `src/commands/`: 77 command directories + 5 shared/test directories (`compression`, `query-engine`, `search-engine`, `worker-bridge`, `test`); about 67.6k non-test lines | `JB:src/commands/` |
| Interpreter | 97 files, about 34.4k lines (+73 test files) | `JB:src/interpreter/` |
| Parser + AST | 11 files / 9.5k lines + 1.1k lines of AST types | `JB:src/parser/`, `JB:src/ast/types.ts` |
| Filesystems | 17 files / 6.9k lines (+36 test files) | `JB:src/fs/` |
| Security layer | 27 files / 10.0k lines (+69 test files) | `JB:src/security/` |
| Network | 6 files / 1.75k lines; SSRF and transport safety delegated to the `guarded-fetch` package | `JB:src/network/fetch.ts:5` |
| Build | `tsc` then esbuild (ESM split bundle, CJS bundle, browser bundle, CLI, shell, workers) | `JB:package.json` scripts |

Line counts come from a one-off script that counts `split('\n')` per file (one extra line per file); they
size the surface, they are not exact.

### 1.2 Public API surface (extracted from `JB:src/index.ts`, `JB:src/Bash.ts`, `JB:src/types.ts`)

| Area | Symbols | Notes |
|---|---|---|
| Entry class | `Bash` (`JB:src/Bash.ts:330-1088`) | `new Bash(options)`; public member `readonly fs: IFileSystem` (line 331); methods `exec(cmd, opts)`, `readFile`, `writeFile`, `getCwd`, `getEnv`, `registerCommand`, `registerTransformPlugin`, `transform` |
| `BashOptions` | `files`, `env`, `cwd`, `fs`, `executionLimits`, `executionLimitProfile`, `maxCallDepth`, `maxCommandCount`, `maxLoopIterations`, `fetch`, `network`, `python`, `javascript`, `commands`, `sleep`, `customCommands`, `logger`, `trace`, `defenseInDepth`, `coverage`, `processInfo {pid, ppid, uid, gid}` | `JB:src/Bash.ts:144` |
| `ExecOptions` | `env`, `replaceEnv`, `cwd`, `rawScript`, `stdin`, `stdinKind ("text"/"bytes")`, `signal` (AbortSignal), `args` | `JB:src/Bash.ts` |
| Result | `BashExecResult { stdout, stderr, exitCode, env, metadata?, stdoutKind? }` | `JB:src/types.ts:15-67` |
| Non-throwing `exec` | Syntax errors (exit 2), abort/deadline (124), `ExecutionLimitError` (its own exit code), arithmetic errors (1), cleanup errors (126) come back as results; only invalid options and `maxSourceBytes` overflow throw | `JB:src/Bash.ts:926-1033`, `:695` |
| Script normalization | Unless `rawScript` is set, leading indentation is stripped outside multi-line quotes and heredocs (`normalizeScript`) | `JB:src/Bash.ts:853,1152` |
| Output encoding | Output is a binary string; valid UTF-8 sequences are decoded, invalid bytes are preserved as latin1 (`decodeBinaryToUtf8`); `stdoutKind`, `ByteString`, `stdoutAsBytes` expose raw bytes | `JB:src/Bash.ts:626,1226`, `JB:src/encoding.ts` |
| State model | Filesystem persists across `exec` calls; shell state (env, cwd, functions) is isolated per `exec`. `exec` is async, not serialized: overlapping calls on one `Bash` interleave at await points over the shared `fs`; commands re-enter through `ctx.exec` / `ctx.origCommand` | `JB:src/Bash.ts:631`, `JB:src/types.ts:218-225` |
| Default layout | Without `cwd`/`files`: cwd and `HOME` = `/home/user` (else `/`); `/bin/<cmd>` and `/usr/bin/<cmd>` stubs for each registered command; `/dev/{null,zero,stdin,stdout,stderr,fd}`; `/proc/{version,self/...}`; env `OSTYPE`, `MACHTYPE`, `HOSTTYPE`, `HOSTNAME`, `SHELLOPTS`, `BASHOPTS` | `JB:src/Bash.ts:380-383`, `JB:src/fs/init.ts:38-120` |
| Filesystems | `InMemoryFs` (standalone object, shareable between `Bash` instances and mounts; also sync `mkdirSync`, `writeFileSync`, `writeFileLazy`), `OverlayFs`, `ReadWriteFs`, `MountableFs` | `JB:src/fs/*/`, `JB:src/fs/init.ts:15-22` |
| FS contract | `IFileSystem`: 23 methods (20 required + optional `readFileBytes`, `createExclusive`, `readdirWithFileTypes`); all I/O methods return Promises; `resolvePath`, `getAllPaths` sync; `createExclusive` is the atomic create used by `mktemp` and `set -C`; `FsStat` may carry `dev`/`ino`/`identity` for same-file detection; lazy files via `LazyFileProvider` | `JB:src/fs/interface.ts:137-332` |
| Custom commands | `defineCommand(name, execute, {trusted})`, `LazyCommand`, `createCommandContext`; context: `fs`, `fsIdentity`, `cwd`, `env`, `exportedEnv`, `assignShellVariable`, `stdin` (`ByteString`), `limits`, `exec`, `origCommand`, `execWithInheritedStdin`, `fetch`, `sleep`, `signal`, `fileDescriptors`, `getRegisteredCommands` | `JB:src/custom-commands.ts`, `JB:src/types.ts:157-331` |
| Command-list helpers | `getCommandNames`, `getNetworkCommandNames`, `getPythonCommandNames`, `getJavaScriptCommandNames` | `JB:src/index.ts:24-29` |
| Network | `NetworkConfig { allowedUrlPrefixes, allowedMethods, dangerouslyAllowFullInternetAccess, maxRedirects, timeoutMs, maxResponseSize, denyPrivateRanges }`, request transforms, typed errors incl. `ResponseTooLargeError` | `JB:src/network/types.ts:56-226` |
| Limits | `ExecutionLimits` (about 50 fields) with two profiles, `normal` (default) and `hardened` | `JB:src/limits.ts:153-262` |
| Parser / transform | `parse()`, AST node types, `BashTransformPipeline`, `TeePlugin`, `CommandCollectorPlugin`, `serialize()` | `JB:src/parser/parser.ts`, `JB:src/transform/` |
| Sandbox facade / CLI | `Sandbox`, `SandboxCommand`; binaries `just-bash`, `just-bash-shell` | `JB:src/sandbox/`, `JB:src/cli/` |
| Security exports | `DefenseInDepthBox`, `SecurityViolationError`, `SecurityViolationLogger` | `JB:src/security/index.ts` |

Reference limit profiles (extracted, `JB:src/limits.ts:164-262`):

| Limit | `normal` | `hardened` |
|---|---:|---:|
| `maxSourceBytes` | 64 MiB (but the parser rejects > 1 000 000 chars, `JB:src/parser/parser.ts:136`) | 8 MiB |
| `maxCommandCount` / `maxLoopIterations` | 100 000 / 100 000 | 10 000 / 10 000 |
| `maxWorkUnits` (aggregate across nested execs) | 100 000 000 | 100 000 |
| `maxExecDepth` / `maxCallDepth` / `maxSubstitutionDepth` / `maxSourceDepth` | 64 / 100 / 50 / 100 | same |
| `maxStringLength` / `maxHeredocSize` | 64 MiB / 64 MiB | 10 MiB / 10 MiB |
| `maxOutputSize` | 256 MiB | 10 MiB |
| `maxLiveBytes` / `maxInputBytes` | 512 MiB / 512 MiB | 64 MiB / 32 MiB |
| `maxFileSystemBytes` | 1 GiB | 128 MiB |
| `maxArchiveBytes` / `maxArchiveEntryBytes` / `maxArchiveEntries` | 1 GiB / 512 MiB / 1 000 000 | 128 MiB / 64 MiB / 100 000 |
| `maxFileDescriptors` | 4 096 | 1 024 |
| `maxExecutionTimeMs` | 1 h | 30 s |

### 1.3 Command inventory and flag-coverage estimate

Registry: `JB:src/commands/registry.ts` (680 lines). `CommandName` has **85** names (81 in the browser
bundle + `tar`, `yq`, `xan`, `sqlite3` on Node only), plus 5 opt-in names (`curl`; `python3`, `python`;
`js-exec`, `node`): **90** `AllCommandName` in total (counted by script). Every command is a lazy `import()`
chunk.

Shell builtins are dispatched in `JB:src/interpreter/builtin-dispatch.ts:476-672` (not all have a file in
`builtins/`): `:`, `true`, `false`, `cd`, `export`, `readonly`, `local`, `declare`/`typeset`, `read`,
`mapfile`/`readarray`, `set`, `shopt`, `unset`, `eval`, `source`/`.`, `exec`, `exit`, `return`, `break`,
`continue`, `shift`, `getopts`, `let`, `hash`, `dirs`/`pushd`/`popd`, `command`, `builtin`, `wait`, `type`,
`compgen`, `complete`, `compopt`, `help`, `test`/`[`. POSIX special builtins (`: . break continue eval exec
exit export readonly return set shift trap unset`, `JB:src/interpreter/helpers/shell-constants.ts:14-29`)
take precedence over functions. `[[ ... ]]` is a shell keyword handled by the parser, not a builtin.

Columns: "LOC" = non-test lines in the command directory; "flags" = options declared in the command's fuzz
metadata (`flag: "-x"`, a lower bound of what is parsed); "tests" = `it`/`test` call sites in the
directory; "coverage" = estimate of option coverage against the GNU/upstream tool (H >= 70%, M 30-70%,
L < 30%). The coverage column is an **estimate**, to be replaced by a per-flag audit in the EVOL rung.

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
| sed | 4066 | 5 | 240 | H | `-e -E -i -n -r`; own lexer/parser, addresses, hold space, branches |
| awk | 6684 | 2 | 664 | H | own lexer/parser/AST interpreter; `-F -v` |
| jq, yq | 671 + 7929 (query-engine) / 1031 | 9 / 5 | 323 / 223 | H / M | shared query engine; yq reads YAML/XML/TOML/INI/CSV via third-party parsers (`JB:src/commands/yq/formats.ts`) |
| xan | 6587 | 0 | 244 | M | CSV toolkit; depends on papaparse |
| find | 2479 | 8 | 217 | M | expression tree with precedence, `-prune`, `-exec ... {} +` |
| ls, tree, du, stat, file | 801 / 335 / 211 / 129 / 513 | 11 / 4 / 5 / 2 / 3 | 74 / 11 / 9 / 13 / 30 | M / M / M / L / M | ls: `-1 -a -A -d -F -h -l -r -R -S -t` |
| cp, mv, rm, mkdir, rmdir, touch, ln, chmod, readlink, mktemp | 227 / 215 / 95 / 78 / 226 / 202 / 163 / 296 / 136 / 450 | 5 / 3 / 4 / 2 / 2 / 4 / 4 / 2 / 2 / 6 | 15 / 21 / 15 / 11 / 1 / 8 / 20 / 13 / 14 / 58 | M | |
| basename, dirname, pwd, which, env, printenv, hostname, whoami | 30-200 each | 0-2 | 2-14 | H | trivial |
| true, false, yes, sleep, seq, expr, clear, help, history, alias, unalias, time, timeout | 30-460 each | 0-4 | 2-26 | M-H | `sleep` is virtual (host `sleep` hook) |
| xargs, tee | 456 / 84 | 6 / 1 | 35 / 9 | M / M | xargs: `-0 -d -I -n -r -t` |
| date | 234 | 4 | 70 | L | `-d -I -R -u`; defaults to UTC unless `$TZ` is set |
| diff | 553 | 6 | 93 | M | relies on the `diff` npm package |
| base64, md5sum, sha1sum, sha256sum | 312 / 303 | 2 / 1 | 35 / 31 | H | |
| gzip, gunzip, zcat, tar | 1108 / 3348 | 12 / 10 | 48 / 183 | M / M | codecs delegated to `node:zlib`, modern-tar and optional native deps |
| bash, sh | 208 | 1 | 52 | M | nested interpreter |
| html-to-markdown | 259 | 0 | 33 | n/a | wraps turndown (non-POSIX) |
| curl (opt-in) | 1180 | about 29 parsed options | 240 | M | goes through the allow-listed fetch |
| sqlite3 | 1933 | - | 167 | n/a | sql.js (WASM) in a worker |
| python3 / python (opt-in) | 2309 | - | 229 | n/a | CPython emscripten build in a worker |
| js-exec / node (opt-in) | 2967 | - | 317 | n/a | QuickJS-based runtime in a worker |

### 1.4 Grammar and expansion coverage (what the spec tests cover)

| Corpus | Size | Source | Use for us |
|---|---|---|---|
| Bash spec cases | 136 `.test.sh` files (+ `LICENSE-APACHE-2.0.txt`), about 46.3k lines, 2 728 `####` cases | `JB:src/spec-tests/bash/cases/` (imported from the Oils project, Apache-2.0) | Intent only. Not vendored. |
| AWK spec cases | 317 files, 77.6k lines | `JB:src/spec-tests/awk/cases/` | Intent only |
| grep / sed / jq spec cases | 7 / 5 / 8 files; 2.1k / 4.2k / 4.1k lines | `JB:src/spec-tests/{grep,sed,jq}/cases/` | Intent only |
| Comparison tests | 62 test files, 57 recorded fixtures (recorded against real bash) | `JB:src/comparison-tests/` | Same method, our own cases |
| Known gaps | 484-line list, internally stale (see below) | `JB:src/spec-tests/bash/KNOWN_LIMITATIONS.md` | Read with the source, not alone |

Feature areas named by the bash spec files: alias; append; arithmetic (`arith`, `arith-context`,
`arith-dynamic`, `dparen`, `let`, `for-expr`); arrays (indexed, associative, sparse, literal, compat);
assignment (`assign`, `assign-extended`, `assign-deferred`, temp bindings); brace expansion; builtins (`cd`,
`dirs`, `echo`, `eval`/`source`, `getopts`, `printf`, `read`, `set`, `trap`, `type`, `vars`, `special`,
`meta`, `misc`, `history`, `fc`, `bind`, `completion`, `kill`, `times`); `case`; command parsing and command
substitution; comments; `[[ ]]` (`dbracket`, with `=~` and `BASH_REMATCH`) and `[`; errexit and strict
options; exit status; extglob; functions; glob, globstar, globignore; here-docs; `if`; loops; nameref;
nocasematch; NUL bytes; parse errors; pipelines and `PIPESTATUS`; process substitution; quoting (including
ANSI-C `$'...'`); redirections (order, multi, command, numeric fds); regex; `set`/`shopt` options;
subshells; tilde; unicode; variable operators (`var-op-*`: test, strip, patsub, slice, length; `${var@Q}`
and the other `${var@op}` transformations), `var-ref`, `var-sub`, special vars; word evaluation and word
splitting; whitespace; xtrace. Interactive areas (`prompt`, `interactive`, `background`, job control) are
present in the corpus but mostly skipped by just-bash.

Behaviour of just-bash 3.6.0 verified in source (the prose in `KNOWN_LIMITATIONS.md` is partly outdated):
- `read -N` is implemented (`JB:src/interpreter/builtins/read.ts:30,83-96`).
- extglob is implemented, off by default (`JB:src/Bash.ts:455`); `globskipdots` defaults to true as in
  bash >= 5.2 (`JB:src/Bash.ts:460`).
- Arithmetic is not 64-bit: bitwise and shift operators use JS signed 32-bit integers (`1 << 31` gives
  `-2147483648`), the other operators use IEEE doubles (`JB:src/interpreter/arithmetic.ts:14-18,71-74`).
- No real job control or signals; `errexit` in compound commands and some IFS/here-doc edge cases are
  listed as pending.

### 1.5 Security model of the references

just-bash (`JBR:THREAT_MODEL.md`, 400 lines):

| Layer | Mechanism | Files |
|---|---|---|
| Architecture | No `child_process` import; only registered JS commands run | `JB:src/commands/registry.ts` |
| Filesystem | Virtual FS by default; `OverlayFs`/`ReadWriteFs` gate every path through root containment, null-byte rejection, symlinks denied by default, hard-link copy-on-write, broken-symlink leaf checks, error sanitization | `JB:src/fs/real-fs-utils.ts`, `JB:src/fs/overlay-fs/`, `JB:src/fs/sanitize-error.ts` |
| Network | Off by default; URL-prefix and method allow-lists, each redirect re-validated, response size cap, private-range denial; DNS pinning by `guarded-fetch` | `JB:src/network/` |
| Parser limits | `MAX_INPUT_SIZE` 1 000 000 chars, `MAX_TOKENS` 100k, `MAX_PARSER_DEPTH` 200, `MAX_PARSE_ITERATIONS` 1M | `JB:src/parser/types.ts:10-13` |
| Execution limits | Counters of section 1.2, checked at every loop, call and expansion; aggregate `maxWorkUnits` | `JB:src/limits.ts` |
| Extension revocation | Each command context is wrapped in a revocable proxy with a deadline; an extension that does not acknowledge an abort within `maxExtensionCleanupTimeMs` poisons its scope | `JB:src/interpreter/builtin-dispatch.ts:110,347,405` |
| ReDoS | User regexes go through `re2js` (linear time) | `JB:src/regex/` |
| Prototype pollution | Null-prototype maps and guards, enforced by a banned-pattern linter | `JB:src/security/prototype-pollution/` |
| Defense in depth | `DefenseInDepthBox`: AsyncLocalStorage-scoped patching that blocks `Function`, `eval`, `setTimeout`, `process.*`, `WebAssembly`, `Proxy`, module loading while sandboxed code runs | `JB:src/security/defense-in-depth-box.ts` |
| Info disclosure | Virtual pid/uid, host paths stripped from errors, `date` defaults to UTC | `JB:src/Bash.ts`, `JBR:THREAT_MODEL.md:179` |

Residual risks stated by the reference (threat model section 4): dynamic `import()` defence depends on the
runtime; pre-captured references bypass the global patches; signal/job control not modelled; python surface
when enabled. The "code execution escape" class exists because the interpreter and the untrusted data share
one JS heap.

MoonBash (`MB:docs/SECURITY.md`): same four layers with lower limit defaults (10 000 commands/iterations)
and a VM-based linear-time regex (`moonbitlang/regexp`). The document describes a fallback to JS `RegExp`
under a timeout wrapper; **no such fallback exists in the source** (no `js_regex_match_with_timeout`
anywhere under `MB:src/`; the wrapper calls plain `new RegExp` at `MB:src/wrapper/parser.ts:387,436`).
Test status: the 2026-02-19 snapshot reports grep spec 227 pass / 54 fail, jq 598 / 170 and one failing
security file; the newer status note (2026-04-20, `MB:docs/ROADMAP.md:60-62`) reports comparison tests
523/523, security attacks 0 failed, bash/grep/jq spec hardening still in progress.

Takeaway for the design: a WASM kernel removes the shared-heap class for everything that runs inside
linear memory. The remaining JS surface is the host glue, which must stay small enough to audit line by
line, and which still needs the revocation discipline of the reference for async extensions.

### 1.6 Test suite structure of just-bash

| Suite | Runner | Content |
|---|---|---|
| Unit / functional | vitest | co-located `*.test.ts`: 73 interpreter, 36 fs, 12 network, about 330 command files |
| Spec | vitest | parses upstream case files, per-suite `skips.ts` |
| Comparison | vitest (`RECORD_FIXTURES=1` to record) | recorded fixtures from real bash, replayed |
| Security | vitest | `attacks/`, `sandbox/`, `limits/`, `prototype-pollution/`, `*.security.test.ts` |
| Fuzz | vitest + fast-check | grammar-based generators, oracles, corpus, coverage (`JB:src/security/fuzzing/`) |
| Dist / syntax | vitest | bundle and package-content checks; 19 parser-level files |

Our equivalent uses `node:test` + `node:assert` only, with our own cases.

### 1.7 Startup path and size of just-bash (measured, published package 3.6.0)

Installed for measurement only under the git-ignored `tmp/bench/` (`npm install --ignore-scripts`). Cold
numbers: 30 fresh processes, interleaved with the baseline and the WASM probes of section 2.1, interpolated
median, nearest-rank p95.

| Metric | Value |
|---|---:|
| npm package | 21.6 MB unpacked, 970 files |
| Install footprint | 102 MB `node_modules`, 73 packages |
| ESM entry `dist/bundle/index.js` | 504 KB |
| ESM entry + 199 lazy chunks | 2.02 MB raw, 557 KB gzip -9 |
| CJS bundle / browser bundle | 1.7 MB / 1.2 MB |
| `import('just-bash')` | 65.9 ms median (p95 73.1) |
| `new Bash()` (first) | 6.4 ms median |
| first `exec('echo hi')` | 47.5 ms median (p95 55.9) |
| import + ctor + first exec | **118.8 ms median** (p95 138.5) |
| Process wall time | 194.6 ms vs 63.3 ms for an empty ESM script (delta 131 ms) |
| RSS after first exec | 93.5 MB vs 58.0 MB bare node (delta 35.5 MB); heap used 16.7 MB vs 3.9 MB |

What is loaded when (extracted): `JB:src/Bash.ts` statically imports the parser, the interpreter, the
filesystems, the limits, the security box and the transform serializer; commands are lazy chunks resolved by
`import()` on first use, which is why the first `exec` costs 47 ms even for `echo`.

Warm costs, same process, 50-200 runs after warm-up (measured; indicative, to be redone by the harness):

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
| Language | MoonBit; `MB:src/lib/` has 93 `.mbt` files / 46.2k lines; `MB:src/` has 98 files / 47.8k lines including the 5 website files | counted by script |
| Target | `moon build --target js --release`: **pure JavaScript, not WASM** (`"supported-targets": "js"`) | `MB:src/moon.mod.json` |
| Architecture | "fat kernel, thin shell": lexer, parser, interpreter, 87 commands, awk/sed/jq VMs and the in-memory FS in MoonBit; TS wrapper for the just-bash-compatible API | `MB:docs/ARCHITECTURE.md` section 2 |
| Kernel artefact | `src/_build/js/release/build/lib/entry/entry.js` after `moon build` | `MB:docs/ARCHITECTURE.md:613-615` |
| Package build | `pnpm@10.33.0` + `vp pack` (Vite+), with `vite`, `vitest`, `vite-plus` resolved to floating `latest` versions | `MB:package.json:60-70`, `MB:pnpm-workspace.yaml` |
| Declared size | 4.56 MB readable, 1.65 MB minified, 434 KB gzip | `MB:docs/ARCHITECTURE.md` section 6.1 |
| Dependencies | 17 MoonBit registry packages plus npm `pyodide`, `sql.js` | `MB:src/moon.mod.json`, `MB:package.json` |
| Availability | `moon-bash` is **not published on npm** (registry 404 on 2026-11-14); `moon` is not installed here | `npm view moon-bash` |

MoonBash's stated reasons for JS over WASM (`MB:docs/ARCHITECTURE.md` section 6.1): string copies at the
boundary, poor gzip ratio of `.wasm`, and "async instantiate prevents synchronous cold start". Section 2.1
puts numbers on each.

---

## 2. Architecture: a WASI 0.2 component kernel

### 2.1 Cost primitives measured on this machine (core modules; see 2.7 for the component)

Synthetic modules generated with `wat2wasm`. Two kinds of measurement:

Isolated primitives (25 fresh processes, median):

| Primitive | Value |
|---|---:|
| `new WebAssembly.Module` (sync, validation only), 0.9 KB / 394 KB / 956 KB | 0.18 / 1.11 / 2.48 ms |
| Reading the `.wasm` file, 394 KB / 956 KB | 0.11 / 0.21 ms |
| `new WebAssembly.Instance` | 0.06 ms |
| JS -> WASM call, hot monomorphic loop (lower bound) | 1.7 ns |
| JS -> WASM -> JS import -> return, hot loop (lower bound) | 5.3 ns |
| `TextEncoder.encodeInto` / `TextDecoder.decode`, 80 bytes | 39 ns / 56 ns |
| Copy 60 KB into linear memory / decode 60 KB out | 0.57 us / 2.8 us |

End-to-end boot probe (30 fresh processes, interleaved with just-bash): dynamic `import()` of an ESM host
module, `readFileSync` of the `.wasm`, module creation, instantiation, one call that **executes every
function of the module once** (forcing V8's lazy baseline compilation of all of it), 185 host map entries:

| Probe | import of host | boot | total | process wall delta vs empty ESM |
|---|---:|---:|---:|---:|
| 394 KB module, 5 000 functions all executed | 7.1 ms | 14.9 ms | 22.3 ms (p95 24.9) | 28 ms |
| 956 KB module, 12 000 functions all executed | 7.2 ms | 35.3 ms | 42.6 ms (p95 47.0) | 49 ms |
| just-bash 3.6.0, `echo hi` | 65.9 ms | 53.9 ms | 118.8 ms (p95 138.5) | 131 ms |

Reading:
- Validation is cheap (about 2.5 ms per MB) but **executed code pays lazy compilation, about 37 us per KB
  of function bodies** in this probe. Cold start is therefore driven by how much kernel code the first
  `exec` touches, not by the module size: code that is not called costs only validation. A first
  `echo hi` that touches 100-200 KB of kernel code would cost roughly 4-8 ms of compilation, plus about
  7 ms of ESM loading for the host. The probe is an upper bound (it runs 100% of the code); the real
  number is only known after the vertical slice (lot L2).
- Instantiation is negligible (0.06 ms).
- Hot-loop boundary numbers are lower bounds: real `run`/`HOST_CALL` transitions go through promises and
  are not inlined. The design rule is unchanged: cross the boundary per `exec` and per host capability
  call, never per line or per character.
- Real `.wasm` files gzip to 36-49% of their raw size (measured: sql.js 688 KB -> 336 KB; CPython
  5.97 MB -> 2.15 MB). The synthetic modules compress far better and are not representative.

Structural constraint from the just-bash API: `IFileSystem` is asynchronous and custom commands are async
JS functions. A WASM engine cannot `await`. Three ways to bridge:
1. JSPI (WebAssembly JS Promise Integration): behind a flag in node 22, not usable as a baseline.
2. Asyncify-style code transformation: needs binaryen at build time, inflates the binary.
3. **Resumable engine**: the interpreter is an explicit-stack VM whose run export returns a status
   (`DONE`, `OUT_OF_FUEL`, `HOST_CALL`); on `HOST_CALL` the JS host performs the async operation, writes the
   result into linear memory and resumes. No toolchain support needed, and the same mechanism provides fuel
   metering, cooperative time slicing, `AbortSignal` support and immunity to native stack overflow.
Option 3 is the execution model of the kernel; section 2.3 expresses it as WIT resources.

### 2.2 Options considered and owner decision A1

Four options were compared in v1/v2: (a) Rust compiled to WebAssembly with zero crates, (b) hand-written
WAT, (c) AssemblyScript / MoonBit / Zig, (d) a TS shell with WASM kernels for hot builtins only. Rust was
retained by the review: it keeps untrusted data out of the JS heap, needs no new language toolchain, and
compiles natively for tests and debugging. (b) is not reviewable at this size, (c) adds a vendor toolchain,
(d) keeps the JS startup cost and the shared-heap surface.

**Owner decision A1 (binding, `owner-feedback-a1.md`)** refines option (a):

| # | Constraint | Effect on the design |
|---|---|---|
| 1 | Target `wasm32-wasip2`, Component Model, dedicated WIT contract. No `wasm32-unknown-unknown`, no `wasm-bindgen` | The deliverable is a component with typed exports, not a core module with an ad-hoc ABI |
| 2 | Runs natively in wasmCloud on K8s, composable with TS orchestrators through `wac`/WIT, without a JS sub-VM; and in the browser through `jco transpile` at packaging time, producing a wrapper with 0 runtime dependency | Three execution environments from one component (section 2.6) |
| 3 | Rust and `wasm-opt` only in the release pipeline of `@sentropic/bash`; the package ships the prebuilt `.wasm`, the `.wit` and the generated bindings | Consumers need no Rust and no native binary in CI (section 3.3) |
| 4 | Cargo profile first (`opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`); `wasm-opt` only on the core module before componentization, and only with a proven gain >= 10% | Pipeline order of section 2.7 |
| 5 | Only compile-time WIT binding generation (no async runtime, no Tokio); 0 dynamic runtime dependency; 0 npm dependency in the deliverable | Binding generation choice (open question O3); zero-import world (section 2.3) |

### 2.3 Component architecture and WIT contract

**Principle: the kernel component has zero imports.** Capabilities are not WIT imports of the kernel; they
are *requests returned to the driver*. The kernel exports an engine whose `run` returns either a result, an
out-of-fuel status, or a host-call request; the driver (JS host, TS orchestrator, or a Rust driver
component) decides whether to serve it and calls `resume`. Reasons:

1. Default deny at the binary level: a component with no import cannot receive an ambient capability, even
   by a deployment mistake. An import audit (`wasm-tools component wit`) is a release gate.
2. WASI 0.2 functions are synchronous at the component boundary. A synchronous import cannot wait for a JS
   promise without JSPI, and JSPI is not available on this node: `WebAssembly.Suspending` is undefined on
   node 22.22.1 (V8 12.4), with and without `--experimental-wasm-jspi` (verified locally). The reference
   API is asynchronous (`IFileSystem`, custom commands), so inverted control is the only shape that works
   unchanged in Node, browsers and Wasmtime today.
3. `jco` emits imports of `@bytecodealliance/preview2-shim` only for `wasi:*` imports of the component
   (reported by the fact note, consistent with the jco documentation, unverified locally). A zero-import
   world therefore yields generated JS with no external module, which satisfies "0 npm dependency in the
   deliverable". The gate checks the generated files for any bare import specifier.
4. The resumable VM, exec handles and epoch fencing of v2 map one to one onto WIT resources.

Consequence for the Rust source: on `wasm32-wasip2`, `std` links `wasi-libc`, which makes the component
import `wasi:cli`, `wasi:io`, `wasi:filesystem` and `wasi:random` interfaces even when the program does no
I/O (reported by the fact note, consistent with public Rust documentation, unverified locally). To keep
zero imports the kernel is built **`no_std` + `alloc` on wasm** and with `std` on the native target for
tests: `Vec`, `String`, `Box`, `BTreeMap`, `core::fmt` and `try_reserve` remain available; `HashMap` is
replaced by an own table with a fixed seed; the global allocator is our own (a few hundred lines over
`memory.grow`), which is also where the memory ceiling and the byte budgets are enforced.

WIT sketch (illustrative; not validated by a tool, since `wasm-tools` is not installed here):

```wit
package sentropic:bash@0.1.0;

interface types {
  type bytes = list<u8>;                 // never `string`: scripts, stdio and files are byte data
  enum profile { micro, contain, hardened, normal }
  record limits { profile: profile, overrides: list<tuple<string, u64>> }
  flags capabilities { host-fs, network, real-clock, real-sleep, entropy, custom-commands }
  record shell-config { limits: limits, grants: capabilities, commands: list<string>,
                        custom-commands: list<string>, env: list<tuple<bytes, bytes>>, cwd: bytes }
  record exec-opts { script: bytes, stdin: bytes, args: list<bytes>, env: list<tuple<bytes, bytes>>,
                     replace-env: bool, cwd: option<bytes>, raw-script: bool }
  record exec-result { exit-code: s32, env: list<tuple<bytes, bytes>>, limit-hit: option<string>,
                       aborted: bool }
  variant host-req { fs-batch(list<fs-op>), lazy-file(bytes), custom-command(command-call),
                     fetch(fetch-req), sleep(u64), clock, entropy(u32) }
  variant host-outcome { fs(list<fs-result>), data(bytes), command(command-result), denied,
                         failed(bytes) }
  record pending-call { epoch: u64, req: host-req }
  variant step { done(exec-result), out-of-fuel, host-call(pending-call) }
  enum cancel-reason { aborted, deadline, limit }
}

interface engine {
  use types.{bytes, shell-config, exec-opts, step, host-outcome, cancel-reason};
  configure: func(memory-ceiling-bytes: u64);          // once per instance, before any resource
  resource vfs {
    constructor(max-bytes: u64);
    op: func(req: fs-op) -> result<fs-result, fs-error>;        // sync path behind `bash.fs`
    read: func(path: bytes, offset: u64, len: u32) -> result<bytes, fs-error>;
    take-dirty: func() -> list<bytes>;                          // journal and compaction
  }
  resource shell { constructor(root: borrow<vfs>, config: shell-config); }
  resource fiber {
    constructor(owner: borrow<shell>, parent: option<borrow<fiber>>, opts: exec-opts);
    run: func(fuel: u64) -> step;
    resume: func(epoch: u64, outcome: host-outcome) -> step;
    cancel: func(reason: cancel-reason);
    take-stdout: func(max: u32) -> bytes;                       // drained in chunks, never flattened
    take-stderr: func(max: u32) -> bytes;
  }
  exec-oneshot: func(files: list<tuple<bytes, bytes>>, config: shell-config, opts: exec-opts)
      -> oneshot-result;                                        // stateless entry, fails on host-call
}

world kernel { export engine; }                                 // zero imports
```

Mapping of the v2 contract onto WIT:

| v2 concept | WIT form | Notes |
|---|---|---|
| `vfs_id` store | `resource vfs` | Handles are unforgeable and dropped through the canonical ABI; several shells may attach to one `vfs` (the kernel keeps a shared reference behind the borrow) |
| per-`Bash` configuration | `resource shell` | Capabilities, command registry and byte/step budgets are scoped per shell and per fiber, never per instance: two `Bash` objects sharing one `InMemoryFs` share an instance but not their grants or limits |
| `exec_id` fiber | `resource fiber` with `parent` | Concurrent and re-entrant execs are distinct resources over one `vfs`; nested fibers charge the parent's aggregate budget |
| `HOST_CALL` + epoch | `step::host-call(pending-call)` and `resume(epoch, outcome)` | A stale epoch is rejected by the kernel; the driver also drops it. `denied` is the answer for an ungranted capability |
| Output | `take-stdout` / `take-stderr` | Chunked drain avoids a second contiguous copy of the output inside linear memory |
| Limit profile | `limits` in `shell-config`, `configure` for the instance ceiling | The linear memory is defined inside the core module, so its `maximum` is fixed at link time and cannot follow the profile; the ceiling is enforced by the kernel allocator (section 4.3) |

Capabilities as imports, where the platform wants them: a second, optional component, the **session
driver** (Rust, same workspace, `no_std`), imports `sentropic:bash/engine` plus the WASI interfaces it is
allowed to use and exports a simple synchronous shell API. It runs the `run`/`resume` loop natively and
maps `host-req` onto its imports. Driver worlds are fixed capability sets:

```wit
world session-pure { import sentropic:bash/engine@0.1.0; export shell-api; }
world session-fs   { include session-pure; import wasi:filesystem/preopens@0.2.0;
                     import wasi:filesystem/types@0.2.0; }
world session-net  { include session-fs; import wasi:http/outgoing-handler@0.2.0;
                     import wasi:clocks/wall-clock@0.2.0; }
```

Composing `kernel` with one driver through `wac plug` gives a component whose import list *is* its
capability list. The kernel binary is identical in all cases.

WASI 0.2 versus 0.3 (facts and status):

| Fact | Status |
|---|---|
| `wasm32-wasip2` is offered by `rustup target list` on the installed stable 1.96.0; not installed yet | verified locally |
| `wasm32-wasip3` is known to `rustc --print target-list` but not offered by `rustup target list` (no prebuilt std on stable) | verified locally |
| `wasm-component-ld` 0.5.22 ships with the installed toolchain and has `--skip-wit-component`, `--component-type <WIT_FILE>`, `--wasi-adapter none` | verified locally (`--help`) |
| JSPI is not available on node 22.22.1 | verified locally |
| WASI 0.2 exports and imports are synchronous; 0.3 adds `async func`, `stream`, `future` | public specification, not checkable here |
| WASI 0.3 ratification date and the Wasmtime/jco versions supporting it | reported by the fact note, **unverified** |
| A `no_std` `cdylib` links on `wasm32-wasip2` without `wasi-libc` and yields a zero-import component | **unverified**; first gate of lot L0 |

v1 is a WASI 0.2 component with synchronous exports. A later 0.3 version can turn the *driver's* exported
`exec` into an `async func` without touching the kernel world.

### 2.4 Kernel and host contract (to be detailed in the EVOL rung)

```
driver (JS facade over jco output | TS orchestrator | session driver component)
   shell-config, capability membrane, run/resume loop, deadline and abort checks
        |  canonical ABI: typed records, list<u8> payloads, resource handles
kernel component (Rust no_std + alloc, zero imports)
   vfs resources: tree, chunked byte files      shell resources: grants, registry, budgets
   fiber resources: VM stack, shell state, fd table, budget, parent link
   lexer -> parser -> bytecode; expansion; i64 arithmetic; builtins and commands
   regex, glob, format, codecs; own allocator with ceiling and budget accounting
```

1. **Explicit exec handles.** Each exec is a `fiber` resource with its own VM stack, shell state and
   budget. Concurrent `bash.exec()` calls on one `Bash` are several fibers over one `vfs`; they interleave
   only at host-call and fuel-slice boundaries, as in the reference (await points). Re-entrant execution
   (`ctx.exec`, `ctx.origCommand`, `execWithInheritedStdin` from a custom command suspended in a host
   call) creates a child fiber with `parent`, charged to the parent's aggregate budget. In-kernel nesting
   (`bash -c`, `eval`, `xargs`, `find -exec`, `timeout`, `time`) stays inside one fiber.
2. **VFS ownership.** *An exec runs in the instance that owns its root VFS.* `new Bash()` creates a
   dedicated instance with one `vfs`. A standalone `new InMemoryFs()` owns a `vfs` in its own instance,
   whose memory ceiling comes from its own size option or the default profile; a `Bash` constructed with it
   attaches a `shell` to that instance. Any other `fs` (`OverlayFs`, `ReadWriteFs`, custom `IFileSystem`)
   is host-backed and reached through host calls. `MountableFs` keeps its mount table in the kernel.
3. **Public `bash.fs` bridging.** `bash.fs` and `InMemoryFs` are JS objects implementing `IFileSystem`
   over the synchronous `vfs.op` / `vfs.read` exports, returning resolved promises (and the sync
   `mkdirSync`/`writeFileSync`/`writeFileLazy`). Fibers never hold VFS borrows across a suspension point.
   Lazy files are stubs; the first read raises a `lazy-file` host call.
4. **Fallible, budgeted allocation.** No infallible growth on untrusted data: every buffer, string,
   array, VFS write and output append goes through a budget check against the limit that applies, then a
   fallible reservation. A breach unwinds the fiber's explicit stack and returns an `ExecutionLimitError`
   **result** with the reference exit code; the instance and its VFS stay valid. Infallible
   `push`/`format!`/`collect`/`to_string` on untrusted-sized data are banned by a source check in the gate.
   Files, stdout/stderr and pipeline buffers use chunked storage: no doubling copy, peak = size + one
   chunk. Canonical-ABI arguments and results are allocated through the same allocator (`cabi_realloc`)
   and are charged to the fiber.
5. **Memory ceiling.** Enforced by the kernel allocator, which is the only caller of `memory.grow`: the
   instance ceiling is set once by `configure`, and the byte budgets of point 4 are sized so that they
   always bind before it (section 4.3). The link-time `maximum` of the memory is a fixed backstop, and the
   platform limiter (wasmCloud, Wasmtime) is a third, independent line.
6. **Traps are bugs, with a defined recovery contract.** With points 4 and 5 no untrusted input is
   supposed to trap. After a trap, linear memory is not trusted: the exec returns a fatal result, the
   instance is **poisoned** (further calls reject with a typed error), every pending fiber of that
   instance fails, and VFS content is not salvaged. Callers who need committed files to survive opt into
   a **journal**: at each exec completion the driver mirrors the files reported by `take-dirty` and can
   rebuild a fresh instance from the mirror. No per-exec fresh instance is offered.
7. **Revocable capability membrane with epochs.** The driver hands async extensions revocable proxies,
   never raw objects. On abort, deadline or limit breach it calls `fiber.cancel`, revokes the fiber's
   proxies, and drops any late `resume` whose epoch is no longer current; the kernel rejects it as well.
   An extension that does not settle within `maxExtensionCleanupTimeMs` is abandoned.
8. **Memory views and retention.** Views over linear memory are owned by the generated bindings, which
   must refresh them after any growth (a release-gate test grows memory mid-call). Linear memory never
   shrinks: the driver offers `bash.compact()` (and an optional idle threshold) that rebuilds a fresh
   instance from the VFS, using the journal path.
9. **Batched host calls.** `fs-batch` carries several operations (directory listing with entry types and
   stats, multi-stat, bounded tree walk), so `find`, `grep -r`, `du`, `cp -r` or `**` cost one driver round
   trip per directory, not per entry.
10. **Bytes at the boundary.** Every payload is `list<u8>`; a WIT `string` is only used for identifiers
    (limit names, command names). Output conversion in the JS facade follows the reference contract: valid
    UTF-8 decoded, invalid bytes preserved as latin1, raw bytes available. `normalizeScript` runs in the
    kernel unless `raw-script` is set.
11. **Stateless entry.** `exec-oneshot` runs one script over a file set and returns the result plus the
    modified files. It needs no resource handle across calls, so it is callable over a wasmCloud link or
    wRPC; it fails with a typed error if the script needs a host call.

### 2.5 Execution environments

| Environment | How the kernel is loaded | Driver | Status |
|---|---|---|---|
| Node >= 22 | `jco transpile` output generated at packaging time: one core module (`kernel.core.wasm`) + generated JS glue + `.d.ts`. The core module is read with `readFileSync(new URL(...))`, compiled synchronously once per process, and passed to the generated `instantiate` with an empty import object | Hand-written JS facade (`Bash`, `InMemoryFs`, `OverlayFs`, `ReadWriteFs`, membrane), no dependency | Synchronous instantiation mode of jco output is **unverified** here (jco not installed); gate in L0 |
| Browser | Same generated files; the core module is compiled with `WebAssembly.compileStreaming` and awaited inside the first `exec` (already async) or through an explicit `Bash.precompile()` | Same facade without the host-FS adapters | In v1 by owner decision. Needs a headless browser as a test-only tool (batch B, B9) |
| wasmCloud on K8s | The component itself (`kernel.component.wasm`), from the npm package or an OCI registry | A TS orchestrator component composed with the kernel through `wac plug` (it drives `run`/`resume` and serves host calls with its own imports), or the Rust session driver for a fixed capability set, or `exec-oneshot` over a link | Platform facts below are **unverified** locally |
| Wasmtime (test) | The component, run natively in the repo test suite as the proxy for wasmCloud | Session driver | Needs `wasmtime` as a test-only tool (section 2.7) |

wasmCloud facts used by the design, all reported by the fact note and not checkable on this machine (no
`wash`, no `wasmtime`); the L1 smoke test must confirm them against the wasmCloud version the owner runs:
- A WASI 0.2 component with a custom WIT world is accepted; `wasi:http` is not required.
- Host capabilities reach a component only through its imports and the links declared for it. With zero
  imports the kernel cannot receive any.
- Components are instantiated per invocation, so no in-memory state survives between invocations. A
  multi-turn session with a persistent VFS therefore needs either one composed component whose
  orchestrator keeps the session inside a single invocation, a host-managed stateful service, or the
  stateless `exec-oneshot` with files passed in and out. This is open question O4.
- Default per-component limits: 256 MiB of linear memory, 10 minutes of execution time, enforced by
  Wasmtime's limiter and epoch interruption.
- In a `wac` composition both components run in one Wasmtime store with separate linear memories; the
  canonical ABI copies bytes from one memory to the other, with no JS engine around the kernel.

The owner's requirement "no JS sub-VM" is met for the kernel in every case. A TS orchestrator is itself
JavaScript compiled to a component; it is the orchestrator's engine, not a wrapper around the kernel.

### 2.6 Build pipeline and toolchain (release lane only)

Verified locally: `rustc`/`cargo` 1.96.0 (LLVM 22.1.2), `wasm-component-ld` 0.5.22, `wasm-ld`, node
22.22.1, `wat2wasm` 1.0.36. Absent: `wasm-tools`, `wit-bindgen`, `jco`, `wac`, `wasm-opt`, `wasmtime`,
`wash`. Nothing was installed for this study.

Commands lot L0 would run (none executed here):

```
# 1. toolchain pin (rust-toolchain.toml: channel = "1.96.0", targets = ["wasm32-wasip2"], profile = "minimal")
rustup target add wasm32-wasip2
# 2. core module: no_std + alloc, release profile (opt-level "z", lto "fat", codegen-units 1,
#    panic "abort", strip true), linker told not to componentize and not to link wasi-libc
cargo build --release --target wasm32-wasip2        # with -C link-arg=--skip-wit-component
# 3. optional, only if lot L2 proved a gain >= 10%
wasm-opt -Oz kernel.core.wasm -o kernel.core.opt.wasm
# 4. componentization from the WIT contract, no adapter
wasm-tools component embed wit/ --world kernel kernel.core[.opt].wasm -o kernel.embedded.wasm
wasm-tools component new kernel.embedded.wasm -o dist/kernel.component.wasm
# 5. gates
wasm-tools validate dist/kernel.component.wasm
wasm-tools component wit dist/kernel.component.wasm    # must list zero imports
# 6. JS bindings for Node and browser
jco transpile dist/kernel.component.wasm -o dist/js    # sync instantiation, no WASI shim, wasm kept as a file
#    gate: no bare import specifier in dist/js/*.js; exactly one core module emitted
# 7. reproducibility: two clean builds must be byte-identical; SHA-256 recorded in the release
```

Development builds can skip steps 3-5 and let the bundled linker componentize directly
(`--component-type wit/world.wit`, `--wasi-adapter none`), which needs `rustup` only; whether that path
produces the same zero-import component is to be confirmed in L0.

Third-party build-time tools. None is a dependency of the published package; none is needed by consumers.

| Tool (version seen) | Kind | Role | Pin method | Avoidable? | Supply-chain note |
|---|---|---|---|---|---|
| `rustc`/`cargo` 1.96.0 + `rust-std` for `wasm32-wasip2` (includes `wasm-component-ld` 0.5.22) | Official native toolchain | Compile the kernel | `rust-toolchain.toml` with an exact channel; rustup verifies its manifests | No | Already on the machine; one extra official component |
| `wit-bindgen-cli` 0.63.0 (crates.io) | Native binary | Generate the Rust export glue from the WIT, as a committed source file | Release tarball + recorded SHA-256, under git-ignored `tmp/tools/`; run only when the WIT changes | Yes: hand-written canonical ABI glue (open question O3) | Bytecode Alliance. Used as a generator, it adds no crate to `Cargo.lock`; the proc-macro crate would add a compile-time crate tree |
| `wasm-tools` 1.263.0 (crates.io) | Native binary | `component embed/new`, validation, import audit | Release tarball + SHA-256, `tmp/tools/` | Partly: the bundled linker can componentize, but the audit gate and the pre-component `wasm-opt` order need it | Bytecode Alliance; same `wit-component` code as the linker already shipped inside rustc |
| `@bytecodealliance/jco` 1.42.0 or `@bytecodealliance/jco-transpile` 0.24.0 (npm) | npm tool | Generate the JS glue and `.d.ts` | Dedicated manifest with `devDependencies` only and a committed lockfile under `tools/`, installed with `npm ci --ignore-scripts` into `tmp/tools/`; never in the root `package.json` | No (owner constraint 2) | Largest surface. `jco` 1.42.0 declares 14 dependencies (among them `rolldown`, `typescript`, `binaryen`, `componentize-js`, `preview2-shim`); `jco-transpile` declares 4 (`binaryen`, `oxc-minify`, `preview2-shim`, `preview3-shim`) (verified with `npm view`). L0 checks whether `jco-transpile` alone is enough |
| `wasm-opt` (Binaryen release) | Native binary | Size pass on the core module | Release tarball + SHA-256, `tmp/tools/` | Yes: used only if L2 proves >= 10% | Prebuilt binary only; Binaryen is never built from source here |
| `wac-cli` 0.12.0 (crates.io) | Native binary | Composition smoke test (kernel + driver); used by consumers for their own compositions | Release tarball + SHA-256, `tmp/tools/` | Yes for publishing; needed for the composition test | Bytecode Alliance |
| `wasmtime` | Native binary | Run the component natively in tests (proxy for wasmCloud) | Release tarball + SHA-256, `tmp/tools/` | Yes if the owner provides a wasmCloud test host instead | Bytecode Alliance; test-only |
| `typescript` | npm tool | `tsc --noEmit` on the hand-written facade declarations and on consumer fixtures | Same `tools/` manifest and lockfile as jco | No, if declarations are to be checked | Test-only; fails closed when absent |
| `moon` | Vendor toolchain | Build MoonBash for the benchmark | `tmp/bench-tools/`, version and artefact hash recorded | Open question O6 | Bench-only |
| `cargo-fuzz` + nightly Rust | Dev crate | Coverage-guided fuzz campaign | `tmp/fuzz/`, out of every manifest | Open question O8 | Fuzz-only, never in the gate |

Version numbers come from `cargo search` and `npm view` on 2026-11-15. The fact note quotes older ones
(`jco` 1.33.x, `wac` 0.11.0, Binaryen 120-128); the registry values above supersede them.

### 2.7 Startup and size under the component model (re-estimate)

Nothing component-specific could be measured: the target and the tools are not installed. What changes
against section 2.1:

| Item | Expectation | Status |
|---|---|---|
| Node/browser load path | `jco` unbundles the component into its core module plus JS glue, so V8 compiles the same core module as in the section 2.1 probe; the component wrapper is not loaded at runtime | reasoning from the jco design, **unverified** |
| Glue cost | Generated JS for a zero-import world with a handful of resources: a few to a few tens of KB, plus resource tables | **unverified**; within the 60 KB host budget only if the facade stays small |
| Call cost | Every `run`/`resume` lowers and lifts a variant; payloads are copied once per direction (0.57 us per 60 KB measured for the raw copy). Records with many small `list<u8>` fields (env) cost one allocation each | **unverified** |
| Kernel size | `no_std` removes `std` I/O, panic formatting and the WASI adapter; the canonical-ABI export glue and an own allocator are added | **unverified**; budget unchanged (<= 1.5 MB raw, <= 700 KB gzip) |
| Package size | The npm package carries two copies of the kernel code: the component (for `wac`/wasmCloud) and the core module extracted by jco (for JS) | certain by construction; open question O2 |
| Wasmtime path | Whole-module compilation at component start (cached by the host), then cheap per-invocation instantiation | **unverified** |

Lot L2 must measure, before any budget becomes a gate: size of the `no_std` core module with the Cargo
profile alone, gain of `wasm-opt -Oz` on it (the >= 10% decision), size of the component versus the core
module, size and initialization time of the jco glue, cold start of `import + new Bash() + exec('echo hi')`
through the generated bindings, per-call cost of `run`/`resume` with a host call, and, with `wasmtime`,
instantiation and first-exec time of the composed component.

---

## 3. Packaging

### 3.1 Core versus companion packages (A4 answered; single binary in batch B, B7)

Constraint: a second `.wasm` module carries its own allocator and cannot reach the core VFS without copies
or host calls, and unused code in a single module costs only validation (2.5 ms/MB). So the proposal is
**one kernel binary built from one Rust workspace**, and companion packages that are JS facades over it.

| Package | Content | Binary |
|---|---|---|
| `@sentropic/bash` (core) | JS host + kernel. Parser, interpreter, all shell builtins of section 1.3, VFS, limits, regex/glob engines. Commands in three delivery tiers: **tier 1** `echo printf cat head tail wc tee true false pwd ls mkdir rmdir rm cp mv ln touch chmod stat readlink mktemp basename dirname env printenv which whoami hostname seq sleep date test expr sort uniq cut tr paste rev tac nl grep egrep fgrep sed find xargs base64 yes bash sh time timeout alias unalias history help clear`; **tier 2** `awk jq`; **tier 3** `diff comm join column fold expand unexpand split strings od md5sum sha1sum sha256sum file tree du gzip gunzip zcat tar rg yq` | the kernel `.wasm` |
| `@sentropic/grep` | Standalone JS API (search a buffer or a VFS with the core regex engine) | none: wraps kernel exports |
| `@sentropic/ls` | Standalone listing/formatting API over a VFS | none: wraps kernel exports |
| `@sentropic/awk`, `@sentropic/jq` | Standalone JS APIs (run a program over a buffer) | none: wraps kernel exports |
| `@sentropic/net` | `curl` + the SSRF-safe fetch adapter (host-side JS, node built-ins only) | kernel side of `curl` is in the kernel; the adapter is JS |
| `@sentropic/make` | make (not in just-bash; owner-named), after parity | in the kernel workspace, feature-gated |
| Not planned for v1 | `python3`/`python`, `js-exec`/`node`, `sqlite3`, `xan`, `html-to-markdown`, `@just-bash/executor` | each needs a large third-party runtime or library; left to custom commands |

All 85 `CommandName` entries are accounted for: 82 in the kernel tiers and `xan`, `sqlite3`,
`html-to-markdown` not planned; of the opt-in names, `curl` is in `@sentropic/net` and `python*`/`js-exec`/
`node` are not planned. `yq` is in tier 3; its input formats are open question O5. A `slim` build (tier 1 only)
can be produced from the same workspace if the size budget requires it.

### 3.2 API compatibility surface with just-bash

| Tier | Surface | v1 stance |
|---|---|---|
| A — drop-in | `new Bash({files, env, cwd, fs, executionLimits, executionLimitProfile, commands, customCommands, sleep, network/fetch, processInfo})`; `bash.fs`; `exec(cmd, {env, replaceEnv, cwd, stdin, stdinKind, signal, args, rawScript})` with the non-throwing result contract, `normalizeScript` and the binary-preserving output decoding; `readFile`, `writeFile`, `getCwd`, `getEnv`, `registerCommand`, `defineCommand` and the full command context (`exec`, `origCommand`, `exportedEnv`, `assignShellVariable`, `fileDescriptors`, `getRegisteredCommands`); `InMemoryFs` with the `IFileSystem` contract incl. `createExclusive` and stat identity; default layout (`/home/user`, `/bin`, `/usr/bin`, `/dev`, `/proc`, default env); command-list helpers; byte/text helpers | Same names and semantics; proven by our own API tests |
| B — compatible behaviour, own implementation | `OverlayFs`, `ReadWriteFs`, `MountableFs`, `NetworkConfig` allow-list and typed errors, limit names and profiles, ESM and CJS entries, `./browser` entry (in v1 by decision A1) | Same options; host-side JS adapters behind explicit capabilities |
| C — later | `parse()` + AST types, transform pipeline and plugins, `Sandbox` facade, CLI binaries, `trace`, `logger`, `coverage` | After parity of tiers A and B |
| D — not reproduced | `DefenseInDepthBox` and related exports, `python`, `javascript`, sqlite3, `@just-bash/executor` | Documented as N-A |
| Additions | `execSync` (B5), `Bash.precompile()`, `bash.compact()`, journal option (B6), profiles `contain` and `micro`, the component and its WIT for `wac`/wasmCloud consumers | Opt-in, never required for drop-in use |

### 3.3 Published artefacts (decision A1, constraint 3)

| Artefact | Produced by | Consumer |
|---|---|---|
| `kernel.component.wasm` | release pipeline (section 2.6) | `wac` compositions, wasmCloud, Wasmtime |
| `wit/` (`sentropic:bash@0.1.0`) | hand-written, versioned | everyone who composes or generates bindings |
| `js/kernel.core.wasm`, `js/kernel.js`, `js/*.d.ts` | `jco transpile` at packaging time | Node and browser, through the facade |
| Facade (`Bash`, filesystems, membrane) + `.d.ts` | hand-written JS, no dependency | Node (`.`) and browser (`./browser`) entries |
| Session driver component(s) | release pipeline | wasmCloud deployments wanting a fixed capability set |

The published `package.json` has empty `dependencies`. Nothing in the package needs Rust, a native binary
or an install script. `*.wasm` is git-ignored in the repository, so consumers depend on published
packages, not on git refs. Whether the component also lives in the npm tarball is open question O2.

---

## 4. Security model draft

### 4.1 Principles

1. Default deny. A `Bash` created without options can compute and use its in-memory VFS; it cannot read a
   host file, open a socket, spawn a process, read the host clock, timezone or environment.
2. Capabilities are explicit objects passed at construction, each mapped to a `host_call` operation code.
   An operation without a granted capability fails inside the sandbox and never reaches host code.
3. The kernel is the only code that parses untrusted bytes. The JS host validates lengths, operation codes
   and the fiber handle and epoch of each resume; it never evaluates, never builds property names from sandbox data, and
   never passes sandbox strings to `import()`, `require`, `Function` or `RegExp`.
4. Limits return results, not traps (section 2.4, points 4-5). A trap is a kernel bug handled by the
   poison contract (point 6).
5. Async extensions only see revocable, epoch-fenced proxies (point 7).

### 4.2 Capability table (default: all denied)

| Capability | Grants | Default | Notes |
|---|---|---|---|
| `fs: memory` | In-kernel VFS seeded by `files` | granted | Size-capped (`maxFileSystemBytes`) |
| `fs: host-read` (`OverlayFs`) | Read under a root, writes stay in memory | denied | Root containment on canonical paths; symlinks denied unless opted in; null bytes rejected; **only regular files and directories are opened** (FIFOs, sockets and device nodes rejected after `lstat`, opened with no-follow semantics); host paths stripped from errors |
| `fs: host-write` (`ReadWriteFs`) | Read/write under a root | denied | Same gates, plus atomic exclusive create (`createExclusive`, mode set at creation), broken-symlink leaf check, hard-link copy-on-write, serialized mutations on overlapping roots |
| `fs: custom` (`IFileSystem`) | Caller-provided async FS | denied | Reached through the membrane; results copied into linear memory |
| `network` | `fetch` through an allow-list | denied | URL-prefix and method allow-lists, each redirect re-validated, size cap. Own SSRF defence with node built-ins: resolve with `node:dns`, validate **the resolved addresses** against private/loopback/link-local/metadata ranges (including IPv4-mapped IPv6 and non-canonical IP literals), connect to the validated address (pinned lookup) so a second resolution cannot differ, strip credentials on cross-origin redirects. `curl` is not registered without the capability |
| `process` | Nothing in v1 | denied, not grantable | `bash`/`sh` are nested interpreters |
| `time: clock` | Wall clock for `date`, `$SECONDS`, `$EPOCHSECONDS` | virtual clock; timezone UTC unless `$TZ` is set in the sandbox env | Real clock is opt-in; host timezone is never read |
| `time: sleep` | `sleep`, `timeout` | virtual (advances the virtual clock) | Real sleep via the `sleep` hook |
| `random` | `$RANDOM`, `mktemp`, `shuf` | deterministic seeded PRNG | Host entropy opt-in |
| `env` | Host environment | denied | Only the `env` option is visible; virtual pid/uid/hostname |
| `custom commands` | Caller JS functions | per command | See below |

Custom command trust without `DefenseInDepthBox`: a command with `trusted: false` receives a copied,
read-only view of the environment, revocable proxies for `fs`/`exec`/`fetch`, output and work-unit
accounting against the calling fiber's budget, the cleanup deadline, and sanitized error messages. A
`trusted: true` command gets the same proxies without the read-only restriction. Neither can reach kernel
memory. What the membrane cannot do, and the documentation must say so, is stop a custom command from using
ambient Node APIs it closes over: custom commands are host code supplied by the embedder.

### 4.3 Resource limits

Limit names follow the reference (`JB:src/limits.ts`, table in 1.2) so existing callers keep working.
Profiles are redefined so that byte caps are coherent with one memory ceiling: in every profile
`VFS + live + output + kernel headroom = ceiling`, so a byte budget always binds before the allocator
ceiling, and the ceiling of the default profile fits wasmCloud's default per-component cap.

| Profile | VFS | Live / input | Output / string | Commands / loop / work units | Wall time | Headroom | Ceiling |
|---|---:|---:|---:|---:|---:|---:|---:|
| `micro` | 32 MiB | 16 / 16 MiB | 8 / 4 MiB | 25k / 25k / 1M | 10 s | 8 MiB | 64 MiB |
| `contain` | 128 MiB | 64 / 64 MiB | 32 / 16 MiB | 100k / 100k / 100M | 60 s | 32 MiB | 256 MiB |
| `hardened` | 128 MiB | 64 / 32 MiB | 10 / 10 MiB | 10k / 10k / 100k | 30 s | 54 MiB | 256 MiB |
| `normal` | 1 024 MiB | 512 / 512 MiB | 256 / 64 MiB | 100k / 100k / 100M | 1 h | 64 MiB | 1 856 MiB |

`hardened` and `normal` keep the reference values; `contain` and `micro` are additions. Explicit
`executionLimits` overrides are validated against the ceiling at construction (a set of byte caps that
does not fit is rejected, never silently clamped). The headroom figures (code, AST, allocator
fragmentation) are estimates to validate in L2. Which profile is the default is open question O1.

CPU and time: kernel fuel is the deterministic limit in every environment. Wall time is checked by the
driver between fuel slices; a synchronous driver without a clock capability (session driver, `execSync`)
relies on fuel and on the platform deadline (wasmCloud: 10 minutes by default, unverified locally).

Enforcement points in our engine:

| Limit family | Enforcement point |
|---|---|
| Steps: `maxCommandCount`, `maxLoopIterations`, awk/sed/jq iteration limits | Fuel decremented in the VM dispatch loop and in every builtin loop |
| Aggregate work: `maxWorkUnits`, `maxExecDepth` | Budget shared by a fiber and all its descendants (nested `bash -c`, `eval`, `$(...)`, `ctx.exec`), so nesting cannot multiply the per-exec counters |
| Wall time: `maxExecutionTimeMs`, `maxExtensionCleanupTimeMs` | Checked by the host at each fuel slice and around each host call |
| Bytes: `maxStringLength`, `maxHeredocSize`, `maxOutputSize`, `maxLiveBytes`, `maxInputBytes`, `maxFileSystemBytes` | Budget charge before every growth (section 2.4, point 4) |
| Memory ceiling | Kernel allocator ceiling set by `configure` (section 2.4, point 5); link-time `maximum` and the platform limiter as backstops |
| Recursion: `maxCallDepth`, `maxSubstitutionDepth`, `maxSourceDepth`, parser depth 200 | Explicit-stack depth counters |
| Input: `maxSourceBytes`, token count | Before parsing. The reference effectively caps scripts at 1 000 000 chars; we enforce `maxSourceBytes` as documented and record the difference in the divergence list |
| Expansion: brace results, `maxGlobOperations`, globstar segment count (reference: 5), `maxArrayElements` | Expansion engine |
| File descriptors: `maxFileDescriptors` | fd table |
| VFS traversal: `maxTraversalEntries`, `maxTraversalDepth`, `maxTraversalWork` | VFS walkers, including batched host walks |
| Archives: `maxArchiveBytes`, `maxArchiveEntryBytes`, `maxArchiveEntries`, `maxArchiveCompressedBytes`, plus an expansion-ratio ceiling | inflate and tar readers |
| Regex | Linear-time matcher by construction; **compile-time caps** on program size (NFA instructions after expansion of counted repetitions and classes) and on lazy-DFA cache bytes; backreferences through a bounded backtracker charged in fuel; no fallback to JS `RegExp` |

### 4.4 Fuzz and adversarial strategy

| Technique | Implementation |
|---|---|
| Grammar-based generation | Own JS generator with a seeded PRNG; every failure is reproducible from a printed seed |
| Mutation fuzzing | Byte-level mutations of our own corpus, run **only against our engine** (`parse`, `exec`) with tight limits |
| Oracles on our engine | No trap and no host exception; termination within fuel and memory limits; no host capability call when none is granted; determinism for a given seed; no stale resume accepted after revocation |
| Differential oracle against the system bash | **Never fed fuzzed or mutated scripts.** Two inputs only: (1) recorded fixtures: hand-written, reviewed scripts run once by a recorder and replayed from committed expected outputs; (2) a generated subset built from an AST generator that can only emit shell builtins, arithmetic, expansions and control flow, with **no external command, no path-bearing redirection outside the work directory, no `eval`/`source` of generated strings, no background jobs**. Execution is confined: fresh empty directory under `TMPDIR` (never `/tmp`), `env -i`, empty `PATH`, `bash --norc --noprofile` (restricted mode `-r` only for generated scripts without file redirections, since `-r` rejects them), `ulimit` caps on CPU time, file size and processes, a wall-clock timeout, and a user/network namespace (`unshare -Urn`) when the host allows it. The generator's safety property (no external command reachable) is itself unit-tested |
| Adversarial suites | Own cases per threat class of section 1.5: parser bombs, expansion bombs, path traversal, symlink and special-file escapes on the host FS adapters, redirect chains and DNS rebinding on the network adapter, output floods, deep recursion, regex blowups (matching and compilation), NUL bytes and invalid UTF-8, keys such as `__proto__`, archive bombs, late async continuations after abort |
| Native-side fuzzing | Kernel compiled natively and driven by the seeded generator in a `cargo test` binary with debug assertions and overflow checks |
| Coverage-guided fuzzing | Valuable for parser, expansion, regex compiler, codecs and path canonicalization. The usual tool (`cargo-fuzz`/libFuzzer) is a third-party dev crate on a nightly toolchain: it is never part of `make check`. Whether a campaign run from git-ignored `tmp/fuzz/` is required in lot L11 before 1.0 is open question O8 |
| Regression corpus | Every failing seed becomes a named test under `test/security/` |
| Budget | A short deterministic slice in `make check`; long runs via `FUZZ_RUNS` outside the gate |

---

## 5. Benchmark protocol draft

### 5.1 Metrics

| Id | Metric | Definition |
|---|---|---|
| M1 | Cold start | Fresh node process: `import` + construct + first `exec('echo hi')`, in-process timing, plus process wall time minus an **empty ESM script** baseline |
| M2 | Warm construct | `new Bash()` in a warm process |
| M3 | Warm exec | Fixed script set (`:`, `echo hi`, a 1 000-iteration loop, a 20-line agent-style script) |
| M4 | Per-builtin microbench | One script per command over fixed inputs (1 KB, 250 KB, 10 MB), in ms and MB/s |
| M5 | Memory | RSS and `heapUsed` deltas after M1 and after a 10 MB workload; linear-memory high-water mark; **retention**: RSS after 100 execs and after a 50 MB transient pipeline has completed |
| M6 | Size | Published file sizes: JS host, `.wasm` raw and gzip -9, install footprint |
| M7 | Limit latency | Time for a script that hits a limit to be stopped |
| M8 | Host-backed FS | `find`, `grep -r`, `cat` over an `OverlayFs` mount of a 1 000-file tree, and one async custom command in a loop: measures the `HOST_CALL` path |
| M9 | Agent session | 25 sequential `exec` calls over a 1 000-file tree (navigation, `grep`, `sed -i`, loop, pipeline) |
| M10 | Heavy pipeline | `cat 10MB | awk … | sort | head`, `jq` over a 10 MB document; first-use latency of `awk`/`jq` |

### 5.2 Harness (node built-ins only)

- `bench/run.mjs` (the `make bench` target already expects it): `node:child_process` for cold runs,
  `performance.now()` / `process.hrtime.bigint()`, `node:fs`, `node:zlib`, `node:os`.
- One adapter module per engine (`bench/engines/<name>.mjs`); the workload list is shared.
- Every engine runs with `LC_ALL=C` and `TZ=UTC`; the system bash runs with `--norc --noprofile`.
- Output: JSON lines under git-ignored `tmp/bench-results/` plus a Markdown table; the JSON carries node
  version, CPU model, engine versions, artefact hashes and commit sha.
- Correctness before speed: each workload has a committed expected output (the GNU bash result). An
  engine's timing is reported only if it produces that output; otherwise the cell reads `divergent`.
  Workloads are chosen in the subset where just-bash agrees with GNU bash, so the comparison stays fair
  under B1.

### 5.3 Reference engines (bench only, never a repo dependency)

| Engine | Install | Location |
|---|---|---|
| just-bash | `npm install --ignore-scripts --no-audit --no-fund just-bash@<pinned>` with a local `--cache` | `tmp/bench-engines/just-bash/` (git-ignored) |
| MoonBash | Mandatory per `rules/MASTER.md`. Not on npm, so it is built once from the reference clone: `moon` toolchain installed under `tmp/bench-tools/`, `moon build --target js --release` (fetches 17 registry packages), kernel artefact `entry.js` wrapped by a small local adapter; commit and artefact SHA-256 recorded. The full package build (pnpm + floating Vite+ versions) is avoided. See O6 | `tmp/bench-engines/moonbash/` |
| System bash | `/bin/bash --norc --noprofile -c`, external reference for M3/M4 (process spawn included, labelled) | n/a |

`make check-no-deps` keeps guarding the manifests: bench engines never appear in any `package.json`.

### 5.4 Statistics

- Cold metrics: 30 fresh processes per engine, **interleaved** across engines and the baseline.
- Warm metrics: 20 warm-up iterations discarded, then at least 200 iterations or 2 s.
- Quantiles: median = mean of the two central values for even N; p95 = nearest rank
  (`sorted[ceil(0.95 N) - 1]`). Report median, p95, min.
- A difference between two engines is reported only when it exceeds the larger p95-to-median spread;
  otherwise "within noise".
- Regression gate against a committed `bench/baseline.json` from the same machine class: a gated metric
  fails only if the regression exceeds **both** `max(10%, absolute floor)` and the interleaved p95 spread
  of the baseline. Absolute floors: 0.02 ms for sub-millisecond metrics, 0.5 ms for the others, 1 MB for
  memory.

### 5.5 Performance budgets (A2: accepted as targets, confirmed after lot L2)

Baselines are the section 1.7 measurements of just-bash 3.6.0 on this machine. All budgets are targets to
confirm with the vertical slice (lot L2); none is a commitment before that measurement.

| Metric | just-bash (measured) | Proposed v1 budget | Ratio |
|---|---:|---:|---:|
| M1 cold start (import + ctor + first exec) | 118.8 ms | <= 25 ms (stretch 15 ms) | >= 4.7x |
| M2 warm `new Bash()` | 0.37 ms | <= 0.20 ms | >= 1.8x |
| M3 `echo hi` | 0.35 ms | <= 0.07 ms | >= 5x |
| M3 1 000-iteration loop | 7.4 ms | <= 1.5 ms | >= 5x |
| M4 `grep -c` on 10k lines | 11.0 ms | <= 2 ms | >= 5x |
| M4 `sed s///p` on 10k lines | 73.7 ms | <= 8 ms | >= 9x |
| M4 `awk` sum on 10k lines | 61.0 ms | <= 8 ms | >= 7x |
| M4 `sort -r` on 10k lines | 9.2 ms | <= 4 ms | >= 2x |
| M5 RSS delta after first exec | 35.5 MB | <= 10 MB | >= 3.5x |
| M6 shipped code | 2.02 MB JS (557 KB gzip), 102 MB installed, 73 packages | kernel `.wasm` <= 1.5 MB raw and <= 700 KB gzip for all tiers; tier-1 `slim` build <= 700 KB raw and <= 350 KB gzip; JS host <= 60 KB; 0 dependency | n/a |
| M8-M10 | not measured yet | "not slower than just-bash" until measured, then set | n/a |

The gzip budgets assume the measured 36-49% ratio of real `.wasm` files. The cold-start budget assumes a
first `exec` touching at most about 300 KB of kernel code. The M6 kernel budget applies to the core module
loaded by JS; the component file is a second copy of the same code (section 2.7, O2). The jco glue counts
in the 60 KB host budget.

---

## 6. Owner decisions and open questions

### 6.1 Answered

| Id | Question | Owner decision | Where applied |
|---|---|---|---|
| A1 (Q1) | WASM toolchain | **ANSWERED.** Rust to `wasm32-wasip2`, Component Model, dedicated WIT; no `wasm32-unknown-unknown`, no `wasm-bindgen`; runs in wasmCloud/K8s (composable through `wac`) and in the browser (`jco transpile` at packaging); Rust and `wasm-opt` only in the release pipeline; Cargo profile first, `wasm-opt` only on the core module before componentization and only with a proven gain >= 10%; compile-time WIT bindings only; 0 runtime dependency, 0 npm dependency in the deliverable | Sections 2.2-2.7, 3.3 |
| A2 (Q2) | Performance budgets | **ANSWERED.** Accepted as targets, confirmed after lot L2 | Section 5.5 |
| A4 (Q4) | Scope of v1 and delivery order | **ANSWERED.** No owner preference: the staged recommendation applies (tier 1, then `awk`/`jq`, then tier 3; python3, js-exec, sqlite3, xan, html-to-markdown out of v1). The `yq` format scope is carved out as O5 | Section 3.1 |
| Q5 | Shipping the `.wasm` | **ANSWERED by A1.** Prebuilt artefacts in the published package; never built by consumers; `*.wasm` stays git-ignored, so consumers use published packages, not git refs. Remaining choice: O2 | Section 3.3 |
| Q11 | Target runtimes | **ANSWERED by A1.** Node, browser and wasmCloud in v1 | Section 2.5 |
| — | Repository and publication | **ANSWERED.** Public GitHub repository; npm pre-release (0.x, `next` tag) once the core passes its parity tests | `BRANCH.md` |
| — | Model mapping per lot | **ANSWERED.** Owned by the design lane (Fable 5.2) and the review lane (Carbon), accountable for spend and velocity | Section 7 |

### 6.2 Open questions

**O1 — Default limit profile (gate A3, redone for the component architecture).**
Context: linear memory never shrinks; wasmCloud's default per-component cap is 256 MiB (unverified
locally); the reference default (`normal`) allows 1 GiB of VFS and needs a 1 856 MiB ceiling; the
reference `hardened` profile stops loops at 10 000 iterations. Section 4.3 defines four profiles in which
byte caps always sum to the ceiling.
Options: (a) `contain` by default everywhere (256 MiB ceiling, reference step limits of `normal`, 60 s);
(b) `hardened` by default (256 MiB ceiling, 10 000 commands and loop iterations, 30 s); (c) `normal` by
default as in the reference (1 856 MiB ceiling; wasmCloud hosts must raise their cap or select a profile);
(d) `normal` in the Node/browser facade and `contain` in the session driver.
Recommendation: (a).
Consequence: one behaviour in every environment and a default that fits an unmodified wasmCloud host.
Scripts that need more than 128 MiB of files or 32 MiB of output get a limit error until the embedder
selects `normal`; this is a documented divergence from the just-bash default.

**O2 — Artefact split between npm and OCI.**
Context: JS consumers need the jco output (core module + glue + types); `wac`/wasmCloud consumers need the
component. Shipping both in one tarball roughly doubles the WASM bytes on disk (only one is loaded).
Options: (a) one npm package with the component, the `.wit` and the jco output, plus the same component
pushed to an OCI registry for wasmCloud deployments; (b) npm carries only the jco output and the `.wit`,
the component is published to OCI and GitHub Releases only; (c) two npm packages (`@sentropic/bash` for JS,
`@sentropic/bash-component` for the component).
Recommendation: (a), revisited if the tarball exceeds 2 MB compressed.
Consequence: TS repositories get everything from npm with no extra tool; the OCI digest and the npm
component file must be the same bytes (checked at release).

**O3 — Generation of the Rust WIT bindings.**
Context: A1 allows compile-time binding generation only. The `wit-bindgen` crate is a proc-macro with its
own compile-time crate tree in `Cargo.lock`; the `wit-bindgen` CLI can emit the same glue as a source
file; the contract is small enough to write the canonical-ABI glue by hand.
Options: (a) `wit-bindgen-cli` as a pinned generator, output committed and reviewed, `Cargo.lock` stays at
one package; (b) the `wit-bindgen` proc-macro crate, pinned in `Cargo.lock`; (c) hand-written glue, checked
by `wasm-tools` validation and round-trip tests.
Recommendation: (a), with (c) as fallback if the generated code cannot be made `no_std` without a runtime
crate (to be checked in L0).
Consequence: zero crates in the kernel; the generator runs only when the WIT changes.

**O4 — wasmCloud linking topology.**
Context: a wasmCloud component keeps no memory between invocations (unverified locally), while a bash
session keeps its VFS across `exec` calls.
Options: (a) static composition: the TS orchestrator and the kernel are linked with `wac` into one
component and a session lives inside one invocation; stateless calls across the lattice use
`exec-oneshot`; (b) stateless only in v1: every call passes its files in and gets the modified files back;
(c) a long-lived host-managed service holds the sessions, addressed over the lattice.
Recommendation: (a).
Consequence: session state lasts as long as the orchestrator's invocation; cross-invocation persistence
is the orchestrator's job (journal to a blob or key-value capability). (c) depends on wasmCloud features
to be confirmed with the owner's version.

**O5 — `yq` input formats in v1.**
Context: the reference `yq` reads YAML, JSON, XML, TOML, INI and CSV through third-party parsers; each
format is an own parser for us.
Options: (a) YAML and JSON only; (b) YAML, JSON, TOML and INI; (c) all six formats.
Recommendation: (a), the others after parity.
Consequence: `yq` is reported `partial` in the parity table until the other formats land.

**O6 — MoonBash in the benchmark.**
Context: `rules/MASTER.md` makes it mandatory. MoonBash is not on npm; building it needs the `moon`
toolchain and a network fetch of 17 registry packages; its full package build also needs pnpm and floating
`latest` versions of Vite+.
Options: (a) install `moon` under `tmp/bench-tools/`, build only the release JS kernel once, wrap it with a
local adapter, record commit and SHA-256; (b) full MoonBash package build; (c) owner amendment of
`rules/MASTER.md` waiving MoonBash until it is published.
Recommendation: (a).
Consequence: one vendor toolchain on the bench machine only; with (c) the rule text changes and the
MoonBash column reads `source-gap`.

**O7 — Clean-room lane separation.**
Context: just-bash, the Oils spec cases and MoonBash are Apache-2.0; our output is MIT.
Options: (a) two rooms: only the design lane reads `tmp/refs/` and writes behavioural specs; build and
test-writing lanes never read it; (b) one room with a written no-copy rule and review; (c) two rooms for
code, one room for tests.
Recommendation: (a), enforced by lane configuration and stated in every lot brief.
Consequence: specs must be complete enough to build from; the design and review overhead of section 7
(about 40%) is the price.

**O8 — Coverage-guided fuzzing before 1.0.**
Context: seeded generators without coverage feedback rarely reach deep states of the parser, the regex
compiler, inflate or tar. `cargo-fuzz` needs a nightly toolchain and third-party dev crates.
Options: (a) a `cargo-fuzz` campaign run from git-ignored `tmp/fuzz/`, out of every manifest and out of
`make check`, required by lot L11 before 1.0; (b) seeded fuzzing only; (c) an own coverage-guided loop on
rustc's coverage instrumentation, with no third-party crate.
Recommendation: (a).
Consequence: a nightly toolchain and dev crates on the fuzz machine only; findings become seeded
regression tests in the repo.

**O9 — Batch B: recommendations to confirm together.**
These had no objection in review. Each line is the recommendation; the alternative is in brackets.

| Id | Topic | Recommendation [alternative] |
|---|---|---|
| B1 | Shell semantics when just-bash differs from GNU bash | GNU bash 5.2 is the truth, just-bash API is the shape, divergence list published [bug-for-bug just-bash] |
| B2 | Host language and type checking | ESM JavaScript with JSDoc and hand-written `.d.ts` for the facade; generated `.d.ts` for the bindings; `tsc --noEmit` from the pinned `tools/` manifest (lockfile committed, `devDependencies` only, never in the root manifest), failing closed when absent [TypeScript sources with a build step] |
| B3 | AST and transform API | Not in v1 [kernel-produced JSON AST shaped like just-bash] |
| B4 | Regex semantics | Linear-time engine with compile-size caps, bounded backtracking for backreferences only, no host `RegExp`; `grep -P` lookaround unsupported in v1 [backtracking engine with fuel] |
| B5 | Synchronous API | `execSync` on Node, throwing a typed error if an async capability is configured [API identical to just-bash] |
| B6 | Concurrency and trap recovery | Interleaved fibers; a trap poisons the instance; dirty-file journal opt-in [journal on by default] |
| B7 | Kernel binaries | One kernel component with all tiers; a `slim` build is decided after L2 against the size budget [one binary per companion] |
| B8 | WIT contract lock | `sentropic:bash@0.1.0` frozen at the end of the EVOL rung after review; additive changes bump the patch, breaking changes the minor, until 1.0 [freeze after L2 measurements] |
| B9 | Browser verification | A headless Chromium as a test-only tool outside every manifest, for the browser entry tests [browser entry shipped as `unverified` in the pre-release] |
| B10 | Fallback if a `no_std` `wasm32-wasip2` build cannot link without `wasi-libc` | Stop and return to the owner: the only known alternative builds the same component from a core-only target plus `wasm-tools`, which conflicts with the letter of A1 [accept WASI imports and stub them] |
| B11 | Native run of the component in tests | `wasmtime` as a pinned test-only tool, proxy for wasmCloud [owner-provided wasmCloud test host] |

---

## 7. Lot breakdown and model routing (sketch; the plan rung refines it)

Sizes in points (S = 1, M = 3, L = 8; intermediate values where a lot was split). Builder tiers and
relative price per point (estimate, includes retries): **Cheap** x1 (Haiku 5.5; Engram Muse 1.3 when its
quota is back), **Mid** x2.5 (Codex Sol 6.1 at medium effort, never high), **Strong** x5 (Sonnet 5.5;
Gemini 4 Argon as second source and for security-heavy lots). Fable 5.2 writes each lot's spec and Carbon
reviews; neither builds. Build cost = points x tier factor.

| Lot | Scope | Pts | Builder (fallback) | Cost | Depends on |
|---|---|---:|---|---:|---|
| L0 | Component pipeline: toolchain pin, `wasm32-wasip2` `no_std` link proof, tool pins with hashes under `tmp/tools/`, `tools/` manifest for jco and tsc, build script, zero-import gate, no-bare-import gate, two-build identity, `make` targets (scope exception) | 3 | Sonnet 5.5 | 15 | A1, O3 |
| L1 | WIT contract, kernel skeleton (`vfs`/`shell`/`fiber` resources, own allocator with ceiling and budgets, epochs), JS facade over the jco output, membrane, Node and browser loaders | 8 | Sonnet 5.5 (Gemini 4 Argon) | 40 | L0 |
| L1c | Session driver component, `wac` composition, `wasmtime` smoke test, `exec-oneshot` | 3 | Gemini 4 Argon (Sonnet 5.5) | 15 | L1, O4 |
| L2 | Vertical slice: lexer/parser subset, resumable VM, chunked buffers, VFS core, command trait + one-file-per-command registry, `echo`; measurements of section 2.7 (size, `wasm-opt` gain, glue, cold start) | 8 | Sonnet 5.5 (Gemini 4 Argon) | 40 | L1 |
| L3 | Full lexer and parser: grammar of 1.4, `[[ ]]`, `$(( ))`, heredocs, `$'...'`, `normalizeScript`, parse errors | 8 | Sonnet 5.5 | 40 | L2 |
| L4 | Expansion engine: parameter ops incl. `${var@op}`, substitutions, brace, tilde, glob/extglob/globstar, word splitting | 8 | Sonnet 5.5 | 40 | L3, L8a |
| L5 | Interpreter semantics: redirections and fd table, functions, subshells, `set`/`shopt`, errexit, traps subset, arrays, namerefs, special-builtin precedence | 8 | Sonnet 5.5 | 40 | L4 |
| L6a | State-coupled builtins: `declare`/`typeset`, `local`, `readonly`, `export`, `unset`, `eval`, `source`, `exec`, `command`, `builtin`, `read`, `mapfile` | 3 | Codex Sol 6.1 medium; escalate to Sonnet 5.5 on the first failed gate | 7.5 | L5 |
| L6b | Leaf builtins: `:`, `true`, `false`, `cd`, `pwd`, `shift`, `dirs`/`pushd`/`popd`, `getopts`, `let`, `hash`, `help`, `type`, `test`/`[`, `wait`, `compgen`/`complete`/`compopt` | 3 | Haiku 5.5 | 3 | L2 (API), L5 (wiring) |
| L7a | VFS complete: default layout, `bash.fs`/`InMemoryFs` facade, `MountableFs`, lazy files, journal and compaction | 3 | Codex Sol 6.1 medium | 7.5 | L2 |
| L7b | Host FS adapters (`OverlayFs`, `ReadWriteFs`, custom `IFileSystem`) with path-security gates and batched calls | 3 | Gemini 4 Argon (Sonnet 5.5) | 15 | L1 |
| L8a | Regex engine (linear-time, compile caps, bounded backrefs) + glob/extglob matcher | 8 | Sonnet 5.5 (Gemini 4 Argon) | 40 | L2 |
| L8b | Shared leaf kernels: printf/strftime engine, number parsing, md5/sha1/sha256, base64 | 3 | Codex Sol 6.1 medium | 7.5 | L2 |
| L9a-1 | Hard stream commands: `grep` family, `sort`, `tr`, `expr` | 4 | Codex Sol 6.1 medium (Sonnet 5.5) | 10 | L2, L8a |
| L9a-2 | Re-entrant commands: `find` with `-exec`, `xargs`, `bash`/`sh`, `timeout`, `time` | 4 | Codex Sol 6.1 medium (Sonnet 5.5) | 10 | L5 |
| L9b | About 44 leaf tier-1 commands, one file each | 8 | Haiku 5.5 / Engram Muse 1.3, parallel | 8 | L2, L7a, L8b |
| L10a | Parity harness: `node:test` layout, confined fixture recorder (section 4.4), API tier A/B tests | 3 | Sonnet 5.5 | 15 | L2 |
| L10b | Parity cases per grammar area and per command, written from the specs | 8 | Haiku 5.5 / Engram Muse 1.3 | 8 | L10a, continuous |
| L11 | Security: adversarial suites, seeded fuzzers (JS and native), membrane and limit-latency tests, coverage-guided campaign (O8) | 8 | Gemini 4 Argon (Sonnet 5.5) | 40 | L5, L7b |
| L12 | Benchmark harness, baseline, noise-floored gate, MoonBash build and adapter | 3 | Codex Sol 6.1 medium | 7.5 | L2, O6 |
| L13 | `sed`: parser, addresses, pattern/hold cycle, branches with fuel, `-i` | 8 | Codex Sol 6.1 medium (Sonnet 5.5) | 20 | L8a |
| L14 | `awk`: lexer, parser, VM, built-in functions | 8 | Sonnet 5.5; built-ins by Haiku 5.5 | 40 | L8a, L8b |
| L15a | `jq`: JSON parser, generator/backtracking evaluator, builtins | 8 | Sonnet 5.5; builtins by Haiku 5.5 | 40 | L8a |
| L15b | `yq` on the jq evaluator, formats per O5 | 3 | Codex Sol 6.1 medium | 7.5 | L15a, O5 |
| L16a-1 | inflate/deflate, gzip family, tar | 5 | Gemini 4 Argon (Sonnet 5.5) | 25 | L2 |
| L16a-2 | `diff` | 3 | Codex Sol 6.1 medium | 7.5 | L2 |
| L16b | `@sentropic/net`: DNS-pinned fetch adapter + `curl` | 3 | Gemini 4 Argon | 15 | L1 |
| L16c | Leaf tier-3 text and fs commands: `comm join column fold expand unexpand split strings od file tree du` | 3 | Haiku 5.5 / Engram Muse 1.3 | 3 | L2, L7a |
| L16d | `rg` on the grep kernel | 3 | Codex Sol 6.1 medium | 7.5 | L9a-1 |
| L17a | `Sandbox` facade, CLI, standalone facades (`@sentropic/grep`, `ls`, `awk`, `jq`) | 3 | Haiku 5.5 | 3 | parity of tiers A/B |
| L17b | AST export, transform pipeline, `serialize` (only if B3 is reversed) | 3 | Sonnet 5.5 | 15 | L3 |
| L18 | `@sentropic/make`, after parity | 8 | Sonnet 5.5 | 40 | parity |
| L19 | Browser entry verification: headless test rig, `precompile`, streaming compile | 3 | Codex Sol 6.1 medium | 7.5 | L1, B9 |

Totals (estimate): 168 points, 640 build units. By tier: Strong 103 points (61% of points, 80% of cost),
Mid 40 points (24%, 16%), Cheap 25 points (15%, 4%). Routing everything to the Strong tier would cost 840
units; the routing saves about 24%. v1 parity without L17b and L18: 585 build units.

Design and review overhead: under O7 (a) the build lanes work only from specs, so Fable 5.2 specs and
Carbon reviews are a real share of the spend. Budgeted at **40% of build cost** (range 35-50%): about 256
units, for a total near **900 units** (about 820 for v1 parity). The plan rung must carry this line per lot.

Review rule: Carbon reviews **every Strong and every Mid lot in full**, and samples Cheap lots (one command
in five) once the first three of a family pass unchanged. A Mid lot that fails its gate twice is re-routed
to its fallback (L6a on the first failure, since it is on the critical path); a Strong lot is never
downgraded mid-way.

Critical path (sequential, 46 points): `L0 -> L1 -> L2 -> L3 -> L4 -> L5 -> L6a`, then the parity gate.
Everything else is decoupled from L3-L5 and starts right after L2:
- `L1c`, `L7a`, `L7b`, `L8a`, `L8b`, `L10a`, `L12`, `L16a-1`, `L16a-2`, `L16b`, `L19` in parallel with L3.
- Leaf commands `L9b` and `L16c` need only the command trait (L2), the VFS (L7a) and the shared kernels
  (L8b): they are tested through the command trait with byte inputs, without a full shell.
- `L9a-1`, `L13`, `L14`, `L15a` start when the regex engine (L8a) lands; `L16d` follows `L9a-1`; only
  `L9a-2` waits for L5.
- One file per command and a generated registry (defined in L2) keep parallel lanes from editing the same
  file.
- L0 is the go/no-go point for the `no_std` `wasm32-wasip2` path (B10); L2 is the go/no-go point for the
  budgets (A2) and for `wasm-opt`.

---

## 8. Sources and reproducibility of this study

- Reference trees: `tmp/refs/just-bash` (v3.6.0 sources), `tmp/refs/MoonBash` (v0.1.0 sources).
- Counts in section 1 come from one-off node scripts over those trees; they size the surface.
- Measurements in sections 1.7 and 2.1 come from one-off node scripts under the git-ignored `tmp/bench/`
  (just-bash 3.6.0 from npm, synthetic WAT modules assembled with wabt 1.0.36). They are a single-machine
  snapshot and must be re-measured by the lot L12 harness before any budget becomes a gate.
- "Fact note" in sections 2 and 4 is the review lane's pre-analysis of WASI 0.2 and the component model
  (`tmp/conductor/lanes/facts-wasip2.md`, outside the repo). Its claims were adopted only where they could
  be checked locally or are stable public specification; the others are marked unverified. Tool versions
  come from `cargo search` and `npm view` on 2026-11-15. No tool or target was installed for v3.
- Items labelled "estimate" (flag coverage classes, kernel size, lot sizes, tier price factors) are
  unverified. Statements about browser and edge-runtime compilation limits are unverified here.

---

## Appendix A — Review reconciliation

Each finding was checked against `tmp/refs/` or re-measured before being applied. Question ids in this
table are the v2 ids. Mapping to section 6: Q1 = A1, Q2 = A2, Q3a = B1, Q3b = O1, Q4 = A4 and O5, Q5 =
answered and O2, Q6 = B2, Q7 = B3, Q8 = O6, Q9 = B4, Q10 = B5, Q11 = answered, Q12 = B6, Q13 = B7, Q14 = O7.
The v2 recommendation of `normal` for Q3b is superseded by O1, and the v2 ABI names by section 2.3.

| Finding | Status | Where fixed / evidence |
|---|---|---|
| B1 VFS vs instances, `bash.fs`, re-entrant and concurrent exec | accepted | 2.4 points 1-3 and 6; per-exec isolation option removed (Q10); Q12 |
| B2 memory cap vs limits, abort on allocation failure | accepted | 2.4 points 4-5; 4.3; profile table in 1.2; Q3b. Verified in `JB:src/limits.ts:164-262` |
| B3 command count and missing commands/builtins | accepted | 1.3 (85 + 5 = 90, counted by script; builtins from `builtin-dispatch.ts`), 3.1, Q4, L6a/L6b |
| B4 host-unsafe differential oracle; no revocation | accepted | 4.4 (differential oracle row), 2.4 point 7, 4.1 |
| M1 stale limitation claims, wrong path | accepted | 1.4 (verified in `read.ts`, `Bash.ts:455-460`, `arithmetic.ts`), 1.2 and 4.3 (source size), `JBR:` prefix |
| M2 MoonBash facts | accepted | 1.5, 1.8 (93 / 98 files counted; no timeout fallback in source; 2026-04-20 status) |
| M3 omitted API contracts | accepted | 1.2, 3.2 tier A, 2.4 point 10 |
| M4 cold-start and size extrapolation, companions | amended | 2.1 re-measured end to end (22-43 ms upper bound), 5.5 budgets relaxed, Q13 single binary. The stated gzip ratio (25-35% reduction) is not confirmed: real `.wasm` files measured here compress to 36-49% of raw; `wasm-opt` is recommended conditionally (Q1), not unconditionally |
| M5 host-call storms, detached buffers, retention, debugging, clean-room | accepted | 2.4 points 8-9, 2.3 known costs, Q14 |
| M6 SSRF, special files, timezone, `trusted`, missing limits | accepted | 4.2, 4.3 |
| M7 coverage-guided fuzzing | amended | 4.4: seeded native fuzzing in v1; `cargo-fuzz` is a third-party dev crate on nightly, so it is deferred as a non-gating tool needing justification |
| M8 benchmark biases and missing metrics | accepted | 1.7 re-measured interleaved with an ESM baseline; 5.1 (M5 retention, M8-M10), 5.2, 5.4 |
| M9 MoonBash waiver vs mandatory rule | accepted | 5.3, Q8 |
| M10 framing of Q1, Q3, Q5, Q6, Q10 | amended | Section 6. Two points differ from the review: Q3b recommends `normal` (hardened step limits break ordinary scripts); the claim that node 22 runs `.ts` natively does not hold here (`process.features.typescript` is false on 22.22.1) |
| M11 missing owner questions | amended | Q11-Q14 added. The browser/edge compilation limits are kept as unverified platform knowledge (engine-specific, not checkable on this machine) |
| M12 lot sizing and routing | amended | Section 7: splits adopted; routing adapted (a Mid tier on Codex Sol 6.1 medium for L3, L6a, L7a, L9a, L13; `rg` separated from archives; VFS split into L7a/L7b) |
| m1 file and directory counts | accepted | 1.1, 1.4 |
| m2 dependency names, executor package | accepted | 1.1, 3.1, 3.2 |
| m3 `[[`, `globskipdots`, `$'...'`, `${var@op}` | accepted | 1.3, 1.4, L3-L5 |
| m4 quantile definitions | accepted | 5.4; re-measurements use the corrected definitions |
| m5 `ls`/`grep` companion boundary | accepted | 3.1 (JS facades over the kernel) |

---

## Appendix B — v3 changes (owner decision A1, fact note, v2 verification)

| Input | Status | Where applied / evidence |
|---|---|---|
| A1: `wasm32-wasip2`, Component Model, WIT | applied | 2.2-2.7, 3.3; the v2 ad-hoc ABI (`exec_create`, `fs_op`) is replaced by WIT resources |
| A1: wasmCloud and browser | applied | 2.5; browser moved into v1 (3.2, L19); wasmCloud platform facts are unverified locally |
| A1: release-only toolchain, prebuilt artefacts | applied | 2.6, 3.3 |
| A1: Cargo profile first, `wasm-opt` before componentization if >= 10% | applied | 2.6; decision measured in L2 |
| A1: compile-time bindings only, 0 npm dependency | applied | 2.3 (zero-import world, `no_std`), O3 |
| Fact note: zero-import world, `no_std` + `alloc`, `list<u8>`, inverted control | adopted | 2.3; linker flags verified locally, the `no_std` link itself is unverified (L0, B10) |
| Fact note: tool versions | corrected | 2.6: registries show `jco` 1.42.0, `jco-transpile` 0.24.0, `wac-cli` 0.12.0, `wit-bindgen` 0.63.0, `wasm-tools` 1.263.0 |
| Fact note: WASI 0.3 ratification and support, jco flags, wasmCloud defaults and statelessness | kept as unverified | 2.3, 2.5 |
| Fact note: per-profile `maximum` pages | amended | The memory is defined in the core module, so the ceiling is enforced by the kernel allocator (2.4 point 5) |
| Fact note: four coherent profiles | adopted | 4.3, O1 |
| v2 dispute 1: coherent byte caps | accepted | 4.3, O1 |
| v2 dispute 2: `yq` scope, 82 kernel commands | accepted | 3.1, O5 |
| v2 dispute 3: `tsc` location | accepted, reconciled | B2: dedicated `tools/` manifest with a committed lockfile, outside the root manifest, failing closed |
| v2 dispute 4: coverage-guided fuzzing | accepted as a question | 4.4, O8, L11 |
| v2 dispute 5: routing and spend | accepted | Section 7: L3 Strong, L12 Mid, L9a and L16a split, full review of Mid lots, 40% design and review overhead |
| v2 note: shared `InMemoryFs` instance versus per-`Bash` grants and limits | accepted | `shell` resource (2.3), 2.4 point 2 |
| v2 note: `bash -r` rejects file redirections | accepted | 4.4 |

