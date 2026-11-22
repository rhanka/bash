// Gate L2 exit gates 1-3: the Wasm-owned default snapshot (spec/lots/L2c.md section 7).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createFacade } from "../host/core/facade.js";
import { linkEngine } from "../host/core/abi/binding.js";
import { INTERNAL } from "../host/core/instance.js";
import { nodeLoader } from "../host/node/load.js";
import { repoRoot } from "./helpers.mjs";

const prodCore = () => new WebAssembly.Module(readFileSync(join(repoRoot, "dist/js/kernel/kernel.core.wasm")));
const testCore = () => new WebAssembly.Module(readFileSync(join(repoRoot, "dist/test/js/kernel-test/kernel-test.core.wasm")));

/** A provenance-clean loader for a compiled core module (no hooks, no custom module). */
const loaderFor = (mod) => ({
  ...nodeLoader,
  syncModule: () => mod,
  asyncModule: () => Promise.resolve(mod),
  precompile: () => Promise.resolve(),
});

/** Builds a shell, recording the engine calls when `observe` is set. */
async function build(options = {}, mod = prodCore(), hooks = {}) {
  let seen = null;
  const { Bash } = createFacade(loaderFor(mod));
  const bash = new Bash({ ...options, [INTERNAL]: { inspect: (s) => { seen = s; }, ...hooks } });
  return { bash, seen: () => seen ?? assert.fail("the store was never linked") };
}

const setupNames = (calls) => calls.filter((c) => c === "__restore_default" || c === "configure" ||
  c === "[static]vfs.create" || c === "[static]shell.create");

test("the predicate restores eligible defaults and keeps the ordinary setup otherwise", async () => {
  for (const options of [{}, { logger: { info() {}, debug() {} } },
    { executionLimitProfile: "contain" }, { executionLimitProfile: "contain", executionLimits: {} }]) {
    const { bash, seen } = await build(options);
    assert.deepEqual(setupNames(seen().calls), ["__restore_default"]);
    assert.equal((await bash.exec("echo hi")).stdout, "hi\n");
  }
  const disqualified = [
    { files: {} }, { env: {} }, { cwd: "/" }, { fs: {} }, { commands: [] }, { commands: ["echo"] },
    { customCommands: [] }, { processInfo: {} }, { sleep() {} }, { fetch() {} }, { network: {} },
    { python: true }, { javascript: {} }, { trace() {} }, { coverage: { hit() {} } },
    { defenseInDepth: {} }, { executionLimitProfile: "normal" }, { executionLimitProfile: "micro" },
    { executionLimitProfile: "hardened" }, { executionLimits: { maxCallDepth: 5 } },
    { maxCallDepth: 5 }, { maxCommandCount: 5 }, { maxLoopIterations: 5 },
  ];
  for (const options of disqualified) {
    const { bash, seen } = await build(options);
    const setup = setupNames(seen().calls);
    assert.deepEqual(setup, ["configure", "[static]vfs.create", "[static]shell.create"],
      JSON.stringify(options));
    assert.ok(!seen().calls.includes("__restore_default"), JSON.stringify(options));
    // `{ commands: [] }` registers nothing, so even echo is absent; the rest behave by default.
    const result = await bash.exec("echo hi");
    assert.equal(result.stdout, options.commands?.length === 0 ? "" : "hi\n", JSON.stringify(options));
  }
  const { Bash } = createFacade(loaderFor(prodCore()));
  assert.throws(() => new Bash({ unknownOption: 1 }), /unknown option/);
  assert.throws(() => new Bash({ cwd: 42 }), /cwd must be a string/);
});

test("internal overrides and caller-supplied modules use the ordinary path", async () => {
  const { seen } = await build({}, prodCore(), { ceilingBytes: 1 << 28 });
  assert.deepEqual(setupNames(seen().calls), ["configure", "[static]vfs.create", "[static]shell.create"]);
  const kernel = await import("../dist/js/kernel/kernel.js");
  const forModule = async (mod) => build({}, prodCore(), { kernel: { instantiate: kernel.instantiate, module: mod } });
  for (const mod of [prodCore(), testCore()]) {
    const { bash, seen: seenModule } = await forModule(mod);
    assert.ok(!seenModule().calls.includes("__restore_default"));
    assert.equal((await bash.exec("echo hi")).stdout, "hi\n");
  }
  const { Bash } = createFacade(loaderFor(prodCore()));
  const provider = new Bash({ [INTERNAL]: { moduleProvider: () => Promise.resolve(prodCore()) } });
  assert.equal((await provider.exec("echo hi")).stdout, "hi\n");
});

