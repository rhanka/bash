// Gates 8, 16 (release bytes) and 18 of lot L1b: the kernel driven directly, and the filesystem slice.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Bash, InMemoryFs, T, bash, countKernelCalls, dec, enc, internal, module } from "./kt.mjs";
import { Instance } from "../host/core/instance.js";
import { repoRoot } from "./helpers.mjs";
import { join } from "node:path";

/** A Bash on the test kernel plus the raw engine of its instance. */
function withEngine(options) {
  let engine;
  const b = new Bash(T(options, { onEngine: (e) => (engine = e) }));
  return { b, engine };
}

const thrown = (fn) => {
  try {
    fn();
  } catch (e) {
    return e;
  }
  assert.fail("did not throw");
};

const config = (custom = []) => ({ limits: { profile: "contain", overrides: [] }, grants: { customCommands: true }, customCommands: custom, env: [], cwd: undefined, defaultLayout: false, trace: false, coverage: false });
const opts = (script) => ({ script: enc(script), args: [], env: [], replaceEnv: false, rawScript: false });

test("gate 8: epochs and cancel, driving the kernel directly", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(1000000n);
  const shell = engine.Shell.create(vfs, config(["foo"]));
  const f = engine.Fiber.spawn(shell, undefined, opts("foo a\n__out 3"));
  const s = f.run(100n);
  assert.equal(s.tag, "host-call");
  assert.equal(s.val.epoch, 1n);
  assert.equal(thrown(() => f.resume(7n, { tag: "command-exit", val: 0 })).code, "stale-epoch");
  assert.equal(thrown(() => f.readCallInput(2n, 0, 10)).code, "stale-epoch");
  assert.equal(thrown(() => f.pushCallData(0n, "stdout", enc("x"))).code, "stale-epoch");
  assert.equal(thrown(() => f.assignVariable(3n, enc("A"), enc("1"), undefined)).code, "stale-epoch");
  assert.equal(thrown(() => f.setCallEnv(5n, [])).code, "stale-epoch");
  f.pushCallData(1n, "stdout", enc("kept"));
  const r = f.cancel("aborted");
  assert.equal(r.status, "aborted");
  assert.equal(r.exitCode, 124);
  for (const call of [() => f.resume(1n, { tag: "completed" }), () => f.readCallInput(1n, 0, 1), () => f.pushCallData(1n, "stdout", enc("x")), () => f.assignVariable(1n, enc("A"), enc("1"), undefined), () => f.setCallEnv(1n, [])]) {
    assert.equal(thrown(call).code, "bad-state");
  }
  assert.equal(dec(f.takeStdout(100)), "kept");
  assert.equal(dec(f.takeStderr(100)), "bash: execution aborted\n");
  assert.equal(f.run(10n).tag, "done");
});

test("epochs: numbering from 1 and re-use of a resumed epoch", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(1000000n);
  const shell = engine.Shell.create(vfs, config(["foo"]));
  const f = engine.Fiber.spawn(shell, undefined, opts("foo\nfoo"));
  assert.equal(f.run(100n).val.epoch, 1n);
  const next = f.resume(1n, { tag: "command-exit", val: 0 });
  assert.equal(next.val.epoch, 2n);
  assert.equal(thrown(() => f.resume(1n, { tag: "command-exit", val: 0 })).code, "stale-epoch");
  assert.equal(f.resume(2n, { tag: "command-exit", val: 4 }).tag, "done");
});

test("kernel assignments preserve existing exports and leave new variables unexported", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(1000000n);
  const c = config(["foo"]);
  c.env = [{ name: enc("OLD"), value: enc("before"), exported: true }];
  const shell = engine.Shell.create(vfs, c);
  const f = engine.Fiber.spawn(shell, undefined, opts("foo"));
  const { epoch } = f.run(100n).val;
  f.assignVariable(epoch, enc("OLD"), enc("after"), undefined);
  f.assignVariable(epoch, enc("NEW"), enc("new"), undefined);
  const { env } = f.resume(epoch, { tag: "command-exit", val: 0 }).val;
  assert.deepEqual(env.filter(({ name }) => ["OLD", "NEW"].includes(dec(name))), [
    { name: enc("OLD"), value: enc("after"), exported: true },
    { name: enc("NEW"), value: enc("new"), exported: false },
  ]);
});

test("a host call uses up its slice and resume cannot reset the fuel", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(1000000n);
  const shell = engine.Shell.create(vfs, config(["foo"]));
  const f = engine.Fiber.spawn(shell, undefined, opts("foo\nfoo\n__out 1"));
  assert.equal(f.run(2n).val.epoch, 1n);
  assert.equal(f.resume(1n, { tag: "command-exit", val: 0 }).val.epoch, 2n);
  assert.equal(f.resume(2n, { tag: "command-exit", val: 0 }).tag, "out-of-fuel");
  assert.equal(f.takeStdout(2).length, 0);
  assert.equal(f.run(1n).tag, "done");
  assert.equal(dec(f.takeStdout(2)), "a");
});

