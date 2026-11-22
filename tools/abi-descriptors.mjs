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

if (process.argv[2] === "--emit" && import.meta.url === pathToFileURL(process.argv[1]).href) {
  writeFileSync(join(repoRoot, "host/core/abi/descriptors.js"),
    descriptorSource(normalizeWit(readWit(join(repoRoot, "wit")))));
}