test("a restored shell matches ordinary setup on every observable", async () => {
  const ordinary = await build({}, prodCore(), { snapshotCapture: true });
  const restored = await build({});
  assert.deepEqual(setupNames(ordinary.seen().calls),
    ["configure", "[static]vfs.create", "[static]shell.create"]);
  assert.deepEqual(setupNames(restored.seen().calls), ["__restore_default"]);
  const [o, r] = [ordinary.bash, restored.bash];
  assert.deepEqual(Object.keys(o.getEnv()), Object.keys(r.getEnv()));
  assert.deepEqual(o.getEnv(), r.getEnv());
  assert.equal(o.getCwd(), r.getCwd());
  assert.deepEqual(o.getCommandNames(), r.getCommandNames());
  for (const dir of ["/bin", "/usr/bin"]) {
    assert.deepEqual(await o.fs.readdir(dir), await r.fs.readdir(dir));
    for (const name of await o.fs.readdir(dir)) {
      const [a, b] = [await o.fs.stat(dir + "/" + name), await r.fs.stat(dir + "/" + name)];
      assert.deepEqual(a, b, dir + "/" + name);
    }
  }
  assert.equal(await o.fs.exists("/home/user"), await r.fs.exists("/home/user"));
  await o.fs.writeFile("/first.txt", "first");
  await r.fs.writeFile("/first.txt", "first");
  assert.equal(await o.readFile("/first.txt"), await r.readFile("/first.txt"));
  await o.fs.rm("/first.txt");
  await r.fs.rm("/first.txt");
  assert.equal(await o.fs.exists("/first.txt"), await r.fs.exists("/first.txt"));
  assert.deepEqual(await o.exec("echo hi"), await r.exec("echo hi"));
});

test("usage, ceiling and high-water agree between restored and ordinary setup", async () => {
  const usages = {};
  for (const [name, extra] of [["ordinary", { snapshotCapture: true }], ["restored", {}]]) {
    let engine = null;
    const { Bash } = createFacade(loaderFor(prodCore()));
    const bash = new Bash({ [INTERNAL]: { inspect() {}, onEngine: (e) => { engine = e; }, ...extra } });
    await bash.exec("echo hi");
    usages[name] = engine.usage();
  }
  assert.deepEqual(usages.restored, usages.ordinary);
  assert.equal(usages.restored.ceiling, 268435456n);
});

test("two restores are isolated and grow past the bootstrap ceiling", async () => {
  const first = await build({});
  const second = await build({});
  await first.bash.fs.writeFile("/only-first.txt", "x");
  assert.equal(await first.bash.readFile("/only-first.txt"), "x");
  assert.equal(await second.bash.fs.exists("/only-first.txt"), false);
  assert.equal((await second.bash.exec("echo ok")).stdout, "ok\n");
  // The restored ceiling is the full default cap, and the volume grows past 1 MiB.
  let engine = null;
  const { Bash } = createFacade(loaderFor(prodCore()));
  const big = new Bash({ [INTERNAL]: { onEngine: (e) => { engine = e; } } });
  assert.equal(engine.usage().ceiling, 268435456n);
  await big.fs.writeFile("/big.bin", "0123456789abcdef".repeat(200000));
  assert.equal((await big.fs.stat("/big.bin")).size, 3200000);
});

test("a trap in one restored test store leaves the other usable", async () => {
  const trapped = await build({}, testCore());
  const other = await build({}, testCore());
  const dead = await trapped.bash.exec("__trap");
  assert.equal(dead.exitCode, 1);
  assert.match(dead.stderr, /internal error/);
  await assert.rejects(trapped.bash.exec("echo hi"), { name: "KernelPoisonedError" });
  assert.equal((await other.bash.exec("echo ok")).stdout, "ok\n");
});

