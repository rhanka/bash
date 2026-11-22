// Node entry: the shell facade.
import { nodeLoader } from "./node/load.js";
import { createFacade, defineCommand, getCommandNames, getJavaScriptCommandNames, getNetworkCommandNames, getPythonCommandNames } from "./core/facade.js";
import { AsyncCapabilityError, ExecutionAbortedError, KernelPoisonedError, NotSupportedError } from "./core/errors.js";
import * as bytes from "./core/bytes.js";

const { Bash, InMemoryFs } = createFacade(nodeLoader);

export { Bash, InMemoryFs, defineCommand, getCommandNames, getNetworkCommandNames, getPythonCommandNames, getJavaScriptCommandNames };
export { AsyncCapabilityError, ExecutionAbortedError, KernelPoisonedError, NotSupportedError };
export { OverlayFs } from "./node/overlay-fs.js";
export { ReadWriteFs } from "./node/read-write-fs.js";
export const { EMPTY_BYTES, bytesOutput, decodeBytesToUtf8, encodeUtf8ToBytes, latin1FromBytes, stdoutAsBytes, stdoutKind, textOutput, unsafeBytesFromLatin1 } = bytes;
