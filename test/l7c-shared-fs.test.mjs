// Lot L7c: execution isolation for a shared InMemoryFs (EVOL D27).
// Separate mode gives each shell a private execution store over a synchronous
// in-memory root mount; shared mode attaches shells to the volume's own store.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { T, bash, internal, module as testModule, INTERNAL } from "./kt.mjs";
import { instantiate as testInstantiate } from "../dist/test/js/kernel-test/kernel-test.js";
import { Bash, InMemoryFs, KernelPoisonedError, defineCommand, AsyncCapabilityError } from "../host/index.js";
import { fsState } from "../host/core/fs.js";
import { serveKernelVolume } from "../host/core/driver.js";
import { decodeUtf8, utf8 } from "../host/core/bytes.js";

const FATAL = "bash: internal error: sandbox instance failed\n";

function fatalResult(r) {
  assert.equal(r.exitCode, 1);
  assert.equal(r.stderr, FATAL);
  assert.equal(Object.getOwnPropertyDescriptor(r, "fatal")?.enumerable, false);
  assert.equal(r.fatal, true);
}

/** A kernel-test volume; returns {fs, engines, ops} with per-store call counts. */
function volume(fsExtra = {}, hookExtra = {}) {
  const engines = [];
  let volOps = 0;
  const hooks = internal({
    ...hookExtra,
    onEngine(engine) {
      engines.push(engine);
      const proto = engine.Vfs.prototype;
      const op = proto.op;
      proto.op = function (...args) {
        volOps++;
        return op.apply(this, args);
      };
      hookExtra.onEngine?.(engine);
    },
  });
  const fs = new InMemoryFs(undefined, { hooks, ...fsExtra });
  return { fs, engines, ops: () => volOps };
}

/** Records every host call and every fiber run/resume epoch of one shell. */
function watcher(hookExtra = {}) {
  const state = { requests: [], runs: [], resumes: [] };
  const extra = {
    ...hookExtra,
    onHostCall: (req) => state.requests.push(req.tag),
    onEngine(engine) {
      const fiber = engine.Fiber.prototype;
      const run = fiber.run;
      fiber.run = function (...args) {
        const step = run.apply(this, args);
        if (step.tag === "host-call") state.runs.push(step.val.epoch);
        return step;
      };
      const resume = fiber.resume;
      fiber.resume = function (epoch, outcome) {
        state.resumes.push({ epoch, tag: outcome.tag });
        return resume.apply(this, [epoch, outcome]);
      };
      hookExtra.onEngine?.(engine);
    },
  };
  return { state, extra };
}

async function contents(fs, path) {
  return decodeUtf8(await fs.readFileBuffer(path));
}

// -------------------------------------------------------------------------------- gate 1

test("gate 1: isolation validates synchronously and rejects a third literal", () => {
  const fs = new InMemoryFs();
  for (const bad of ["shared!", "SHARED", "", 0, 1, false, true, null, {}, []]) {
    assert.throws(() => new Bash({ fs, isolation: bad }), TypeError);
  }
  assert.throws(() => new Bash({ isolation: "bogus" }), TypeError);
  assert.throws(() => new Bash({ fs: {}, isolation: "shared" }), TypeError);
  assert.throws(() => new Bash({ fs: 3 }), TypeError);
  // A prototype or a lookalike gives no synchronous kernel capability.
  assert.throws(() => new Bash({ fs: InMemoryFs.prototype, isolation: "shared" }), TypeError);
  assert.throws(() => new Bash({ fs: Object.create(InMemoryFs.prototype), isolation: "shared" }), TypeError);
  // Both literals construct; omitted means separate.
  for (const options of [{ fs }, { fs, isolation: "separate" }, { fs, isolation: "shared" }, {}]) {
    new Bash(T(options));
  }
});

test("gate 1: one fresh store per unshared shell, three memories for two separate shells, one for shared", () => {
  const seen = new Set();
  const extra = { onEngine: (engine) => void seen.add(engine) };
  new Bash(T({}, extra));
  const { fs } = volume();
  const before = seen.size;
  const s1 = new Bash(T({ fs }, extra));
  const s2 = new Bash(T({ fs }, extra));
  void s1;
  void s2;
  assert.equal(seen.size - before, 2, "two separate shells add two execution stores");
  const h1 = new Bash(T({ fs, isolation: "shared" }, extra));
  const h2 = new Bash(T({ fs, isolation: "shared" }, extra));
  void h1;
  void h2;
  assert.equal(seen.size - before, 2, "shared shells add no store");
});

test("gate 1: both modes expose the original FS object; a mixed pair has two memories", () => {
  const seen = new Set();
  const extra = { onEngine: (engine) => void seen.add(engine) };
  const { fs } = volume({}, extra);
  const s = new Bash(T({ fs }, extra));
  const h = new Bash(T({ fs, isolation: "shared" }, extra));
  assert.equal(s.fs, fs);
  assert.equal(h.fs, fs);
  assert.ok(s.fs instanceof InMemoryFs);
  assert.ok(h.fs instanceof InMemoryFs);
  assert.equal(seen.size, 2, "one volume store plus one separate execution store");
  const other = volume();
  const o1 = new Bash(T({ fs: other.fs, isolation: "shared" }));
  void o1;
  assert.notEqual(fsState(fs).holder, fsState(other.fs).holder, "two shared volumes never collapse");
});

