import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { scratch } from "./helpers.mjs";
import { guardPromisingProbes, writeFiles } from "../tools/transpile.mjs";
import { instantiate } from "../dist/test/js/oracle/kernel/kernel.js";
import { shims } from "../dist/test/js/oracle/kernel/kernel.shims.js";

test("generated files are written inside the output directory", () => {
  const out = scratch("transpile");
  writeFiles(out, { "a.js": "x", "sub/b.js": "y" });
  assert.equal(readFileSync(join(out, "sub", "b.js"), "utf8"), "y");
});

test("a generated file name that escapes the output directory is refused", () => {
  const out = scratch("transpile-escape");
  assert.throws(() => writeFiles(out, { "../escape.js": "x" }), /escapes the output directory/);
  assert.throws(() => writeFiles(out, { "/abs-escape.js": "x" }), /escapes the output directory/);
  assert.equal(existsSync(join(out, "..", "escape.js")), false);
});

const reallocProbe = "\n\t\ttry {\n\t\t\tWebAssembly.promising(exports1.cabi_realloc);\n\t\t} catch {}";
const alias = "\n\tlet WebAssembly = options?.shim?.globals?.WebAssembly ?? globalThis.WebAssembly;\n";
const postReturnProbe = (name) => {
  const ref = `exports1["cabi_post_${name}"]`;
  return `\n\t\ttry {\n\t\t\tWebAssembly.promising(${ref});\n\t\t} catch {\n\t\t\t${ref};\n\t\t}`;
};

test("the probe guard rejects a fixture with an unexpected count", () => {
  assert.throws(() => guardPromisingProbes(reallocProbe), /probe drift.*expected 14 references, found 1/);
});

test("the probe guard rejects a changed catch body even with the expected count", () => {
  const fixture = alias + Array.from({ length: 13 }, (_, index) => postReturnProbe(index)).join("") + reallocProbe;
  assert.equal(guardPromisingProbes(fixture), fixture.replaceAll(".promising(", ".promising?.("));
  const changed = fixture.replace("} catch {}", "} catch { throw new Error(); }");
  assert.throws(() => guardPromisingProbes(changed), /probe drift.*expected 1 realloc probes/);
});

for (const minify of [false, true]) {
  const binding = minify ? "let r=n?.shim?.globals?.WebAssembly??globalThis.WebAssembly;" : alias;
  const post = minify
    ? (index) => {
      const ref = "n[`cabi_post_" + index + "`]";
      return "try{r.promising(" + ref + ")}catch{" + ref + "}";
    }
    : postReturnProbe;
  const fixture = binding + Array.from({ length: 13 }, (_, index) => post(index)).join("") +
    (minify ? "try{r.promising(n.cabi_realloc)}catch{}" : reallocProbe);

  test(`the probe guard rejects alternate access and alias drift (minify=${minify})`, () => {
    const options = { minify };
    assert.equal(guardPromisingProbes(fixture, options), fixture.replaceAll(".promising(", ".promising?.("));
    for (const extra of ["const { promising } = r;", "r[`promising`];", '"promising" in r;']) {
      assert.throws(() => guardPromisingProbes(fixture + extra, options), /expected 14 references, found 15/);
    }
    assert.throws(() => guardPromisingProbes(fixture.replace("globalThis.WebAssembly", "globalThis.Other"), options),
      /WebAssembly alias changed/);
    assert.throws(() => guardPromisingProbes(fixture.replace(/catch\s*\{\}$/, "catch{throw 1}"), options),
      /expected 1 realloc probes/);
  });
}

