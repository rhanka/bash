# Docs: Design ladder for the bash sandbox (STUDY, EVOL, plan)

## Objective
Produce the reviewed design ladder of `@sentropic/bash`: STUDY, then EVOL spec, then a lot-based plan with per-lot cost, model routing and the benchmark protocol.

## Scope / Guardrails
- Scope limited to `spec/`, `plan/` and Track records for the design act.
- Make-only workflow; branch work in worktree `tmp/design` (stacked on `chore/harness-adoption`).
- Commits via `make commit`; no AI/agent attribution (hook + CI).
- Zero Python. All new text in English. Reference repos analysed under the git-ignored `tmp/refs/`, never copied.

## Branch Scope Boundaries (MANDATORY)
- **Allowed Paths (implementation scope)**:
  - `BRANCH.md`
  - `spec/**`
  - `plan/**`
  - `.track/**`
- **Forbidden Paths (must not change in this branch)**:
  - `Makefile`
  - `package.json`
  - `.github/workflows/**`
  - `.githooks/**`
  - `LICENSE`
- **Conditional Paths (allowed only with explicit exception when not already listed in Allowed Paths)**:
  - `README.md`
  - `rules/**`
- **Exception process**:
  - Declare exception ID `BRxx-EXn` in `## Feedback Loop` before touching any conditional/forbidden path.
  - Include reason, impact, and rollback strategy.

## Feedback Loop
- owner: `acknowledge` — model mapping per lot is owned by the design lane (Fable 5.2) and the review lane (Carbon), accountable for spend and velocity; candidates Haiku 5.5, Sonnet 5.5, Gemini 4 Argon, Engram Muse 1.3 (quota back soon), Codex Sol 6.1 below high.
- owner: `acknowledge` — GitHub repo public; npm publish as pre-release (0.x, `next` tag) once the core passes parity tests.
- owner: `acknowledge` (15/11) — study gates O1-O8 answered (Track decisions). Governance: the owner owns the upstream (just-bash) features and the perf targets; every other technical choice is settled by a two-pass consensus of Fable 5.2 and Carbon, and each recommendation names who poses it.
- owner: `acknowledge` (15/11) — technical questions to the owner remain welcome, but each one first states what upstream (just-bash) does, with a source path.
- `BR01-EX1` — `rules/MASTER.md` (conditional path). Reason: owner decision O6 (15/11): MoonBash is a feature-alignment reference, not a perf benchmark target; building it with `moon` is rejected. Impact: the perf bench runs against just-bash only; a feature matrix covers MoonBash. Rollback: revert the WASM-first bullet change in `rules/MASTER.md`.

## Plan / Todo (lot-based)
- [x] **Lot 0 — Baseline & constraints**
  - [x] Worktree `tmp/design`; reference clones under `tmp/refs/`.
- [x] **Lot 1 — STUDY (Fable 5.2)**
  - [x] `spec/SPEC_STUDY_BASH_SANDBOX.md`
- [ ] **Lot 2 — STUDY review (Carbon) + owner gates**
  - [ ] Adversarial review reconciled into the STUDY.
  - [ ] Owner answers Q1-Q10 recorded in Track.
- [ ] **Lot 3 — EVOL spec (Fable 5.2) + review (Carbon)**
  - [ ] `spec/SPEC_EVOL_BASH_SANDBOX.md` with numbered decisions D1..Dn.
- [ ] **Lot 4 — Plan (Fable 5.2) + review (Carbon)**
  - [ ] Lot-based plan with per-lot cost estimate, model routing and benchmark protocol.
- [ ] **Lot N — Final validation**
  - [ ] `make check`; owner sign-off; remove `BRANCH.md` in the last commit before merge.
