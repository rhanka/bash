# SPEC STUDY — @sentropic/bash sandbox (rung 1: study, v2)

Status: study only, v2 after the adversarial review (verdict `approve-with-changes`, architecture
option (a) accepted as target). No decision is taken here; every choice marked **OWNER** is listed in
section 6. Appendix A maps each review finding to its resolution.
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

## 2. Architecture options for a WASM-first engine

### 2.1 Cost primitives measured on this machine

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
Option 3 is assumed by every option below.

### 2.2 Options

**(a) Rust -> `wasm32-unknown-unknown`, zero crates.** Engine in Rust using only `core`/`alloc`/`std`, no
`wasm-bindgen`, hand-written `extern "C"` ABI, hand-written JS glue. `panic = "abort"`, size-oriented
profile, LTO, one codegen unit. `Cargo.lock` holds a single package.

**(b) Hand-written WAT.** Engine written directly in WebAssembly text, assembled with `wat2wasm`.

**(c) AssemblyScript / MoonBit / Zig.** AssemblyScript is an npm package (third-party JS toolchain).
MoonBit needs the `moon` toolchain and registry packages. Zig needs a tarball install (single static
binary, std only).

**(d) Hybrid: TS host shell + WASM kernels for hot builtins only.**

| Criterion | (a) Rust zero-crate | (b) hand WAT | (c) AS / MoonBit / Zig | (d) hybrid TS + kernels |
|---|---|---|---|---|
| Startup model | One module; validation 2.5 ms/MB, lazy compile paid only for executed code; `Module` cached per process | Smallest binary | Zig similar to (a); AS adds its runtime/GC; MoonBit wasm-gc needs recent engines | JS parse/compile of the interpreter dominates (what just-bash pays today) |
| JS<->WASM boundary for the VFS | VFS in linear memory: zero crossings for in-memory work; host FS via batched `HOST_CALL` | Same as (a) | Same as (a) | VFS in JS: every kernel call copies file content in and out, pipelines cross per stage |
| Binary size | Medium; `std::fmt` and panic paths need care | Smallest | Zig small; AS medium | Small WASM, large JS |
| Memory isolation | Untrusted data never becomes JS objects; Rust memory safety inside the module | Isolation from JS, no memory safety inside | Zig: manual memory safety; AS: GC-managed | Interpreter state on the JS heap: shared-heap classes remain |
| Fuel / limits | VM loop decrements fuel; budgeted fallible allocation (section 2.4); memory ceiling from the limit profile | Same, by hand | Same | Two limit systems |
| Build reproducibility | `rust-toolchain.toml` pin + `rustup target add wasm32-unknown-unknown` (native, official, no Docker); fixed flags; two builds must be byte-identical | `wat2wasm` pinned | New toolchain installs | Needs a TS build step or JS with JSDoc |
| Supply chain | rustc/cargo already installed (1.96); one official target component; zero crates | wabt already installed | Zig: one new binary; MoonBit: new vendor toolchain + registry; AS: npm tree | Same as the kernel language, plus JS tooling |
| Contributor cost | Mainstream language; kernel also compiles natively for unit tests and debugging with real stack traces | Very high, not reviewable at this size | Zig medium; MoonBit high; AS medium | Two languages on every hot path |
| Performance ceiling | High: byte-oriented, no GC | Highest in theory | Zig comparable | Bounded by JS and boundary copies |

### 2.3 Recommendation (OWNER gate Q1; accepted as target by the review)

**Option (a): a Rust zero-crate fat kernel compiled to `wasm32-unknown-unknown`, with a thin dependency-free
JS host, and the resumable-VM execution model.** Hand-written WAT only for micro-kernels where a benchmark
proves a gain and the code stays reviewable. Hybrid (d) is rejected as the target: it keeps the JS startup
cost of a large interpreter and the shared-heap security surface. The same reasoning applies to companion
commands: a separate `.wasm` per companion would reintroduce the copy cost of (d) for `awk`/`jq` reading the
core VFS, hence the single-binary recommendation of Q13.

Rationale:
1. Startup: the measured upper bound (22-43 ms with all code executed) is already 3-5 times under
   just-bash's 119 ms, and a first `exec` touches a fraction of the kernel.
2. Security: linear-memory isolation removes the JS escape and prototype-pollution classes for the kernel;
   the JS host stays small and auditable.
3. Zero-dependency rule: Rust `std`/`core` gives collections, UTF-8 and formatting without any crate.
4. Testability: kernel logic compiles natively for fast unit tests, debugging and fuzzing loops.
5. Cheapest delivery: per-command work is separable and specifiable (section 7).

