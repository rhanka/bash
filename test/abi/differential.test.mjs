import { test } from "node:test";
import assert from "node:assert/strict";
import { functions, resourceKinds, namespace } from "../../host/core/abi/layout.js";
import { adapterPair, binary, bytes, compareValues, layoutCases, opts, snapshotResult, testConfig } from "./support.mjs";

const root = bytes("/");
const file = bytes("/dir/data");
const missing = bytes("/missing");
const pair = { source: file, target: missing, recursive: false };
const fsOps = [
  { tag: "mkdir", val: { path: bytes("/dir"), recursive: true, mode: 0o755 } },
  { tag: "write-file", val: { path: file, data: binary, offset: 0n, truncate: true, mode: 0o600, mtimeMs: -1n } },
  { tag: "stat", val: file },
  { tag: "lstat", val: missing },
  { tag: "exists", val: file },
  { tag: "read-link", val: file },
  { tag: "real-path", val: file },
  { tag: "read-dir", val: root },
  { tag: "read-dir-plus", val: root },
  { tag: "walk", val: { root, maxDepth: 2, maxEntries: 10 } },
  { tag: "all-paths" },
  { tag: "read-file", val: { path: file, offset: 0n, len: 4 } },
  { tag: "write-lazy", val: { path: missing, mode: 0o644 } },
  { tag: "create-exclusive", val: { path: missing, mode: 0o644, directory: false } },
  { tag: "copy", val: pair },
  { tag: "rename", val: pair },
  { tag: "symlink", val: pair },
  { tag: "hard-link", val: pair },
  { tag: "chmod", val: { path: file, mode: 0o400 } },
  { tag: "utimes", val: { path: file, atimeMs: -2n, mtimeMs: 3n } },
  { tag: "remove", val: { path: file, recursive: false, force: false } },
];

