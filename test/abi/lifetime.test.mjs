import { test } from "node:test";
import assert from "node:assert/strict";
import { resourceKinds } from "../../host/core/abi/layout.js";
import { adapterPair, binary, bytes, coreName, instrumentCore, opts, postName, snapshotResult, testConfig } from "./support.mjs";
import { resourceModule, reps, shellTupleBytes } from "./resources.mjs";

const methods = {
  vfs: ["usedBytes", [], (1n << 64n) - 1n],
  shell: ["env", [], []],
  fiber: ["run", [1n], { tag: "out-of-fuel" }],
};

function fixture(module) {
  const notifications = [];
  const tokens = new Map();
  let finalize;
  class Registry {
    constructor(fn) { finalize = fn; }
    register(wrapper, token, key) {
      assert.equal(token, key);
      tokens.set(wrapper, token);
    }
    unregister(token) {
      for (const [wrapper, current] of tokens) {
        if (current === token) return tokens.delete(wrapper);
      }
      return false;
    }
  }
  const instance = instrumentCore(module, { Registry, onTrap: (error) => notifications.push(error) });
  return { ...instance, tokens, finalize, notifications };
}

function createOwners(engine) {
  const vfs = engine.Vfs.create(1n);
  const shell = engine.Shell.create(vfs, testConfig());
  const fiber = engine.Fiber.spawn(shell, undefined, opts(""));
  return { vfs, shell, fiber };
}

const dtorCalls = ({ events }, kind) => events.filter(({ name }) =>
  kind ? name === coreName("[dtor]" + kind) : name.startsWith(coreName("[dtor]")));

for (const kind of resourceKinds) {
  const module = resourceModule(kind);
  const [method, args, value] = methods[kind];
  test(kind + " ownership is branded per store and cleanup retires one representation", () => {
    const f = fixture(module);
    const other = fixture(module);
    const owners = createOwners(f.engine);
    const resource = owners[kind];
    const foreign = createOwners(other.engine)[kind];
    const wrong = owners[resourceKinds.find((otherKind) => otherKind !== kind)];
    const call = resource[method];
    const dispose = resource[Symbol.dispose];
    for (const invalid of [wrong, foreign, Object.create(Object.getPrototypeOf(resource)), 1]) {
      assert.throws(() => call.call(invalid, ...args), TypeError);
      assert.throws(() => dispose.call(invalid), TypeError);
    }
    assert.deepEqual(call.call(resource, ...args), value);
    assert.equal(dtorCalls(f).length, 0, "a borrow does not drop its owner or other resources");

    const token = f.tokens.get(resource);
    assert.ok(token);
    resource[Symbol.dispose]();
    resource[Symbol.dispose]();
    f.finalize(token);
    assert.throws(() => call.call(resource, ...args), TypeError);
    assert.deepEqual(dtorCalls(f, kind).map(({ args }) => args), [[reps[kind]]]);

    // A guest-owned entry may reuse the representation after the old owner has been retired.
    f.engine.configure(1n);
    f.finalize(token);
    assert.deepEqual(dtorCalls(f, kind).map(({ args }) => args), [[reps[kind]], [reps[kind]]]);

    const replacement = kind === "vfs" ? f.engine.Vfs.create(1n) :
      kind === "shell" ? f.engine.Shell.create(owners.vfs, testConfig()) :
        f.engine.Fiber.spawn(owners.shell, undefined, opts(""));
    const current = f.tokens.get(replacement);
    assert.ok(current);
    f.finalize(token);
    assert.deepEqual(call.call(replacement, ...args), value);
    f.finalize(current);
    f.finalize(current);
    replacement[Symbol.dispose]();
    assert.throws(() => call.call(replacement, ...args), TypeError);
    assert.deepEqual(dtorCalls(f, kind).map(({ args }) => args), [[reps[kind]], [reps[kind]], [reps[kind]]]);
    assert.deepEqual(f.notifications, []);
    f._util.stop();
    other._util.stop();
    assert.equal(f.tokens.size, 0);
    assert.equal(other.tokens.size, 0);
  });

  for (const scenario of ["repeated drop", "wrong-kind drop", "drop during borrow"]) {
    test(kind + " traps on " + scenario + " and keeps the destructor count unchanged", (t) => {
      const f = fixture(module);
      let invalidDrop;
      if (scenario === "repeated drop") {
        f.engine.configure(1n);
        assert.deepEqual(dtorCalls(f).map(({ args }) => args), [[reps[kind]]]);
        invalidDrop = () => f.engine.configure(2n);
      } else if (scenario === "wrong-kind drop") {
        invalidDrop = () => f.engine.configure(3n);
      } else {
        const owner = createOwners(f.engine)[kind];
        assert.deepEqual(owner[method](...args), value);
        f.engine.configure(4n);
        invalidDrop = () => owner[method](...args);
      }
      const count = dtorCalls(f).length;
      const tokens = [...f.tokens.values()];
      assert.throws(invalidDrop, WebAssembly.RuntimeError);
      assert.equal(f.notifications.length, 1);
      assert.equal(f.tokens.size, 0);
      for (const token of tokens) f.finalize(token);
      assert.throws(invalidDrop, { name: "KernelPoisonedError" });
      f._util.stop();
      assert.equal(dtorCalls(f).length, count);
      t.diagnostic("poison notifications 1; destructors " + count + " before and after invalid drop");
    });
  }
}