Known costs to accept: own regex engine, glob matcher, printf/strftime engine, hashing, deflate/inflate,
tar, and JSON/YAML parsers; an explicit-stack VM is harder to debug than a recursive interpreter (mitigated
by the native build and a VM trace mode); WebAssembly linear memory never shrinks (section 2.4, point 8).

### 2.4 Kernel and host contract (to be detailed in the EVOL rung)

```
JS host (no dependency)                          WASM kernel instance (Rust, zero crates)
  Bash class, options, limit profile                VFS stores (vfs_id): tree, chunked byte files
  module cache (one WebAssembly.Module)             exec fibers (exec_id): VM stack, shell state,
  capability membrane (revocable, epoch-fenced)       fd table, budget; parent link for nested exec
  fs proxy for `bash.fs` / `InMemoryFs`             lexer -> parser -> bytecode; expansion; i64 arithmetic
  exec(): exec_create -> loop exec_run(fuel)        builtins and commands (one module per command)
     HOST_CALL  -> membrane -> exec_resume          regex, glob, format, codecs
     OUT_OF_FUEL -> deadline / abort check          budget accounting on every growth
  view(): fresh typed-array view per entry          imports: host_call family, capability-gated
```

1. **Explicit exec handles.** Exports: `exec_create(vfs_id, parent_exec_id) -> exec_id`,
   `exec_run(exec_id, fuel) -> status`, `exec_resume(exec_id, call_epoch, result) -> status`,
   `exec_result(exec_id)`, `exec_destroy(exec_id)`. Each exec is a fiber with its own VM stack, shell state
   and budget; nothing in the ABI is implicit or global. This covers:
   - **Concurrent** `bash.exec()` calls on one `Bash`: several fibers over one VFS. JS is single-threaded,
     so fibers interleave only at `HOST_CALL` and fuel-slice boundaries, which matches the reference
     behaviour (interleaving at await points). No host mutex, so no deadlock on re-entry.
   - **Re-entrant** execution (`ctx.exec`, `ctx.origCommand`, `execWithInheritedStdin` from a custom
     command suspended in a `HOST_CALL`): a child fiber with `parent_exec_id`, charged to the parent's
     aggregate budget (`maxWorkUnits`, `maxExecDepth`). In-kernel nesting (`bash -c`, `eval`, `xargs`,
     `find -exec`, `timeout`, `time`) stays inside one fiber's explicit stack.
2. **VFS ownership.** A VFS store is a kernel object (`vfs_id`). Rule: *an exec runs in the instance that
   owns its root VFS.* `new Bash()` creates a dedicated instance with one store. A standalone
   `new InMemoryFs()` owns a store in its own instance; a `Bash` constructed with it attaches to that
   instance, so two `Bash` objects sharing one `InMemoryFs` share an instance and see the same files with
   no copy (they share all their data anyway, so no isolation is lost). Any other `fs` (`OverlayFs`,
   `ReadWriteFs`, custom `IFileSystem`, a mount from another instance) is host-backed and reached through
   `HOST_CALL`. `MountableFs` keeps its mount table in the kernel and routes each path to an in-kernel store
   or to the host.
3. **Public `bash.fs` bridging.** `bash.fs` and `InMemoryFs` are JS proxies implementing `IFileSystem`
   over synchronous kernel exports (`fs_op(vfs_id, op, args)`), returning resolved promises (and the sync
   `mkdirSync`/`writeFileSync`/`writeFileLazy`). They are valid between and during execs because fibers
   never hold VFS borrows across a suspension point. Lazy files (`LazyFileProvider`) are stubs in the store;
   first read raises a `HOST_CALL` that materializes the content.
4. **Fallible, budgeted allocation.** No infallible growth on untrusted data: every buffer, string, array,
   VFS write and output append goes through a budget check (`charge(bytes)` against the limit that applies:
   `maxStringLength`, `maxOutputSize`, `maxFileSystemBytes`, `maxLiveBytes`, ...) followed by a fallible
   reservation (`try_reserve`-style). A breach unwinds the fiber's explicit stack and returns an
   `ExecutionLimitError` **result** with the reference exit code; the instance and its VFS stay valid.
   Infallible `push`/`format!`/`collect`/`to_string` on untrusted-sized data are banned by a source check
   in the gate. Large data (files, stdout/stderr, pipeline buffers) uses chunked storage, so growth never
   needs a doubling copy and peak memory is size + one chunk.
5. **Memory ceiling fitted to the limit profile.** The `maximum` pages of the instance are derived from the
   active profile: `maxFileSystemBytes + maxLiveBytes + maxOutputSize + fixed kernel overhead (64 MiB)`,
   capped by the wasm32 limit (4 GiB). `hardened`: 128 + 64 + 10 + 64 = 266 MiB. `normal`: 1024 + 512 + 256
   + 64 = 1 856 MiB. Pages are committed on demand, so a large ceiling costs nothing until used. The byte
   budgets of point 4 always bind before the page ceiling; `memory.grow` failure is a last line that also
   returns a limit error.