test("gate 1: the no-FS case makes zero guest fs-batch calls", async () => {
  const w = watcher();
  const b = bash({}, w.extra);
  const r = await b.exec("echo hi");
  assert.equal(r.stdout, "hi\n");
  assert.deepEqual(w.state.requests, []);
});

// -------------------------------------------------------------------------------- gate 2

async function layoutSnapshot(b) {
  const fs = b.fs;
  return {
    proc: await contents(fs, "/proc/version"),
    cmdline: await contents(fs, "/proc/self/cmdline"),
    status: await contents(fs, "/proc/self/status"),
    stubBin: await contents(fs, "/bin/echo"),
    stubUsr: await contents(fs, "/usr/bin/echo"),
    stubMode: (await fs.stat("/bin/echo")).mode,
    procMode: (await fs.stat("/proc/version")).mode,
    dirMode: (await fs.stat("/usr/bin")).mode,
    stubMtime: (await fs.stat("/bin/echo")).mtime.getTime(),
    procMtime: (await fs.stat("/proc/self/status")).mtime.getTime(),
    dirMtime: (await fs.stat("/usr/bin")).mtime.getTime(),
    cwd: b.getCwd(),
  };
}

for (const isolation of ["separate", "shared"]) {
  test(`gate 2 (${isolation}): initial files match the local control, layout is identical`, async () => {
    const control = bash({});
    const vol = volume();
    await vol.fs.mkdir("/app", { recursive: true });
    await vol.fs.writeFile("/app/data.txt", "payload");
    await vol.fs.writeFile("/app/keep.bin", new Uint8Array([1, 2, 3]));
    const b = bash({ fs: vol.fs, isolation });
    assert.deepEqual(await layoutSnapshot(b), await layoutSnapshot(control));
    assert.equal(await contents(b.fs, "/app/data.txt"), "payload");
    assert.deepEqual(await b.fs.readFileBuffer("/app/keep.bin"), new Uint8Array([1, 2, 3]));
    assert.equal(b.getCwd(), "/home/user");
    // The reference wins for `files` on a fresh volume: the local control keeps
    // initial-file metadata (explicit mode/mtime, virtual time otherwise).
    const seeded = bash({ files: { "/s.txt": { content: "s", mode: 0o600, mtime: new Date(42) }, "/t.txt": "t" } });
    assert.deepEqual([(await seeded.fs.stat("/s.txt")).mode, (await seeded.fs.stat("/s.txt")).mtime.getTime()], [0o600, 42]);
    assert.equal((await seeded.fs.stat("/t.txt")).mtime.getTime(), 0);
  });

  test(`gate 2 (${isolation}): a supplied volume wins over files, cwd falls back to /`, async () => {
    const vol = volume();
    await vol.fs.writeFile("/sentinel", "volume-wins");
    const b = bash({ fs: vol.fs, files: { "/intruder": "ignored" }, isolation });
    assert.equal(await contents(b.fs, "/sentinel"), "volume-wins");
    assert.equal(await b.fs.exists("/intruder"), false);
    assert.equal(b.getCwd(), "/");
  });

  test(`gate 2 (${isolation}): guest writes are visible both ways, removal included`, async () => {
    const vol = volume();
    const a = bash({ fs: vol.fs, isolation });
    const c = bash({ fs: vol.fs, isolation });
    let r = await a.exec("__write /note hello");
    assert.equal(r.exitCode, 0);
    assert.equal(await contents(c.fs, "/note"), "hello");
    r = await c.exec("__cat /note");
    assert.deepEqual([r.exitCode, r.stdout], [0, "hello"]);
    await vol.fs.writeFile("/blob", new Uint8Array([0, 255, 1, 2]));
    assert.deepEqual(await c.fs.readFileBuffer("/blob"), new Uint8Array([0, 255, 1, 2]));
    r = await a.exec("__cat /blob");
    assert.equal(r.exitCode, 0);
    await c.fs.rm("/note", {});
    r = await a.exec("__cat /note");
    assert.equal(r.exitCode, 1);
  });

  test(`gate 2 (${isolation}): shell results match, custom dispatch stays private`, async () => {
    const vol = volume();
    const a = bash({ fs: vol.fs, isolation, customCommands: [defineCommand("mine", async () => ({ stdout: "m\n", stderr: "", exitCode: 0 }))] });
    const c = bash({ fs: vol.fs, isolation });
    for (const [script, want] of [["__write /x 1", ""], ["__cat /x", "1"], ["/bin/echo hi", "hi\n"], ["__cat /missing", ""]]) {
      const ra = await a.exec(script);
      const rc = await c.exec(script);
      assert.equal(ra.stdout, want, script);
      assert.equal(rc.stdout, want, script);
      assert.equal(ra.exitCode, rc.exitCode, script);
      assert.equal(ra.stderr, rc.stderr, script);
    }
    const r = await c.exec("mine");
    assert.equal(r.exitCode, 127, "B never runs A's callback");
  });

  test(`gate 2 (${isolation}): layout bypasses public overrides and stamps zero mtimes`, async () => {
    let mkdirCalls = 0;
    let writeCalls = 0;
    class ThrowingFs extends InMemoryFs {
      mkdirSync() {
        mkdirCalls++;
        throw new Error("override must not run");
      }
      writeFileSync() {
        writeCalls++;
        throw new Error("override must not run");
      }
    }
    const fs = new ThrowingFs(undefined, { hooks: internal() });
    const b = bash({ fs, isolation });
    assert.equal(await contents(b.fs, "/proc/version"), "Linux version 5.15.0-generic (just-bash) #1 SMP PREEMPT\n");
    assert.equal(await contents(b.fs, "/bin/echo"), "#!/bin/bash\n# Built-in command: echo\n");
    assert.equal(await contents(b.fs, "/usr/bin/echo"), "#!/bin/bash\n# Built-in command: echo\n");
    assert.deepEqual([(await fs.stat("/bin/echo")).mode, (await fs.stat("/proc/version")).mode, (await fs.stat("/usr/bin")).mode], [0o644, 0o644, 0o755]);
    assert.deepEqual([(await fs.stat("/bin/echo")).mtime.getTime(), (await fs.stat("/proc/self/status")).mtime.getTime(), (await fs.stat("/usr/bin")).mtime.getTime()], [0, 0, 0]);
    const r = await b.exec("/bin/echo hi");
    assert.deepEqual([r.exitCode, r.stdout], [0, "hi\n"]);
    assert.equal(mkdirCalls, 0);
    assert.equal(writeCalls, 0);
  });

  test(`gate 2 (${isolation}): write metadata and explicit mkdir mode reach stat`, async () => {
    const { fs } = volume();
    const b = bash({ fs, isolation });
    fs.writeFileSync("/meta", "x", { encoding: "utf8", mode: 0o600, mtime: new Date(123456789) });
    fs.writeFileSync("/latin", "y", "latin1");
    fs.mkdirSync("/mode-dir", { recursive: true, mode: 0o700 });
    assert.deepEqual([(await fs.stat("/meta")).mode, (await fs.stat("/meta")).mtime.getTime()], [0o600, 123456789]);
    assert.equal(await contents(fs, "/latin"), "y");
    assert.equal((await fs.stat("/mode-dir")).mode, 0o700);
    assert.equal((await b.fs.stat("/meta")).mtime.getTime(), 123456789);
  });

  test(`gate 2 (${isolation}): late registration writes stubs for its own registry only`, async () => {
    const { fs } = volume();
    const a = bash({ fs, isolation });
    const c = bash({ fs, isolation });
    a.registerCommand(defineCommand("late", async () => ({ stdout: "late\n", stderr: "", exitCode: 0 })));
    assert.equal(await contents(a.fs, "/bin/late"), "#!/bin/bash\n# Built-in command: late\n");
    assert.equal((await c.exec("late")).exitCode, 127);
    assert.equal((await a.exec("late")).stdout, "late\n");
  });

  test(`gate 2 (${isolation}): a system-path file collision fails before any directory is created`, async () => {
    const { fs } = volume();
    await fs.writeFile("/proc", "collision");
    assert.throws(() => bash({ fs, isolation }), /EEXIST/);
    assert.equal(await fs.exists("/bin"), false, "no system directory is created before the collision");
    assert.equal(await contents(fs, "/proc"), "collision", "the colliding file survives");
  });
}

