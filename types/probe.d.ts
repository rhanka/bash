export interface PipelineProbe {
  ping(x: number): number;
  echoBytes(data: Uint8Array): Uint8Array;
  grow(pages: number): number;
}