test("gate 18: every operation of the filesystem slice", () => {
  const { engine } = withEngine();
  const v = engine.Vfs.create(2000n);
  const p = (s) => enc(s);
  const ok = (r) => (assert.equal(r.tag, "ok", JSON.stringify(r, (_, x) => (typeof x === "bigint" ? String(x) : x))), r.val);
  const err = (r) => (assert.equal(r.tag, "err"), r.val.code);
  assert.equal(ok(v.op({ tag: "stat", val: p("/") })).val.kind, "directory");
  assert.equal(ok(v.op({ tag: "exists", val: p("/nope") })).val, false);
  assert.equal(err(v.op({ tag: "stat", val: p("/nope") })), "not-found");
  assert.equal(err(v.op({ tag: "write-file", val: { path: p("/no/dir/f"), data: p("x"), offset: 0n, truncate: true } })), "not-found");
  ok(v.op({ tag: "write-file", val: { path: p("/a"), data: p("hello"), offset: 0n, truncate: true, mode: 0o600, mtimeMs: 5n } }));
  const st = ok(v.op({ tag: "lstat", val: p("/a") })).val;
  assert.deepEqual([st.kind, st.size, st.mode, st.mtimeMs], ["file", 5n, 0o600, 5n]);
  assert.equal(dec(ok(v.op({ tag: "read-file", val: { path: p("/a"), offset: 1n, len: 3 } })).val), "ell");
  ok(v.op({ tag: "write-file", val: { path: p("/a"), data: p("!!"), offset: undefined, truncate: false } }));
  assert.equal(dec(ok(v.op({ tag: "read-file", val: { path: p("/a"), offset: 0n, len: 100 } })).val), "hello!!");
  ok(v.op({ tag: "mkdir", val: { path: p("/d/e"), recursive: true } }));
  assert.equal(err(v.op({ tag: "mkdir", val: { path: p("/d"), recursive: false } })), "exists");
  assert.equal(err(v.op({ tag: "mkdir", val: { path: p("/x/y"), recursive: false } })), "not-found");
  ok(v.op({ tag: "write-file", val: { path: p("/d/b"), data: p(""), offset: 0n, truncate: true } }));
  const names = ok(v.op({ tag: "read-dir", val: p("/d") })).val.map((e) => `${dec(e.name)}:${e.kind}`);
  assert.deepEqual(names, ["b:file", "e:directory"]);
  assert.equal(err(v.op({ tag: "read-dir", val: p("/a") })), "not-directory");
  assert.equal(err(v.op({ tag: "read-file", val: { path: p("/d"), offset: 0n, len: 1 } })), "is-directory");
  assert.deepEqual(ok(v.op({ tag: "all-paths" })).val.map(dec), ["/", "/a", "/d", "/d/b", "/d/e"]);
  assert.equal(dec(ok(v.op({ tag: "read-file", val: { path: p("/d/../a"), offset: 0n, len: 2 } })).val), "he");
  assert.equal(err(v.op({ tag: "remove", val: { path: p("/d"), recursive: false, force: false } })), "not-empty");
  ok(v.op({ tag: "remove", val: { path: p("/d"), recursive: true, force: false } }));
  assert.equal(err(v.op({ tag: "remove", val: { path: p("/d"), recursive: false, force: false } })), "not-found");
  ok(v.op({ tag: "remove", val: { path: p("/d"), recursive: false, force: true } }));
  assert.equal(err(v.op({ tag: "write-file", val: { path: p("/big"), data: new Uint8Array(5000), offset: 0n, truncate: true } })), "no-space");
  assert.ok(v.usedBytes() <= 2000n);
  for (const tag of ["read-link", "real-path", "read-dir-plus"]) assert.equal(err(v.op({ tag, val: p("/a") })), "unsupported");
  assert.equal(err(v.op({ tag: "copy", val: { source: p("/a"), target: p("/b"), recursive: false } })), "unsupported");
  assert.equal(err(v.op({ tag: "chmod", val: { path: p("/a"), mode: 0 } })), "unsupported");
  const shell = engine.Shell.create(v, config());
  void shell;
  const other = engine.Vfs.create(2000n);
  assert.equal(thrown(() => v.mountVfs(p("/m"), other)).code, "unsupported");
  assert.equal(thrown(() => v.mountHost(p("/m"), 1)).code, "unsupported");
  assert.equal(thrown(() => v.unmount(p("/m"))).code, "unsupported");
  assert.deepEqual(v.takeDirty(), []);
});

