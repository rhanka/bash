import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { Codec } from "../../host/core/abi/codec.js";
import { camelCase, layout } from "../../host/core/abi/layout.js";
import { run, toolBin } from "../../tools/lib.mjs";
import { transpileComponent } from "../../tools/transpile.mjs";
import { repoRoot, scratch } from "../helpers.mjs";
import { binary, bytes } from "./support.mjs";

const headers = [{ name: "x-sandbox", value: binary }];
const url = "https://example.invalid/é";
const fetchReq = { method: "POST", url, headers, inputLen: 4n, followRedirects: true,
  maxRedirects: 0xffffffff, timeoutMs: 50, maxResponseBytes: (1n << 64n) - 1n };
const fixtures = [
  ["trace-event", [{ category: "builtin", name: "é", durationMs: -0, details: [
    { key: "time", value: { tag: "number", val: -0 } },
    { key: "text", value: { tag: "text", val: binary } },
  ] }]],
  ["walk-entry", [{ path: bytes("/é"), info: { kind: "symlink", size: 1n << 53n, mode: 0o755,
    mtimeMs: -(1n << 63n), dev: 4n, ino: 5n } }]],
  ["fetch-req", [fetchReq, { ...fetchReq, maxRedirects: undefined }]],
  ["fetch-resp", [{ status: 201, statusText: "Créé", url, headers, redirectChain: [
    { url: "https://example.invalid/first", status: 307, statusText: "Redirect", headers },
  ] }]],
];

async function wireOracle() {
  const dir = scratch("abi-wire");
  const wit = join(dir, "fixture.wit");
  const wat = join(dir, "fixture.wat");
  const core = join(dir, "fixture.core.wasm");
  const embedded = join(dir, "embedded.wasm");
  const component = join(dir, "fixture.component.wasm");
  const output = join(dir, "js");
  const names = fixtures.map(([name]) => name);
  writeFileSync(wit, readFileSync(join(repoRoot, "wit/sentropic-bash.wit"), "utf8") + `
world abi-wire {
  use types.{${names.join(", ")}};
  ${names.map((name) => `export echo-${name}: func(values: list<${name}>) -> list<${name}>;`).join("\n  ")}
}
`);
  // The core echoes the pair unchanged. Neither the allocator nor the echo knows any field offsets.
  writeFileSync(wat, `(module
    (memory (export "memory") 1)
    (global $heap (export "heap") (mut i32) (i32.const 32))
    (func (export "cabi_realloc") (param $old i32) (param $old-size i32)
      (param $align i32) (param $size i32) (result i32)
      (local $ptr i32)
      (if (i32.or (local.get $old) (local.get $old-size)) (then unreachable))
      (if (i32.eqz (local.get $size)) (then (return (local.get $align))))
      (local.set $ptr (i32.and
        (i32.add (global.get $heap) (i32.sub (local.get $align) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get $align))))
      (global.set $heap (i32.add (local.get $ptr) (local.get $size)))
      (local.get $ptr))
    (func $echo (param $ptr i32) (param $len i32) (result i32)
      (i32.store (i32.const 8) (local.get $ptr))
      (i32.store (i32.const 12) (local.get $len))
      (i32.const 8))
    ${names.map((name) => `(export "echo-${name}" (func $echo))`).join("\n    ")}
  )`);
  const wasmTools = toolBin("wasm-tools");
  run(wasmTools, ["parse", wat, "-o", core]);
  run(wasmTools, ["component", "embed", wit, "--world", "abi-wire", core, "-o", embedded]);
  run(wasmTools, ["component", "new", embedded, "-o", component]);
  await transpileComponent(component, output);
  const { instantiate } = await import(pathToFileURL(join(output, "fixture.js")).href);
  const module = new WebAssembly.Module(readFileSync(join(output, "fixture.core.wasm")));
  return { instantiate, module };
}

test("jco and Codec put trace-event, walk-entry and fetch records on the same wire", async () => {
  const { instantiate, module } = await wireOracle();
  for (const [name, input] of fixtures) {
    let oracleCore;
    const oracle = instantiate(() => module, {}, (compiled, imports) => {
      const instance = new WebAssembly.Instance(compiled, imports);
      oracleCore = instance.exports;
      return instance;
    });
    const exportName = "echo-" + name;
    assert.deepEqual(oracle[camelCase(exportName)](input), input, name + " jco projection");

    const core = new WebAssembly.Instance(module).exports;
    const codec = new Codec({ core });
    const type = layout(["list", name]);
    const slots = [];
    codec.flatten(type, codec.stage(type, input, name), slots);
    const result = core[exportName](...slots);
    const wire = (exports) => new Uint8Array(exports.memory.buffer, 0, exports.heap.value);
    assert.deepEqual(wire(core), wire(oracleCore), name + " canonical bytes");
    assert.deepEqual(new Codec({ core: oracleCore }).read(type, result), input, name + " Codec lift of jco bytes");
    oracle._util.stop();
  }
});
