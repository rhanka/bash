// Normalize wasm-tools' WIT graph without numeric IDs or documentation.
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { repoRoot, run, toolBin } from "./lib.mjs";

export function readWit(path, wasmTools = toolBin("wasm-tools")) {
  return JSON.parse(run(wasmTools, ["component", "wit", "--json", path]));
}

export function normalizeWit(wit) {
  const world = wit.worlds[0];
  const exports = Object.values(world.exports).map((item) => item.interface?.id);
  const [types, engine] = exports.map((id) => wit.interfaces[id]);
  const pkg = wit.packages[types?.package];
  if (wit.worlds.length !== 1 || Object.keys(world.imports).length || exports.length !== 2 ||
      types?.name !== "types" || engine?.name !== "engine" || engine.package !== types.package ||
      Object.keys(types.functions).length) {
    throw new Error("ABI WIT world must export only types and engine and have no imports");
  }

  const labels = (entries) => entries.map(({ name }) => name);
  const fields = (entries) => entries.map(({ name, type }) => [name, ref(type)]);
  function body(kind) {
    if (kind === "resource") return kind;
    if ("type" in kind) return ref(kind.type);
    if ("list" in kind) return ["list", ref(kind.list)];
    if ("option" in kind) return ["option", ref(kind.option)];
    if ("record" in kind) return ["record", fields(kind.record.fields)];
    if ("variant" in kind) return ["variant", fields(kind.variant.cases)];
    if ("enum" in kind) return ["enum", labels(kind.enum.cases)];
    if ("flags" in kind) return ["flags", labels(kind.flags.flags)];
    if ("result" in kind) return ["result", ref(kind.result.ok), ref(kind.result.err)];
    if ("handle" in kind) {
      const [[ownership, id]] = Object.entries(kind.handle);
      return [ownership, wit.types[id].name];
    }
    throw new Error("unsupported WIT descriptor " + JSON.stringify(kind));
  }

  function ref(id) {
    if (typeof id !== "number") return id;
    const { name, kind } = wit.types[id];
    return name ?? body(kind);
  }

  const definitions = (iface) => Object.entries(iface.types).map(([name, id]) => [name, body(wit.types[id].kind)]);
  const functions = Object.entries(engine.functions).map(([name, f]) => {
    const receiver = f.kind === "freestanding" ? null : Object.entries(f.kind)[0];
    if (name !== f.name || (receiver && !name.startsWith("[" + receiver[0] + "]" + ref(receiver[1]) + "."))) {
      throw new Error("unexpected WIT receiver for " + name);
    }
    return [name, fields(f.params), ref(f.result ?? null)];
  });
  return {
    namespace: pkg.name.replace("@", "/engine@"),
    types: definitions(types),
    engineTypes: definitions(engine),
    functions,
  };
}

function definition(name, entries) {
  return "  " + name + ": [\n" + entries.map((entry) => "    " + JSON.stringify(entry) + ",\n").join("") + "  ],\n";
}

export function descriptorSource(graph) {
  return "// Generated from wit/sentropic-bash.wit by tools/abi-descriptors.mjs --emit.\n" +
    "export const descriptors = {\n" +
    "  namespace: " + JSON.stringify(graph.namespace) + ",\n" +
    definition("types", graph.types) + definition("engineTypes", graph.engineTypes) +
    definition("functions", graph.functions) +
    "};\n";
}

