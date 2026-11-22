import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { decideGateL2, measureWarm, packageSplit, packedJsFiles, stagePackage } from "../tools/measure-l2.mjs";
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
        if (command === "echo hi") return { stdout: "hi\\n", stderr: "", exitCode: 0 };
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
  assert.deepEqual(Object.keys(warm), ["M2", "M_paired", "M3a", "M3aSync", "M_host"]);
  assert.ok(Object.values(warm).every((samples) => samples.length === 1000));
  assert.deepEqual(hits, [
    { constructors: 1020, async: 0, sync: 0, custom: 0, collections: 1, beforeSamples: [20, 0, 0, 0, 0], finalizedAtStart: 20 },
    { constructors: 1020, async: 1020, sync: 0, custom: 0, collections: 1, beforeSamples: [20, 20, 0, 0, 0], finalizedAtStart: 20 },
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

test("packageSplit classifies dynamic literal import() chunks under B_total and excludes them from N and cold path", () => {
  const tree = scratch("split-l2");
  for (const d of ["dist/js/core", "dist/js/node"]) mkdirSync(join(tree, d), { recursive: true });
  writeFileSync(join(tree, "package.json"), JSON.stringify({
    name: "l2-split", version: "0.0.0", files: ["dist/js/"],
  }));
  writeFileSync(join(tree, "dist/js/shared.js"), `export const f = () => import("./core/ext.js");\n`);
  writeFileSync(join(tree, "dist/js/browser.js"), `export { f } from "./shared.js";\n`);
  writeFileSync(join(tree, "dist/js/core/ext.js"), `export const x = 1;\n`);
  writeFileSync(join(tree, "dist/js/node/load.js"), `export const l = 1;\n`);
  writeFileSync(join(tree, "dist/js/node/adapters.js"), `export const a = 1;\n`);
  writeFileSync(join(tree, "dist/js/index.js"), `export { f } from "./shared.js"; export { l } from "./node/load.js"; export { a } from "./node/adapters.js";\n`);
  const env = { ...process.env, npm_config_cache: join(tree, "cache"), TMPDIR: tree };
  const s = packageSplit(tree, env);
  assert.deepEqual(s.browserColdFiles, ["dist/js/browser.js", "dist/js/shared.js"]);
  assert.deepEqual(s.coldFiles, ["dist/js/browser.js", "dist/js/index.js", "dist/js/node/load.js", "dist/js/shared.js"]);
  assert.deepEqual(s.browserFiles, ["dist/js/browser.js", "dist/js/core/ext.js", "dist/js/shared.js"]);
  assert.deepEqual(s.nodeFiles, ["dist/js/index.js", "dist/js/node/adapters.js", "dist/js/node/load.js"]);
  assert.equal(s.B_cold, s.perFile["dist/js/browser.js"].raw + s.perFile["dist/js/shared.js"].raw);
  assert.equal(s.B_total, s.B_cold + s.perFile["dist/js/core/ext.js"].raw);
  assert.equal(s.B_total + s.N, s.T);
  assert.equal(s.Z_cold, s.coldFiles.reduce((acc, p) => acc + s.perFile[p].gzip, 0));
  assert.equal(s.Z_total, Object.values(s.perFile).reduce((acc, v) => acc + v.gzip, 0));
});

test("the verdict function decides each bound at equality and just over", () => {
  // Owner decision of 22/11: cold path (B_cold, Z_cold) split from total shipped (B_total, Z_total, T).
  const passing = {
    m1: 25.0, m2: 0.20, m3a: 0.07, m5: 10000000, wasmRaw: 1500000, wasmGzip: 700000,
    bCold: 58000, zCold: 30000, bTotal: 75000, zTotal: 50000, t: 110000, suitesPass: true,
  };
  assert.equal(decideGateL2(passing).verdict, "go");
  // Just over each kernel bound is a kernel miss; just over is measured unrounded.
  for (const [key, over] of [["m1", 25.000001], ["m2", 0.200001], ["m3a", 0.070001], ["m5", 10000001],
    ["wasmRaw", 1500001], ["wasmGzip", 700001]]) {
    const miss = decideGateL2({ ...passing, [key]: over });
    assert.equal(miss.verdict, "no-go", key);
    assert.equal(miss.reason, "kernel miss");
  }
  // Just over each of the five JS bounds is a JS-only miss while the kernel passes.
  for (const [key, over] of [["bCold", 58001], ["zCold", 30001], ["bTotal", 75001], ["zTotal", 50001], ["t", 110001]]) {
    const miss = decideGateL2({ ...passing, [key]: over });
    assert.equal(miss.verdict, "go-kernel-only", key);
    assert.equal(miss.reason, "JS-only miss");
    assert.equal(miss.js[key], false);
  }
  // A simultaneous kernel and JS miss is still a kernel miss.
  assert.equal(decideGateL2({ ...passing, m1: 30, bCold: 60000 }).verdict, "no-go");
  // A red correctness suite blocks the verdict whatever the numbers say.
  assert.equal(decideGateL2({ ...passing, suitesPass: false }).verdict, "no-go");
});

test("the verdict function passes all kernel and JS bounds on the measured numbers", (t) => {
  // The benchmark runs separately on the green release artifacts; without its output there is
  // nothing to tie to the report, so this test skips instead of failing.
  const results = new URL("../tmp/bench-l2/committed/results.json", import.meta.url);
  if (!existsSync(results)) {
    t.skip("no committed benchmark run yet");
    return;
  }
  const measured = JSON.parse(readFileSync(results));
  const { summary, split, sizes } = measured;
  const f = (v) => typeof v === "number" ? v : v.median;
  const out = decideGateL2({
    m1: f(summary.M1), m2: f(summary.M2), m3a: f(summary.M3a), m5: summary.M5,
    wasmRaw: sizes.wasm.loader.raw, wasmGzip: sizes.wasm.loader.gzip,
    bCold: split.B_cold, zCold: split.Z_cold, bTotal: split.B_total, zTotal: split.Z_total, t: split.T,
    suitesPass: true,
  });
  assert.equal(out.kernel.m1, true);
  assert.equal(out.kernel.m2, true);
  assert.equal(out.kernel.m3a, true);
  assert.equal(out.kernel.m5, true);
  assert.equal(out.kernel.wasm, true);
  assert.equal(out.js.bCold, true);
  assert.equal(out.js.zCold, true);
  assert.equal(out.js.bTotal, true);
  assert.equal(out.js.zTotal, true);
  assert.equal(out.js.t, true);
  assert.equal(out.verdict, "go");
  assert.equal(out.reason, "all thresholds pass");
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
