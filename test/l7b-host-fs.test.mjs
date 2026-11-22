import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";
import { Bash, OverlayFs, ReadWriteFs } from "@sentropic/bash";
import * as browser from "@sentropic/bash/browser";
import { bash, dec, enc } from "./kt.mjs";
import { repoRoot, scratch } from "./helpers.mjs";
import { hostTestConfig } from "../host/node/host-paths.js";

async function fixture(run) {
  const dir = scratch("test-host-fs");
  const root = join(dir, "root");
  fs.mkdirSync(root);
  try {
    await run(root, dir);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const token = () => "SENTINEL-" + randomUUID();
const chmodMode = (stat) => stat.mode & 0o777;
const assertMessage = (call, message) => assert.rejects(call, { message });

function customFs(initial) {
  const files = new Map([["/t", enc(initial)]]);
  const info = { isFile: true, isDirectory: false, isSymbolicLink: false, mode: 0o644, mtime: new Date() };
  const notFound = () => new Error("ENOENT:");
  const read = async (path) => {
    if (!files.has(path)) throw notFound();
    return files.get(path);
  };
  const meta = async (path) => ({ ...info, size: (await read(path)).length });
  const write = async (path, value) => { files.set(path, typeof value === "string" ? enc(value) : value); };
  const copy = async (src, to) => write(to, await read(src));
  return {
    readFile: async (path) => dec(await read(path)), readFileBuffer: read,
    writeFile: write, appendFile: async (path, value) => write(path, Buffer.concat([await read(path), enc(value)])),
    exists: async (path) => files.has(path), stat: meta, lstat: meta, mkdir: async () => {},
    readdir: async () => [...files.keys()].map((p) => p.slice(1)), rm: async (path) => { files.delete(path); },
    cp: copy, mv: async (src, to) => { await copy(src, to); files.delete(src); },
    getAllPaths: () => [...files.keys()], resolvePath: (base, path) => path.startsWith("/") ? path : join(base, path),
    chmod: async () => {}, symlink: async () => { throw new Error("ENOSYS:"); }, link: copy,
    readlink: async () => { throw new Error("EINVAL:"); }, realpath: async (path) => path, utimes: async () => {},
  };
}

test("D26: explicit adapters read sentinels and the overlay leaves the host inode intact", () => fixture(async (root, dir) => {
  assert.equal("OverlayFs" in browser, false);
  assert.equal("ReadWriteFs" in browser, false);
  const hostFile = join(root, "sentinel.txt");
  const content = token();
  fs.writeFileSync(hostFile, content);
  const before = fs.statSync(hostFile);
  const overlay = new OverlayFs({ root });
  const virtual = overlay.getMountPoint() + "/sentinel.txt";
  const sandbox = bash({ fs: overlay });
  assert.equal(sandbox.getCwd(), "/home/user");
  assert.equal(sandbox.getEnv().HOME, "/home/user");
  assert.equal(await sandbox.readFile("/proc/self/cmdline"), "bash\0");
  assert.equal(await sandbox.readFile(virtual), content);
  assert.equal((await sandbox.exec("__cat " + virtual)).stdout, content);
  await sandbox.writeFile(virtual, "A");
  assert.equal(await sandbox.readFile(virtual), "A");
  assert.equal((await sandbox.exec("__write " + virtual + " B")).exitCode, 0);
  await overlay.appendFile(virtual, "C");
  await overlay.chmod(virtual, 0o600);
  assert.equal(await sandbox.readFile(virtual), "BC");
  assert.equal(chmodMode(await overlay.stat(virtual)), 0o600);
  await overlay.rm(virtual);
  assert.equal(await overlay.exists(virtual), false);
  const after = fs.statSync(hostFile);
  assert.deepEqual([after.ino, after.size, after.mtimeMs], [before.ino, before.size, before.mtimeMs]);
  assert.equal(fs.readFileSync(hostFile, "utf8"), content);

  const second = token();
  const root2 = join(dir, "second");
  fs.mkdirSync(root2);
  fs.writeFileSync(join(root2, "sentinel2.txt"), second);
  const rw = bash({ fs: new ReadWriteFs({ root: root2 }) });
  assert.equal(await rw.readFile("/sentinel2.txt"), second);
  assert.equal((await rw.exec("__cat /sentinel2.txt")).stdout, second);
  await rw.writeFile("/written.txt", "W");
  assert.equal(fs.readFileSync(join(root2, "written.txt"), "utf8"), "W");

  const third = token();
  const custom = bash({ fs: customFs(third) });
  assert.equal(await custom.readFile("/t"), third);
  assert.equal((await custom.exec("__cat /t")).stdout, third);
  assert.throws(() => custom.execSync("__cat /t"), { name: "AsyncCapabilityError" });
  const closed = new Bash();
  for (const name of [virtual, hostFile, "/sentinel2.txt", join(root2, "sentinel2.txt"), "/t"]) {
    await assert.rejects(closed.readFile(name), { code: "ENOENT" });
  }
}));

test("D10: virtual traversal, NUL, prototype names and symlink policy", () => fixture(async (root, dir) => {
  const secret = join(dir, "outside-secret.txt");
  const outside = join(dir, "outside");
  fs.mkdirSync(outside);
  fs.writeFileSync(secret, "SECRET");
  fs.writeFileSync(join(root, "inside"), "INSIDE");
  fs.symlinkSync(secret, join(root, "out"));
  fs.symlinkSync(outside, join(root, "out-dir"));
  fs.symlinkSync(join(dir, "missing"), join(root, "dangling"));
  fs.symlinkSync("inside", join(root, "in"));
  fs.symlinkSync("b", join(root, "a"));
  fs.symlinkSync("a", join(root, "b"));
  fs.symlinkSync(join(root, "inside"), join(root, "absolute-in"));
  fs.mkdirSync(join(root, "level", "sub"), { recursive: true });
  fs.writeFileSync(join(root, "level", "value"), "LEVEL");
  fs.symlinkSync("level/sub", join(root, "shortcut"));
  fs.symlinkSync("shortcut/../value", join(root, "through"));
  for (const Adapter of [OverlayFs, ReadWriteFs]) {
    const overlay = Adapter === OverlayFs;
    const adapter = new Adapter({ root, mountPoint: "/" });
    for (const name of ["%2e%2e", "__proto__", "constructor", "toString"]) {
      await adapter.writeFile("/../" + name, name);
      assert.equal(await adapter.readFile("/../../" + name), name);
    }
    assert.equal(await adapter.exists("a\0b"), false);
    for (const method of ["readFile", "readFileBytes", "readFileBuffer", "writeFile", "appendFile", "stat", "lstat", "mkdir",
      "createExclusive", "readdir", "readdirWithFileTypes", "rm", "cp", "mv", "chmod", "symlink", "link", "readlink", "realpath", "utimes"]) {
      await assert.rejects(adapter[method]("a\0b", method === "createExclusive" ? { mode: 0o600 } : "x"), /ENOENT: path contains null byte/);
    }
    assert.throws(() => adapter.resolvePath("a\0b", "x"), /ENOENT: path contains null byte/);
    await assert.rejects(adapter.cp("/inside", "/a\0b"), /ENOENT: path contains null byte/);
    for (const [name, target] of [["out", "outside-secret.txt"], ["dangling", "missing"], ["in", "inside"]]) {
      const path = "/" + name;
      assert.equal(await adapter.exists(path), false);
      assert.equal((await adapter.lstat(path)).isSymbolicLink, true);
      assert.equal(await adapter.readlink(path), target);
      for (const method of ["readFile", "stat"]) {
        await assert.rejects(adapter[method](path), { code: overlay ? "ENOENT" : "EACCES" });
      }
    }
    assert.equal(await adapter.readlink("/absolute-in"), "/inside");
    if (overlay) assert.deepEqual(await adapter.readdir("/out-dir"), []);
    else await assert.rejects(adapter.readdir("/out-dir"), /resolves outside sandbox/);
    for (const [method, args] of [
      ["createExclusive", ["/out-dir/exclusive", { mode: 0o600 }]],
      ["writeFile", ["/out-dir/write", "W"]],
      ["appendFile", ["/out-dir/append", "A"]],
      ["mkdir", ["/out-dir/dir"]],
    ]) {
      // Fresh overlay so every operation sees the host symlink.
      if (overlay) await new OverlayFs({ root, mountPoint: "/" })[method](...args);
      else await assert.rejects(adapter[method](...args), /resolves outside sandbox/);
    }
    await assertMessage(adapter.symlink("inside", "/new"), "EPERM: operation not permitted, symlink '/new'");
    const permitted = new Adapter({ root, mountPoint: "/", allowSymlinks: true });
    assert.equal(await permitted.readFile("/in"), "INSIDE");
    assert.equal(await permitted.realpath("/in"), "/inside");
    assert.equal(await permitted.readFile("/through"), "LEVEL");
    assert.equal(await permitted.realpath("/through"), "/level/value");
    await permitted.writeFile("/through", "update");
    assert.equal(await permitted.readFile("/level/value"), "update");
    if (overlay) assert.equal(fs.readFileSync(join(root, "level", "value"), "utf8"), "LEVEL");
    await assert.rejects(permitted.readFile("/out"), { code: "EACCES" });
    await assert.rejects(permitted.readFile("/dangling"), { code: "EACCES" });
    await assert.rejects(permitted.realpath("/a"), { code: "ELOOP" });
  }
  assert.equal(fs.readFileSync(secret, "utf8"), "SECRET");
  assert.deepEqual(fs.readdirSync(outside), []);

  const rw = new ReadWriteFs({ root, allowSymlinks: true });
  fs.mkdirSync(join(root, "one", "two"), { recursive: true });
  fs.symlinkSync("../../inside", join(root, "one", "two", "link"));
  await assertMessage(rw.mv("/one/two/link", "/one/link"), "EACCES: permission denied, mv '/one/two/link' -> '/one/link' would create symlink escaping sandbox");
  fs.mkdirSync(join(root, "two"));
  await assertMessage(rw.mv("/one/two", "/two"), "EACCES: permission denied, mv '/one/two' -> '/two' would create symlinks escaping sandbox");
  assert.deepEqual(fs.readdirSync(join(root, "two")), []);
  assert.equal(await rw.readFile("/one/two/link"), "INSIDE");
}));

test("D10: FIFO methods settle without opening the FIFO for data", { skip: process.platform !== "linux" }, () =>
  fixture(async (root) => {
    execFileSync("/usr/bin/mkfifo", [join(root, "fifo")]);
    for (const Adapter of [ReadWriteFs, OverlayFs]) {
      const overlay = Adapter === OverlayFs;
      const adapter = new Adapter({ root, mountPoint: "/" });
      const fast = async (run) => {
        const start = performance.now();
        await run();
        assert.ok(performance.now() - start < 500);
      };
      for (const name of ["readFile", "readFileBuffer", "readFileBytes"]) {
        await fast(() => assertMessage(adapter[name]("/fifo"), "EACCES: cannot read special file '/fifo'"));
      }
      await fast(() => assertMessage(adapter.appendFile("/fifo", "x"), "EACCES: cannot append special file '/fifo'"));
      await fast(() => overlay ? adapter.cp("/fifo", "/copy") : assertMessage(adapter.cp("/fifo", "/copy"), "EIO: cp '/fifo'"));
      assert.equal(await adapter.exists("/copy"), false);
      await fast(async () => {
        assert.equal((await adapter.stat("/fifo")).isFile, false);
        assert.equal((await adapter.lstat("/fifo")).isFile, false);
        assert.equal(await adapter.exists("/fifo"), true);
        assert.deepEqual(await adapter.readdir("/"), ["fifo"]);
      });
      await fast(() => overlay ? adapter.writeFile("/fifo", "memory") : assertMessage(adapter.writeFile("/fifo", "x"), "EACCES: cannot write special file '/fifo'"));
      assert.ok(fs.lstatSync(join(root, "fifo")).isFIFO());
    }
  }));

test("D10: atomic exclusive creation checks inode identity and preserves host hard links", () => fixture(async (root, dir) => {
  for (const Adapter of [OverlayFs, ReadWriteFs]) {
    const adapter = new Adapter({ root, mountPoint: "/" });
    const results = await Promise.allSettled(Array.from({ length: 20 }, () => adapter.createExclusive("/lock", { mode: 0o600 })));
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    for (const result of results.filter((r) => r.status === "rejected")) {
      assert.equal(result.reason.message, "EEXIST: file already exists, open '/lock'");
    }
    assert.equal(chmodMode(await adapter.stat("/lock")), 0o600);
    await assertMessage(adapter.createExclusive("/nodir/x", { mode: 0o600 }), "ENOENT: no such file or directory, open '/nodir/x'");
    await adapter.createExclusive("/lockdir", { mode: 0o700, directory: true });
    assert.equal(chmodMode(await adapter.stat("/lockdir")), 0o700);
  }
  const rw = new ReadWriteFs({ root, maxCopyOnWriteSize: 16 });
  fs.mkdirSync(join(root, "swap"));
  const outsideDir = join(dir, "elsewhere");
  fs.mkdirSync(outsideDir);
  fs.writeFileSync(join(outsideDir, "lock"), "OUTSIDE");
  rw._afterExclusiveCreate = () => {
    fs.renameSync(join(root, "swap"), join(root, "original"));
    fs.symlinkSync(outsideDir, join(root, "swap"));
  };
  await assertMessage(rw.createExclusive("/swap/lock", { mode: 0o600 }), "EACCES: permission denied, '/swap/lock' resolves outside sandbox during open");
  assert.equal(fs.readFileSync(join(outsideDir, "lock"), "utf8"), "OUTSIDE");
  assert.ok(fs.existsSync(join(root, "original", "lock")));
  const original = join(dir, "outside-hardlink.txt");
  fs.writeFileSync(original, "ORIGINAL");
  const before = fs.statSync(original);
  const linked = join(root, "linked.txt");
  for (const run of [
    () => rw.appendFile("/linked.txt", "+"), () => rw.chmod("/linked.txt", 0o600),
    () => rw.utimes("/linked.txt", new Date(1_000), new Date(2_000)), () => rw.writeFile("/linked.txt", "CHANGED"),
    () => rw.cp("/source", "/linked.txt"),
  ]) {
    fs.writeFileSync(join(root, "source"), "COPY");
    fs.linkSync(original, linked);
    assert.equal(fs.statSync(linked).nlink, 2);
    await run();
    const after = fs.statSync(original);
    assert.deepEqual([after.mode, after.mtimeMs, fs.readFileSync(original, "utf8")], [before.mode, before.mtimeMs, "ORIGINAL"]);
    assert.equal(fs.statSync(linked).nlink, 1);
    fs.unlinkSync(linked);
  }
  fs.writeFileSync(original, "x".repeat(19));
  fs.linkSync(original, join(root, "l2.txt"));
  await assertMessage(rw.appendFile("/l2.txt", "+"), "EFBIG: file too large for copy-on-write append '/l2.txt' (19 bytes, max 16)");
  await rw.writeFile("/l2.txt", "ok");
  assert.equal(fs.readFileSync(original, "utf8"), "x".repeat(19));
  assert.equal(await rw.readFile("/l2.txt"), "ok");
}));

test("D10: mutations on canonical overlapping roots serialize while disjoint roots overlap", () => fixture(async (root, dir) => {
  const sub = join(root, "sub");
  const other = join(dir, "other");
  fs.mkdirSync(sub);
  fs.mkdirSync(other);
  const events = [];
  const adapters = [root, sub, other].map((root, id) => {
    const adapter = new ReadWriteFs({ root });
    adapter._onMutation = async (event, op, path) => {
      events.push({ id, event, op, path, time: performance.now() });
      if (event === "start") await sleep(2);
    };
    return adapter;
  });
  const calls = Array.from({ length: 20 }, (_, i) => {
    const id = i % 3;
    const adapter = adapters[id];
    const name = "/f" + i;
    if (i % 2 === 0) return adapter.writeFile(name, String(i));
    fs.writeFileSync(join([root, sub, other][id], name), String(i));
    return adapter.mv(name, name + "-moved");
  });
  await Promise.all(calls);
  assert.equal(events.length, 40);
  const running = new Set();
  let overlapped = false;
  for (const { id, event } of events) {
    if (event === "end") {
      assert.ok(running.delete(id));
    } else {
      assert.ok(!running.has(id));
      if (id < 2) assert.ok(!running.has(1 - id));
      if (running.size) overlapped = true;
      running.add(id);
    }
  }
  assert.ok(overlapped);
  assert.deepEqual(events.filter(({ id, event }) => id < 2 && event === "start").map(({ path }) => Number(path.slice(2))),
    Array.from({ length: 20 }, (_, i) => i).filter((i) => i % 3 !== 2));
}));

test("S7: adapter messages, stacks and exec stderr contain only virtual names", () => fixture(async (root, dir) => {
  fs.mkdirSync(join(root, "dir"));
  fs.writeFileSync(join(root, "dir", "file"), "abc");
  fs.symlinkSync(join(dir, "outside"), join(root, "escape"));
  fs.symlinkSync("loop", join(root, "loop"));
  const rw = new ReadWriteFs({ root, allowSymlinks: true, maxFileReadSize: 2 });
  const ro = new OverlayFs({ root, mountPoint: "/", readOnly: true });
  const overlay = new OverlayFs({ root, mountPoint: "/" });
  const verify = (error, code) => {
    if (code) assert.equal(error.code, code);
    for (const path of [root, dir, repoRoot, homedir()]) {
      assert.ok(!error.message.includes(path), error.message);
      assert.ok(!error.stack.includes(path), error.stack);
    }
    assert.equal(error.path, undefined);
    assert.equal(error.dest, undefined);
    assert.equal(error.syscall, undefined);
    return true;
  };
  for (const [call, code] of [
    [() => rw.readFile("/missing"), "ENOENT"], [() => rw.readFile("/escape"), "EACCES"],
    [() => rw.mkdir("/dir"), "EEXIST"], [() => rw.readFile("/dir"), "EISDIR"],
    [() => rw.readdir("/dir/file"), "ENOTDIR"], [() => overlay.rm("/dir"), "ENOTEMPTY"],
    [() => rw.stat("/loop"), "ELOOP"], [() => rw.readFile("/dir/file"), "EFBIG"],
    [() => ro.writeFile("/dir/file", "x"), "EROFS"], [() => rw.rm("/dir"), "ERR_FS_EISDIR"],
    [() => rw.mkdir("/dir/file", { recursive: true }), "EEXIST"],
  ]) await assert.rejects(call(), (error) => verify(error, code));
  for (const Adapter of [ReadWriteFs, OverlayFs]) {
    assert.throws(() => new Adapter({ root: join(root, "missing") }), (error) => {
      assert.equal(error.message, Adapter.name + " root does not exist");
      return verify(error);
    });
    assert.throws(() => new Adapter({ root: join(root, "dir", "file") }), (error) => {
      assert.equal(error.message, Adapter.name + " root is not a directory");
      return verify(error);
    });
  }
  const r = await bash({ fs: rw }).exec("__cat /escape");
  assert.equal(r.exitCode, 1);
  assert.equal(r.stderr, "cat: /escape: Permission denied\n");
}));

test("the facade seeds layout and custom command stubs in synchronous host filesystems", () => fixture(async (root) => {
  const overlay = new OverlayFs({ root, readOnly: true });
  const cmd = (name) => ({ name, execute: () => ({ stdout: "", stderr: "", exitCode: 0 }) });
  const sandbox = bash({ fs: overlay, customCommands: [cmd("first")], processInfo: { pid: 22, ppid: 9 } });
  assert.equal(await sandbox.readFile("/bin/first"), "#!/bin/bash\n# Built-in command: first\n");
  assert.match(await sandbox.readFile("/proc/self/status"), /Pid:\t22\nPPid:\t9\n/);
  sandbox.registerCommand(cmd("second"));
  assert.equal(await sandbox.readFile("/usr/bin/second"), "#!/bin/bash\n# Built-in command: second\n");
  assert.deepEqual(fs.readdirSync(root), []);

  const order = [];
  const custom = {
    mkdirSync(path) {
      order.push(path);
      if (path === "/bin") throw new Error("ENOSYS: unsupported");
    },
    writeFileSync(path) { order.push(path); },
  };
  const withCustomCwd = bash({ fs: custom, cwd: "/workspace" });
  assert.equal(withCustomCwd.getEnv().HOME, "/");
  assert.deepEqual(order.slice(0, 5), ["/bin", "/usr/bin", "/dev", "/dev/fd", "/dev/null"]);
  assert.ok(order.indexOf("/workspace") > order.indexOf("/proc/self/fd/2"));
  assert.match(order.at(-1), /^\/(usr\/)?bin\//);
  assert.throws(() => bash({ fs: { ...custom, mkdirSync() { throw new Error("EIO: mkdir"); } } }), { message: "EIO: mkdir" });
}));

test("overlay merges files and tombstones without reviving removed host descendants", () => fixture(async (root) => {
  for (const name of ["_", "B", "a"]) fs.writeFileSync(join(root, name), "host");
  fs.mkdirSync(join(root, "dir"));
  fs.writeFileSync(join(root, "dir", "old"), "host");
  const overlay = new OverlayFs({ root, mountPoint: "/", maxMemoryBytes: 8 });
  assert.deepEqual(await overlay.readdir("/"), ["B", "_", "a", "dir"]);
  await overlay.writeFile("/a", "own");
  assert.match((await overlay.stat("/a")).identity, /^overlay:[0-9]+$/);
  await overlay.rm("/dir", { recursive: true });
  assert.equal(await overlay.exists("/dir/old"), false);
  await overlay.writeFile("/dir/new", "new");
  assert.deepEqual(await overlay.readdir("/dir"), ["new"]);
  assert.deepEqual(overlay.getAllPaths(), ["/", "/B", "/_", "/a", "/dir", "/dir/new"]);
  await assertMessage(overlay.writeFile("/unknown/parent/name", "abc"), "ENOSPC: overlay memory byte limit exceeded (8 bytes)");
  assert.equal(await overlay.exists("/unknown"), false);
  assert.equal(fs.readFileSync(join(root, "dir", "old"), "utf8"), "host");
  assert.equal(new OverlayFs({ root }).getAllPaths().includes("/home/user/project/dir"), false);
}));

test("D10: a 1,000-file walk uses one host call and eleven listings per adapter", (t) => fixture(async (root) => {
  for (let d = 0; d < 10; d++) {
    const dir = join(root, "d" + d);
    fs.mkdirSync(dir);
    for (let i = 0; i < 100; i++) fs.writeFileSync(join(dir, "f" + i), "");
  }
  for (const [Adapter, path] of [[OverlayFs, "/home/user/project"], [ReadWriteFs, "/"]]) {
    const adapter = new Adapter({ root });
    let listings = 0;
    const original = adapter._listHost.bind(adapter);
    adapter._listHost = (dir) => { listings++; return original(dir); };
    const seen = [];
    const b = bash({ fs: adapter }, { onHostCall: (request) => { seen.push(request.tag); } });
    const start = performance.now();
    const result = await b.exec(`__walk ${path} 10 2000`);
    const elapsed = performance.now() - start;
    assert.equal(result.exitCode, 0, result.stderr);
    assert.equal(result.stdout, "1010\n");
    assert.deepEqual(seen, ["fs-batch"]);
    assert.equal(listings, 11);
    t.diagnostic(`${Adapter.name}: ${seen.length} host call, ${listings} listings, ${elapsed.toFixed(2)} ms`);
  }
}));

test("kernel mount lookup uses the longest component-boundary match and checks grants", () => fixture(async (root) => {
  fs.writeFileSync(join(root, "t"), "TOKEN");
  const calls = [];
  const hooks = {
    onHostCall: (request) => calls.push(request.val),
    onEngine: (engine) => {
      const create = engine.Shell.create;
      engine.Shell.create = (vfs, config) => {
        vfs.mountHost(enc("/deep//./dir/.."), 1);
        assert.throws(() => vfs.mountHost(enc("/deep"), 8), { code: "exists" });
        return create(vfs, config);
      };
    },
  };
  const adapter = new ReadWriteFs({ root });
  const allowed = bash({ fs: adapter }, hooks);
  assert.equal((await allowed.exec("__cat /deep/t")).stdout, "TOKEN");
  assert.equal(dec(calls[0].ops[0].val.path), "/t");
  const missing = await allowed.exec("__cat /deeper/t");
  assert.equal(missing.exitCode, 1);
  assert.equal(dec(calls[1].ops[0].val.path), "/deeper/t");

  const denied = bash({ fs: adapter }, {
    onHostCall() { assert.fail("ungranted host call reached driver"); },
    onEngine(engine) {
      const create = engine.Shell.create;
      engine.Shell.create = (vfs, config) => create(vfs, { ...config, grants: {} });
    },
  });
  const result = await denied.exec("__cat /t");
  assert.equal(result.exitCode, 1);
  assert.equal(result.stderr, "cat: /t: Permission denied\n");
}));

test("S6: abort discards a late filesystem outcome and stops the rest of the batch", async () => {
  const adapter = customFs("TOKEN");
  let starts;
  const started = new Promise((resolve) => { starts = resolve; });
  adapter.readFileBuffer = async () => {
    starts();
    await sleep(2000);
    return enc("late");
  };
  const abort = new AbortController();
  const run = bash({ fs: adapter }).exec("__cat /t", { signal: abort.signal });
  await started;
  setTimeout(() => abort.abort(), 20);
  const result = await run;
  assert.equal(result.exitCode, 124);
  assert.equal(result.stdout, "");

  let release, entered;
  const resumed = new Promise((resolve) => { release = resolve; });
  const inMkdir = new Promise((resolve) => { entered = resolve; });
  const seen = [];
  const batch = bash({ fs: {
    mkdir: () => { entered(); return resumed; },
    createExclusive: () => { seen.push("exclusive"); },
    writeFile: () => { seen.push("write"); },
  } });
  const next = new AbortController();
  const pending = batch.exec("__fsbatch 1", { signal: next.signal });
  await inMkdir;
  next.abort();
  assert.equal((await pending).exitCode, 124);
  release();
  await sleep(0);
  assert.deepEqual(seen, []);
});

test("Linux Node open flags reject a symlink leaf and permit a real directory", { skip: process.platform !== "linux" }, () =>
  fixture(async (root) => {
    const path = join(root, "real");
    fs.mkdirSync(path);
    const link = join(root, "link");
    fs.symlinkSync("real", link);
    const { O_RDONLY, O_DIRECTORY, O_NOFOLLOW, O_NONBLOCK } = fs.constants;
    assert.throws(() => fs.openSync(link, O_RDONLY | O_NOFOLLOW | O_NONBLOCK), { code: "ELOOP" });
    const fd = fs.openSync(path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    try {
      assert.equal(fs.fstatSync(fd).isDirectory(), true);
    } finally {
      fs.closeSync(fd);
    }
  }));

test("finding 1: command resolution and options.files reach host mounts", () => fixture(async (root) => {
  const overlay = new OverlayFs({ root });
  const seen = [];
  const b = bash({ fs: overlay }, { onHostCall: (request) => seen.push(request.tag) });
  // An explicit host path dispatches instead of failing 127 ...
  const explicit = await b.exec("/bin/echo hi");
  assert.equal(explicit.exitCode, 0, explicit.stderr);
  assert.equal(explicit.stdout, "hi\n");
  assert.ok(seen.includes("fs-batch"));
  // ... a bare name resolves through the mount ...
  const before = seen.length;
  const bare = await b.exec("echo again");
  assert.equal(bare.exitCode, 0, bare.stderr);
  assert.equal(bare.stdout, "again\n");
  assert.ok(seen.length > before);
  // ... but PATH='' with a seeded /usr/bin does not fall back (fresh shell, so
  // no recorded path answers first, per the path-table gate).
  assert.equal((await bash({ fs: overlay }).exec("PATH='' echo hi")).exitCode, 127);
  // A host fs without /usr/bin still runs commands through the fallback,
  // consulting the mount first.
  const rwSeen = [];
  const fallback = bash({ fs: new ReadWriteFs({ root }) }, { onHostCall: (request) => rwSeen.push(request.tag) });
  assert.equal((await fallback.exec("echo hi")).exitCode, 0);
  assert.deepEqual(rwSeen, ["fs-batch", "fs-batch", "fs-batch"]);
  // options.files land in the host fs, not the shadowed kernel volume.
  const withFiles = bash({ fs: new OverlayFs({ root, mountPoint: "/" }), files: { "/hello.txt": "HI" } });
  assert.equal(await withFiles.readFile("/hello.txt"), "HI");
  assert.equal((await withFiles.exec("__cat /hello.txt")).stdout, "HI");
  // ReadWriteFs (no synchronous seed methods) seeds in the background.
  const rwFiles = bash({ fs: new ReadWriteFs({ root }), files: { "/rw.txt": "RW" } });
  assert.equal(await rwFiles.readFile("/rw.txt"), "RW");
  assert.equal((await rwFiles.exec("__cat /rw.txt")).stdout, "RW");
  // Lazy file values are rejected on host mounts instead of going missing.
  assert.throws(() => bash({ fs: new OverlayFs({ root }), files: { "/x": () => "lazy" } }), { name: "TypeError" });
}));

test("finding 3: '..' never skips an intermediate symlink", () => fixture(async (root, dir) => {
  fs.writeFileSync(join(root, "inside"), "INSIDE");
  const outside = join(dir, "outside");
  fs.mkdirSync(outside);
  fs.symlinkSync(outside, join(root, "out-dir"));
  fs.mkdirSync(join(root, "level", "sub"), { recursive: true });
  fs.writeFileSync(join(root, "level", "value"), "LEVEL");
  fs.symlinkSync("level/sub", join(root, "shortcut"));
  for (const Adapter of [ReadWriteFs, OverlayFs]) {
    const overlay = Adapter === OverlayFs;
    const adapter = new Adapter({ root, mountPoint: "/" });
    const code = overlay ? "ENOENT" : "EACCES";
    // A `..` collapsing across `out-dir` must not skip its check.
    await assert.rejects(adapter.readFile("/out-dir/../inside"), { code });
    await assert.rejects(adapter.stat("/out-dir/../inside"), { code });
    if (overlay) assert.equal(await adapter.exists("/out-dir/../inside"), false);
    else await assert.rejects(adapter.writeFile("/out-dir/../inside", "x"), { code });
  }
  for (const Adapter of [ReadWriteFs, OverlayFs]) {
    const adapter = new Adapter({ root, mountPoint: "/", allowSymlinks: true });
    assert.equal(await adapter.readFile("/shortcut/../value"), "LEVEL");
    assert.equal(await adapter.realpath("/shortcut/../value"), "/level/value");
  }
}));

test("finding 4: the root opens directly and works without procfs descriptors", () => fixture(async (root, dir) => {
  fs.writeFileSync(join(root, "sentinel"), "S");
  // Traverse-only ancestors above the root need no read permission.
  const locked = join(dir, "locked");
  const inner = join(locked, "root");
  fs.mkdirSync(inner, { recursive: true });
  fs.writeFileSync(join(inner, "file"), "DATA");
  fs.chmodSync(locked, 0o111);
  try {
    for (const Adapter of [ReadWriteFs, OverlayFs]) {
      const adapter = Adapter === OverlayFs ? new Adapter({ root: inner, mountPoint: "/" }) : new Adapter({ root: inner });
      assert.equal(await adapter.readFile("/file"), "DATA");
      await adapter.writeFile("/new", "N");
      assert.equal(await adapter.readFile("/new"), "N");
    }
  } finally {
    fs.chmodSync(locked, 0o755);
  }
  // Without /proc/self/fd the adapters fall back to validated canonical paths.
  hostTestConfig.forceFallback = true;
  try {
    const rw = new ReadWriteFs({ root });
    assert.equal(await rw.readFile("/sentinel"), "S");
    await rw.writeFile("/w", "W");
    assert.equal(fs.readFileSync(join(root, "w"), "utf8"), "W");
    assert.equal(await rw.exists("/missing"), false);
    const overlay = new OverlayFs({ root, mountPoint: "/" });
    assert.equal(await overlay.readFile("/sentinel"), "S");
    await overlay.writeFile("/w2", "W");
    assert.equal(await overlay.readFile("/w2"), "W");
  } finally {
    hostTestConfig.forceFallback = false;
  }
}));

test("finding 5: maxFileReadSize applies to memory-copied host files", () => fixture(async (root) => {
  fs.writeFileSync(join(root, "big"), "x".repeat(20 << 20));
  for (const [method, args, target] of [
    ["chmod", [0o600], "/big"], ["utimes", [new Date(1000), new Date(2000)], "/big"],
    ["appendFile", ["+"], "/big"], ["cp", ["/big", "/copy"], "/copy"], ["link", ["/big", "/linked"], "/linked"],
  ]) {
    const overlay = new OverlayFs({ root, mountPoint: "/" });
    if (method === "cp") await overlay.cp(...args);
    else if (method === "link") await overlay.link(...args);
    else await overlay[method]("/big", ...args);
    await assert.rejects(overlay.readFile(target), { code: "EFBIG" });
    await assert.rejects(overlay.readFileBuffer(target), { code: "EFBIG" });
  }
  const overlay = new OverlayFs({ root, mountPoint: "/" });
  await assert.rejects(overlay.readFile("/big"), { code: "EFBIG" });
  assert.equal(fs.readFileSync(join(root, "big"), "utf8").length, 20 << 20);
}));

test("finding 7: dot command names do not corrupt the seeded layout", () => fixture(async (root) => {
  const overlay = new OverlayFs({ root });
  const cmd = (name) => ({ name, execute: () => ({ stdout: "", stderr: "", exitCode: 0 }) });
  const sandbox = bash({ fs: overlay, customCommands: [cmd("."), cmd("..")] });
  assert.equal((await overlay.stat("/bin")).isDirectory, true);
  assert.equal((await overlay.stat("/usr/bin")).isDirectory, true);
  sandbox.registerCommand(cmd("."));
  sandbox.registerCommand(cmd(".."));
  assert.equal((await overlay.stat("/bin")).isDirectory, true);
  assert.equal((await overlay.stat("/usr/bin")).isDirectory, true);
}));

test("finding 8: virtual paths survive error sanitizing", () => fixture(async (root) => {
  for (const Adapter of [ReadWriteFs, OverlayFs]) {
    const adapter = new Adapter({ root, mountPoint: "/" });
    await assert.rejects(adapter.readFile("/tmp/foo"), {
      message: "ENOENT: no such file or directory, open '/tmp/foo'",
    });
    await assert.rejects(adapter.stat("/usr/bar"), {
      message: "ENOENT: no such file or directory, stat '/usr/bar'",
    });
  }
}));

test("finding 10: recursive cp follows contained links when allowed", () => fixture(async (root) => {
  fs.mkdirSync(join(root, "a"));
  fs.writeFileSync(join(root, "a", "file"), "DATA");
  fs.symlinkSync("a", join(root, "link"));
  fs.symlinkSync("file", join(root, "a", "alias"));
  const rw = new ReadWriteFs({ root, allowSymlinks: true });
  await rw.cp("/link", "/copy", { recursive: true });
  assert.equal(await rw.readFile("/copy/file"), "DATA");
  await rw.cp("/a", "/a2", { recursive: true });
  assert.equal(await rw.readFile("/a2/alias"), "DATA");
  const strict = new ReadWriteFs({ root });
  await assert.rejects(strict.cp("/a", "/a3", { recursive: true }), { code: "EIO" });
}));

test("overlay '..' resolves the host suffix from the mount boundary of the walk", () => fixture(async (root) => {
  // The mount point itself is the host root: `/home/user/project/X` is `<root>/X`.
  fs.writeFileSync(join(root, "sentinel.txt"), "S");
  const overlay = new OverlayFs({ root, mountPoint: "/home/user/project" });
  // A `..` beside the mount: the uncollapsed text is not under the mount, so
  // the old suffix was null (spurious ENOENT).
  assert.equal(await overlay.readFile("/home/user/other/../project/sentinel.txt"), "S");
  assert.equal(await overlay.realpath("/home/user/other/../project/sentinel.txt"), "/home/user/project/sentinel.txt");
  // A `..` at the mount re-enters it: the suffix restarts at the boundary, so
  // the old `/..`-prefixed suffix (which clamped at the root and missed the
  // file) is never handed to the host.
  assert.equal(await overlay.readFile("/home/user/project/../project/sentinel.txt"), "S");
  assert.equal(await overlay.realpath("/home/user/project/../project/sentinel.txt"), "/home/user/project/sentinel.txt");
}));
