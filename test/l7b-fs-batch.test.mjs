import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { OverlayFs, ReadWriteFs } from "@sentropic/bash";
import { serveFsBatch } from "../host/core/fs-batch.js";
import { Scope } from "../host/core/membrane.js";
import { profileLimits } from "../host/core/limits.js";
import { bash, dec, enc } from "./kt.mjs";
import { repoRoot } from "./helpers.mjs";

const op = (tag, val) => ({ tag, val: typeof val === "string" ? enc(val) : val });
const pathOp = (tag, path, options) => op(tag, { path: enc(path), ...options });
const pairOp = (tag, source, target, recursive = false) => op(tag, { source: enc(source), target: enc(target), recursive });

async function fixture(run) {
  const root = mkdtempSync(join(repoRoot, "tmp", "test-host-fs-batch-"));
  try {
    await run(root);
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
}

async function serve(fs, ops, overrides = {}, headroom = 8 << 20) {
  const B = { mounts: new Map([[1, fs]]), limits: { ...profileLimits("contain"), ...overrides }, headroom };
  function* extension(_x, start) {
    const scope = new Scope();
    try {
      return { outcome: yield start(scope) };
    } finally {
      scope.revoke();
    }
  }
  const gen = serveFsBatch({ B }, { mountId: 1, ops }, extension);
  const start = gen.next();
  return (start.done ? start : gen.next(await start.value)).value.outcome;
}

test("all 21 operations and all 8 result variants are dispatched in order", () => fixture(async (root) => {
  const fs = new ReadWriteFs({ root, allowSymlinks: true });
  const request = [
    pathOp("mkdir", "/d", { recursive: false, mode: 0o700 }),
    pathOp("create-exclusive", "/d/f", { mode: 0o600, directory: false }),
    pathOp("write-file", "/d/f", { offset: 0n, truncate: true, data: enc("abcd") }),
    pathOp("write-file", "/d/f", { offset: undefined, truncate: false, data: enc("ef") }),
    pathOp("write-file", "/d/f", { offset: 1n, truncate: true, data: enc("XYZ"), mode: 0o640, mtimeMs: 2000n }),
    pairOp("hard-link", "/d/f", "/d/h"),
    pairOp("symlink", "f", "/d/s"),
    op("stat", "/d/s"), op("lstat", "/d/s"), op("exists", "/d/f"),
    op("read-link", "/d/s"), op("real-path", "/d/s"),
    op("read-dir", "/d"), op("read-dir-plus", "/d"),
    op("walk", { root: enc("/"), maxDepth: 4, maxEntries: 30 }), op("all-paths"),
    pathOp("read-file", "/d/f", { offset: 1n, len: 2 }),
    pathOp("write-lazy", "/d/l", { mode: 0o600 }),
    pairOp("copy", "/d/f", "/d/c"), pairOp("rename", "/d/c", "/d/r"),
    pathOp("chmod", "/d/r", { mode: 0o644 }),
    pathOp("utimes", "/d/r", { atimeMs: 1000n, mtimeMs: 3000n }),
    pathOp("remove", "/d/h", { force: false, recursive: false }),
  ];
  const outcome = await serve(fs, request);
  assert.equal(outcome.tag, "fs");
  assert.deepEqual(outcome.val.map((x) => x.tag === "ok" ? x.val.tag : x.val.code), [
    "none", "none", "none", "none", "none", "none", "none", "info", "info", "flag",
    "path", "path", "entries", "entries-plus", "entries-plus", "paths", "data", "unsupported",
    "none", "none", "none", "none", "none",
  ]);
  const value = (i) => outcome.val[i].val.val;
  assert.equal(value(7).kind, "file");
  assert.equal(value(7).mtimeMs, 2000n);
  assert.equal(value(7).ino, BigInt(statSync(join(root, "d", "f")).ino));
  assert.equal(value(8).kind, "symlink");
  assert.equal(value(9), true);
  assert.equal(dec(value(10)), "f");
  assert.equal(dec(value(11)), "/d/f");
  assert.deepEqual(value(12).map(({ name }) => dec(name)), ["f", "h", "s"]);
  assert.deepEqual(value(13).map(({ path, info }) => [dec(path), info.kind]), [
    ["/d/f", "file"], ["/d/h", "file"], ["/d/s", "symlink"],
  ]);
  assert.deepEqual(value(14).map(({ path }) => dec(path)), ["/d", "/d/f", "/d/h", "/d/s"]);
  assert.deepEqual(value(15).map(dec), ["/d", "/d/f", "/d/h", "/d/s"]);
  assert.equal(dec(value(16)), "XY");
  assert.deepEqual(outcome.val[17].val, { code: "unsupported", path: enc("/d/l") });
  assert.equal(await fs.readFile("/d/r"), "aXYZ");
  assert.equal((await fs.stat("/d/r")).mtime.getTime(), 3000);
  assert.equal(await fs.exists("/d/h"), false);
}));

test("batched walks and reads stop at the requested and profile limits", () => fixture(async (root) => {
  const fs = new ReadWriteFs({ root });
  await fs.writeFile("/d/f", "one");
  await fs.writeFile("/z", "z".repeat(2048));
  const request = op("walk", { root: enc("/"), maxDepth: 2, maxEntries: 8 });
  const paths = (outcome) => outcome.val[0].val.val.map(({ path }) => dec(path));
  assert.deepEqual(paths(await serve(fs, [request], { maxTraversalDepth: 1 })), ["/d", "/z"]);
  assert.deepEqual(paths(await serve(fs, [request], { maxTraversalEntries: 2 })), ["/d", "/z"]);
  assert.deepEqual(paths(await serve(fs, [request], { maxTraversalWork: 1 })), ["/d"]);
  assert.deepEqual(paths(await serve(fs, [request], {}, 250)), ["/d"]);
  const read = await serve(fs, [pathOp("read-file", "/z", { offset: 10n, len: 1000 })], {}, 400);
  assert.equal(read.val[0].val.val.length, 134);
}));

test("filesystem batches fall back to required custom object methods and map errors safely", async () => {
  const touched = [];
  const fs = {
    exists: async () => false,
    stat: async (path) => {
      if (path === "/") return { isDirectory: true };
      throw Object.assign(new Error("irrelevant host detail"), { code: "ELOOP" });
    },
    mkdir: async (path, options) => { touched.push(["mkdir", path, options.recursive]); },
    chmod: async (path, mode) => { touched.push(["chmod", path, mode]); },
    readdir: async (path) => { touched.push(["readdir", path]); return ["x"]; },
    lstat: async () => ({ isSymbolicLink: true }),
    writeFile: async (path, data) => { touched.push(["write", path, data.length]); },
  };
  const r = await serve(fs, [
    pathOp("create-exclusive", "/x", { mode: 0o600, directory: false }),
    pathOp("create-exclusive", "/d", { mode: 0o700, directory: true }),
    op("read-dir", "/"),
    op("stat", "/loop"),
  ]);
  assert.deepEqual(touched, [
    ["write", "/x", 0], ["chmod", "/x", 0o600], ["mkdir", "/d", false], ["chmod", "/d", 0o700], ["readdir", "/"],
  ]);
  assert.deepEqual(r.val.map((x) => x.val), [
    { tag: "none" }, { tag: "none" }, { tag: "entries", val: [{ name: enc("x"), kind: "symlink" }] },
    { code: "link-loop", path: enc("/loop") },
  ]);
});

test("kernel-test batch returns six typed results and files cross the ABI in chunks", () => fixture(async (root) => {
  const fs = new OverlayFs({ root });
  const body = "x".repeat((1 << 20) + 17);
  writeFileSync(join(root, "large"), body);
  const seen = [];
  const b = bash({ fs }, { onHostCall: (request) => { seen.push(request.tag); } });
  const batch = await b.exec("__fsbatch 1");
  assert.equal(batch.exitCode, 0, batch.stderr);
  assert.equal(batch.stdout, "ok none\nok none\nok none\nok entries\nok info\nok none\n");
  assert.deepEqual(seen, ["fs-batch"]);
  const read = await b.exec("__cat " + fs.getMountPoint() + "/large");
  assert.equal(read.exitCode, 0, read.stderr);
  assert.equal(read.stdout.length, body.length);
  assert.ok(read.stdout === body);
  assert.deepEqual(seen, ["fs-batch", "fs-batch", "fs-batch"]);
  const text = "雪".repeat(360_000);
  const write = await b.exec("__write /large-output " + text);
  assert.equal(write.exitCode, 0, write.stderr);
  assert.ok(await b.readFile("/large-output") === text);
  assert.deepEqual(seen, ["fs-batch", "fs-batch", "fs-batch", "fs-batch"]);
}));
