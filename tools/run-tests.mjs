#!/usr/bin/env node
// The single test runner: every test/**/*.test.mjs, recursively, so a subdirectory suite is never skipped.
// Used by `make test` and by tools/check.mjs. Usage: node tools/run-tests.mjs
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { repoRoot, walk } from "./lib.mjs";

export function testFiles(root = repoRoot) {
  return walk(join(root, "test"), (p) => p.endsWith(".test.mjs")).sort();
}

export function runTests(root = repoRoot) {
  const files = testFiles(root);
  if (files.length === 0) {
    console.error("run-tests: no test files found under test/");
    return 1;
  }
  return spawnSync("node", ["--test", ...files], { cwd: root, stdio: "inherit" }).status ?? 1;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(runTests());
