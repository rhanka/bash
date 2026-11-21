import { test } from "node:test";
import assert from "node:assert/strict";
import { linkEngine } from "../../host/core/abi/binding.js";
import { functions, layout } from "../../host/core/abi/layout.js";
import { adapterPair, binary, bytes, coreName, instrumentCore, opts, postName, testConfig } from "./support.mjs";

test("invalid lowering leaves allocation and call counts at zero and the store usable", async () => {
  const { module } = await adapterPair("kernel");
  const notifications = [];
  const { engine, _util, events } = instrumentCore(module, { onTrap: (error) => notifications.push(error) });
  const other = instrumentCore(module);
  for (const e of [engine, other.engine]) e.configure(64n << 20n);
  const vfs = engine.Vfs.create(1n << 20n);
  const sh = engine.Shell.create(vfs, testConfig());
  const f = engine.Fiber.spawn(sh, undefined, opts("echo ok"));
  const foreignVfs = other.engine.Vfs.create(1n << 20n);
  const foreignShell = other.engine.Shell.create(foreignVfs, testConfig());
  const foreignFiber = other.engine.Fiber.spawn(foreignShell, undefined, opts(""));
  const disposed = engine.Fiber.spawn(sh, undefined, opts(""));
  disposed[Symbol.dispose]();

  const detached = new Uint8Array(2);
  structuredClone(detached.buffer, { transfer: [detached.buffer] });
  const bad = [
    [RangeError, "signed u64", () => engine.raiseCeiling(-1n)],
    [RangeError, "wide u64", () => engine.raiseCeiling(1n << 64n)],
    [RangeError, "fractional u32", () => f.takeStdout(1.5)],
    [RangeError, "wide u32", () => f.takeStdout(2 ** 32)],
    [RangeError, "wide s32", () => f.resume(1n, { tag: "command-exit", val: 2 ** 31 })],
    [RangeError, "wide s64", () => vfs.op({ tag: "utimes", val: { path: bytes("/"), atimeMs: -(1n << 63n) - 1n, mtimeMs: 0n } })],
    [RangeError, "wide u16", () => f.resume(1n, { tag: "fetched", val: {
      status: 65536, statusText: "", url: "", headers: [], redirectChain: [],
    } })],
    [RangeError, "stream chunk", () => f.writeStdin(new Uint8Array((1 << 20) + 1), true)],
    [RangeError, "script bytes", () => engine.Fiber.spawn(sh, undefined, opts("", { script: new Uint8Array((3 << 20) + 1) }))],
    [RangeError, "oneshot aggregate", () => engine.execOneshot({
      files: [{ path: bytes("/f"), data: new Uint8Array(4 << 20), mode: 0o644 }],
      config: testConfig(), opts: opts(""), stdin: new Uint8Array(), fuel: 1n,
    })],
    [TypeError, "Number for u64", () => engine.raiseCeiling(1)],
    [TypeError, "BigInt for u32", () => f.takeStdout(1n)],
    [TypeError, "object for string", () => sh.registerCommand({ toString: () => "x" })],
    [TypeError, "truthy bool", () => f.writeStdin(binary, 1)],
    [TypeError, "unknown enum", () => f.cancel("unavailable")],
    [TypeError, "unknown tag", () => vfs.op({ tag: "no-such-op", val: bytes("/") })],
    [TypeError, "unit payload", () => vfs.op({ tag: "all-paths", val: 0 })],
    [TypeError, "missing payload", () => vfs.op({ tag: "exists" })],
    [TypeError, "list as object", () => f.setCallEnv(1n, { length: 0 })],
    [TypeError, "hole in list", () => f.setCallEnv(1n, new Array(1))],
    [TypeError, "byte array brand", () => f.writeStdin({ [Symbol.toStringTag]: "Uint8Array", length: 0 }, true)],
    [TypeError, "detached byte array", () => f.writeStdin(detached, true)],
    [TypeError, "invalid nested field after valid payloads", () => engine.Shell.create(vfs, testConfig({
      customCommands: ["valid"], env: [{ name: binary, value: binary, exported: false }, { name: binary, value: binary }],
    }))],
    [TypeError, "unknown flag name", () => engine.Shell.create(vfs, testConfig({ grants: { 7: true } }))],
    [TypeError, "nonboolean grant", () => engine.Shell.create(vfs, testConfig({ grants: { customCommands: 1 } }))],
    [TypeError, "wrong-kind receiver", () => engine.Vfs.prototype.op.call(sh, { tag: "all-paths" })],
    [TypeError, "numeric receiver", () => engine.Vfs.prototype.usedBytes.call(1)],
    [TypeError, "forged receiver", () => engine.Shell.prototype.env.call(Object.create(engine.Shell.prototype))],
    [TypeError, "foreign source", () => vfs.mountVfs(bytes("/"), foreignVfs)],
    [TypeError, "foreign owner", () => engine.Fiber.spawn(foreignShell, undefined, opts(""))],
    [TypeError, "foreign optional parent", () => engine.Fiber.spawn(sh, foreignFiber, opts(""))],
    [TypeError, "wrong-kind optional parent", () => engine.Fiber.spawn(sh, vfs, opts(""))],
    [TypeError, "disposed optional parent", () => engine.Fiber.spawn(sh, disposed, opts(""))],
    [TypeError, "dispose during validation", () => engine.Shell.create(vfs, {
      ...testConfig(), get grants() { vfs[Symbol.dispose](); },
    })],
    [TypeError, "reenter during validation", () => engine.Shell.create(vfs, {
      ...testConfig(), get grants() { engine.usage(); },
    })],
    [TypeError, "stop during validation", () => engine.Shell.create(vfs, {
      ...testConfig(), get grants() { _util.stop(); },
    })],
  ];
  for (const [error, category, call] of bad) {
    events.length = 0;
    assert.throws(call, error, category);
    assert.deepEqual(events, [], category);
    assert.equal(engine.usage().ceiling, 64n << 20n, category);
  }
  assert.deepEqual(notifications, []);
  f.writeStdin(new Uint8Array(0), true);
  assert.equal(f.run(100n).tag, "done");
  assert.deepEqual(f.takeStdout(100), bytes("ok\n"));
  for (const owner of [f, sh, vfs, foreignFiber, foreignShell, foreignVfs]) owner[Symbol.dispose]();
  other._util.stop();
  _util.stop();
});

