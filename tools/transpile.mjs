#!/usr/bin/env node
// Generates dependency-free JavaScript bindings for a component with jco-transpile.
// Usage: node tools/transpile.mjs <component.wasm> <out-dir>
import { mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { toolsDir } from "./lib.mjs";

export async function transpileComponent(componentPath, outDir) {
  const entry = join(toolsDir(), "npm", "node_modules", "@bytecodealliance", "jco-transpile", "dist", "index.js");
  const { transpile } = await import(pathToFileURL(entry).href);
  const name = basename(componentPath).replace(/\.component\.wasm$/, "").replace(/\.wasm$/, "");
  const result = await transpile(resolve(componentPath), {
    name,
    instantiation: "sync",
    base64Cutoff: 0,
    validLiftingOptimization: true,
    tlaCompat: false,
    minify: false,
    wasiShim: false,
    nodejsCompat: false,
    optimize: false,
  });
  if (result.imports.length !== 0) throw new Error(`component has imports: ${result.imports.join(", ")}`);
  rmSync(outDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  for (const [file, bytes] of Object.entries(result.files).sort(([a], [b]) => (a < b ? -1 : 1))) {
    const dest = join(outDir, file);
    mkdirSync(join(dest, ".."), { recursive: true });
    writeFileSync(dest, bytes);
  }
  return readdirSync(outDir).sort();
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [component, out] = process.argv.slice(2);
  if (!component || !out) {
    console.error("usage: node tools/transpile.mjs <component.wasm> <out-dir>");
    process.exit(2);
  }
  const files = await transpileComponent(component, out);
  console.log(`transpiled ${component} -> ${out}: ${files.join(", ")}`);
}
