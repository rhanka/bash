// Node entry (placeholder until L1b).
import { pipelineProbe as instantiateProbe } from "./browser.js";
import { compileProbeCore } from "./node/probe.js";

/** Loads the pipeline probe synchronously and returns its exports. */
export function pipelineProbe() {
  return instantiateProbe(compileProbeCore());
}