test("staging reads getters once and copies payloads before later getters can change them", async () => {
  const { module } = await adapterPair("kernel");
  const { engine, _util } = instrumentCore(module);
  engine.configure(64n << 20n);
  const vfs = engine.Vfs.create(1n << 20n);
  const data = Uint8Array.from(binary);
  let reads = 0;
  const path = bytes("/file");
  const request = {
    tag: "write-file",
    val: {
      path,
      get data() { reads++; return data; },
      offset: 0n,
      get truncate() {
        structuredClone(data.buffer, { transfer: [data.buffer] });
        path.fill(0);
        return true;
      },
    },
  };
  assert.equal(vfs.op(request).tag, "ok");
  assert.equal(reads, 1);
  assert.deepEqual(vfs.op({ tag: "read-file", val: { path: bytes("/file"), offset: 0n, len: 4 } }).val.val, binary);

  let grants = 0;
  const shell = engine.Shell.create(vfs, testConfig({
    grants: { get customCommands() { assert.equal(grants++, 0); return true; } },
  }));
  shell.registerCommand("stub");
  const fiber = engine.Fiber.spawn(shell, undefined, opts("stub"));
  assert.equal(fiber.run(100n).tag, "host-call");
  for (const owner of [fiber, shell, vfs]) owner[Symbol.dispose]();
  _util.stop();
});

