// Node entry: the facade and the pipeline probe.
import { pipelineProbe as instantiateProbe } from "./browser-probe.js";
import { compileProbeCore } from "./node/probe.js";
import { nodeLoader } from "./node/load.js";
import { createFacade, defineCommand, getCommandNames, getJavaScriptCommandNames, getNetworkCommandNames, getPythonCommandNames } from "./core/facade.js";
import { AsyncCapabilityError, ExecutionAbortedError, KernelPoisonedError, NotSupportedError } from "./core/errors.js";
import * as bytes from "./core/bytes.js";

const { Bash, InMemoryFs } = createFacade(nodeLoader);

export { Bash, InMemoryFs, defineCommand, getCommandNames, getNetworkCommandNames, getPythonCommandNames, getJavaScriptCommandNames };
export { AsyncCapabilityError, ExecutionAbortedError, KernelPoisonedError, NotSupportedError };
export const { EMPTY_BYTES, bytesOutput, decodeBytesToUtf8, encodeUtf8ToBytes, latin1FromBytes, stdoutAsBytes, stdoutKind, textOutput, unsafeBytesFromLatin1 } = bytes;

/** Loads the pipeline probe synchronously and returns its exports. */
export function pipelineProbe() {
  return instantiateProbe(compileProbeCore());
}