test("oversized filesystem paths are rejected before the kernel call", async () => {
  const { hooks, state } = countKernelCalls();
  const b = bash({ executionLimits: { maxStringLength: 64 } }, hooks);
  const before = state.calls;
  const path = "/" + "é".repeat(32);
  assert.throws(() => b.fs.writeFileSync(path, "x"), { code: "EFBIG" });
  assert.throws(() => b.fs.mkdirSync(path), { code: "EFBIG" });
  await assert.rejects(b.fs.readFile(path), { code: "EFBIG" });
  await assert.rejects(b.fs.rm(path), { code: "EFBIG" });
  assert.equal(state.calls, before);
  const standalone = new InMemoryFs(undefined, { hooks: internal(), profile: "micro", maxStringLength: 8 << 20 });
  await assert.rejects(standalone.stat("/" + "x".repeat(4 << 20)), { code: "EFBIG" });
});

test("filesystem return copies near the Wasm ceiling report no-space without trapping", () => {
  const { engine } = new Instance(module, internal());
  engine.configure(8n * 65_536n);
  const vfs = engine.Vfs.create(400_000n);
  const path = enc("/data");
  const bytes = new Uint8Array(150_000).fill(97);
  assert.equal(vfs.op({ tag: "write-file", val: { path, data: bytes, offset: 0n, truncate: true } }).tag, "ok");
  const full = vfs.op({ tag: "read-file", val: { path, offset: 0n, len: 120_000 } });
  assert.deepEqual(full, { tag: "err", val: { code: "no-space", path } });
  const small = vfs.op({ tag: "read-file", val: { path, offset: 0n, len: 64 } });
  assert.deepEqual(small, { tag: "ok", val: { tag: "data", val: bytes.subarray(0, 64) } });
});

test("the ABI reserve keeps pending stdout drainable near the Wasm ceiling", () => {
  const { engine } = new Instance(module, internal());
  engine.configure(10n << 20n);
  const vfs = engine.Vfs.create(5n << 20n);
  const fill = new Uint8Array((5 << 20) - 170_000);
  const val = { path: enc("/fill"), data: fill, offset: 0n, truncate: true };
  assert.equal(vfs.op({ tag: "write-file", val }).tag, "ok");
  const c = config();
  c.limits = {
    profile: "micro",
    overrides: ["maxFileSystemBytes", "maxLiveBytes", "maxOutputSize"].map((name) => ({ name, value: 256_000n })),
  };
  const shell = engine.Shell.create(vfs, c);
  const fiber = engine.Fiber.spawn(shell, undefined, opts("__out 200000"));
  assert.equal(fiber.run(5n).val.exitCode, 0);
  const chunks = [];
  for (;;) {
    const bytes = fiber.takeStdout(1 << 20);
    if (bytes.length === 0) break;
    chunks.push(bytes);
  }
  assert.equal(Buffer.concat(chunks).toString(), "a".repeat(200_000));
});

test("a limit result keeps its name when the ordinary Wasm allocation space is exhausted", () => {
  const ceiling = 10 << 20;
  function fillBody(size) {
    const { engine } = new Instance(module, internal());
    engine.configure(BigInt(ceiling));
    const vfs = engine.Vfs.create(BigInt(ceiling));
    const c = config(["foo"]);
    c.limits = {
      profile: "micro",
      overrides: ["maxFileSystemBytes", "maxLiveBytes", "maxOutputSize"].map((name) => ({ name, value: 500_000n })),
    };
    const shell = engine.Shell.create(vfs, c);
    const fiber = engine.Fiber.spawn(shell, undefined, opts("foo"));
    const { epoch } = fiber.run(5n).val;
    // Pre-fill memory without staging the file contents across the ABI.
    const val = { path: enc("/fill"), data: new Uint8Array(0), offset: BigInt(ceiling - 350_000), truncate: true };
    assert.equal(vfs.op({ tag: "write-file", val }).tag, "ok");
    const before = engine.usage().used;
    fiber.pushCallData(epoch, "body", new Uint8Array(size));
    return { fiber, epoch, fits: engine.usage().used - before === BigInt(size) };
  }

  // Find the largest body that fits with its ABI input copy, accounting for allocator layout.
  let lo = 0;
  let hi = 175_000;
  while (hi - lo > 1) {
    const size = Math.floor((lo + hi) / 2);
    if (fillBody(size).fits) lo = size;
    else hi = size;
  }
  const { fiber, epoch, fits } = fillBody(lo - 4);
  assert.ok(fits);
  // The one-byte input fits, but the body cannot grow. Diagnostics run before the body is released.
  fiber.pushCallData(epoch, "body", enc("x"));
  const result = fiber.resume(epoch, { tag: "command-exit", val: 0 });
  assert.equal(result.tag, "done");
  assert.equal(result.val.status, "limit-exceeded");
  assert.equal(result.val.exitCode, 126);
  assert.equal(fiber.takeStderr(100).length, 0);
  assert.equal(result.val.limitName, "memory-ceiling");
});