6. **Traps are bugs, with a defined recovery contract.** With points 4 and 5, no untrusted input is
   supposed to trap. If a trap happens (kernel bug, engine stack exhaustion), linear memory is no longer
   trusted: the `exec` returns a fatal result (flagged, never silently retried), the instance is
   **poisoned** (further `exec` and `bash.fs` calls reject with a typed error) and every pending fiber of
   that instance fails. VFS content is not salvaged from a trapped instance. Callers who need committed
   files to survive opt into a **journal**: at each exec completion the host mirrors the files dirtied by
   that exec (dirty set tracked by the kernel; 0.57 us per 60 KB copied) and can rebuild a fresh instance
   from the mirror. Whether the journal is on by default is owner question Q12. A per-exec fresh instance
   is not offered: it would either lose the persistent VFS or copy it on every call.
7. **Revocable capability membrane with epochs.** Every `HOST_CALL` carries `(exec_id, call_epoch)`. The
   host hands async extensions (custom commands, custom `IFileSystem`, `fetch`, `sleep`) revocable proxies,
   never raw objects. On `AbortSignal`, deadline or limit breach the host revokes the fiber's proxies at
   once: later `fs`/`exec`/`fetch` calls through them reject with an abort error, and a late
   `exec_resume` whose `(exec_id, call_epoch)` is no longer current is dropped by both host and kernel.
   `exec_id` values are never reused within an instance. An extension that does not settle within
   `maxExtensionCleanupTimeMs` is abandoned and its fiber destroyed.
8. **Memory views and retention.** `memory.grow` detaches the previous `ArrayBuffer`: the host never
   caches a typed-array view across a kernel entry; a `view()` helper re-reads `memory.buffer` each time.
   Linear memory never shrinks, so a spike pins RSS for the life of the instance: the host offers
   `bash.compact()` (and an optional idle threshold) that exports the VFS to a fresh instance, using the
   same path as the journal.
9. **Batched host calls.** For host-backed filesystems the kernel issues batched operations (read a
   directory with entry types and stats in one call, multi-stat, bounded tree walk for the built-in
   adapters, `readdirWithFileTypes` when a custom `IFileSystem` provides it), so that `find`, `grep -r`,
   `du`, `cp -r` or `**` cost one host round trip per directory, not per entry.
10. **Bytes at the boundary.** All strings and buffers are (pointer, length) pairs; no JS object enters
    the kernel. Output conversion follows the reference contract: valid UTF-8 decoded, invalid bytes
    preserved as latin1, raw bytes available (`stdoutKind`, `ByteString`). `normalizeScript` runs in the
    kernel unless `rawScript` is set.
11. **Runtime loading.** On Node the module is read with `readFileSync(new URL(...))` and compiled
    synchronously once per process. Where synchronous compilation of a large module is not available
    (browser main thread in some engines, edge runtimes that only accept statically imported modules), the
    instance is created lazily and awaited inside the first `exec`, which is already async; this is owner
    question Q11 and is unverified on this machine (no browser).

---

## 3. Packaging

### 3.1 Core versus companion packages (proposal, OWNER gates Q4 and Q13)

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

All 85 `CommandName` entries are accounted for: 80 in the kernel tiers, `xan`, `sqlite3`, `html-to-markdown`
not planned, and `curl` in `@sentropic/net`; opt-in `python*`/`js-exec`/`node` not planned. `yq` is in tier 3
with YAML first; its XML/TOML/INI/CSV inputs are a scope decision inside Q4. A `slim` build (tier 1 only)
can be produced from the same workspace if the size budget requires it.

### 3.2 API compatibility surface with just-bash

| Tier | Surface | v1 stance |
|---|---|---|
| A — drop-in | `new Bash({files, env, cwd, fs, executionLimits, executionLimitProfile, commands, customCommands, sleep, network/fetch, processInfo})`; `bash.fs`; `exec(cmd, {env, replaceEnv, cwd, stdin, stdinKind, signal, args, rawScript})` with the non-throwing result contract, `normalizeScript` and the binary-preserving output decoding; `readFile`, `writeFile`, `getCwd`, `getEnv`, `registerCommand`, `defineCommand` and the full command context (`exec`, `origCommand`, `exportedEnv`, `assignShellVariable`, `fileDescriptors`, `getRegisteredCommands`); `InMemoryFs` with the `IFileSystem` contract incl. `createExclusive` and stat identity; default layout (`/home/user`, `/bin`, `/usr/bin`, `/dev`, `/proc`, default env); command-list helpers; byte/text helpers | Same names and semantics; proven by our own API tests |
| B — compatible behaviour, own implementation | `OverlayFs`, `ReadWriteFs`, `MountableFs`, `NetworkConfig` allow-list and typed errors, limit names and both profiles, ESM and CJS entries | Same options; host-side JS adapters behind explicit capabilities |
| C — later | `parse()` + AST types, transform pipeline and plugins, `Sandbox` facade, CLI binaries, `trace`, `logger`, `coverage`, `./browser` entry (Q11) | After parity of tiers A and B |
| D — not reproduced | `DefenseInDepthBox` and related exports, `python`, `javascript`, sqlite3, `@just-bash/executor` | Documented as N-A |
| Additions | `execSync` (Q10), `Bash.precompile()`, `bash.compact()`, journal option (Q12), fuel and memory knobs | Opt-in, never required for drop-in use |

