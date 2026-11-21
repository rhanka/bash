import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { linkEngine } from "../../host/core/abi/binding.js";
import { layout, namespace } from "../../host/core/abi/layout.js";
import { run, sha256, toolBin } from "../../tools/lib.mjs";
import { scratch } from "../helpers.mjs";

export const bytes = (value) => new TextEncoder().encode(value);
export const binary = Uint8Array.of(10, 0, 0x7f, 0x80, 0xff, 20).subarray(1, 5);
export const testConfig = (extra = {}) => ({
  limits: { profile: "micro", overrides: [] },
  grants: { customCommands: true },
  customCommands: [],
  env: [],
  defaultLayout: false,
  trace: false,
  coverage: false,
  ...extra,
});
export const opts = (script, extra = {}) => ({
  script: bytes(script), args: [], env: [], replaceEnv: false, rawScript: false, ...extra,
});
export const coreName = (fn) => namespace + "#" + fn;
export const postName = (fn) => "cabi_post_" + coreName(fn);
export const importsName = "[export]" + namespace;

export async function adapterPair(name) {
  const first = name === "kernel" ? "../../dist/js/kernel/" : "../../dist/test/js/kernel-test/";
  const oracle = "../../dist/test/js/oracle/" + name + "/";
  const source = (base) => new URL(base + name + ".core.wasm", import.meta.url);
  const firstBytes = readFileSync(source(first));
  const oracleBytes = readFileSync(source(oracle));
  const hash = sha256(firstBytes);
  assert.equal(sha256(oracleBytes), hash);
  const firstModule = new WebAssembly.Module(firstBytes);
  const oracleModule = new WebAssembly.Module(oracleBytes);
  const ours = await import(first + name + ".js");
  const jco = await import(oracle + name + ".js");
  const { shims } = await import(oracle + name + ".shims.js");
  return {
    hash,
    module: firstModule,
    jcoHooks: { instantiate: jco.instantiate, shims, module: oracleModule },
    firstParty() {
      const loaded = [];
      const store = ours.instantiate((path) => (loaded.push(path), firstModule), {});
      assert.deepEqual(loaded, [name + ".core.wasm"]);
      return store;
    },
    oracle() {
      const loaded = [];
      const store = jco.instantiate((path) => {
        if (path in shims) return new WebAssembly.Module(shims[path]);
        loaded.push(path);
        return oracleModule;
      }, {});
      assert.deepEqual(loaded, [name + ".core.wasm"]);
      return store;
    },
  };
}

export function snapshotResult(fn) {
  try {
    return { value: fn() };
  } catch (error) {
    if (error instanceof Error) throw error;
    return { error };
  }
}

export function watModule(source) {
  const dir = scratch("abi-wat");
  const wat = join(dir, "fixture.wat");
  const wasm = join(dir, "fixture.wasm");
  writeFileSync(wat, source);
  run(toolBin("wasm-tools"), ["parse", wat, "-o", wasm]);
  return new WebAssembly.Module(readFileSync(wasm));
}

/** Track calls at the actual Wasm boundary while leaving the core's behavior intact. */
export function instrumentCore(module, options) {
  const events = [];
  let raw;
  const store = linkEngine((imports) => {
    raw = new WebAssembly.Instance(module, imports).exports;
    return Object.fromEntries(Object.entries(raw).map(([name, value]) => [name,
      typeof value === "function" ? (...args) => {
        const event = { name, args };
        events.push(event);
        event.result = value(...args);
        return event.result;
      } : value,
    ]));
  }, options);
  return { ...store, raw, events };
}

// Compare fixtures by field so failures in a transcript point to the first differing value.
export function compareValues(actual, expected, path) {
  if (Object.is(actual, expected)) return;
  if (actual && expected && typeof actual === "object" && typeof expected === "object") {
    assert.equal(Object.getPrototypeOf(actual), Object.getPrototypeOf(expected), path + " prototype");
    assert.deepEqual(Object.keys(actual), Object.keys(expected), path + " fields");
    for (const key of Object.keys(actual)) compareValues(actual[key], expected[key], path + "." + key);
    return;
  }
  assert.deepEqual(actual, expected, path);
}

export function layoutCases(type, corpus) {
  assert.deepEqual(corpus.map(({ tag }) => tag).sort(), layout(type).cases.map(({ tag }) => tag).sort());
}

