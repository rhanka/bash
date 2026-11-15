import type { PipelineProbe } from "./probe.js";
export type { PipelineProbe };
export * from "./core.js";
export function pipelineProbe(coreModule: WebAssembly.Module): PipelineProbe;