---

## 4. Security model draft

### 4.1 Principles

1. Default deny. A `Bash` created without options can compute and use its in-memory VFS; it cannot read a
   host file, open a socket, spawn a process, read the host clock, timezone or environment.
2. Capabilities are explicit objects passed at construction, each mapped to a `host_call` operation code.
   An operation without a granted capability fails inside the sandbox and never reaches host code.
3. The kernel is the only code that parses untrusted bytes. The JS host validates lengths, operation codes
   and `(exec_id, call_epoch)`; it never evaluates, never builds property names from sandbox data, and
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

Limit names, both profiles and their values follow the reference (`JB:src/limits.ts`, table in 1.2) so
existing callers keep working. Enforcement points in our engine:

| Limit family | Enforcement point |
|---|---|
| Steps: `maxCommandCount`, `maxLoopIterations`, awk/sed/jq iteration limits | Fuel decremented in the VM dispatch loop and in every builtin loop |
| Aggregate work: `maxWorkUnits`, `maxExecDepth` | Budget shared by a fiber and all its descendants (nested `bash -c`, `eval`, `$(...)`, `ctx.exec`), so nesting cannot multiply the per-exec counters |
| Wall time: `maxExecutionTimeMs`, `maxExtensionCleanupTimeMs` | Checked by the host at each fuel slice and around each host call |
| Bytes: `maxStringLength`, `maxHeredocSize`, `maxOutputSize`, `maxLiveBytes`, `maxInputBytes`, `maxFileSystemBytes` | Budget charge before every growth (section 2.4, point 4) |
| Memory ceiling | Instance `maximum` pages derived from the profile (point 5) |
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
| Differential oracle against the system bash | **Never fed fuzzed or mutated scripts.** Two inputs only: (1) recorded fixtures: hand-written, reviewed scripts run once by a recorder and replayed from committed expected outputs; (2) a generated subset built from an AST generator that can only emit shell builtins, arithmetic, expansions and control flow, with **no external command, no path-bearing redirection outside the work directory, no `eval`/`source` of generated strings, no background jobs**. Execution is confined: fresh empty directory under `TMPDIR` (never `/tmp`), `env -i`, empty `PATH`, `bash --norc --noprofile -r` (restricted mode), `ulimit` caps on CPU time, file size and processes, a wall-clock timeout, and a user/network namespace (`unshare -Urn`) when the host allows it. The generator's safety property (no external command reachable) is itself unit-tested |
| Adversarial suites | Own cases per threat class of section 1.5: parser bombs, expansion bombs, path traversal, symlink and special-file escapes on the host FS adapters, redirect chains and DNS rebinding on the network adapter, output floods, deep recursion, regex blowups (matching and compilation), NUL bytes and invalid UTF-8, keys such as `__proto__`, archive bombs, late async continuations after abort |
| Native-side fuzzing | Kernel compiled natively and driven by the seeded generator in a `cargo test` binary with debug assertions and overflow checks |
| Coverage-guided fuzzing | Valuable for parser, expansion, regex compiler, codecs and path canonicalization. The usual tool (`cargo-fuzz`/libFuzzer) is a third-party dev crate on a nightly toolchain: it is not part of the v1 gate; adopting it as a non-gating, out-of-manifest dev tool needs a tooling justification and is deferred to the EVOL rung |
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
  under Q3.

### 5.3 Reference engines (bench only, never a repo dependency)

| Engine | Install | Location |
|---|---|---|
| just-bash | `npm install --ignore-scripts --no-audit --no-fund just-bash@<pinned>` with a local `--cache` | `tmp/bench-engines/just-bash/` (git-ignored) |
| MoonBash | Mandatory per `rules/MASTER.md`. Not on npm, so it is built once from the reference clone: `moon` toolchain installed under `tmp/bench-tools/`, `moon build --target js --release` (fetches 17 registry packages), kernel artefact `entry.js` wrapped by a small local adapter; commit and artefact SHA-256 recorded. The full package build (pnpm + floating Vite+ versions) is avoided. See Q8 | `tmp/bench-engines/moonbash/` |
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