// Flat numeric/op tables precompiled from the WIT at build time. The shipped JS decodes
// these instead of carrying the WIT AST (descriptors.js) and the layout compiler.
// Kind codes: primitives 0-8 + unit 9 are implicit; composite entries use 10-18.
//   10 list [10, elem]  11 record [11, isLimits, [[key, type, maxBytes?], ...]]
//   12 enum [12, [label, ...]]  13 flags [13, [label, ...]]
//   14 variant [14, [[tag, type], ...]]  15 option [15, type]
//   16 result [16, ok, err]  17 own [17, res]  18 borrow [18, res]
// Runtime strings live once in S (field keys camelCase, variant tags as-is, enum labels
// as-is, flags labels camelCase, function names as-is); types are numeric indices
// (0-9 primitives/unit, 10+ entries in T). Functions reference type indices. Type names
// for layout("name") lookups (tests only) live in TN; N maps TN indices to type indices
// so the shipped bundle drops TN+N entirely via DCE.
export function tableSource(graph) {
  const camel = (s) => s.replace(/-([a-z])/g, (_, l) => l.toUpperCase());
  const MIB = 1 << 20;
  const prim = { bool: 0, u8: 1, u16: 2, u32: 3, s32: 4, u64: 5, s64: 6, f64: 7, string: 8 };
  const resourceKinds = graph.engineTypes.filter(([, t]) => t === "resource").map(([n]) => n);
  const resIdx = new Map(resourceKinds.map((n, i) => [n, i]));
  const S = [];
  const strIdx = new Map();
  const intern = (s) => {
    let i = strIdx.get(s);
    if (i === undefined) {
      i = S.length;
      S.push(s);
      strIdx.set(s, i);
    }
    return i;
  };
  // Test-only type names, kept separate so shipped DCE drops them with N.
  const TN = [];
  const tnIdx = new Map();
  const internTN = (s) => {
    let i = tnIdx.get(s);
    if (i === undefined) {
      i = TN.length;
      TN.push(s);
      tnIdx.set(s, i);
    }
    return i;
  };
  const T = [];
  const wrapMemo = new Map();
  const namedIdx = new Map();
  graph.types.forEach(([name], i) => namedIdx.set(name, 10 + i));
  // Reserve the named slots; wrappers and params records append after.
  for (let i = 0; i < graph.types.length; i++) T.push(null);

  const maxBytesFor = (recordName, fieldName) => {
    if (recordName === "exec-opts" && fieldName === "script") return 3 * MIB;
    if (recordName === "write-req" && fieldName === "data") return MIB;
    if ((recordName === "[method]fiber.write-stdin" || recordName === "[method]fiber.push-call-data") &&
      fieldName === "chunk") return MIB;
    return undefined;
  };

  function refToIdx(ref) {
    if (ref === null || ref === undefined) return 9;
    if (typeof ref === "string") {
      if (ref in prim) return prim[ref];
      const idx = namedIdx.get(ref);
      if (idx === undefined) throw new Error("unknown ABI type reference " + ref);
      return idx;
    }
    const [kind, first, second] = ref;
    if (kind === "list") {
      const elem = refToIdx(first);
      const key = JSON.stringify([kind, elem]);
      let idx = wrapMemo.get(key);
      if (idx === undefined) {
        idx = 10 + T.length;
        wrapMemo.set(key, idx);
        T.push([10, elem]);
      }
      return idx;
    }
    if (kind === "option") {
      const elem = refToIdx(first);
      const key = JSON.stringify([kind, elem]);
      let idx = wrapMemo.get(key);
      if (idx === undefined) {
        idx = 10 + T.length;
        wrapMemo.set(key, idx);
        T.push([15, 2, intern("none"), 9, intern("some"), elem]);
      }
      return idx;
    }
    if (kind === "result") {
      const ok = refToIdx(first);
      const err = refToIdx(second);
      const key = JSON.stringify([kind, ok, err]);
      let idx = wrapMemo.get(key);
      if (idx === undefined) {
        idx = 10 + T.length;
        wrapMemo.set(key, idx);
        T.push([16, 2, intern("ok"), ok, intern("err"), err]);
      }
      return idx;
    }
    if (kind === "own" || kind === "borrow") {
      const r = resIdx.get(first);
      if (r === undefined) throw new Error("unknown ABI resource " + first);
      const key = JSON.stringify([kind, r]);
      let idx = wrapMemo.get(key);
      if (idx === undefined) {
        idx = 10 + T.length;
        wrapMemo.set(key, idx);
        T.push([kind === "own" ? 17 : 18, r]);
      }
      return idx;
    }
    throw new Error("unsupported anonymous ABI descriptor " + JSON.stringify(ref));
  }

  // Flat numeric layout: records/variants/enums/flags store counts followed by flat
  // pairs (no nested arrays) to cut JSON brackets. Record transfer caps (only 4 fields)
  // live in the side table M as [typeIdx, fieldPos, maxBytes] so most fields stay pairs.
  const M = [];
  function recordEntry(recordName, fields, typeIdx) {
    const out = [11, fields.length];
    let pos = 0;
    for (const [name, ref] of fields) {
      out.push(intern(camel(name)), refToIdx(ref));
      const cap = maxBytesFor(recordName, name);
      if (cap !== undefined) M.push([typeIdx, pos, cap]);
      pos++;
    }
    return out;
  }

  function bodyEntry(witName, body, typeIdx) {
    if (!Array.isArray(body)) {
      if (body === "resource") throw new Error("resources are not stored in T");
      return null;
    }
    const [kind, first, second] = body;
    switch (kind) {
      case "list": return [10, refToIdx(first)];
      case "record": return recordEntry(witName, first, typeIdx);
      case "enum": return [12, first.length, ...first.map((l) => intern(l))];
      case "flags": return [13, first.length, ...first.map((l) => intern(camel(l)))];
      case "variant": {
        const out = [14, first.length];
        for (const [tag, ref] of first) out.push(intern(tag), refToIdx(ref));
        return out;
      }
      case "option": return [15, 2, intern("none"), 9, intern("some"), refToIdx(first)];
      case "result": return [16, 2, intern("ok"), refToIdx(first), intern("err"), refToIdx(second)];
      default: throw new Error("unsupported ABI type body " + kind);
    }
  }

  graph.types.forEach(([name, body], i) => {
    T[i] = bodyEntry(name, body, 10 + i);
  });

  const streamed = {
    "[method]fiber.read-call-input": 1,
    "[method]fiber.take-stdout": 1,
    "[method]fiber.take-stderr": 1,
    "[method]fiber.take-coverage": 2,
  };
  const F = graph.functions.map(([name, params, result]) => {
    // Build the params record first: its field wrappers append to T before the record.
    // Caps need the final paramsIdx, so build fields first, then assign caps.
    const fieldRefs = params.map(([fname, ref]) => [intern(camel(fname)), refToIdx(ref), maxBytesFor(name, fname)]);
    const paramsIdx = 10 + T.length;
    const entry = [11, params.length];
    let pos = 0;
    for (const [k, t, cap] of fieldRefs) {
      entry.push(k, t);
      if (cap !== undefined) M.push([paramsIdx, pos, cap]);
      pos++;
    }
    T.push(entry);
    const resultIdx = refToIdx(result);
    // Unwrap only anonymous `result<_,_>` returns (not named result types like "fs-result").
    const unwrap = Array.isArray(result) && result[0] === "result" ? 1 : 0;
    // Compress WIT function names: methods/statics share "[method]/[static]" + resource
    // prefixes (24/29 names). Store kind+resource+suffix; decoder rebuilds the full name
    // for exportName/error paths. Freestanding keeps the full name in S.
    const m = /^\[(method|static)\]([^.]+)\.(.+)$/.exec(name);
    let out;
    if (!m) {
      out = [0, intern(name), paramsIdx, resultIdx, unwrap];
    } else {
      const kind = m[1] === "method" ? 1 : 2;
      const res = resIdx.get(m[2]);
      if (res === undefined) throw new Error("unknown ABI resource in " + name);
      out = [kind, res, intern(m[3]), paramsIdx, resultIdx, unwrap];
    }
    if (streamed[name] !== undefined) out.push(streamed[name]);
    return out;
  });

  const N = graph.types.map(([name]) => [internTN(name), namedIdx.get(name)]);
  const limitsIdx = namedIdx.get("limits");
  return "// Generated from wit/sentropic-bash.wit by tools/abi-descriptors.mjs --emit.\n" +
    "// Flat numeric/op tables: the shipped JS decodes these instead of the WIT AST.\n" +
    "export const tableNamespace = " + JSON.stringify(graph.namespace) + ";\n" +
    "export const tableResourceKinds = " + JSON.stringify(resourceKinds) + ";\n" +
    "export const S = " + JSON.stringify(S) + ";\n" +
    "export const T = " + JSON.stringify(T) + ";\n" +
    "export const F = " + JSON.stringify(F) + ";\n" +
    "export const TN = " + JSON.stringify(TN) + ";\n" +
    "export const N = " + JSON.stringify(N) + ";\n" +
    "export const limitsIdx = " + limitsIdx + ";\n" +
    "export const M = " + JSON.stringify(M) + ";\n";
}

if (process.argv[2] === "--emit" && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const graph = normalizeWit(readWit(join(repoRoot, "wit")));
  writeFileSync(join(repoRoot, "host/core/abi/descriptors.js"), descriptorSource(graph));
  writeFileSync(join(repoRoot, "host/core/abi/tables.js"), tableSource(graph));
}