// -------------------------------------------------------------------------------- gate 3

test("gate 3: one six-op batch is one request, six ordered results, one same-epoch resume, zero yields", async () => {
  const vol = volume();
  const w = watcher();
  const a = bash({ fs: vol.fs }, w.extra);
  const r = await a.exec("__fsbatch 1");
  assert.equal(r.exitCode, 0);
  assert.deepEqual(r.stdout.split("\n").filter(Boolean), ["ok none", "err unsupported", "ok none", "ok entries", "ok info", "ok none"]);
  assert.deepEqual(w.state.requests, ["fs-batch"], "exactly one host call");
  assert.equal(w.state.runs.length, 1, "one host-call step");
  assert.deepEqual(w.state.resumes, [{ epoch: w.state.runs[0], tag: "fs" }], "one same-epoch resume");
  // execSync proves the branch performs zero awaits: any yield would throw.
  const s = a.execSync("__fsbatch 1");
  assert.deepEqual(s.stdout.split("\n").filter(Boolean), ["ok none", "err unsupported", "ok none", "ok entries", "ok info", "ok none"]);
});

test("gate 3: unshared and shared local echo produce zero FS crossings", async () => {
  const plain = watcher();
  const p = bash({}, plain.extra);
  assert.equal((await p.exec("echo hi")).stdout, "hi\n");
  assert.deepEqual(plain.state.requests, []);
  const vol = volume();
  const s = watcher();
  const h = bash({ fs: vol.fs, isolation: "shared" }, s.extra);
  assert.equal((await h.exec("echo hi")).stdout, "hi\n");
  assert.deepEqual(s.state.requests, []);
});

