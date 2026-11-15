// Timing parts of gates 10 and 11 of lot L1a-2: the B-tree maps natively in a release build.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { repoRoot } from "./helpers.mjs";

test("1 000 000 KMap and KInsertMap operations stay within their time limits (release build)", () => {
  const target = join(repoRoot, "tmp", "perf-target");
  mkdirSync(target, { recursive: true });
  const env = { ...process.env, CARGO_TARGET_DIR: target };
  delete env.CARGO_ENCODED_RUSTFLAGS;
  const r = spawnSync("cargo", ["test", "--release", "--offline", "-p", "sentropic-bash-kernel", "--test", "perf", "--", "--nocapture"], {
    cwd: repoRoot,
    env,
    encoding: "utf8",
  });
  const lines = (r.stdout + r.stderr).split("\n").filter((l) => l.startsWith("perf "));
  for (const l of lines) console.log(`# ${l}`);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.ok(lines.some((l) => l.includes("KMap insert+lookup+remove")));
  assert.ok(lines.some((l) => l.includes("KInsertMap insert+remove")));
});
