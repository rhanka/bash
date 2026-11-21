#!/usr/bin/env node
// The single test runner: every test/**/*.test.mjs, recursively, so a subdirectory suite is never skipped.
// Used by `make test` and by tools/check.mjs. Usage: node tools/run-tests.mjs
//
// Runtime enforcement (layer B of the sandbox hardening): every test process runs with
// --disallow-code-generation-from-strings (eval, Function and their kin throw, whatever their syntax) and
// with the module resolve hook of tools/test-hooks.mjs preloaded. A test that legitimately needs code
// generation must be listed, with a reason, in tools/codegen-allow.json; it then runs without the flag.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { repoRoot, walk } from "./lib.mjs";

const here = dirname(fileURLToPath(import.meta.url));
export const CODEGEN_FLAG = "--disallow-code-generation-from-strings";
export const HOOK_PRELOAD = pathToFileURL(join(here, "test-hooks-preload.mjs")).href;

export function testFiles(root = repoRoot) {
  return walk(join(root, "test"), (p) => p.endsWith(".test.mjs")).sort();
}

/** The allow-list of tools/codegen-allow.json under `root`, as absolute paths. Throws on a malformed entry. */
export function codegenAllowed(root = repoRoot) {
  const f = join(root, "tools", "codegen-allow.json");
  if (!existsSync(f)) return new Set();
  const out = new Set();
  for (const e of JSON.parse(readFileSync(f, "utf8")).allow ?? []) {
    if (typeof e?.file !== "string" || typeof e?.reason !== "string" || e.reason.trim() === "") throw new Error(`tools/codegen-allow.json: every entry needs a file and a non-empty reason: ${JSON.stringify(e)}`);
    out.add(join(root, e.file));
  }
  return out;
}

export function runTests(root = repoRoot, { stdio = "inherit" } = {}) {
  const files = testFiles(root);
  if (files.length === 0) {
    console.error("run-tests: no test files found under test/");
    return 1;
  }
  const allowed = codegenAllowed(root);
  const strict = files.filter((f) => !allowed.has(f));
  const loose = files.filter((f) => allowed.has(f));
  const env = { ...process.env, SENTROPIC_HOOK_ROOT: root };
  delete env.NODE_TEST_CONTEXT; // when started from inside a test, the child must run as a fresh top-level runner
  let status = 0;
  for (const [flags, list] of [[[CODEGEN_FLAG], strict], [[], loose]]) {
    if (list.length === 0) continue;
    const r = spawnSync("node", [...flags, "--import", HOOK_PRELOAD, "--test", ...list], { cwd: root, stdio, env });
    status ||= r.status ?? 1;
  }
  return status;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(runTests());