test("gate 18: InMemoryFs methods, and ENOSYS for the ones not backed yet", async () => {
  const fs = new InMemoryFs({ "/a/b.txt": "hi", "/bin": new Uint8Array([1, 2, 3]), "/m": { content: "x", mode: 0o700 } }, { hooks: internal(), profile: "contain", maxFileSystemBytes: 1 << 20 });
  assert.equal(await fs.readFile("/a/b.txt"), "hi");
  assert.deepEqual([...(await fs.readFileBuffer("/bin"))], [1, 2, 3]);
  assert.equal((await fs.stat("/m")).mode, 0o700);
  assert.equal(await fs.exists("/nope"), false);
  await fs.mkdir("/x/y", { recursive: true });
  await fs.writeFile("/x/y/z", "1");
  await fs.appendFile("/x/y/z", "2");
  assert.equal(await fs.readFile("/x/y/z"), "12");
  assert.deepEqual(await fs.readdir("/x/y"), ["z"]);
  assert.deepEqual((await fs.readdirWithFileTypes("/x")).map((e) => [e.name, e.isDirectory]), [["y", true]]);
  await assert.rejects(fs.mkdir("/x"), { code: "EEXIST" });
  await assert.rejects(fs.readFile("/nope"), { code: "ENOENT" });
  await assert.rejects(fs.readFile("/x"), { code: "EISDIR" });
  await assert.rejects(fs.rm("/x"), { code: "ENOTEMPTY" });
  await fs.rm("/x", { recursive: true });
  await fs.rm("/x", { force: true });
  assert.ok(fs.getAllPaths().includes("/a/b.txt"));
  assert.equal(fs.resolvePath("/a/b", "../c/./d"), "/a/c/d");
  for (const [m, args] of [["cp", ["/a", "/b"]], ["mv", ["/a", "/b"]], ["chmod", ["/a", 1]], ["symlink", ["/a", "/l"]], ["link", ["/a", "/l"]], ["readlink", ["/a"]], ["realpath", ["/a"]], ["utimes", ["/a", new Date(), new Date()]], ["createExclusive", ["/a", { mode: 1 }]]]) {
    await assert.rejects(fs[m](...args), (e) => e.message.startsWith("ENOSYS"), m);
  }
  fs.writeFileLazy("/lazy", async () => "later");
  await assert.rejects(fs.readFile("/lazy"), { name: "NotSupportedError" });
  assert.equal(await fs.exists("/lazy"), true);
});

test("gate 16: the release kernel runs none of the test-only commands", async () => {
  const r = await new Bash().exec("__out 3");
  assert.equal(r.exitCode, 127);
  assert.equal(r.stderr, "bash: __out: command not found\n");
});

test("gate 16: the release kernel holds none of the test-only command names", () => {
  const names = ["__out", "__err", "__stdin", "__alloc", "__spin", "__exit", "__setenv", "__write", "__cat", "__hostfs", "__lazy", "__fetch", "__sleep", "__clock", "__entropy", "__trap"];
  const release = [readFileSync(join(repoRoot, "dist", "kernel.core.wasm")), readFileSync(join(repoRoot, "dist", "kernel.component.wasm"))];
  const test = readFileSync(join(repoRoot, "dist", "test", "kernel-test.core.wasm"));
  for (const n of names) {
    assert.ok(test.includes(n), `${n} is in the test kernel`);
    for (const r of release) assert.ok(!r.includes(n), `${n} must not be in the release kernel`);
  }
});

test("shell.create refuses unknown limits and byte caps that exceed the ceiling", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(1000n);
  const c = config();
  c.limits = { profile: "contain", overrides: [{ name: "nope", value: 1n }] };
  assert.equal(thrown(() => engine.Shell.create(vfs, c)).code, "invalid-config");
  c.limits = { profile: "contain", overrides: [{ name: "maxLiveBytes", value: 1n << 40n }] };
  assert.equal(thrown(() => engine.Shell.create(vfs, c)).code, "invalid-config");
  c.limits = { profile: "normal", overrides: [] };
  assert.equal(thrown(() => engine.Shell.create(vfs, c)).code, "invalid-config", "normal needs a larger ceiling than the instance has");
});
