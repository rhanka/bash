# Feature: L0 component pipeline and package skeleton

## Objective
Reproducible pipeline: no_std Rust crate -> WASI 0.2 component with zero imports -> dependency-free JS
bindings -> eight gates -> installable npm tarball. Ends with the Gate L0 report. Spec: `spec/lots/L0.md`.

## Scope / Guardrails
- Scope limited to the L0 lot spec section 3.
- Make-only workflow; branch work in worktree `tmp/l0-pipeline`.
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `rust-toolchain.toml`, `Cargo.toml`, `Cargo.lock`
  - `kernel/**`, `probe/**`, `wit/**`, `tools/**`, `host/**`, `types/**`, `test/**`
  - `DIVERGENCES.md`, `.gitignore`, `BRANCH.md`, `plan/reports/GATE_L0.md`
- **Forbidden Paths (must not change in this branch)**:
  - `LICENSE`, `.githooks/**`, `rules/**`, `spec/**`, `.track/**`, `harness.profile.json`
  - the rest of `plan/**`
- **Conditional Paths (allowed only with explicit exception)**:
  - `Makefile` (BR1-EX1), `package.json` (BR1-EX2), `.github/workflows/check.yml` (BR1-EX3)
- **Exception process**:
  - Declared in `## Feedback Loop` before touching the path.

## Feedback Loop
- BR1-EX1 `Makefile`. Reason: add `build`, `check-quick`; make `test`, `check`, `bench` delegate to `tools/`. Impact: existing targets keep names and behaviour. Rollback: revert the file.
- BR1-EX2 `package.json`. Reason: entry points, `files`, engine range. Impact: no dependency added; `private` stays true. Rollback: revert the file.
- BR1-EX3 `.github/workflows/check.yml`. Reason: run `make check` on pull requests. Impact: new file. Rollback: delete the file.

## Plan / Todo (lot-based)
- [ ] **Lot 0 — Baseline**: BRANCH.md, scope exceptions
- [ ] **Lot 1 — Link proof** (spec 6.2)
- [ ] **Lot 2 — WIT, kernel, probe, tools, package skeleton, CI, tests**
- [ ] **Lot N — Final validation**: `make check`, Gate L0 report
