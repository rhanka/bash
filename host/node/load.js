// Node loader: reads the core module from disk and compiles it synchronously, once per process.
// The only place of the core path that imports `node:` modules.
import { readFileSync } from "node:fs";
import { addHostPathHints } from "../core/driver.js";
import { fileURLToPath } from "node:url";

const CORE = new URL("../../dist/js/kernel/kernel.core.wasm", import.meta.url);

let compiled = null;

addHostPathHints(process.cwd(), fileURLToPath(new URL("../..", import.meta.url)).replace(/\/$/, ""));

/** Compiles the kernel core module (cached for the process). */
export function nodeKernelModule() {
  compiled ??= new WebAssembly.Module(readFileSync(CORE));
  return compiled;
}

/** The loader object of the facade for Node. */
export const nodeLoader = {
  syncModule: nodeKernelModule,
  asyncModule: () => Promise.resolve(nodeKernelModule()),
  precompile: () => {
    nodeKernelModule();
    return Promise.resolve();
  },
};
