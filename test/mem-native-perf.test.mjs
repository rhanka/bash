// Timing parts of gates 10 and 11 of lot L1a-2: the B-tree maps natively in a release build.
import { test } from "node:test";
import assert from "node:assert/strict";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { repoRoot, scratch } from "./helpers.mjs";

test("1 000 000 KMap and KInsertMap operations stay within their time limits (release build)", () => {
  const target = join(repoRoot, "tmp", "perf-target");
  mkdirSync(target, { recursive: true });
  const env = { ...process.env, CARGO_TARGET_DIR: target };
  delete env.CARGO_ENCODED_RUSTFLAGS;
  const r = spawnSync("cargo", ["test", "--release", "--locked", "--offline", "-p", "sentropic-bash-kernel", "--test", "perf", "--", "--nocapture"], {
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

test("native timing and wasm allocator builds reject an outdated Cargo.lock", () => {
  const cwd = scratch("mem-stale-lock");
  for (const file of ["kernel/Cargo.toml", "test/helpers.mjs", "test/memwasm.mjs", "test/mem-native-perf.test.mjs"]) {
    const dest = join(cwd, file);
    mkdirSync(dirname(dest), { recursive: true });
    copyFileSync(join(repoRoot, file), dest);
  }
  writeFileSync(join(cwd, "Cargo.toml"), '[workspace]\nmembers = ["kernel"]\nresolver = "2"\n');
  mkdirSync(join(cwd, "kernel", "src"));
  writeFileSync(join(cwd, "kernel", "src", "lib.rs"), "");
  mkdirSync(join(cwd, "kernel", "tests"));
  writeFileSync(join(cwd, "kernel", "tests", "perf.rs"), "");
  const lock = join(cwd, "Cargo.lock");
  const stale = "version = 4\n";
  const commands = [
    ["--test", "--test-name-pattern=operations stay within", "test/mem-native-perf.test.mjs"],
    ["--input-type=module", "--eval", 'import { buildMemModule } from "./test/memwasm.mjs"; buildMemModule();'],
  ];
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  for (const args of commands) {
    writeFileSync(lock, stale);
    const r = spawnSync(process.execPath, args, { cwd, env, encoding: "utf8" });
    assert.notEqual(r.status, 0);
    assert.match(r.stdout + r.stderr, /lock file [^\n]*--locked was passed/);
    assert.equal(readFileSync(lock, "utf8"), stale);
  }
});
