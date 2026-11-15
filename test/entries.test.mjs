import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

test("import of the package root", async () => {
  const m = await import("@sentropic/bash");
  assert.equal(typeof m.pipelineProbe, "function");
});

test("require of the package root", () => {
  const m = createRequire(import.meta.url)("@sentropic/bash");
  assert.equal(typeof m.pipelineProbe, "function");
});

test("import of the browser entry, with a compiled module", async () => {
  const m = await import("@sentropic/bash/browser");
  assert.equal(typeof m.pipelineProbe, "function");
  const { readFileSync } = await import("node:fs");
  const mod = new WebAssembly.Module(readFileSync(new URL("../dist/js/probe/probe.core.wasm", import.meta.url)));
  assert.equal(m.pipelineProbe(mod).ping(1), 2);
});