### 5.5 Proposed performance budgets (OWNER gate Q2)

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
first `exec` touching at most about 300 KB of kernel code.

---

## 6. Open OWNER questions

**Q1 — WASM toolchain.**
Context: only the x86_64 Rust target is installed; wabt is installed; no zig, no moon, no binaryen. The
review accepts Rust as target. `wasm-opt` (binaryen) is a native build-time binary, not a runtime
dependency; it usually reduces size and helps the M6 budget, at the price of one more pinned tool.
Options: (a) Rust zero-crate, plus `wasm-opt` as a pinned native release-build step; (b) Rust zero-crate
without `wasm-opt`; (c) Zig; (d) hand-written WAT only.
Recommendation: (a), with the build still working without `wasm-opt` (dev builds skip it) and the tool
kept only if lot L2 measures a size gain of at least 10%.
Consequence: one rustup step pinned by `rust-toolchain.toml`, one pinned binaryen release installed
outside the repo; we own regex, codecs and parsers.

**Q2 — Performance budgets.**
Context: section 5.5, based on measured just-bash numbers and on a boot probe that showed executed code
costs about 37 us per KB to compile.
Options: (a) accept the table as targets and confirm after L2; (b) tighten cold start to <= 15 ms now;
(c) gate only on "never slower than just-bash" for v1.
Recommendation: (a), with the noise-floored regression gate of section 5.4.
Consequence: budgets drive the single-binary decision and forbid lazy JS chunks on the hot path.

**Q3a — Shell semantics when just-bash differs from GNU bash.**
Context: just-bash has 32-bit bitwise arithmetic, double-based arithmetic elsewhere, and pending
`errexit`/IFS edge cases (section 1.4).
Options: (a) bug-for-bug just-bash; (b) GNU bash 5.2 behaviour as the truth, just-bash API as the shape;
(c) just-bash by default with a `strict-bash` switch.
Recommendation: (b), with a published divergence list.
Consequence: differential fixtures are recorded against the system bash; a few just-bash users may see
different, bash-conformant output.

**Q3b — Default limit profile.**
Context: the reference defaults to `normal` (1 GiB VFS, 256 MiB output, 100k commands, 1 h) and offers
`hardened` (128 MiB VFS, 10 MiB output, 10k commands and loop iterations, 30 s). Our memory ceiling is
derived from the profile: 1 856 MiB for `normal`, 266 MiB for `hardened`; linear memory never shrinks.
Options: (a) `normal` by default, as the reference; (b) `hardened` by default; (c) `normal` byte and step
limits with a lower default memory ceiling (512 MiB) that returns a limit error first.
Recommendation: (a) for drop-in compatibility (10 000 loop iterations and 10 MiB of output break ordinary
agent scripts), with `hardened` documented as the setting for untrusted multi-tenant workloads.
Consequence: with (a) a single instance may grow to 1.8 GiB if a script uses its full allowance;
embedders running many sandboxes should select `hardened` or set explicit limits.

**Q4 — Scope of v1 and delivery order.**
Context: 85 command names plus 5 opt-in in the reference; tiers of section 3.1.
Options: (a) tier 1 first (pre-release), then `awk`/`jq`, then tier 3; python3, js-exec, sqlite3, xan,
html-to-markdown out of v1; `yq` limited to YAML/JSON first; (b) all 80 kernel commands before any
release; (c) shell and tier 1 only for v1, everything else after.
Recommendation: (a).
Consequence: parity is reached in stages; the parity report lists each command as `covered`, `partial` or
`not covered`.

**Q5 — Shipping the `.wasm` artefact.**
Context: consumers must not need Rust. Committing the binary lets a fresh clone run tests without Rust but
adds roughly 1 MB to git history per kernel change. Embedding base64 in JS adds 33% size and a decode on
cold start.
Options: (a) `.wasm` not committed: built by `make build`, shipped as a binary file in the npm tarball,
verified by a two-build byte-identity check and a recorded SHA-256; (b) `.wasm` committed and verified by a
rebuild-and-compare gate; (c) as (a), plus an opt-in `/inline` entry with the binary embedded for bundlers
that cannot load a file.
Recommendation: (c). Contributors and CI need the pinned Rust toolchain anyway; section 2.2 is aligned on
this (no committed binary).
Consequence: `make test` requires a prior `make build`; releases are reproducible from a tag.