const configure = functions.find(({ name }) => name === "configure");
const span = 64;
const malformed = [
  ["bool", "bool", false, () => 2, /bool discriminant/],
  ["enum", "profile", false, () => 4, /enum discriminant/],
  ["variant", "step", true, (m) => (m.setUint8(span, 3), span), /variant discriminant/],
  ["option", ["option", "bytes"], true, (m) => (m.setUint8(span, 2), span), /option discriminant/],
  ["result", ["result", null, "kernel-error"], true, (m) => (m.setUint8(span, 2), span), /result discriminant/],
  ["flags", "grants", false, () => 64, /flag bits/],
  ["bad UTF-8", "string", true, (m) => {
    m.setUint32(span, 8, true);
    m.setUint32(span + 4, 1, true);
    m.setUint8(8, 0xc0);
    return span;
  }, /utf-8/i],
  ["misaligned empty list", ["list", "u16"], true, (m) => (m.setUint32(span, 3, true), span), /misaligned/],
  ["misaligned record", "memory-usage", true, () => span + 4, /misaligned/],
  ["out-of-bounds length", ["list", "u16"], true, (m) => {
    m.setUint32(span, 8, true);
    m.setUint32(span + 4, 0xffffffff, true);
    return span;
  }, /out-of-bounds/],
  ["empty list beyond memory", "bytes", true, (m) => (m.setUint32(span, m.byteLength + 1, true), span), /out-of-bounds/],
  ["invalid owned handle", ["own", "vfs"], false, () => 77, /invalid owned vfs handle/],
];

for (const [category, ref, indirectResult, malformedReturn, detail] of malformed) {
  test("a malformed guest return poisons once without calling post-return for " + category, () => {
    const memory = new WebAssembly.Memory({ initial: 1 });
    const events = [];
    const notifications = [];
    const { engine, _util } = linkEngine(() => ({
      memory,
      [coreName("configure")]() {
        events.push("enter");
        return malformedReturn(new DataView(memory.buffer));
      },
      [postName("configure")]() { events.push("post"); },
    }), { onTrap: (error) => notifications.push(error) }, [
      { ...configure, result: layout(ref), indirectResult, unwrap: false },
    ]);
    assert.throws(() => engine.configure(1n), { name: "RuntimeError", message: detail });
    assert.throws(() => engine.configure(1n), { name: "KernelPoisonedError" });
    assert.deepEqual(events, ["enter"]);
    assert.equal(notifications.length, 1);
    _util.stop();
    _util.stop();
  });
}

test("oneshot lifting uses the portion of the profile's transfer budget left after lowering", () => {
  const mib = 1 << 20;
  const memory = new WebAssembly.Memory({ initial: 65 });
  const fn = functions.find(({ name }) => name === "exec-oneshot");
  const result = fn.result;
  const stdout = span + result.payloadOffset + result.cases[0].type.fields.find(({ key }) => key === "stdout").offset;
  const data = new DataView(memory.buffer);
  data.setUint32(stdout, 4096, true);
  data.setUint32(stdout + 4, 4 * mib - fn.params.size, true);
  const notifications = [];
  const actions = [];
  let next = 256;
  const { engine } = linkEngine(() => ({
    memory,
    cabi_realloc(_ptr, _old, align, size) {
      const result = Math.ceil(next / align) * align;
      next = result + size;
      return result;
    },
    [fn.exportName]() { actions.push("enter"); return span; },
    [fn.postName]() { actions.push("post"); },
  }), { onTrap: (error) => notifications.push(error) });
  const request = { files: [], config: testConfig(), opts: opts(""), stdin: new Uint8Array(), fuel: 1n };
  assert.equal(engine.execOneshot(request).stdout.length, 4 * mib - fn.params.size);
  assert.throws(() => engine.execOneshot({ ...request, stdin: Uint8Array.of(1) }), /result exceeds the transfer budget/);
  assert.deepEqual(actions, ["enter", "post", "enter"]);
  assert.equal(notifications.length, 1);
});