test("gate 3: all 21 op tags match the volume's direct results", async () => {
  const vol = volume();
  const a = bash({ fs: vol.fs });
  void a;
  await vol.fs.mkdir("/md");
  await vol.fs.writeFile("/tag", "old");
  const st = fsState(vol.fs);
  const headroom = 1 << 20;
  const x = { B: { headroom } };
  const enc = utf8;
  const ops = [
    { tag: "stat", val: enc("/bin/echo") },
    { tag: "lstat", val: enc("/bin/echo") },
    { tag: "exists", val: enc("/bin/echo") },
    { tag: "read-link", val: enc("/bin/echo") },
    { tag: "real-path", val: enc("/bin/echo") },
    { tag: "read-dir", val: enc("/bin") },
    { tag: "read-dir-plus", val: enc("/bin") },
    { tag: "walk", val: { root: enc("/"), maxDepth: 4, maxEntries: 30 } },
    { tag: "all-paths" },
    { tag: "read-file", val: { path: enc("/bin/echo"), offset: 0n, len: 64 } },
    { tag: "write-file", val: { path: enc("/tag"), data: enc("v"), offset: 0n, truncate: true, mode: undefined, mtimeMs: undefined } },
    { tag: "write-lazy", val: { path: enc("/lazy"), mode: 0o644, mtimeMs: undefined } },
    { tag: "create-exclusive", val: { path: enc("/excl"), mode: 0o600, directory: false } },
    { tag: "mkdir", val: { path: enc("/md"), recursive: true, mode: undefined } },
    { tag: "remove", val: { path: enc("/absent"), recursive: false, force: true } },
    { tag: "copy", val: { source: enc("/bin/echo"), target: enc("/cp"), recursive: false } },
    { tag: "rename", val: { source: enc("/excl"), target: enc("/ren"), recursive: false } },
    { tag: "symlink", val: { source: enc("echo"), target: enc("/sl"), recursive: false } },
    { tag: "hard-link", val: { source: enc("/bin/echo"), target: enc("/hl"), recursive: false } },
    { tag: "chmod", val: { path: enc("/bin/echo"), mode: 0o644 } },
    { tag: "utimes", val: { path: enc("/bin/echo"), atimeMs: 1n, mtimeMs: 2n } },
  ];
  const outcome = serveKernelVolume(x, { req: { val: { mountId: 1, ops } } }, vol.fs);
  assert.equal(outcome.outcome.tag, "fs");
  const results = outcome.outcome.val;
  assert.equal(results.length, 21);
  // The bridge is a pass-through: each result equals a direct vfs.op on the
  // volume, including the unsupported cases L7a has yet to implement.
  const direct = ops.map((op) => st.holder.instance.call(() => st.vfs.op(op)));
  assert.deepEqual(results, direct);
  const codes = results.map((r) => (r.tag === "ok" ? `ok ${r.val.tag}` : `err ${r.val.code}`));
  assert.deepEqual(codes.slice(0, 10), ["ok info", "ok info", "ok flag", "err unsupported", "err unsupported", "ok entries", "err unsupported", "err unsupported", "ok paths", "ok data"]);
  assert.deepEqual(codes.slice(10), ["ok none", "err unsupported", "err unsupported", "ok none", "ok none", "err unsupported", "err unsupported", "err unsupported", "err unsupported", "err unsupported", "err unsupported"]);
});

test("gate 3: unknown mount ids are denied before touching any owner, missing grants make zero calls", async () => {
  const vol = volume();
  const w = watcher();
  const a = bash({ fs: vol.fs }, w.extra);
  const before = vol.ops();
  // An unknown id is denied by the driver's mount map; the request is observed
  // but no volume op runs.
  const r = await a.exec("__fsbatch 9");
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, "denied\n");
  assert.deepEqual(w.state.requests, ["fs-batch"]);
  assert.equal(vol.ops(), before, "the denied batch touches no volume op");
  // A shell without G_HOST_FS never emits a host call, even over a volume mount.
  const denied = bash({ fs: vol.fs }, {
    onHostCall() { assert.fail("ungranted host call reached the driver"); },
    onEngine(engine) {
      const create = engine.Shell.create;
      engine.Shell.create = (vfs, config) => create(vfs, { ...config, grants: {} });
    },
  });
  await vol.fs.writeFile("/sentinel", "s");
  const missing = await denied.exec("__cat /sentinel");
  assert.equal(missing.exitCode, 1);
  assert.equal(missing.stderr, "cat: /sentinel: Permission denied\n");
});

