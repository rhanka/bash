import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

test("cold entries do not load the test-only probe", () => {
  for (const entry of ["@sentropic/bash", "@sentropic/bash/browser"]) {
    const js = `const m = await import(process.argv[1]); if ('pipelineProbe' in m) process.exit(1);`;
    const trace = spawnSync(process.execPath, ["--input-type=module", "-e", js, entry], {
      encoding: "utf8", env: { ...process.env, NODE_DEBUG: "esm" },
      stdio: ["ignore", "ignore", "pipe"],
    });
    assert.equal(trace.status, 0, trace.stderr);
    // The packed entries resolve to the built minified graph, never to host sources.
    assert.match(trace.stderr, /dist\/js\/shared\.js/);
    assert.match(trace.stderr, entry.endsWith("browser") ? /dist\/js\/browser\.js/ : /dist\/js\/index\.js/);
    assert.doesNotMatch(trace.stderr, /\/host\//);
    assert.doesNotMatch(trace.stderr, /\/probe\/probe\.js|browser-probe\.js/);
    assert.doesNotMatch(trace.stderr, /dist\/js\/core\/ext\.js/);
    if (entry.endsWith("browser")) {
      assert.doesNotMatch(trace.stderr, /dist\/js\/node\/load\.js/);
      assert.doesNotMatch(trace.stderr, /dist\/js\/node\/adapters\.js/);
    }
  }
});

test("cold new Bash() and first echo do not load lazy extension or node-fs-only builtins", () => {
  const js = `
    const seen = [];
    const orig = process.getBuiltinModule;
    process.getBuiltinModule = (name) => { seen.push(name); return orig.call(process, name); };
    const { Bash } = await import("@sentropic/bash");
    const r = await new Bash().exec("echo hi");
    if (r.stdout !== "hi\\n") process.exit(2);
    console.log(JSON.stringify(seen));
  `;
  const r = spawnSync(process.execPath, ["--input-type=module", "-e", js], {
    encoding: "utf8", env: { ...process.env, NODE_DEBUG: "esm" },
  });
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(JSON.parse(r.stdout), ["node:fs", "node:url"]);
  assert.doesNotMatch(r.stderr, /dist\/js\/core\/ext\.js/);
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