function transcript({ engine, _util }, testKernel) {
  const rows = [];
  const coverage = new Set();
  const owners = new WeakMap();
  let id = 0;
  function invoke(name, category, fn) {
    coverage.add(name);
    const result = snapshotResult(fn);
    const kind = resourceKinds.find((kind) => result.value instanceof engine[kind[0].toUpperCase() + kind.slice(1)]);
    if (kind) {
      const owner = { kind, id: ++id };
      owners.set(result.value, owner);
      rows.push([name, category, { value: owner }]);
    } else {
      rows.push([name, category, result]);
    }
    return result;
  }
  const value = (name, category, fn) => {
    const result = invoke(name, category, fn);
    assert.ok("value" in result, name + " " + category + " " + result.error?.detail);
    return result.value;
  };
  const err = (name, category, code, fn) => {
    const { error } = invoke(name, category, fn);
    assert.equal(error?.code, code, name + " " + category);
    return error;
  };
  const drop = (owner) => {
    const identity = owners.get(owner);
    assert.ok(identity);
    owner[Symbol.dispose]();
    owners.delete(owner);
    rows.push(["dispose", identity.kind, identity]);
  };

  try {
    value("usage", "before configure", () => engine.usage());
    err("[static]vfs.create", "unconfigured", "not-configured", () => engine.Vfs.create(100_000n));
    value("configure", "valid ceiling", () => engine.configure(64n << 20n));
    err("configure", "already configured", "already-configured", () => engine.configure(64n << 20n));
    value("raise-ceiling", "valid ceiling", () => engine.raiseCeiling(80n << 20n));
    err("raise-ceiling", "lower ceiling", "invalid-config", () => engine.raiseCeiling(64n << 20n));
    assert.equal(value("usage", "raised ceiling", () => engine.usage()).ceiling, 80n << 20n);
    value("coverage-features", "feature indices", () => engine.coverageFeatures());
    const v = value("[static]vfs.create", "primary volume", () => engine.Vfs.create(1n << 20n));
    const source = value("[static]vfs.create", "mount source", () => engine.Vfs.create(100_000n));
    for (const req of fsOps) {
      const result = value("[method]vfs.op", req.tag, () => v.op(req));
      if (req.tag === "lstat") assert.deepEqual(result, { tag: "err", val: { code: "not-found", path: missing } });
      if (req.tag === "read-file") assert.deepEqual(result, { tag: "ok", val: { tag: "data", val: binary } });
    }
    assert.deepEqual(err("[method]vfs.mount-vfs", "same-store source", "unsupported", () => v.mountVfs(root, source)).path, root);
    assert.deepEqual(err("[method]vfs.mount-host", "maximum ID", "unsupported", () => v.mountHost(root, 0xffffffff)).path, root);
    assert.deepEqual(err("[method]vfs.unmount", "root", "unsupported", () => v.unmount(root)).path, root);
    value("[method]vfs.take-dirty", "L2 drain", () => v.takeDirty());
    value("[method]vfs.used-bytes", "after writes", () => v.usedBytes());
    drop(source);

    const config = testConfig({
      commands: testKernel ? ["echo", "__out"] : ["echo"],
      cwd: root,
      processInfo: { pid: 4, ppid: 3, uid: 2, gid: 1 },
      env: [{ name: bytes("SEED"), value: bytes("é"), exported: true }],
      coverage: true,
    });
    err("[static]shell.create", "unknown limit override", "invalid-config", () =>
      engine.Shell.create(v, testConfig({ limits: { profile: "micro", overrides: [{ name: "noSuchLimit", value: 1n }] } })));
    const sh = value("[static]shell.create", "optional config fields", () => engine.Shell.create(v, config));
    value("[method]shell.register-command", "stub", () => sh.registerCommand("stub"));
    value("[method]shell.env", "seeded", () => sh.env());
    assert.deepEqual(value("[method]shell.cwd", "explicit root", () => sh.cwd()), root);

    const parent = value("[static]fiber.spawn", "no parent", () => engine.Fiber.spawn(sh, undefined, opts("stub a")));
    value("[method]fiber.write-stdin", "sliced non-final bytes", () => parent.writeStdin(binary, false));
    value("[method]fiber.write-stdin", "empty final stdin", () => parent.writeStdin(new Uint8Array(), true));
    assert.equal(value("[method]fiber.run", "zero fuel", () => parent.run(0n)).tag, "out-of-fuel");
    const step = value("[method]fiber.run", "registered custom command", () => parent.run(100n));
    assert.equal(step.tag, "host-call");
    const { epoch } = step.val;
    const child = value("[static]fiber.spawn", "borrowed parent", () =>
      engine.Fiber.spawn(sh, parent, opts(testKernel ? "__out 2" : "echo child")));
    assert.equal(value("[method]fiber.run", "child done", () => child.run(100n)).tag, "done");
    value("[method]fiber.take-stdout", "child drain", () => child.takeStdout(100));
    drop(child);
    err("[method]fiber.resume", "stale epoch", "stale-epoch", () => parent.resume(epoch + 1n, { tag: "completed" }));
    assert.deepEqual(value("[method]fiber.read-call-input", "fd zero", () => parent.readCallInput(epoch, 0, 100)), binary);
    value("[method]fiber.push-call-data", "binary stdout", () => parent.pushCallData(epoch, "stdout-bytes", binary));
    value("[method]fiber.push-call-data", "stderr", () => parent.pushCallData(epoch, "stderr", bytes("error")));
    const env = [{ name: bytes("NAME"), value: bytes("before"), exported: true }];
    value("[method]fiber.set-call-env", "replace pending environment", () => parent.setCallEnv(epoch, env));
    value("[method]fiber.assign-variable", "no subscript", () => parent.assignVariable(epoch, bytes("NAME"), bytes("after"), undefined));
    const done = value("[method]fiber.resume", "command exit", () => parent.resume(epoch, { tag: "command-exit", val: 7 }));
    assert.equal(done.tag, "done");
    assert.equal(done.val.exitCode, 7);
    assert.deepEqual(value("[method]fiber.take-stdout", "binary drain", () => parent.takeStdout(100)), binary);
    assert.deepEqual(value("[method]fiber.take-stderr", "stderr drain", () => parent.takeStderr(100)), bytes("error"));
    value("[method]fiber.take-trace", "L2 drain", () => parent.takeTrace(100));
    assert.ok(value("[method]fiber.take-coverage", "feature ID drain", () => parent.takeCoverage(100)) instanceof Uint16Array);
    drop(parent);

    const cancelled = value("[static]fiber.spawn", "cancellable", () => engine.Fiber.spawn(sh, undefined, opts("stub")));
    const pending = value("[method]fiber.run", "before cancel", () => cancelled.run(100n));
    assert.equal(value("[method]fiber.cancel", "aborted", () => cancelled.cancel("aborted")).status, "aborted");
    err("[method]fiber.resume", "after cancel", "bad-state", () =>
      cancelled.resume(pending.val.epoch, { tag: "command-exit", val: 0 }));
    drop(cancelled);
    drop(sh);
    drop(v);

    const oneshot = { files: [{ path: bytes("/f"), data: binary, mode: 0o644 }], config: testConfig(),
      opts: opts("echo hi"), stdin: new Uint8Array(), fuel: 100n };
    const one = value("exec-oneshot", "isolated run", () => engine.execOneshot(oneshot));
    assert.deepEqual(one.stdout, bytes("hi\n"));
    err("exec-oneshot", "trace refusal", "invalid-config", () =>
      engine.execOneshot({ ...oneshot, config: testConfig({ trace: true }) }));
    value("usage", "after disposal and oneshot", () => engine.usage());
    assert.deepEqual([...coverage].sort(), functions.map(({ name }) => name).sort());
    return rows;
  } finally {
    _util.stop();
  }
}

layoutCases("fs-op", fsOps);

for (const name of ["kernel", "kernel-test"]) {
  test(name + " compares all 29 exports and their subsequent state with the guarded oracle", async (t) => {
    const pair = await adapterPair(name);
    const imports = WebAssembly.Module.imports(pair.module);
    assert.deepEqual(imports.map(({ module, name, kind }) => [module, name, kind]).sort(),
      resourceKinds.flatMap((kind) => ["new", "drop"].map((op) =>
        ["[export]" + namespace, "[resource-" + op + "]" + kind, "function"])).sort());
    assert.equal(functions.length, 29);
    assert.equal(resourceKinds.length, 3);
    const first = transcript(pair.firstParty(), name === "kernel-test");
    const oracle = transcript(pair.oracle(), name === "kernel-test");
    assert.equal(first.length, oracle.length);
    first.forEach(([fn, input, result], i) => {
      assert.deepEqual(oracle[i].slice(0, 2), [fn, input]);
      compareValues(result, oracle[i][2], fn + " (" + input + ")");
    });
    t.diagnostic("29/29 functions, 21/21 fs-op tags, 3/3 step tags, core SHA-256 " + pair.hash);
  });
}