test("gate 3: shared mode grants no host FS, separate mode keeps it", () => {
  const seen = [];
  const extra = {
    onEngine(engine) {
      const create = engine.Shell.create;
      engine.Shell.create = (vfs, config) => { seen.push(config.grants.hostFs); return create(vfs, config); };
    },
  };
  const { fs } = volume({}, extra);
  void bash({ fs }, extra);
  void bash({ fs, isolation: "shared" }, extra);
  assert.deepEqual(seen, [true, false], "separate keeps G_HOST_FS, shared has no host mount");
});

// -------------------------------------------------------------------------------- gate 4

for (const isolation of ["separate", "shared"]) {
  test(`gate 4 (${isolation}): live budgets belong to each shell`, async () => {
    const { fs } = volume();
    const small = bash({ fs, isolation, executionLimitProfile: "micro", executionLimits: { maxLiveBytes: 100000 } });
    const large = bash({ fs, isolation });
    const capped = await small.exec("__alloc 200000");
    assert.equal(capped.exitCode, 126);
    assert.match(capped.stderr, /exceeded/);
    assert.equal(capped.fatal, undefined);
    assert.equal((await large.exec("__alloc 200000")).exitCode, 0);
  });

  test(`gate 4 (${isolation}): twenty interleaved writes are all visible`, async () => {
    const { fs } = volume();
    const a = bash({ fs, isolation });
    const c = bash({ fs, isolation });
    for (let i = 0; i < 20; i++) {
      const w = i % 2 === 0 ? a : c;
      const r = await w.exec(`__write /w-${i} value-${i}`);
      assert.equal(r.exitCode, 0);
    }
    for (let i = 0; i < 20; i++) {
      assert.equal(await contents(a.fs, `/w-${i}`), `value-${i}`);
      assert.equal(await contents(c.fs, `/w-${i}`), `value-${i}`);
    }
  });

  test(`gate 4 (${isolation}): a filled volume refuses the next write on both shells`, async () => {
    const { fs } = volume({ maxFileSystemBytes: 100000 });
    const a = bash({ fs, isolation });
    const c = bash({ fs, isolation });
    const st = fsState(fs);
    const used = () => st.holder.instance.call(() => st.vfs.usedBytes());
    const chunk = "x".repeat(20000);
    let written = 0;
    for (let i = 0; ; i++) {
      const w = i % 2 === 0 ? a : c;
      try {
        await w.fs.writeFile(`/fill-${i}`, chunk);
        written += chunk.length;
      } catch (e) {
        assert.match(e.message, /^ENOSPC/);
        break;
      }
      if (i > 20) throw new Error("the quota never refused");
    }
    assert.ok(written > 0);
    const retained = used();
    // The next write of the same size refuses on both shells; retained bytes agree.
    await assert.rejects(a.fs.writeFile("/one-more", chunk), /ENOSPC/);
    await assert.rejects(c.fs.writeFile("/one-more", chunk), /ENOSPC/);
    assert.equal(used(), retained, "both shells see the same retained bytes");
    assert.equal(await contents(a.fs, "/fill-0"), chunk);
  });
}

test("gate 4: separate ceilings stay independent, shared ceilings rise monotonically", () => {
  const MIB = 1024 * 1024;
  const seen = [];
  const extra = { onEngine: (engine) => void seen.push(engine) };
  const { fs } = volume({}, extra);
  const ceilingOf = (engine) => Number(engine.usage().ceiling);
  const volumeCeiling = () => ceilingOf(seen[0]);
  assert.equal(volumeCeiling(), 256 * MIB);
  void new Bash(T({ fs, executionLimitProfile: "micro" }, extra));
  assert.equal(seen.length, 2, "a separate micro shell adds one store");
  assert.equal(ceilingOf(seen[1]), 64 * MIB);
  assert.equal(volumeCeiling(), 256 * MIB, "the volume ceiling is never raised for an exec store");
  void new Bash(T({ fs, executionLimitProfile: "normal", isolation: "shared" }, extra));
  assert.equal(seen.length, 2, "a shared shell adds no store");
  assert.equal(volumeCeiling(), 1856 * MIB, "shared mode raises the common ceiling monotonically");
  void new Bash(T({ fs }, extra));
  assert.equal(volumeCeiling(), 1856 * MIB, "attaching a smaller shell never lowers it");
});

// The synchronous mount bounds: min(1 MiB, floor(headroom / 2)), counting the
// encoded records (72 per op, 16 per result) and their variable-length payloads.
function boundServe(headroom, ops, fs) {
  return serveKernelVolume({ B: { headroom } }, { req: { val: { mountId: 1, ops } } }, fs).outcome;
}