test("each engine oracle contains exactly 14 guarded probes and no unguarded calls", () => {
  for (const name of ["kernel", "kernel-test"]) {
    const source = readFileSync(new URL(`../dist/test/js/oracle/${name}/${name}.js`, import.meta.url), "utf8");
    assert.doesNotMatch(source, /\.promising\s*\(/);
    assert.equal([...source.matchAll(/\.promising\?\.\(/g)].length, 14);
    assert.equal([...source.matchAll(/\bpromising\b/g)].length, 14);
  }
});

const encode = (text) => new TextEncoder().encode(text);
const decode = (bytes) => new TextDecoder().decode(bytes);
const core = readFileSync(new URL("../dist/test/js/oracle/kernel/kernel.core.wasm", import.meta.url));

for (const mode of ["throws", "returns a wrapper"]) {
  test(`oracle kernel exports work when WebAssembly.promising ${mode}`, async () => {
    const probes = [];
    const promising = function (fn) {
      probes.push({ receiver: this, fn });
      if (mode === "throws") throw new TypeError("stub refused the probe");
      return () => assert.fail("a synchronous export invoked the discarded wrapper");
    };
    const wasm = Object.create(WebAssembly, { promising: { value: promising } });
    const { engine, _util } = instantiate(
      (name) => new WebAssembly.Module(shims[name] ?? core),
      {},
      undefined,
      { shim: { globals: { WebAssembly: wasm } } },
    );
    assert.equal(probes.length, 14);
    for (const { receiver, fn } of probes) {
      assert.equal(receiver, wasm);
      assert.equal(typeof fn, "function");
    }

    engine.configure(16n << 20n);
    engine.raiseCeiling(64n << 20n);
    assert.equal(engine.usage().ceiling, 64n << 20n);
    assert.ok(engine.coverageFeatures().every((feature) => typeof feature === "string"));
    const vfs = engine.Vfs.create(1n << 20n);
    const root = encode("/");
    assert.deepEqual(vfs.op({ tag: "exists", val: root }), { tag: "ok", val: { tag: "flag", val: true } });
    assert.throws(() => vfs.mountVfs(root, vfs), { code: "unsupported" });
    vfs.mountHost(root, 1);
    assert.throws(() => vfs.mountHost(root, 2), { code: "exists" });
    vfs.unmount(root);
    assert.throws(() => vfs.unmount(root), { code: "not-found" });
    assert.deepEqual(vfs.takeDirty(), []);
    const usedBytes = vfs.usedBytes();
    assert.ok(usedBytes > 0n && usedBytes < 1n << 20n);

    const config = {
      limits: { profile: "micro", overrides: [] }, grants: { customCommands: true },
      customCommands: [], env: [], defaultLayout: false, trace: false, coverage: false,
    };
    const shell = engine.Shell.create(vfs, config);
    shell.registerCommand("stub");
    assert.ok(Array.isArray(shell.env()));
    assert.deepEqual(shell.cwd(), root);
    const opts = (script) => ({ script: encode(script), args: [], env: [], replaceEnv: false, rawScript: false });
    const fiber = engine.Fiber.spawn(shell, undefined, opts("stub; echo $VALUE"));
    fiber.writeStdin(encode("input"), true);
    const step = fiber.run(100n);
    assert.equal(step.tag, "host-call");
    const { epoch } = step.val;
    assert.equal(decode(fiber.readCallInput(epoch, 0, 100)), "input");
    fiber.pushCallData(epoch, "stdout", encode("custom\n"));
    fiber.assignVariable(epoch, encode("VALUE"), encode("value"), undefined);
    assert.equal(fiber.resume(epoch, { tag: "command-exit", val: 0 }).val.exitCode, 0);
    assert.equal(decode(fiber.takeStdout(100)), "custom\nvalue\n");
    assert.equal(fiber.takeStderr(100).length, 0);
    assert.deepEqual(fiber.takeTrace(100), []);
    assert.deepEqual(fiber.takeCoverage(100), new Uint16Array());
    fiber[Symbol.dispose]();

    const cancelled = engine.Fiber.spawn(shell, undefined, opts("stub"));
    const pending = cancelled.run(100n);
    assert.equal(pending.tag, "host-call");
    cancelled.setCallEnv(pending.val.epoch, []);
    assert.equal(cancelled.cancel("aborted").status, "aborted");
    cancelled[Symbol.dispose]();
    shell[Symbol.dispose]();
    vfs[Symbol.dispose]();

    const result = engine.execOneshot({ files: [], config, opts: opts("echo hi"), stdin: encode(""), fuel: 100n });
    assert.equal(result.outcome.exitCode, 0);
    assert.equal(decode(result.stdout), "hi\n");
    await _util.whenIdle();
    _util.stop();
  });
}
