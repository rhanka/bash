#!/usr/bin/env node
// Runs the eight gates, the kernel core check, cargo test, then the node:test suite. Usage: node tools/check.mjs [--quick] [--only G<k>]
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { coreImports, foreignImports, repoRoot, toolsDir } from "./lib.mjs";
import { GATES } from "./gates.mjs";
import { runTests } from "./run-tests.mjs";

const argv = process.argv.slice(2);
const quick = argv.includes("--quick");
const oi = argv.indexOf("--only");
const only = oi >= 0 ? argv[oi + 1] : null;

const ctx = { root: repoRoot, out: join(repoRoot, "dist"), toolsDir: toolsDir(), quick };
let failed = false;
for (const [id, name, fn] of GATES) {
  if (only && only !== id) continue;
  ctx.detail = null;
  let reason;
  try {
    reason = await fn(ctx);
  } catch (e) {
    reason = `error: ${e.message}`;
  }
  if (reason === null) console.log(`${id} ${name}: OK`);
  else if (reason.startsWith("SKIPPED")) console.log(`${id} ${name}: ${reason}`);
  else {
    console.log(`${id} ${name}: FAIL ${reason}`);
    failed = true;
  }
  for (const line of ctx.detail ?? []) console.log(line);
}
// The kernel core module (built by tools/build.mjs) must exist and have no import.
if (!only) {
  const core = join(ctx.out, "kernel.core.wasm");
  if (!existsSync(core)) {
    console.log("K1 kernel core: FAIL dist/kernel.core.wasm is missing");
    failed = true;
  } else {
    const imports = foreignImports(coreImports(core, ctx.toolsDir));
    if (imports.length > 0) {
      console.log(`K1 kernel core: FAIL ${imports.length} import(s), first: ${imports[0]}`);
      failed = true;
    } else console.log(`K1 kernel core zero-import: OK (${statSync(core).size} bytes)`);
  }
}
if (failed) process.exit(1);
if (!only) {
  // Native unit and integration tests of the Rust workspace, then the node tests.
  const cargo = spawnSync("cargo", ["test", "--workspace", "--locked", "--offline", "--quiet"], {
    cwd: repoRoot,
    stdio: "inherit",
    env: { ...process.env, CARGO_TARGET_DIR: join(repoRoot, "dist", "target") },
  });
  if (cargo.status !== 0) {
    console.log("cargo test: FAIL");
    process.exit(cargo.status ?? 1);
  }
  const cmds = spawnSync("cargo", ["test", "-p", "sentropic-bash-kernel", "--lib", "--features", "test-commands", "--offline", "--quiet"], {
    cwd: repoRoot,
    stdio: "inherit",
    env: { ...process.env, CARGO_TARGET_DIR: join(repoRoot, "dist", "target") },
  });
  if (cmds.status !== 0) {
    console.log("cargo test (test-commands): FAIL");
    process.exit(cmds.status ?? 1);
  }
  console.log("cargo test: OK");
  const bind = spawnSync("node", [join(repoRoot, "tools", "build.mjs"), "--check-bindings", "--component", "kernel"], { cwd: repoRoot, stdio: "inherit" });
  if (bind.status !== 0) {
    console.log("bindings regeneration check: FAIL");
    process.exit(bind.status ?? 1);
  }
  process.exit(runTests());
}