test("a second restore and a restore after ordinary setup refuse cleanly", async () => {
  const { testConfig } = await import("./abi/support.mjs");
  const fresh = () => {
    let seen = null;
    const store = linkEngine((imports) => new WebAssembly.Instance(prodCore(), imports).exports,
      { inspect: (s) => { seen = s; } });
    return { store, seen: () => seen };
  };
  {
    const { store, seen } = fresh();
    assert.ok(typeof store._util.restoreDefault() === "object");
    const before = Buffer.from(seen().exports.memory.buffer);
    const pages = before.length / 65536;
    const handles = seen().handles().map(({ id, kind }) => [id, kind]);
    assert.equal(store._util.restoreDefault(), 0n);
    assert.deepEqual(Buffer.from(seen().exports.memory.buffer), before);
    assert.equal(seen().exports.memory.buffer.byteLength / 65536, pages);
    assert.deepEqual(seen().handles().map(({ id, kind }) => [id, kind]), handles);
  }
  {
    // Refusal immediately after configure, while USED is still zero.
    const { store } = fresh();
    store.engine.configure(268435456n);
    assert.equal(store.engine.usage().used, 0n);
    assert.equal(store._util.restoreDefault(), 0n);
    assert.equal(store.engine.usage().ceiling, 268435456n);
  }
  {
    // Restore after ordinary setup refuses, and the next ordinary call succeeds.
    const { store } = fresh();
    store.engine.configure(268435456n);
    const vfs = store.engine.Vfs.create(134217728n);
    store.engine.Shell.create(vfs, testConfig());
    assert.equal(store._util.restoreDefault(), 0n);
    assert.equal(store.engine.usage().ceiling, 268435456n);
    vfs[Symbol.dispose]();
  }
});

test("a refused growth writes nothing and registers nothing", async () => {
  let seen = null;
  const store = linkEngine((imports) => new WebAssembly.Instance(prodCore(), imports).exports,
    { inspect: (s) => { seen = s; } });
  const raw = seen.exports;
  const before = Buffer.from(raw.memory.buffer);
  assert.equal(raw.__restore_grow(1 << 20), 0);
  assert.deepEqual(Buffer.from(raw.memory.buffer), before);
  assert.deepEqual(seen.handles(), []);
  // The helper leaves the latch untouched, so the restore still succeeds once...
  assert.ok(typeof store._util.restoreDefault() === "object");
  // ...and a retry on that core attempts no growth.
  const pages = raw.memory.buffer.byteLength / 65536;
  const bytes = Buffer.from(raw.memory.buffer);
  assert.equal(store._util.restoreDefault(), 0n);
  assert.equal(raw.memory.buffer.byteLength / 65536, pages);
  assert.deepEqual(Buffer.from(raw.memory.buffer), bytes);
});

test("post-return and explicit disposal of both restored owners", async () => {
  let seen = null;
  const store = linkEngine((imports) => new WebAssembly.Instance(prodCore(), imports).exports,
    { inspect: (s) => { seen = s; } });
  const { vfs, shell } = store._util.restoreDefault();
  assert.equal(seen.handles().length, 2);
  vfs[Symbol.dispose]();
  shell[Symbol.dispose]();
  assert.deepEqual(seen.handles(), []);
  assert.equal(store.engine.usage().ceiling, 268435456n);
  const replacement = store.engine.Vfs.create(134217728n);
  assert.equal(seen.handles().length, 1);
  replacement[Symbol.dispose]();
});

