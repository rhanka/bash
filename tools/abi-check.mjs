// Verify the checked-in descriptors against the WIT and the exact loader core.
import { join } from "node:path";
import { descriptors } from "../host/core/abi/descriptors.js";
import { functions, namespace, resourceKinds } from "../host/core/abi/layout.js";
import { normalizeWit, readWit } from "./abi-descriptors.mjs";
import { run, sha256, sha256File } from "./lib.mjs";

export function assertAbiEqual(actual, expected, where) {
  if (actual === expected) return;
  if (actual && expected && typeof actual === "object" && typeof expected === "object" &&
      Array.isArray(actual) === Array.isArray(expected)) {
    for (const key of new Set([...Object.keys(actual), ...Object.keys(expected)])) {
      const a = Object.hasOwn(actual, key) ? actual[key] : undefined;
      const b = Object.hasOwn(expected, key) ? expected[key] : undefined;
      assertAbiEqual(a, b, where + "." + key);
    }
    return;
  }
  throw new Error(where + " differs: expected " + JSON.stringify(expected) + ", found " + JSON.stringify(actual));
}

export function checkWit(source, component) {
  assertAbiEqual(normalizeWit(source), descriptors, "ABI WIT");
  const forComponent = ({ functions, ...other }) => ({ ...other,
    functions: Object.fromEntries(functions.map(([name, params, result]) => [name, [params, result]])),
  });
  assertAbiEqual(forComponent(normalizeWit(component)), forComponent(descriptors), "ABI component WIT");
}

const wasmSignature = (params = [], results = []) => ({ params, results });
const signature = (line) => {
  const types = (clause) => [...line.matchAll(new RegExp("\\(" + clause + " ([^)]+)\\)", "g"))]
    .flatMap(([, slots]) => slots.split(/\s+/).filter((token) => !token.startsWith("$")));
  return wasmSignature(types("param"), types("result"));
};

const id = /\$"[^"]+"|\$[^\s()]+|\d+/;
const coreFunc = new RegExp("\\(func(?: (" + id.source + "))? \\(;(\\d+);\\)");

