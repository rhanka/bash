// Node loader: reads the core module from disk and compiles it synchronously, once per process.
// The only place of the core path that uses Node built-ins. They are loaded synchronously and
// lazily through `process.getBuiltinModule` (a deferred lookup, not a static import), so the
// first Wasm read happens on construction and bundlers keep this module separate.
import { addHostPathHints } from "../core/driver.js";

const CORE = new URL("../../dist/js/kernel/kernel.core.wasm", import.meta.url);

let compiled = null;
let fsMod = null;
let urlMod = null;

/** Synchronously resolves the Node built-ins for the first construction. */
function nodeBuiltins() {
  const proc = globalThis.process;
  const getBuiltin = proc?.getBuiltinModule;
  if (typeof getBuiltin !== "function") throw new Error("the Node loader needs process.getBuiltinModule");
  fsMod ??= getBuiltin.call(proc, "node:fs");
  urlMod ??= getBuiltin.call(proc, "node:url");
  return { fs: fsMod, url: urlMod };
}

/** Compiles the kernel core module (cached for the process). */
export function nodeKernelModule() {
  if (!compiled) {
    const builtins = nodeBuiltins();
    addHostPathHints(globalThis.process.cwd(), builtins.url.fileURLToPath(new URL("../..", import.meta.url)).replace(/\/$/, ""));
    compiled = new WebAssembly.Module(builtins.fs.readFileSync(CORE));
  }
  return compiled;
}

/** The loader object of the facade for Node: the package's own production loader core. */
export const nodeLoader = {
  syncModule: nodeKernelModule,
  asyncModule: () => Promise.resolve(nodeKernelModule()),
  precompile: () => {
    nodeKernelModule();
    return Promise.resolve();
  },
  defaultBinding: true,
};