test("a staged default shell restores at attach on the packaged browser entry", async () => {
  // The shell is constructed before compilation, so construction, queued writes and the
  // registration all wait for attach; the restore runs first, exactly once.
  const coreBytes = readFileSync(join(repoRoot, "dist/js/kernel/kernel.core.wasm"));
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    assert.match(String(url), /kernel\.core\.wasm$/);
    return new Response(coreBytes, { headers: { "Content-Type": "application/octet-stream" } });
  };
  try {
    const browser = await import("../dist/js/browser.js");
    let seen = null;
    const staged = new browser.Bash({ [INTERNAL]: { inspect: (s) => { seen = s; } } });
    staged.registerCommand({ name: "late", execute: () => ({ stdout: "late\n", stderr: "", exitCode: 0 }) });
    await browser.Bash.precompile();
    assert.equal((await staged.exec("late")).stdout, "late\n");
    assert.deepEqual(setupNames(seen.calls), ["__restore_default"]);
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("precompiled browser shells restore and support synchronous access", async () => {
  const browser = await import("../dist/js/browser.js");
  let seen = null;
  const shell = new browser.Bash({ [INTERNAL]: { inspect: (s) => { seen = s; } } });
  assert.deepEqual(setupNames(seen.calls), ["__restore_default"]);
  assert.equal(shell.execSync("echo hi").stdout, "hi\n");
  assert.equal((await shell.exec("echo hi")).stdout, "hi\n");
});

test("precompile with a caller-supplied module never restores", async () => {
  // Runs last among the browser tests: the custom call taints the shared loader.
  const browser = await import("../dist/js/browser.js");
  await browser.Bash.precompile(new WebAssembly.Module(readFileSync(
    join(repoRoot, "dist/test/js/kernel-test/kernel-test.core.wasm"))));
  let seen = null;
  const shell = new browser.Bash({ [INTERNAL]: { inspect: (s) => { seen = s; } } });
  assert.deepEqual(setupNames(seen.calls), ["configure", "[static]vfs.create", "[static]shell.create"]);
  assert.equal((await shell.exec("echo hi")).stdout, "hi\n");
  assert.equal(shell.execSync("echo hi").stdout, "hi\n");
});

test("the jco binding uses ordinary initialization over the same component", async () => {
  // The jco glue reports no restore entry, so the facade counts its ordinary setup at the
  // engine level and never attempts a restore.
  const jco = await import("../packages/bash-jco/dist/js/index.js");
  const setup = [];
  const wrap = (obj, label) => {
    const orig = obj.create ?? obj.configure;
    const name = obj.create ? "create" : "configure";
    obj[name] = function (...args) {
      setup.push(label + "." + name);
      return orig.apply(this, args);
    };
  };
  const probe = new jco.Bash({ [INTERNAL]: { onEngine: (engine) => {
    wrap(engine, "engine");
    wrap(engine.Vfs, "Vfs");
    wrap(engine.Shell, "Shell");
  } } });
  assert.deepEqual(setup, ["engine.configure", "Vfs.create", "Shell.create"]);
  assert.equal((await probe.exec("echo hi")).stdout, "hi\n");
  assert.equal(probe.execSync("echo hi").stdout, "hi\n");
});

test("loggers are delivered on the restored path", async () => {
  const lines = [];
  const { bash } = await build({ logger: { info: (...a) => lines.push(["info", ...a]), debug: (...a) => lines.push(["debug", ...a]) } });
  assert.equal((await bash.exec("echo hi")).exitCode, 0);
  assert.ok(lines.some(([level, event]) => level === "info" && event === "exec"));
  assert.ok(lines.some(([level, event]) => level === "info" && event === "exit"));
});

test("the image covers a nonzero-to-zero word and stays sorted, bounded and disjoint", async () => {
  const { sectionBodies, parseSections, initialImage } = await import("../tools/snapshot.mjs");
  const coreBytes = readFileSync(join(repoRoot, "dist/js/kernel/kernel.core.wasm"));
  const sections = sectionBodies(coreBytes, parseSections(coreBytes));
  const { image } = initialImage(sections);
  const manifest = JSON.parse(readFileSync(join(repoRoot, "tmp", "snapshot-kernel.json"), "utf8"));
  const snap = Buffer.from(manifest.image, "base64");
  assert.equal(snap.length, manifest.imageBytes);
  let covered = 0;
  let offset = 0;
  let prev = -1;
  assert.ok(manifest.ranges.length > 0 && manifest.ranges.length <= 8192);
  for (const { dst, len } of manifest.ranges) {
    assert.ok(dst > prev, "ranges are sorted and disjoint");
    prev = dst;
    for (let i = 0; i < len; i++) {
      if (image[dst + i] !== 0 && snap[offset + i] === 0) covered++;
    }
    offset += len;
  }
  assert.equal(offset, snap.length);
  assert.ok(covered > 0, "a nonzero-to-zero word survives restoration");
  assert.equal(manifest.initialPages, 17);
  assert.ok(manifest.finalPages * 65536 <= 268435456);
  assert.equal(manifest.stackPointer, 1048576);
});