**Q6 — Host language and type checking.**
Context: the host is a few hundred lines. node 22.22.1 does not strip types without a flag
(`process.features.typescript` is false). Runtime tests cannot validate `.d.ts` files, so declarations
need a real type check, and `typescript` is a third-party tool.
Options: (a) ESM JavaScript with JSDoc + hand-written `.d.ts`, checked by `tsc --noEmit` on consumer
fixtures with a pinned `typescript` installed under git-ignored `tmp/` (in no manifest); (b) same sources,
`typescript` declared as a justified devDependency; (c) TypeScript sources with a `tsc` build step.
Recommendation: (a).
Consequence: no build step for the host; one out-of-manifest dev tool to pin and justify; type parity with
just-bash is checked, not assumed.

**Q7 — AST and transform API compatibility.**
Context: just-bash exports `parse()`, AST node types and a transform/serialize pipeline used by plugins.
Options: (a) not in v1; (b) expose an AST as JSON produced by the kernel, shape-compatible with just-bash;
(c) own AST shape.
Recommendation: (a) for v1, (b) afterwards if a consumer needs it.
Consequence: v1 is a drop-in for `Bash.exec` users, not for transform-plugin users.

**Q8 — MoonBash in the benchmark.**
Context: `rules/MASTER.md` makes the benchmark against MoonBash mandatory. MoonBash is not on npm. Building
it needs the `moon` toolchain (vendor installer, not present) and a network fetch of 17 registry packages;
its full package build additionally needs pnpm and floating `latest` versions of Vite+.
Options: (a) install `moon` under `tmp/bench-tools/`, build only the release JS kernel once, wrap it with
a local adapter, record commit and SHA-256; (b) full MoonBash package build with pnpm and Vite+; (c) owner
amendment of `rules/MASTER.md` waiving MoonBash until it is published (a rule change through a branch
exception, not a study decision).
Recommendation: (a).
Consequence: one more vendor toolchain on the bench machine only; with (c) the rule text changes and the
MoonBash column is reported as `source-gap`.

**Q9 — Regex semantics.**
Context: no third-party engine allowed; agents use ERE and Perl-like syntax in grep/sed/awk.
Options: (a) linear-time engine for BRE/ERE + common Perl classes, bounded backtracking only for
backreferences, compile-size caps; (b) full backtracking engine with fuel; (c) host `RegExp` fallback.
Recommendation: (a); `grep -P` lookaround reported as unsupported in v1.
Consequence: ReDoS-free by construction; a small set of PCRE-only patterns fails with a clear error.

**Q10 — Synchronous execution API.**
Context: when a script needs no async capability the kernel finishes in one call, so a synchronous
`execSync` is possible on Node; just-bash has none.
Options: (a) add `execSync`, which throws a typed error at call time if an async capability is configured
(host-backed or custom `fs`, lazy files with async providers, `fetch`, real `sleep`, async custom
commands); (b) keep the API identical to just-bash.
Recommendation: (a).
Consequence: a small documented addition; no per-exec instance isolation option is offered (it would lose
the persistent VFS, section 2.4 point 6).

**Q11 — Target runtimes for v1.**
Context: just-bash ships a browser bundle and runs on edge runtimes. Our Node path compiles the module
synchronously; browsers may restrict synchronous compilation of large modules on the main thread and some
edge runtimes only accept statically imported modules (general platform knowledge, unverified on this
machine). `exec` is async, so lazy async instantiation is compatible with the API.
Options: (a) Node >= 22 only in v1, browser/edge entry later; (b) Node + browser in v1; (c) Node + browser
+ edge workers in v1.
Recommendation: (a), with the host written so that the loader is the only runtime-specific part.
Consequence: with (a) `./browser` is tier C; with (b)/(c) a browser test rig is needed and `execSync` is
Node-only.

**Q12 — Concurrency and trap recovery on one `Bash`.**
Context: the reference lets overlapping `exec` calls interleave. Our fibers support that. After a trap
(kernel bug) linear memory is untrusted.
Options: (a) interleaved fibers; trap poisons the instance; journal of dirty files opt-in; (b) interleaved
fibers; journal on by default (committed files survive a trap, at the cost of a host-side copy of modified
files); (c) FIFO-serialized top-level execs with re-entrant bypass; trap poisons the instance.
Recommendation: (a).
Consequence: with (a) a kernel bug can lose the in-memory VFS of that `Bash` unless the embedder enabled
the journal or uses a host-backed `fs`; traps are release-blocking defects.

**Q13 — Single kernel binary versus companion binaries.**
Context: a separate `.wasm` per companion duplicates the allocator and forces copies or host calls to reach
the core VFS; unused code in one binary costs only validation.
Options: (a) one kernel binary with all tiers, companions as JS facades; (b) one binary per companion;
(c) one workspace producing a `full` and a `slim` binary.
Recommendation: (a), keeping (c) available if the size budget is exceeded.
Consequence: `awk`/`jq` work zero-copy on the VFS; the published core grows with every command added.

