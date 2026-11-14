---
description: "Consolidated development rules for @sentropic/bash (sentropic h2a harness), loaded in every conversation"
alwaysApply: true
paths: ["**/*"]
tags: [master]
---

# MASTER RULES — @sentropic/bash

`@sentropic/bash` is an MIT-licensed, WASM-first bash sandbox. It is a clean-room reimplementation that
reaches functional parity with `vercel-labs/just-bash`, and it is optimized for performance (startup and
per-builtin cost) and for security. Companion packages are allowed (`@sentropic/ls`, `@sentropic/grep`,
`@sentropic/make`, ...). Repo-specific policy lives in `harness.profile.json`.

## License and clean-room (MANDATORY)
- Everything here is MIT (`LICENSE`). Inbound license equals outbound license.
- Clean-room reimplementation. Reference repos (just-bash, MoonBash, ...) may be analysed in full for
  behaviour, architecture and test intent.
- No source code is copied from a reference repo. Reference test suites are not vendored unless their
  license allows it and the attribution is recorded. Parity is proven by our own functional tests.
- Reference clones live outside the repo or under the git-ignored `tmp/`.

## Zero third-party dependency (MANDATORY)
- `dependencies` stays empty, and no third-party TypeScript or JS package is allowed at runtime.
- Build and test tooling must be justified in the PR. Prefer Node built-ins (`node:test`, `node:assert`)
  and native toolchains (the WASM toolchain is an owner decision).
- Internal `@sentropic/*` packages from this repo are allowed.

## WASM-first and performance (MANDATORY)
- Hot paths (parser, interpreter, builtins) target WASM. Startup time and per-function cost are tracked by
  a reproducible benchmark against `just-bash` and MoonBash on the same machine.
- A perf regression greater than the agreed budget fails the gate.

## Security (MANDATORY)
- Default-deny sandbox: no host FS, network or process access unless an explicit capability is granted.
- Fuzz and adversarial tests cover the parser and expansion.

## No Docker, zero Python (MANDATORY)
- No Docker for build or test. Toolchains run natively, pinned by version.
- No Python file, script, job or image anywhere.

## Temp files (MANDATORY)
- Never write to `/tmp`. Use the TMPDIR given by h2a or the lane, or the git-ignored `tmp/`.
- Remove a worktree right after its PR merges.
- Follow the machine janitor protocol: `/home/antoinefa/src/ubuntu/janitor/PROTOCOL.md`.

## Branch scope control (MANDATORY)
- Every branch declares `Allowed Paths`, `Forbidden Paths` and `Conditional Paths` in `BRANCH.md`, based on
  `plan/BRANCH_TEMPLATE.md`.
- Forbidden defaults are listed in `harness.profile.json`. A scope exception requires `BRxx-EXn` with a
  rationale, its impact and a rollback.
- Branch work happens in `tmp/<slug>` worktrees. `main` only changes through reviewed PRs.

## Commit discipline (MANDATORY)
- Install the hook once per clone: `git config core.hooksPath .githooks`.
- Selective staging only. Atomic commits.
- **No AI or agent attribution, ever.** That means no `Co-Authored-By` naming an AI, no `Claude-Session`,
  no "Generated with ...", no 🤖, and no AI vendor author or committer identity. The commit-msg hook
  enforces it (`tools/check-attribution.sh`), and this rule overrides any agent-harness default.

## Track
- Realization state lives in Track (the `track` CLI is the single writer; `.track/` is append-only).

## Language policy
- Code, comments, commits, PR titles and docs are in **English**. Discussion with the owner is in
  **French**.
- Never use the words "honest" or "honnête" or their derivatives. Use factual status terms (`unknown`,
  `unverified`, `partial`, ...).
