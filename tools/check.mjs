#!/usr/bin/env node
// Runs the eight gates, then the node:test suite. Usage: node tools/check.mjs [--quick] [--only G<k>]
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { repoRoot, toolsDir } from "./lib.mjs";
import { GATES } from "./gates.mjs";

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
if (failed) process.exit(1);
if (!only) {
  const r = spawnSync("node", ["--test", "test/*.test.mjs"], { cwd: repoRoot, stdio: "inherit" });
  process.exit(r.status ?? 1);
}
