// Browser entry: no `node:` import, directly or transitively. The kernel module is compiled by
// `Bash.precompile(source)`, or at the first awaited call from the default URL next to this file.
import { createFacade, defineCommand, getCommandNames, getJavaScriptCommandNames, getNetworkCommandNames, getPythonCommandNames } from "./core/facade.js";
import { AsyncCapabilityError, ExecutionAbortedError, KernelPoisonedError, NotSupportedError } from "./core/errors.js";
import * as bytes from "./core/bytes.js";

const DEFAULT_URL = new URL("../dist/js/kernel/kernel.core.wasm", import.meta.url);

let compiled = null;
let compiling = null;

async function compileFrom(source) {
  if (source instanceof WebAssembly.Module) return source;
  if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) return WebAssembly.compile(source);
  const r = await (source === undefined ? fetch(DEFAULT_URL) : source instanceof URL || typeof source === "string" ? fetch(source) : source);
  const mime = r.headers?.get("Content-Type");
  if (mime === "application/wasm" && typeof WebAssembly.compileStreaming === "function") return WebAssembly.compileStreaming(r);
  return WebAssembly.compile(await r.arrayBuffer());
}

const loader = {
  syncModule: () => compiled,
  asyncModule: () => loader.precompile().then(() => compiled),
  precompile(source) {
    if (compiled) return Promise.resolve();
    compiling ??= compileFrom(source).then(
      (m) => { compiled = m; },
      (error) => {
        compiling = null;
        throw error;
      },
    );
    return compiling;
  },
};

const { Bash, InMemoryFs } = createFacade(loader);

export { Bash, InMemoryFs, defineCommand, getCommandNames, getNetworkCommandNames, getPythonCommandNames, getJavaScriptCommandNames };
export { AsyncCapabilityError, ExecutionAbortedError, KernelPoisonedError, NotSupportedError };
export const { EMPTY_BYTES, bytesOutput, decodeBytesToUtf8, encodeUtf8ToBytes, latin1FromBytes, stdoutAsBytes, stdoutKind, textOutput, unsafeBytesFromLatin1 } = bytes;

/** Loads and instantiates the pipeline probe on demand. */
export async function pipelineProbe(coreModule) {
  const { pipelineProbe: instantiateProbe } = await import("./browser-probe.js");
  return instantiateProbe(coreModule);
}
