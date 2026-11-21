import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

test("cold entries do not load the test-only probe", () => {
  for (const entry of ["@sentropic/bash", "@sentropic/bash/browser"]) {
    const js = "const m = await import(process.argv[1]); if ('pipelineProbe' in m) process.exit(1);";
    const trace = spawnSync(process.execPath, ["--input-type=module", "-e", js, entry], {
      encoding: "utf8", env: { ...process.env, NODE_DEBUG: "esm" },
      stdio: ["ignore", "ignore", "pipe"],
    });
    assert.equal(trace.status, 0, trace.stderr);
    assert.match(trace.stderr, /dist\/js\/kernel\/kernel\.js/);
    assert.doesNotMatch(trace.stderr, /\/probe\/probe\.js|browser-probe\.js/);
  }
});

test("require of the package root", () => {
  const m = createRequire(import.meta.url)("@sentropic/bash");
  assert.equal(typeof m.Bash, "function");
  assert.equal("pipelineProbe" in m, false);
});

test("browser entry executes on a precompiled module", async () => {
  const m = await import("@sentropic/bash/browser");
  const mod = new WebAssembly.Module(readFileSync(new URL("../dist/js/kernel/kernel.core.wasm", import.meta.url)));
  await m.Bash.precompile(mod);
  assert.equal(new m.Bash().execSync("echo hi").stdout, "hi\n");
});
