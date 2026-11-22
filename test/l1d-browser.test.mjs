import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { Bash as BrowserBash, defineCommand as defineBrowserCommand } from "../host/browser.js";
import { Bash as NodeBash } from "../host/index.js";

test("browser entry filters kernel cmd:flag:* hits while node entry preserves them", async () => {
  const wasmBytes = readFileSync(new URL("../dist/js/kernel/kernel.core.wasm", import.meta.url));
  await BrowserBash.precompile(wasmBytes);

  const browserHits = [];
  const browserBash = new BrowserBash({ coverage: { hit: (f) => browserHits.push(f) } });
  const br = await browserBash.exec("echo -n -e hi");
  assert.equal(br.exitCode, 0);
  assert.deepEqual(browserHits, [
    "bash:cmd:SimpleCommand",
    "bash:expansion:word_glob",
    "bash:expansion:word_glob",
    "bash:expansion:word_glob",
    "bash:builtin:echo",
  ]);

  const nodeHits = [];
  const nodeBash = new NodeBash({ coverage: { hit: (f) => nodeHits.push(f) } });
  const nr = await nodeBash.exec("echo -n -e hi");
  assert.equal(nr.exitCode, 0);
  assert.deepEqual(nodeHits, [
    "bash:cmd:SimpleCommand",
    "bash:expansion:word_glob",
    "bash:expansion:word_glob",
    "bash:expansion:word_glob",
    "bash:builtin:echo",
    "cmd:flag:echo:-n",
    "cmd:flag:echo:-e",
  ]);

  const customHits = [];
  const custom = defineBrowserCommand("custom", (_args, ctx) => {
    ctx.coverage.hit("cmd:flag:custom:-x");
    return { stdout: "", stderr: "", exitCode: 0 };
  });
  const browserCustom = new BrowserBash({
    customCommands: [custom],
    coverage: { hit: (f) => customHits.push(f) },
  });
  assert.equal((await browserCustom.exec("custom")).exitCode, 0);
  assert.ok(customHits.includes("cmd:flag:custom:-x"));
});
