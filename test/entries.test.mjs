import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { statSync } from "node:fs";

test("cold entries leave the probe unloaded and use minified kernel glue", () => {
  for (const entry of ["@sentropic/bash", "@sentropic/bash/browser"]) {
    const js = "const m = await import(process.argv[1]); if (typeof m.pipelineProbe !== 'function') process.exit(1);";
    const trace = spawnSync(process.execPath, ["--input-type=module", "-e", js, entry], {
      encoding: "utf8", env: { ...process.env, NODE_DEBUG: "esm" },
      stdio: ["ignore", "ignore", "pipe"],
    });
    assert.equal(trace.status, 0, trace.stderr);
    assert.match(trace.stderr, /dist\/js\/kernel\/kernel\.js/);
    assert.doesNotMatch(trace.stderr, /browser-probe\.js|dist\/js\/probe\/probe\.js/);
  }
  assert.ok(statSync(new URL("../dist/js/kernel/kernel.js", import.meta.url)).size < 350_000);
});

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
  assert.equal((await m.pipelineProbe(mod)).ping(1), 2);
});
