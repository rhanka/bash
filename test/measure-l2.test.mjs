import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { measureWarm, packedJsFiles } from "../tools/measure-l2.mjs";
import { scratch } from "./helpers.mjs";

test("each warm workload starts its own process with only its own shell setup", () => {
  const fixture = `
    export class Bash {
      constructor({ customCommands } = {}) { this.custom = !!customCommands; hits.constructors++; }
      execSync() { hits.sync++; }
      async exec(command) {
        hits[this.custom ? "custom" : "async"]++;
        if (command === "noop") return { stdout: "", stderr: "", exitCode: 0 };
      }
    }
    export const defineCommand = () => ({});
  `;
  const hits = [];
  const setup = `
    globalThis.hits = { constructors: 0, async: 0, sync: 0, custom: 0 };
    let tick = 0;
    performance.now = () => tick += 2;
  `;
  const entry = JSON.stringify("data:text/javascript," + encodeURIComponent(fixture));
  const warm = measureWarm(entry, (source) => {
    const path = scratch("warm-l2");
    const log = join(path, "counts.json");
    const hook = `process.on("exit", () => writeFileSync(${JSON.stringify(log)}, JSON.stringify(hits)));`;
    const code = 'import { writeFileSync } from "node:fs";' + setup + hook + source;
    const result = execFileSync(process.execPath, ["--input-type=module", "--eval", code], {
      encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    });
    hits.push(JSON.parse(readFileSync(log, "utf8")));
    return result;
  });
  assert.deepEqual(Object.keys(warm), ["M2", "M3a", "M3aSync", "M_host"]);
  assert.ok(Object.values(warm).every((samples) => samples.length === 1000));
  assert.deepEqual(hits, [
    { constructors: 1020, async: 0, sync: 0, custom: 0 },
    { constructors: 1, async: 1020, sync: 0, custom: 0 },
    { constructors: 1, async: 0, sync: 1020, custom: 0 },
    { constructors: 1, async: 0, sync: 0, custom: 1021 },
  ]);
});

test("package sizes take their JavaScript files from the variant tree", () => {
  const tree = scratch("pack-l2");
  mkdirSync(join(tree, "dist/js/kernel"), { recursive: true });
  writeFileSync(join(tree, "package.json"), JSON.stringify({ name: "l2-variant", version: "0.0.0", files: ["dist/js/"] }));
  writeFileSync(join(tree, "dist/js/kernel/variant-only.js"), "export {}");
  assert.deepEqual(packedJsFiles(tree, { ...process.env, npm_config_cache: join(tree, "cache"), TMPDIR: tree }),
    ["dist/js/kernel/variant-only.js"]);
});

test("the wasm-opt request records that Binaryen optimization was not measured", () => {
  const script = new URL("../tools/measure-l2.mjs", import.meta.url);
  const result = execFileSync(process.execPath, [script.pathname, "--variant", "wasm-opt"], { encoding: "utf8" });
  const expected = {
    variant: "wasm-opt",
    status: "not measured",
    reason: "Binaryen is not pinned for offline core optimization",
  };
  assert.deepEqual(JSON.parse(result), expected);
  assert.deepEqual(JSON.parse(readFileSync(new URL("../tmp/bench-l2/wasm-opt/results.json", import.meta.url))), expected);
});