**Q14 — Clean-room lane separation.**
Context: just-bash, the Oils spec cases and MoonBash are Apache-2.0; our output is MIT. Build lanes that
read reference sources while writing code or tests risk structural copying.
Options: (a) two rooms: only the design lane reads `tmp/refs/` and writes behavioural specs; build and
test-writing lanes never read it and validate against the spec, POSIX/GNU documentation and the system
bash; (b) one room with a written no-copy rule and review; (c) two rooms for code, one room for tests.
Recommendation: (a), enforced by lane configuration (no `tmp/refs/` in build worktrees) and stated in
every lot brief.
Consequence: specs must be complete enough to build from; design cost per lot rises, licensing risk drops.

---

## 7. Lot breakdown and model routing (sketch; the plan rung refines it)

Sizes: S = 1, M = 3, L = 8 points. Builder tiers and relative price per point (estimate, includes
retries): **Cheap** x1 (Haiku 5.5; Engram Muse 1.3 when its quota is back), **Mid** x2.5 (Codex Sol 6.1 at
medium effort, never high), **Strong** x5 (Sonnet 5.5; Gemini 4 Argon as second source and for
security-heavy lots). Fable 5.2 writes each lot's spec and Carbon reviews each lot; neither builds. Cost =
points x tier factor, in relative units.

| Lot | Scope | Size | Builder (fallback) | Cost | Depends on |
|---|---|---:|---|---:|---|
| L0 | Toolchain pin, node build script, reproducibility check, `make` targets (scope exception) | S | Haiku 5.5 | 1 | Q1, Q5 |
| L1 | ABI and host: exec handles, `HOST_CALL` protocol with epochs, revocable membrane, `view()`, module cache, limit-profile plumbing | L | Sonnet 5.5 (Gemini 4 Argon) | 40 | L0 |
| L2 | Vertical slice: lexer/parser subset, resumable VM, budgeted allocator and chunked buffers, VFS core, **command trait + one-file-per-command registry**, `echo`, realistic cold-start benchmark | L | Sonnet 5.5 (Gemini 4 Argon) | 40 | L1 |
| L3 | Full lexer and parser: grammar of 1.4, `[[ ]]`, `$(( ))`, heredocs, `$'...'`, `normalizeScript`, parse errors | L | Codex Sol 6.1 medium (Sonnet 5.5) | 20 | L2 |
| L4 | Expansion engine: parameter ops incl. `${var@op}`, command/arithmetic/process substitution, brace, tilde, glob/extglob/globstar, word splitting | L | Sonnet 5.5 | 40 | L3, L8a |
| L5 | Interpreter semantics: redirections and fd table, functions, subshells, `set`/`shopt`, errexit, traps subset, arrays, namerefs, special-builtin precedence | L | Sonnet 5.5 | 40 | L4 |
| L6a | State-coupled builtins: `declare`/`typeset`, `local`, `readonly`, `export`, `unset`, `eval`, `source`, `exec`, `command`, `builtin`, `read`, `mapfile` | M | Codex Sol 6.1 medium (Sonnet 5.5) | 7.5 | L5 |
| L6b | Leaf builtins: `:`, `true`, `false`, `cd`, `pwd`, `shift`, `dirs`/`pushd`/`popd`, `getopts`, `let`, `hash`, `help`, `type`, `test`/`[`, `wait`, `compgen`/`complete`/`compopt` | M | Haiku 5.5 | 3 | L2 (API), L5 (final wiring) |
| L7a | VFS complete: default layout, `bash.fs`/`InMemoryFs` proxies, `MountableFs`, lazy files, journal and compaction export | M | Codex Sol 6.1 medium | 7.5 | L2 |
| L7b | Host FS adapters (`OverlayFs`, `ReadWriteFs`, custom `IFileSystem`) with path-security gates and batched calls | M | Gemini 4 Argon (Sonnet 5.5) | 15 | L1 |
| L8a | Regex engine (linear-time, compile caps, bounded backrefs) + glob/extglob matcher | L | Sonnet 5.5 (Gemini 4 Argon) | 40 | L2 |
| L8b | Shared leaf kernels: printf/strftime engine, number parsing, md5/sha1/sha256, base64 | M | Codex Sol 6.1 medium for format; Haiku 5.5 for hashes | 7.5 | L2 |
| L9a | Hard tier-1 commands: `find`, `xargs`, `sort`, `grep` family, `expr`, `tr`, `bash`/`sh`, `timeout`, `time` | L | Codex Sol 6.1 medium (Sonnet 5.5) | 20 | L2, L8a; re-entrant ones need L5 |
| L9b | About 38 leaf tier-1 commands, one file each | L | Haiku 5.5 / Engram Muse 1.3, parallel | 8 | L2, L7a, L8b |
| L10a | Parity harness: `node:test` layout, fixture recorder with the confinement of 4.4, API tier A/B tests | M | Sonnet 5.5 | 15 | L2 |
| L10b | Parity cases per grammar area and per command, written from the specs | L | Haiku 5.5 / Engram Muse 1.3 | 8 | L10a, continuous |
| L11 | Security: adversarial suites, seeded fuzzers (JS and native), membrane and limit-latency tests | L | Gemini 4 Argon (Sonnet 5.5) | 40 | L5, L7b |
| L12 | Benchmark harness, baseline, noise-floored gate, MoonBash adapter | M | Haiku 5.5 | 3 | L2, Q8 |
| L13 | `sed`: parser, addresses, pattern/hold cycle, branches with fuel, `-i` | L | Codex Sol 6.1 medium (Sonnet 5.5) | 20 | L8a |
| L14 | `awk`: lexer, parser, VM, built-in functions | L | Sonnet 5.5; built-ins by Haiku 5.5 | 40 | L8a, L8b |
| L15a | `jq`: JSON parser, generator/backtracking evaluator, builtins | L | Sonnet 5.5; builtins by Haiku 5.5 | 40 | L8a |
| L15b | `yq`: YAML reader/writer on the jq evaluator | M | Codex Sol 6.1 medium | 7.5 | L15a, Q4 |
| L16a | inflate/deflate, gzip family, tar, `diff` | L | Gemini 4 Argon (Sonnet 5.5) | 40 | L2 |
| L16b | `@sentropic/net`: DNS-pinned fetch adapter + `curl` | M | Gemini 4 Argon | 15 | L1 |
| L16c | Leaf tier-3 text and fs commands: `comm join column fold expand unexpand split strings od file tree du` | M | Haiku 5.5 / Engram Muse 1.3 | 3 | L2, L7a |
| L16d | `rg` on the grep kernel | M | Codex Sol 6.1 medium | 7.5 | L9a |
| L17a | `Sandbox` facade, CLI, standalone facades (`@sentropic/grep`, `ls`, `awk`, `jq`) | M | Haiku 5.5 | 3 | parity of tiers A/B |
| L17b | AST export, transform pipeline, `serialize` (only if Q7 = b) | M | Sonnet 5.5 | 15 | L3 |
| L18 | `@sentropic/make`, after parity | L | Sonnet 5.5 | 40 | parity |

