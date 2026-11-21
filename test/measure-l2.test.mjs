import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { measureWarm, packedJsFiles, stagePackage } from "../tools/measure-l2.mjs";
import { scratch } from "./helpers.mjs";

test("each warm process collects discarded warmup objects before sampling its workload", () => {
  const fixture = `
    export class Bash {
      constructor({ customCommands } = {}) {
        this.custom = !!customCommands;
        hits.constructors++;
        registry.register(this, null);
      }
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
    let finalized = 0;
    globalThis.registry = new FinalizationRegistry(() => finalized++);
    performance.now = () => {
      hits.finalizedAtStart ??= finalized;
      return tick += 2;
    };
    const collect = globalThis.gc;
    globalThis.gc = () => {
      hits.collections = (hits.collections ?? 0) + 1;
      hits.beforeSamples = [hits.constructors, hits.async, hits.sync, hits.custom, tick];
      collect();
    };
  `;
  const entry = JSON.stringify("data:text/javascript," + encodeURIComponent(fixture));
  const warm = measureWarm(entry, (source, flags = []) => {
    const path = scratch("warm-l2");
    const log = join(path, "counts.json");
    const hook = `process.on("exit", () => writeFileSync(${JSON.stringify(log)}, JSON.stringify(hits)));`;
    const code = 'import { writeFileSync } from "node:fs";' + setup + hook + source;
    const result = execFileSync(process.execPath, [...flags, "--input-type=module", "--eval", code], {
      encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    });
    hits.push(JSON.parse(readFileSync(log, "utf8")));
    return result;
  });
  assert.deepEqual(Object.keys(warm), ["M2", "M3a", "M3aSync", "M_host"]);
  assert.ok(Object.values(warm).every((samples) => samples.length === 1000));
  assert.deepEqual(hits, [
    { constructors: 1020, async: 0, sync: 0, custom: 0, collections: 1, beforeSamples: [20, 0, 0, 0, 0], finalizedAtStart: 20 },
    { constructors: 1, async: 1020, sync: 0, custom: 0, collections: 1, beforeSamples: [1, 20, 0, 0, 0], finalizedAtStart: 0 },
    { constructors: 1, async: 0, sync: 1020, custom: 0, collections: 1, beforeSamples: [1, 0, 20, 0, 0], finalizedAtStart: 0 },
    { constructors: 1, async: 0, sync: 0, custom: 1020, collections: 1, beforeSamples: [1, 0, 0, 20, 0], finalizedAtStart: 0 },
  ]);
});

test("staging a variant preserves packed JavaScript and excludes unpacked files", () => {
  const source = scratch("pack-l2-source");
  const tree = scratch("pack-l2-variant");
  for (const dir of ["dist/js/kernel", "host", "types", "wit"]) {
    mkdirSync(join(source, dir), { recursive: true });
  }
  writeFileSync(join(source, "package.json"), JSON.stringify({
    name: "l2-variant", version: "0.0.0", files: ["dist/js/", "host/index.js", "entry.js"],
  }));
  for (const path of ["LICENSE", "DIVERGENCES.md", "dist/kernel.component.wasm", "dist/kernel.core.wasm",
    "dist/js/kernel/commands.js", "entry.js", "host/index.js", "host/unpacked.js"]) {
    writeFileSync(join(source, path), path);
  }
  const env = { ...process.env, npm_config_cache: join(source, "cache"), TMPDIR: source };
  stagePackage(source, tree, env);
  assert.deepEqual(packedJsFiles(tree, env), packedJsFiles(source, env));
  assert.equal(readFileSync(join(tree, "entry.js"), "utf8"), "entry.js");
  assert.equal(existsSync(join(tree, "host/unpacked.js")), false);
});

test("package sizes take their JavaScript files from the variant tree", () => {
  const tree = scratch("pack-l2");
  mkdirSync(join(tree, "dist/js/kernel"), { recursive: true });
  mkdirSync(join(tree, "dist/test"), { recursive: true });
  writeFileSync(join(tree, "package.json"), JSON.stringify({
    name: "l2-variant", version: "0.0.0", files: ["dist/js/", "entry.js"],
  }));
  writeFileSync(join(tree, "dist/js/kernel/variant-only.js"), "export {}");
  writeFileSync(join(tree, "entry.js"), "export {}");
  writeFileSync(join(tree, "dist/test/test-kernel.js"), "export {}");
  assert.deepEqual(packedJsFiles(tree, { ...process.env, npm_config_cache: join(tree, "cache"), TMPDIR: tree }),
    ["dist/js/kernel/variant-only.js", "entry.js"]);
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
