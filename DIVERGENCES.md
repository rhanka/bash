# Divergences from the reference

This list is the complete set of intended differences between `@sentropic/bash` and just-bash 3.6.0; any other difference is a defect.

| # | Difference from just-bash 3.6.0 | Source of the decision |
|---|---|---|
| V1 | Default limit profile is `contain` (reference: `normal`); `contain` and `micro` exist | Owner, O1 |
| V2 | Not available in v1: `python3`/`python`, `js-exec`/`node` (until their companions, D25), `sqlite3`, `xan`, `html-to-markdown`, `@just-bash/executor`. With `python` or `javascript` set, construction succeeds and the commands return exit 127, as in the reference's browser bundle; the limits specific to those runtimes are accepted and unused | Owner, A4 and 15/11 |
| V3 | Not available in v1: `parse`, AST types, transform pipeline and plugins | Consensus, B3 |
| V4 | The browser entry runs `tar` and `yq`, which the reference's browser bundle excludes, and `gzip`/`gunzip`/`zcat`, which the reference registers but which fail at run time in a browser | Owner, P1 |
| V5 | Deferred to companions: in the v1 kernel `defenseInDepth` is accepted and has no effect, and `DefenseInDepthBox`, `SecurityViolationLogger`, `SecurityViolationError`, `createConsoleViolationCallback` and their types are not exported; they are exported by the companion packages (D25) | Owner, 15/11 |
| V8 | Command line: one binary `stpsh` instead of `just-bash` and `just-bash-shell`; by default it mounts no host directory (the reference mounts the current directory), `--root` and `--just-bash` restore the reference's behaviour (D26) | Owner, 15/11 |
| V6 | Additions that do not alter existing behaviour and add no oracle exclusion: `execSync`, `precompile`, `compact`, journal, profiles `contain`/`micro`, byte helpers and `MethodNotAllowedError` exported from `./browser` as well, the component and its WIT | Consensus |
