// Node-only loader of the probe's core module. The only place that may import `node:` modules.
import { readFileSync } from "node:fs";

export function compileProbeCore() {
  return new WebAssembly.Module(readFileSync(new URL("../../dist/js/probe/probe.core.wasm", import.meta.url)));
}