test("Shell.create hands one indirect tuple to the guest and posts once on success and typed error", (t) => {
  const f = fixture(resourceModule("shell"));
  const vfs = f.engine.Vfs.create(1n);
  for (const trace of [false, true]) {
    const name = "[static]shell.create";
    f.events.length = 0;
    const before = [f.raw.outerAllocs.value, f.raw.outerFrees.value];
    const result = snapshotResult(() => f.engine.Shell.create(vfs, testConfig({ trace })));
    assert.deepEqual(f.events.map(({ name }) => name), ["cabi_realloc", coreName(name), postName(name)]);
    const [allocated, entered, posted] = f.events;
    assert.deepEqual(allocated.args.slice(0, 2), [0, 0]);
    assert.equal(allocated.args[3], shellTupleBytes);
    assert.deepEqual(entered.args, [allocated.result]);
    assert.deepEqual(posted.args, [entered.result]);
    assert.equal(f.raw.outerPointer.value, allocated.result);
    assert.deepEqual([f.raw.outerAllocs.value, f.raw.outerFrees.value], before.map((count) => count + 1));
    assert.deepEqual(f.notifications, []);
    if (trace) assert.deepEqual(result.error, { code: "invalid-config", detail: "no" });
    else result.value[Symbol.dispose]();
  }
  t.diagnostic("indirect tuple " + shellTupleBytes + " bytes; 2 allocations, 2 guest frees and 2 post-returns across ok and err");
  vfs[Symbol.dispose]();
  f._util.stop();
});

for (const [stage, expected] of [
  ["guest", [coreName("configure")]],
  ["post-return", [coreName("configure"), postName("configure")]],
  ["realloc", ["cabi_realloc"]],
  ["destructor", [coreName("[dtor]vfs")]],
  ["finalizer", [coreName("[dtor]vfs")]],
]) {
  test(stage + " trap stops entry, post-return and any further cleanup", () => {
    const f = fixture(resourceModule("vfs"));
    const { engine } = f;
    const owners = createOwners(engine);
    const tokens = [...f.tokens.values()];
    if (stage === "realloc") engine.configure(8n);
    if (stage === "destructor" || stage === "finalizer") engine.configure(6n);
    f.events.length = 0;
    if (stage === "guest") assert.throws(() => engine.configure(5n), WebAssembly.RuntimeError);
    if (stage === "post-return") assert.throws(() => engine.configure(7n), WebAssembly.RuntimeError);
    if (stage === "realloc") assert.throws(() => owners.shell.registerCommand("x"), WebAssembly.RuntimeError);
    if (stage === "destructor") assert.throws(() => owners.vfs[Symbol.dispose](), WebAssembly.RuntimeError);
    if (stage === "finalizer") f.finalize(f.tokens.get(owners.vfs));
    assert.equal(f.notifications.length, 1);
    assert.equal(f.tokens.size, 0);
    for (const token of tokens) f.finalize(token);
    for (const owner of Object.values(owners)) owner[Symbol.dispose]();
    assert.throws(() => engine.configure(0n), { name: "KernelPoisonedError" });
    f._util.stop();
    f._util.stop();
    assert.deepEqual(f.events.map(({ name }) => name), expected);
  });
}

test("owned Wasm results retain exact bytes across real memory growth and disposal", async (t) => {
  const { module } = await adapterPair("kernel-test");
  const f = instrumentCore(module);
  const { engine, raw } = f;
  engine.configure(64n << 20n);
  const vfs = engine.Vfs.create(8n << 20n);
  const sh = engine.Shell.create(vfs, testConfig({ coverage: true }));
  const fiber = engine.Fiber.spawn(sh, undefined, opts("echo hi"));
  const path = bytes("/file");
  const write = (path, data, offset = 0n) =>
    vfs.op({ tag: "write-file", val: { path, data, offset, truncate: offset === 0n, mtimeMs: 1n } });
  assert.equal(write(path, binary).tag, "ok");
  const retained = vfs.op({ tag: "read-file", val: { path, offset: 0n, len: 4 } }).val.val;
  assert.equal(fiber.run(100n).tag, "done");
  const stdout = fiber.takeStdout(100);
  const features = engine.coverageFeatures();
  const coverage = fiber.takeCoverage(100);
  assert.ok(coverage.length > 0);
  const indices = Array.from(coverage);
  assert.ok(indices.some((id) => features[id] === "bash:builtin:echo"));
  const before = raw.memory.buffer;
  const beforeBytes = before.byteLength;
  const payload = new Uint8Array(1 << 20).fill(233);
  for (let i = 0; i < 3; i++) assert.equal(write(bytes("/growth"), payload, BigInt(i * payload.length)).tag, "ok");
  assert.notEqual(raw.memory.buffer, before);
  assert.ok(raw.memory.buffer.byteLength > beforeBytes);
  assert.equal(vfs.op({ tag: "exists", val: path }).val.val, true);
  for (const owner of [fiber, sh, vfs]) owner[Symbol.dispose]();
  f._util.stop();
  assert.deepEqual(retained, binary);
  assert.deepEqual(stdout, bytes("hi\n"));
  assert.deepEqual(coverage, Uint16Array.from(indices));
  assert.equal(dtorCalls(f).length, 3);
  t.diagnostic("memory buffer changed from " + beforeBytes + " to " + raw.memory.buffer.byteLength +
    " bytes; retained byte and u16 buffers survived all 3 destructors");
});

