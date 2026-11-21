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
  engine.Shell.create(engine.Vfs.create(16_384n), config());
  const other = engine.Vfs.create(2000n);
  assert.equal(thrown(() => v.mountVfs(p("/m"), other)).code, "unsupported");
  v.mountHost(p("/m"), 1);
  assert.equal(err(v.op({ tag: "stat", val: p("/m") })), "unsupported");
  v.unmount(p("/m"));
  assert.equal(thrown(() => v.unmount(p("/m"))).code, "not-found");
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
  // Transfer each chunk below the 64 KiB headroom cap. Repeating the call also checks that
  // the generated Vec drops return their charges.
  for (let offset = 0; offset < bytes.length; offset += 50_000) {
    const data = bytes.subarray(offset, offset + 50_000);
    assert.equal(vfs.op({ tag: "write-file", val: { path, data, offset: BigInt(offset), truncate: false } }).tag, "ok");
  }
  const fill = { path: enc("/fill"), data: new Uint8Array(0), offset: 100_000n, truncate: true };
  assert.equal(vfs.op({ tag: "write-file", val: fill }).tag, "ok");
  const full = vfs.op({ tag: "read-file", val: { path, offset: 0n, len: 120_000 } });
  assert.deepEqual(full, { tag: "err", val: { code: "no-space", path } });
  const small = vfs.op({ tag: "read-file", val: { path, offset: 0n, len: 64 } });
  assert.deepEqual(small, { tag: "ok", val: { tag: "data", val: bytes.subarray(0, 64) } });
});

test("post-return releases the transfer charge for an input echoed in an error", () => {
  const { engine } = new Instance(module, internal());
  engine.configure(8n * 65_536n);
  const vfs = engine.Vfs.create(400_000n);
  const path = new Uint8Array(40_000).fill(97);
  const before = engine.usage().used;
  for (let i = 0; i < 3; i++) {
    const error = thrown(() => vfs.mountHost(path, 1));
    assert.equal(error.code, "invalid");
    assert.deepEqual(error.path, path);
  }
  assert.equal(engine.usage().used, before);
});

test("the ceiling cannot be configured below the heap's existing footprint", () => {
  const { engine } = new Instance(module, internal());
  // Lowering the script grows the ordinary arena before the call reaches the configuration check.
  const initial = thrown(() => engine.execOneshot({ files: [], config: config(), opts: opts(" ".repeat(90_000)), stdin: enc(""), fuel: 1n }));
  assert.deepEqual(initial, { code: "not-configured", detail: "configure was not called" });
  assert.equal(engine.usage().used, 0n);
  assert.equal(thrown(() => engine.configure(2n * 65_536n)).code, "invalid-config");
  engine.configure(3n * 65_536n);
  assert.equal(engine.usage().ceiling, 3n * 65_536n);
});

test("a cold engine rejects a ceiling without room for an ordinary Wasm page", () => {
  const { engine } = new Instance(module, internal());
  assert.equal(thrown(() => engine.configure(65_536n)).code, "invalid-config");
  engine.configure(2n * 65_536n);
  const vfs = engine.Vfs.create(4096n);
  assert.deepEqual(vfs.op({ tag: "exists", val: enc("/") }), { tag: "ok", val: { tag: "flag", val: true } });
});

test("the ABI reserve keeps pending stdout drainable near the Wasm ceiling", () => {
  const { engine } = new Instance(module, internal());
  const ceiling = 10n << 20n;
  engine.configure(ceiling);
  const vfs = engine.Vfs.create(ceiling);
  const c = config();
  c.limits = {
    profile: "micro",
    overrides: ["maxFileSystemBytes", "maxLiveBytes", "maxOutputSize"].map((name) => ({ name, value: 256_000n })),
  };
  const shell = engine.Shell.create(vfs, c);
  const fiber = engine.Fiber.spawn(shell, undefined, opts("__out 200000"));
  const fill = { path: enc("/fill"), data: new Uint8Array(0), offset: ceiling - 330_000n, truncate: true };
  assert.equal(vfs.op({ tag: "write-file", val: fill }).tag, "ok");
  assert.equal(fiber.run(5n).val.exitCode, 0);
  const first = fiber.takeStdout(1 << 20);
  assert.equal(first.length, 16 << 10, "the first drain needs the bounded reserve copy");
  const chunks = [first];
  for (;;) {
    const bytes = fiber.takeStdout(1 << 20);
    if (bytes.length === 0) break;
    chunks.push(bytes);
  }
  assert.equal(Buffer.concat(chunks).toString(), "a".repeat(200_000));
});

test("a limit result includes its name and diagnostic after ordinary heap growth is refused", () => {
  const ceiling = 10n << 20n;
  const { engine } = new Instance(module, internal());
  engine.configure(ceiling);
  const vfs = engine.Vfs.create(ceiling);
  const c = config(["foo"]);
  c.limits = {
    profile: "micro",
    overrides: ["maxFileSystemBytes", "maxLiveBytes", "maxOutputSize"].map((name) => ({ name, value: 500_000n })),
  };
  const shell = engine.Shell.create(vfs, c);
  const fiber = engine.Fiber.spawn(shell, undefined, opts("foo"));
  const { epoch } = fiber.run(5n).val;
  const fill = (path, offset) => vfs.op({ tag: "write-file", val: { path: enc(path), data: new Uint8Array(0), offset, truncate: true } });
  // Pre-fill the heap in the kernel so the transfer account only sees the paths and body chunks.
  assert.equal(fill("/fill", ceiling - 460_000n).tag, "ok");
  const before = engine.usage().used;
  fiber.pushCallData(epoch, "body", new Uint8Array(2 * 65_536 + 32_768));
  assert.equal(engine.usage().used - before, 163_840n + 3n * 4n);
  assert.equal(fill("/fill2", 130_000n).tag, "ok");
  // The one-byte input fits, but the 32 KiB tail cannot grow to its next size class.
  fiber.pushCallData(epoch, "body", enc("x"));
  const result = fiber.resume(epoch, { tag: "command-exit", val: 0 });
  assert.equal(result.tag, "done");
  assert.equal(result.val.status, "limit-exceeded");
  assert.equal(result.val.exitCode, 126);
  assert.equal(dec(fiber.takeStderr(100)), "bash: memory-ceiling exceeded\n");
  assert.equal(result.val.limitName, "memory-ceiling");
});