test("gate 4: the request and response bounds hold at equality and one byte over", async () => {
  const { fs } = volume();
  void bash({ fs });
  // Small headroom fixture: bound 100. Request 72 + 28 = 100 completes at
  // equality; one more byte overflows.
  let out = boundServe(200, [{ tag: "read-file", val: { path: utf8("/p".padEnd(28, "x")), offset: 0n, len: 1 } }], fs);
  assert.equal(out.tag, "fs", "72 + 28 completes at equality");
  out = boundServe(200, [{ tag: "read-file", val: { path: utf8("/p".padEnd(29, "x")), offset: 0n, len: 1 } }], fs);
  assert.equal(out.tag, "limit-exceeded");
  assert.equal(out.val, "headroom");
  // Response: 16 + data. 84 completes at equality, 85 overflows.
  await fs.writeFile("/resp", "r".repeat(200));
  out = boundServe(200, [{ tag: "read-file", val: { path: utf8("/resp"), offset: 0n, len: 84 } }], fs);
  assert.equal(out.tag, "fs");
  assert.equal(out.val[0].val.val.length, 84);
  out = boundServe(200, [{ tag: "read-file", val: { path: utf8("/resp"), offset: 0n, len: 85 } }], fs);
  assert.equal(out.tag, "limit-exceeded");
  // Sufficient headroom fixture: the 1 MiB bound. 72 + 2 + 1048502 completes;
  // one more byte overflows with the write never running.
  const write = (data) => ({ tag: "write-file", val: { path: utf8("/w"), data: utf8(data), offset: 0n, truncate: true, mode: undefined, mtimeMs: undefined } });
  out = boundServe(64 << 20, [write("c".repeat(1048502))], fs);
  assert.equal(out.tag, "fs");
  out = boundServe(64 << 20, [write("c".repeat(1048503))], fs);
  assert.equal(out.tag, "limit-exceeded");
  assert.equal(out.val, "headroom");
  assert.equal(await contents(fs, "/w"), "c".repeat(1048502), "the overflowing write never ran");
});

test("gate 4: oversized listings stop the batch, keep mutations and deliver no partial list", async () => {
  const { fs } = volume();
  void bash({ fs });
  await fs.mkdir("/big");
  for (let i = 0; i < 60; i++) await fs.writeFile(`/big/f-${String(i).padStart(3, "0")}`, "x");
  const readDir = { tag: "read-dir", val: utf8("/big") };
  let out = boundServe(2000, [readDir], fs);
  assert.equal(out.tag, "limit-exceeded");
  assert.equal(out.val, "headroom");
  out = boundServe(1 << 20, [readDir], fs);
  assert.equal(out.tag, "fs", "the same listing fits a sufficient bound");
  // A write preceding the overflow is applied once; later ops never run.
  const write = { tag: "write-file", val: { path: utf8("/kept"), data: utf8("k"), offset: 0n, truncate: true, mode: undefined, mtimeMs: undefined } };
  out = boundServe(2000, [write, readDir], fs);
  assert.equal(out.tag, "limit-exceeded");
  assert.equal(await contents(fs, "/kept"), "k");
  // all-paths over a large tree overflows the same way.
  out = boundServe(2000, [{ tag: "all-paths" }], fs);
  assert.equal(out.tag, "limit-exceeded");
  // read-dir-plus and walk stay unsupported: no truncation, no partial listing.
  for (const op of [{ tag: "read-dir-plus", val: utf8("/big") }, { tag: "walk", val: { root: utf8("/big"), maxDepth: 9, maxEntries: 9999 } }]) {
    out = boundServe(2000, [op], fs);
    assert.equal(out.tag, "fs");
    assert.equal(out.val[0].tag, "err");
    assert.equal(out.val[0].val.code, "unsupported");
  }
});

test("gate 4: read-dir-plus and walk overflow via fixture results", () => {
  // The volume leaves both tags unsupported, so the response bound is covered
  // with fixture entries-plus results (56 per entry plus the path, as lowered).
  // 15 ten-byte paths encode to 16 + 15 * (56 + 10) = 1006.
  const { fs } = volume();
  void bash({ fs });
  const st = fsState(fs);
  const entries = Array.from({ length: 15 }, (_, i) => ({
    path: utf8(`/big/f-${String(i).padStart(3, "0")}`),
    info: { kind: "file", size: 1n, mode: 0o644, mtimeMs: 0n, dev: 0n, ino: 0n },
  }));
  const orig = st.vfs.op;
  st.vfs.op = (op) => (op.tag === "read-dir-plus" || op.tag === "walk")
    ? { tag: "ok", val: { tag: "entries-plus", val: entries } }
    : orig.call(st.vfs, op);
  for (const op of [
    { tag: "read-dir-plus", val: utf8("/big") },
    { tag: "walk", val: { root: utf8("/big"), maxDepth: 9, maxEntries: 9999 } },
  ]) {
    let out = boundServe(2012, [op], fs);
    assert.equal(out.tag, "fs", `${op.tag} completes at equality (bound 1006)`);
    assert.equal(out.val[0].val.val.length, 15, "no partial listing is delivered");
    out = boundServe(2010, [op], fs);
    assert.equal(out.tag, "limit-exceeded", `${op.tag} overflows one byte over`);
    assert.equal(out.val, "headroom");
  }
});

// -------------------------------------------------------------------------------- gate 5