export function checkLoaderCore(wat) {
  const imports = Object.create(null);
  const exports = Object.create(null);
  const funcs = new Map();
  const memories = [];
  // Use top-level declarations from the pinned wasm-tools skeleton printer.
  for (const line of wat.split("\n")) {
    const declaration = line.match(/^  \((\w+)\b/);
    if (!declaration) continue;
    switch (declaration[1]) {
      case "import": {
        const match = line.match(/^  \(import "([^"]+)" "([^"]+)" \(func\b/);
        if (!match) throw new Error("ABI core has an unexpected import " + line.trim());
        const name = match[1] + "." + match[2];
        if (Object.hasOwn(imports, name)) throw new Error("ABI core has duplicate import " + name);
        imports[name] = signature(line);
        break;
      }
      case "func": {
        const entry = line.match(coreFunc);
        if (!entry) throw new Error("ABI core has an unexpected function declaration " + line.trim());
        const type = signature(line);
        funcs.set(entry[2], type);
        if (entry[1]) funcs.set(entry[1], type);
        break;
      }
      case "memory":
        memories.push(line);
        break;
      case "export": {
        const entry = line.match(/^  \(export "([^"]+)" \((\w+) (.+)\)\)$/);
        if (!entry) throw new Error("ABI core has an unexpected export " + line.trim());
        exports[entry[1]] = [entry[2], entry[3]];
        break;
      }
    }
  }
  for (const [name, [kind, index]] of Object.entries(exports)) {
    if (kind !== "func") continue;
    if (!funcs.has(index)) throw new Error("ABI core cannot resolve " + name);
    exports[name] = funcs.get(index);
  }

  const expectedImports = Object.create(null);
  const expectedExports = {
    memory: ["memory", "0"],
    cabi_realloc: wasmSignature(["i32", "i32", "i32", "i32"], ["i32"]),
    memcmp: wasmSignature(["i32", "i32", "i32"], ["i32"]),
  };
  if (memories.length !== 1 || !/^  \(memory \(;0;\) \d+\)$/.test(memories[0])) {
    throw new Error("ABI core must have one unshared wasm32 memory without a maximum");
  }
  for (const kind of resourceKinds) {
    expectedImports["[export]" + namespace + ".[resource-new]" + kind] = wasmSignature(["i32"], ["i32"]);
    expectedImports["[export]" + namespace + ".[resource-drop]" + kind] = wasmSignature(["i32"]);
    expectedExports[namespace + "#[dtor]" + kind] = wasmSignature(["i32"]);
  }
  for (const fn of functions) {
    expectedExports[fn.exportName] = wasmSignature(fn.coreParams, fn.coreResults);
    if (fn.postName) expectedExports[fn.postName] = wasmSignature(fn.coreResults);
  }
  assertAbiEqual(imports, expectedImports, "ABI core import");
  assertAbiEqual(exports, expectedExports, "ABI core export");
}

export function checkCanonicalLifts(wat) {
  const mapping = Object.create(null);
  const aliases = new Map();
  const coreAlias = new RegExp('^  \\(alias core export \\$main "([^"]+)" \\(core (?:func|memory) (' + id.source + ') \\(;(\\d+);\\)\\)\\)$');
  const liftStart = new RegExp("\\(canon lift \\(core func (" + id.source + ")\\)(.*)\\)\\)$");
  const options = new RegExp("\\((memory|realloc|post-return) (" + id.source + ")\\)|string-encoding=(\\w+)", "g");
  for (const line of wat.split("\n")) {
    const alias = line.match(coreAlias);
    if (alias) {
      aliases.set(alias[2], alias[1]);
      aliases.set(alias[3], alias[1]);
    }
    if (!line.startsWith("  (func ") || !line.includes("(canon lift ")) continue;
    const match = line.match(liftStart);
    const name = match && aliases.get(match[1]);
    if (!name || Object.hasOwn(mapping, name)) throw new Error("ABI component has an unexpected canonical lift " + line.trim());
    const [, , tail] = match;
    const normalized = Object.create(null);
    for (const [, key, target, encoding] of tail.matchAll(options)) {
      if (key && !aliases.has(target)) throw new Error("ABI component has unresolved " + key + " for " + name);
      normalized[key ?? "encoding"] = key ? aliases.get(target) : encoding;
    }
    if (tail.replace(options, "").trim() !== "") throw new Error("ABI component has unknown canonical options for " + name);
    if (normalized.encoding === "utf8") delete normalized.encoding;
    mapping[name] = normalized;
  }

  const expected = Object.fromEntries(functions.map((fn) => {
    const options = {};
    if (fn.indirectParams || fn.params.dynamic || fn.indirectResult || fn.result.dynamic) options.memory = "memory";
    if (fn.indirectParams || fn.params.dynamic) options.realloc = "cabi_realloc";
    if (fn.postName) options["post-return"] = fn.postName;
    return [fn.exportName, options];
  }));
  assertAbiEqual(mapping, expected, "ABI component canonical lift");
}

export function verifyEngineAbi(root, componentPath, corePath, wasmTools) {
  const witPath = join(root, "wit", "sentropic-bash.wit");
  checkWit(readWit(witPath, wasmTools), readWit(componentPath, wasmTools));
  const skeleton = (path) => run(wasmTools, ["print", "--skeleton", path], { maxBuffer: 1 << 24 });
  checkLoaderCore(skeleton(corePath));
  checkCanonicalLifts(skeleton(componentPath));
  console.log("ABI conformance", JSON.stringify({
    functions: functions.length,
    resources: resourceKinds.length,
    imports: 2 * resourceKinds.length,
    postReturns: functions.filter((fn) => fn.postName).length,
    descriptors: sha256(JSON.stringify(descriptors)),
    wit: sha256File(witPath),
    loaderCore: sha256File(corePath),
  }));
}

