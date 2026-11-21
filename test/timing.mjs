import assert from "node:assert/strict";

export function assertUnderTime(label, elapsedMs, localLimitMs, ciFactor = 3) {
  // Shared CI runners can lose wall time to scheduling contention. Keep the spec limit locally.
  const limitMs = process.env.CI === "true" ? localLimitMs * ciFactor : localLimitMs;
  const measurement = `${label}: ${elapsedMs.toFixed(2)} ms (limit ${limitMs} ms)`;
  console.log(measurement);
  assert.ok(elapsedMs < limitMs, measurement);
}
