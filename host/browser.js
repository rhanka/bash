// Browser entry (placeholder until L1b). Imports nothing from `node:`.
import { instantiate } from "../dist/js/probe/probe.js";

/** Instantiates the pipeline probe from a compiled core module and returns its exports. */
export function pipelineProbe(coreModule) {
  return instantiate(() => coreModule, {});
}