test("cancel preserves large stderr and its diagnostic near the Wasm ceiling", () => {
  const ceiling = 10n << 20n;
  const { engine } = new Instance(module, internal());
  engine.configure(ceiling);
  const vfs = engine.Vfs.create(ceiling);
  const c = config(["foo"]);
  c.limits = {
    profile: "micro",
    overrides: ["maxFileSystemBytes", "maxLiveBytes", "maxOutputSize"].map((name) => ({ name, value: 500_000n })),
  };
  const shell = engine.Shell.create(vfs, c);
  const fiber = engine.Fiber.spawn(shell, undefined, opts("__err 50000\n__out 200000\nfoo"));
  assert.equal(fiber.run(5n).tag, "host-call");
  // The ordinary arena can no longer grow stderr, which is also too large for amortized
  // growth into the reserve page.
  const fill = (path, offset) => vfs.op({ tag: "write-file", val: { path: enc(path), data: new Uint8Array(0), offset, truncate: true } });
  assert.equal(fill("/fill", ceiling - 380_000n).tag, "ok");
  assert.equal(fiber.cancel("aborted").status, "aborted");
  const chunks = [];
  for (;;) {
    const bytes = fiber.takeStderr(1 << 20);
    if (bytes.length === 0) break;
    chunks.push(bytes);
  }
  const output = Buffer.concat(chunks);
  const diagnostic = "bash: execution aborted\n";
  assert.equal(output.length, 50_000 + diagnostic.length);
  assert.equal(dec(chunks.at(-1)), diagnostic);
  assert.equal(output.toString(), "a".repeat(50_000) + diagnostic);
});

test("host-supplied limit names are bounded at a UTF-8 character boundary", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(16_384n);
  const c = config(["foo"]);
  c.grants.realSleep = true;
  const shell = engine.Shell.create(vfs, c);
  const prefix = "a".repeat(127);
  for (const script of ["foo", "__sleep 1"]) {
    const fiber = engine.Fiber.spawn(shell, undefined, opts(script));
    const call = fiber.run(5n);
    assert.equal(call.tag, "host-call");
    const result = fiber.resume(call.val.epoch, { tag: "limit-exceeded", val: prefix + "é" + "x".repeat(300) });
    assert.equal(result.tag, "done");
    assert.equal(result.val.status, "limit-exceeded");
    assert.equal(result.val.limitName, prefix);
    assert.equal(dec(fiber.takeStderr(1024)), `bash: ${prefix} exceeded\n`);
  }
});

test("shell cwd copies are bounded only when the full ordinary copy is refused", () => {
  const ceiling = 10n << 20n;
  const { engine } = new Instance(module, internal());
  engine.configure(ceiling);
  const vfs = engine.Vfs.create(ceiling);
  const c = config();
  c.limits = {
    profile: "micro",
    overrides: ["maxFileSystemBytes", "maxLiveBytes", "maxOutputSize"].map((name) => ({ name, value: 500_000n })),
  };
  c.cwd = enc("/" + "a".repeat(99_999));
  const shell = engine.Shell.create(vfs, c);
  assert.deepEqual(shell.cwd(), c.cwd);
  const fill = (path, offset) => vfs.op({ tag: "write-file", val: { path: enc(path), data: new Uint8Array(0), offset, truncate: true } });
  assert.equal(fill("/fill", ceiling - 445_000n).tag, "ok");
  assert.equal(fill("/fill2", 95_000n).tag, "ok");
  assert.deepEqual(shell.cwd(), c.cwd.subarray(0, 4096));
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
  const names = ["__out", "__err", "__stdin", "__alloc", "__spin", "__exit", "__setenv", "__write", "__cat", "__hostfs", "__lazy", "__fetch", "__sleep", "__clock", "__entropy", "__trap", "__fsalloc", "__work", "__walk", "__fsbatch"];
  const release = [readFileSync(join(repoRoot, "dist", "kernel.core.wasm")), readFileSync(join(repoRoot, "dist", "kernel.component.wasm"))];
  const test = readFileSync(join(repoRoot, "dist", "test", "kernel-test.core.wasm"));
  for (const n of names) {
    assert.ok(test.includes(n), `${n} is in the test kernel`);
    for (const r of release) assert.ok(!r.includes(n), `${n} must not be in the release kernel`);
  }
});

test("shell.create refuses unknown limits and byte caps that exceed the ceiling", () => {
  const { engine } = withEngine();
  const vfs = engine.Vfs.create(16_384n);
  const c = config();
  c.limits = { profile: "contain", overrides: [{ name: "nope", value: 1n }] };
  assert.equal(thrown(() => engine.Shell.create(vfs, c)).code, "invalid-config");
  c.limits = { profile: "contain", overrides: [{ name: "maxLiveBytes", value: 1n << 40n }] };
  assert.equal(thrown(() => engine.Shell.create(vfs, c)).code, "invalid-config");
  c.limits = { profile: "normal", overrides: [] };
  assert.equal(thrown(() => engine.Shell.create(vfs, c)).code, "invalid-config", "normal needs a larger ceiling than the instance has");
});