/** A custom command that blocks until released; resolves the arrival gate on entry. */
function pendingCustom(name = "waiter") {
  let arrived;
  const arrival = new Promise((r) => { arrived = r; });
  let release;
  const gate = new Promise((r) => { release = r; });
  const command = defineCommand(name, async () => {
    arrived();
    await gate;
    return { stdout: "stale\n", stderr: "", exitCode: 0 };
  });
  return { command, arrival, release: () => release() };
}

/** Counts Vfs.op calls on every store built with the returned hooks. */
function countOps(hookExtra = {}) {
  const state = { ops: 0 };
  const extra = {
    ...hookExtra,
    onEngine(engine) {
      const proto = engine.Vfs.prototype;
      const op = proto.op;
      proto.op = function (...args) {
        state.ops++;
        return op.apply(this, args);
      };
      hookExtra.onEngine?.(engine);
    },
  };
  return { state, extra };
}

test("gate 5 (separate, fs: a.fs): trapping the attached store leaves the owner healthy", async () => {
  const a = bash({});
  await a.exec("__write /sentinel pre-trap");
  const pc = pendingCustom();
  const b = bash({ fs: a.fs, customCommands: [pc.command] });
  const p1 = b.exec("waiter");
  await pc.arrival;
  const p2 = b.exec("__trap");
  const [r1, r2] = await Promise.all([p1, p2]);
  fatalResult(r1);
  fatalResult(r2);
  pc.release();
  await assert.rejects(b.exec("echo hi"), KernelPoisonedError);
  await assert.rejects(b.exec("echo hi"), KernelPoisonedError, "no stale resume reaches the guest");
  await assert.rejects(b.readFile("/sentinel"), KernelPoisonedError, "dead-shell reads observe D8");
  await assert.rejects(b.writeFile("/dead-write", "x"), KernelPoisonedError, "dead-shell writes observe D8");
  assert.equal((await a.exec("echo ok")).stdout, "ok\n");
  assert.equal(await a.readFile("/sentinel"), "pre-trap");
  assert.equal((await a.exec("__write /after yes")).exitCode, 0);
  assert.equal(await a.readFile("/after"), "yes");
});

test("gate 5 (shared): trapping the common store rejects the sibling and the volume", async () => {
  const a = bash({});
  await a.exec("__write /sentinel pre-trap");
  const pc = pendingCustom();
  const b = bash({ fs: a.fs, isolation: "shared", customCommands: [pc.command] });
  const p1 = b.exec("waiter");
  await pc.arrival;
  const p2 = b.exec("__trap");
  const [r1, r2] = await Promise.all([p1, p2]);
  fatalResult(r1);
  fatalResult(r2);
  pc.release();
  await assert.rejects(b.exec("echo hi"), KernelPoisonedError);
  await assert.rejects(a.exec("echo hi"), KernelPoisonedError, "the sibling shares the trap domain");
  await assert.rejects(a.fs.readFile("/sentinel"), KernelPoisonedError, "the common volume is unavailable");
  const c = bash({});
  assert.equal((await c.exec("echo ok")).stdout, "ok\n", "a third, independent shell succeeds");
});

test("gate 5 (separate survivor): a dead volume fails guest I/O but not healthy computation", async () => {
  const counter = countOps();
  const a = bash({}, counter.extra);
  await a.exec("__write /sentinel pre-trap");
  const b = bash({ fs: a.fs });
  await a.exec("__trap");
  await assert.rejects(a.exec("echo hi"), KernelPoisonedError);
  await assert.rejects(a.fs.readFile("/sentinel"), KernelPoisonedError);
  const before = counter.state.ops;
  const cat = await b.exec("__cat /sentinel");
  assert.equal(cat.exitCode, 1);
  assert.equal(cat.stderr, "cat: /sentinel: error\n", "an ordinary FS io result for a healthy guest");
  assert.equal((await b.exec("echo ok")).stdout, "ok\n", "the live store computes on");
  assert.equal(counter.state.ops - before, 0, "zero guest calls to the failed owner");
});

test("gate 5 (mixed pair): trapping the volume isolates the separate shell with io errors", async () => {
  const vol = volume();
  const s1 = bash({ fs: vol.fs });
  const pc = pendingCustom();
  const s2 = bash({ fs: vol.fs, isolation: "shared", customCommands: [pc.command] });
  await s1.exec("__write /sentinel pre-trap");
  const p1 = s2.exec("waiter");
  await pc.arrival;
  const p2 = s2.exec("__trap");
  const [r1, r2] = await Promise.all([p1, p2]);
  fatalResult(r1);
  fatalResult(r2);
  pc.release();
  await assert.rejects(s2.exec("echo hi"), KernelPoisonedError);
  await assert.rejects(vol.fs.readFile("/sentinel"), KernelPoisonedError);
  const before = vol.ops();
  const cat = await s1.exec("__cat /sentinel");
  assert.equal(cat.exitCode, 1);
  assert.equal((await s1.exec("echo ok")).stdout, "ok\n");
  assert.equal(vol.ops() - before, 0, "zero guest calls to the failed owner");
  const c = bash({});
  assert.equal((await c.exec("echo ok")).stdout, "ok\n");
});