Totals (estimate): 160 points, 587 cost units. By tier: Strong 92 points (58% of points, 78% of cost), Mid
39 points (24%, 17%), Cheap 29 points (18%, 5%). Routing everything to the Strong tier would cost 800
units; the routing above saves about 27%, and the Mid tier is where further savings are tested first (a
Mid lot that fails its gate twice is re-routed to its fallback, a Strong lot is never downgraded mid-way).

Critical path (sequential, 44 points): `L0 -> L1 -> L2 -> L3 -> L4 -> L5 -> L6a`, then the parity gate.
Everything else is decoupled from L3-L5 and starts right after L2:
- `L7a`, `L7b`, `L8a`, `L8b`, `L10a`, `L12`, `L16a`, `L16b` in parallel with L3.
- Leaf commands `L9b` and `L16c` need only the command trait (L2), the VFS (L7a) and the shared kernels
  (L8b): they are tested through the command trait with byte inputs, without a full shell.
- `L9a`, `L13`, `L14`, `L15a` start when the regex engine (L8a) lands.
- One file per command and a generated registry (defined in L2) keep parallel lanes from editing the same
  file.
- L2 is the go/no-go point for Q1 and Q2: it produces the first real cold-start, size and `wasm-opt`
  numbers before any volume work starts.

Spend and velocity rules for the plan rung: each lot brief is self-contained (Q14: no access to
`tmp/refs/`), carries its test list, and is sized so that a builder session ends in a reviewable PR;
Carbon reviews Strong and security lots in full and samples Cheap lots (one command in five) once the
first three of a family pass unchanged.

---

## 8. Sources and reproducibility of this study

- Reference trees: `tmp/refs/just-bash` (v3.6.0 sources), `tmp/refs/MoonBash` (v0.1.0 sources).
- Counts in section 1 come from one-off node scripts over those trees; they size the surface.
- Measurements in sections 1.7 and 2.1 come from one-off node scripts under the git-ignored `tmp/bench/`
  (just-bash 3.6.0 from npm, synthetic WAT modules assembled with wabt 1.0.36). They are a single-machine
  snapshot and must be re-measured by the lot L12 harness before any budget becomes a gate.
- Items labelled "estimate" (flag coverage classes, kernel size, lot sizes, tier price factors) are
  unverified. Statements about browser and edge-runtime compilation limits are unverified here.

---

## Appendix A — Review reconciliation

Each finding was checked against `tmp/refs/` or re-measured before being applied.

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