// -------------------------------------------------------------------------------- gate 6

test("gate 6: after precompile, ready built-in mounts support immediate sync use", async () => {
  const browser = await import("../host/browser.js?l7c-ready");
  const core = readFileSync(new URL("../dist/test/js/kernel-test/kernel-test.core.wasm", import.meta.url));
  await browser.Bash.precompile(core);
  const hooks = internal();
  const fs = new browser.InMemoryFs(undefined, { hooks });
  const a = new browser.Bash({ fs, [INTERNAL]: hooks });
  assert.equal(a.execSync("echo hi").stdout, "hi\n");
  assert.equal(a.fs, fs);
  const b = new browser.Bash({ fs, isolation: "shared", [INTERNAL]: hooks });
  // A sync guest read in the other shell sees a sync guest write.
  a.execSync("__write /x 1");
  assert.equal(b.execSync("__cat /x").stdout, "1");
});

test("gate 6: browser attachments stage in order and sync calls fail closed before compile", async () => {
  const browser = await import("../host/browser.js?l7c-staged");
  const hooks = { kernel: { instantiate: testInstantiate }, moduleProvider: () => Promise.resolve(testModule) };
  const fs = new browser.InMemoryFs({ "/early": "E" }, { hooks });
  fs.writeFileSync("/staged-sync", "S");
  const stagedWrite = fs.writeFile("/staged-async", "A");
  const a = new browser.Bash({ fs, [INTERNAL]: hooks });
  const c = new browser.Bash({ fs, isolation: "shared", [INTERNAL]: hooks });
  assert.throws(() => a.execSync("echo hi"), AsyncCapabilityError, "compile-first for sync calls");
  await stagedWrite;
  assert.equal(await fs.readFile("/early"), "E");
  assert.equal(await fs.readFile("/staged-sync"), "S");
  assert.equal(await fs.readFile("/staged-async"), "A");
  assert.equal((await a.exec("__cat /early")).stdout, "E");
  assert.equal((await c.exec("__cat /staged-async")).stdout, "A");
  assert.equal(a.execSync("__cat /staged-sync").stdout, "S");
});

test("gate 4: a guest read past the 1 MiB response bound ends 126 with no partial output", async () => {
  // A script line cannot carry a >1 MiB write payload (the parser caps lines
  // below the bound), so the facade-level request case is unreachable; the
  // helper tests above prove it. A >1 MiB read chunk is guest-reachable.
  const vol = volume();
  const a = bash({ fs: vol.fs });
  await vol.fs.writeFile("/just-over", "o".repeat((1 << 20) + 8));
  const r = await a.exec("__cat /just-over");
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, "bash: headroom exceeded\n");
  assert.equal(r.stdout, "", "zero partial output on response overflow");
  const ok = await a.exec("__cat /proc/version");
  assert.equal(ok.exitCode, 0, "the shell keeps serving after an overflow");
});

test("gate 4: ordered writes within one batch keep the last writer's bytes", async () => {
  const { fs } = volume();
  void bash({ fs });
  const x = { B: { headroom: 1 << 20 } };
  const out = serveKernelVolume(x, {
    req: {
      val: {
        mountId: 1,
        ops: [
          { tag: "write-file", val: { path: utf8("/w"), data: utf8("v1"), offset: 0n, truncate: true, mode: undefined, mtimeMs: undefined } },
          { tag: "write-file", val: { path: utf8("/w"), data: utf8("v2"), offset: 0n, truncate: true, mode: undefined, mtimeMs: undefined } },
        ],
      },
    },
  }, fs);
  assert.equal(out.outcome.tag, "fs");
  assert.equal(await contents(fs, "/w"), "v2");
});

test("gate 3: the custom-FS case still rejects execSync with AsyncCapabilityError", () => {
  const calls = [];
  const custom = {
    readFile: async (p) => { calls.push(p); return "x"; },
    readFileBuffer: async () => new Uint8Array(0),
    writeFile: async () => {},
    appendFile: async () => {},
    exists: async () => true,
    stat: async () => ({ isFile: true, isDirectory: false, isSymbolicLink: false, mode: 0o644, size: 1, mtime: new Date(0) }),
    lstat: async () => ({ isFile: true, isDirectory: false, isSymbolicLink: false, mode: 0o644, size: 1, mtime: new Date(0) }),
    mkdir: async () => {},
    readdir: async () => [],
    rm: async () => {},
    cp: async () => {},
    mv: async () => {},
    resolvePath: (b, p) => (p.startsWith("/") ? p : `${b}/${p}`),
    getAllPaths: () => [],
    chmod: async () => {},
    symlink: async () => {},
    link: async () => {},
    readlink: async () => "/",
    realpath: async (p) => p,
    utimes: async () => {},
  };
  const b = bash({ fs: custom });
  assert.throws(() => b.execSync("__cat /t"), { name: "AsyncCapabilityError" });
});
