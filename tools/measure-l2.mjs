#!/usr/bin/env node
// Offline Gate L2 benchmark (spec/lots/L2.md, section 9).
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { cpus, release, type } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";
import { repoRoot, toolBin, toolsDir } from "./lib.mjs";
import { stripComments } from "./gates.mjs";
import { transpileComponent } from "./transpile.mjs";

const HOST_SETUP =
  'const host = new Bash({ customCommands: [defineCommand("noop", () => ({ stdout: "", stderr: "", exitCode: 0 }))] });';

export function measureWarm(entry, node) {
  const workloads = {
    M2: ["", "new Bash()"],
    M3a: ["const bash = new Bash();", 'await bash.exec("echo hi")'],
    M3aSync: ["const bash = new Bash();", 'bash.execSync("echo hi")'],
    M_host: [HOST_SETUP, 'await host.exec("noop")'],
  };
  return Object.fromEntries(Object.entries(workloads).map(([name, [setup, op]]) => {
    const source = [
      'const { Bash, defineCommand } = await import(' + entry + ');',
      setup,
      'await (async function warmup() {',
      '  for (let i = 0; i < 20; i++) { ' + op + '; }',
      '})();',
      'globalThis.gc();',
      // Let V8 run the finalizers queued by the warmup collection before timing.
      'await new Promise(setImmediate);',
      'const samples = [];',
      'const start = performance.now();',
      'while (samples.length < 1000 || performance.now() - start < 2000) {',
      '  const from = performance.now();',
      '  ' + op + ';',
      '  samples.push(performance.now() - from);',
      '}',
      'console.log(JSON.stringify(samples));',
    ].join("\n");
    return [name, JSON.parse(node(source, ["--expose-gc"]))];
  }));
}

function shuffled(kinds, state) {
  for (let i = kinds.length - 1; i > 0; i--) {
    state.seed = (Math.imul(1664525, state.seed) + 1013904223) >>> 0;
    const j = state.seed % (i + 1);
    [kinds[i], kinds[j]] = [kinds[j], kinds[i]];
  }
  return kinds;
}

function distribution(values) {
  const sorted = values.slice().sort((a, b) => a - b);
  const mid = (sorted.length - 1) / 2;
  return {
    median: (sorted[Math.floor(mid)] + sorted[Math.ceil(mid)]) / 2,
    p95: sorted[Math.ceil(sorted.length * 0.95) - 1],
    min: sorted[0],
    count: values.length,
  };
}

function runSeries(seed, coldCode, entry, node) {
  const random = { seed };
  const cold = [];
  for (let i = 0; i < 30; i++) {
    const order = shuffled(Object.keys(coldCode), random);
    const samples = Object.fromEntries(order.map((kind) => [kind, JSON.parse(node(coldCode[kind]))]));
    cold.push({ order, samples });
  }
  const warm = measureWarm(entry, node);
  const full = (field) => distribution(cold.map(({ samples }) => samples["cold-full"][field]));
  const rss = {
    full: full("rss").median,
    baseline: distribution(cold.map(({ samples }) => samples.baseline.rss)).median,
  };
  const metrics = {
    M1: full("tCold"), tImport: full("tImport"), tNew: full("tNew"), tExec: full("tExec"),
    coldImport: distribution(cold.map(({ samples }) => samples["cold-import"].tImport)),
    M5: rss.full - rss.baseline,
    ...Object.fromEntries(Object.entries(warm).map(([name, samples]) => [name, distribution(samples)])),
  };
  return { seed, cold, warm, rss, metrics };
}

function browserImports(tree, entry) {
  const reached = new Set();
  const visit = (file) => {
    const path = relative(tree, file);
    if (reached.has(path)) return;
    reached.add(path);
    const src = stripComments(readFileSync(file, "utf8"));
    // Matches readable and minified static imports and re-exports with a literal specifier.
    const pattern = /(?:^|[;\n}])\s*(?:export\s[^;]*?\sfrom\s*|import\s*[^;]*?from\s*)["'](\.[^"']+\.js)["']/g;
    for (const match of src.matchAll(pattern)) {
      visit(resolve(dirname(file), match[1]));
    }
  };
  visit(entry);
  return reached;
}

function size(tree, path) {
  const bytes = readFileSync(join(tree, path));
  return { raw: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
}

function packageFiles(tree, env) {
  const pack = execFileSync("npm", ["pack", "--offline", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: tree, env, encoding: "utf8",
  });
  return JSON.parse(pack)[0].files.map(({ path }) => path);
}

export function packedJsFiles(tree, env) {
  return packageFiles(tree, env).filter((path) => path.endsWith(".js"));
}

export function stagePackage(source, tree, env) {
  for (const path of packageFiles(source, env)) {
    cpSync(join(source, path), join(tree, path));
  }
}

function sizes(tree, env) {
  const files = packedJsFiles(tree, env);
  const entry = existsSync(join(tree, "dist", "js", "browser.js"))
    ? join(tree, "dist", "js", "browser.js")
    : join(tree, "host", "browser.js");
  const browser = browserImports(tree, entry);
  const groups = Object.fromEntries(["glue", "registry", "coreEntry", "nodeOnly"].map((name) => [name, {}]));
  for (const path of files) {
    const name = /^dist\/js\/kernel\/kernel(?:\.shims)?\.js$/.test(path) ? "glue" :
      path === "dist/js/kernel/commands.js" ? "registry" : browser.has(path) ? "coreEntry" : "nodeOnly";
    groups[name][path] = size(tree, path);
  }
  const sum = (values) => values.reduce((total, item) => ({
    raw: total.raw + item.raw, gzip: total.gzip + item.gzip,
  }), { raw: 0, gzip: 0 });
  const groupSizes = Object.fromEntries(Object.entries(groups).map(([name, files]) =>
    [name, { files, ...sum(Object.values(files)) }]));
  // The linked core ships only in the repository tree; installed trees carry the same
  // bytes as their loader core.
  const corePath = existsSync(join(tree, "dist", "kernel.core.wasm"))
    ? "dist/kernel.core.wasm"
    : "dist/js/kernel/kernel.core.wasm";
  return {
    wasm: {
      loader: size(tree, "dist/js/kernel/kernel.core.wasm"),
      core: size(tree, corePath),
      component: size(tree, "dist/kernel.component.wasm"),
    },
    js: {
      groups: groupSizes,
      ...sum(Object.values(groupSizes)),
    },
  };
}

/**
 * The L2c package split (spec section 5): B is the raw bytes of the browser-reachable shipped
 * JS graph (shared code, glue and registry), N the raw bytes of all remaining shipped JS,
 * T = B + N, and Z the sum of per-file gzip level-9 lengths. Decimal bytes.
 */
export function packageSplit(tree, env) {
  const files = packedJsFiles(tree, env);
  // The browser-reachable graph is the built entry: prefer the packed layout, which is also
  // what the installed trees carry.
  const entry = existsSync(join(tree, "dist", "js", "browser.js"))
    ? join(tree, "dist", "js", "browser.js")
    : join(tree, "host", "browser.js");
  const browser = browserImports(tree, entry);
  const perFile = {};
  let B = 0, N = 0, Z = 0;
  const browserFiles = [];
  const nodeFiles = [];
  for (const path of files.sort()) {
    const s = perFile[path] = size(tree, path);
    Z += s.gzip;
    if (browser.has(path)) {
      B += s.raw;
      browserFiles.push(path);
    } else {
      N += s.raw;
      nodeFiles.push(path);
    }
  }
  return { B, N, T: B + N, Z, perFile, browserFiles, nodeFiles };
}

/**
 * The Gate L2 verdict function (spec section 6). Compares unrounded median-of-run medians against
 * the inclusive kernel thresholds and the fixed JS bounds, and reports the browser graph against
 * both candidate caps while the owner choice is pending. Returns `{ verdict, kernel, js }` where
 * verdict is one of "go", "go-kernel-only" or "no-go". A red correctness suite forces "no-go".
 */
export function decideGateL2({ m1, m2, m3a, m5, wasmRaw, wasmGzip, b, n, t, z, suitesPass }) {
  const kernel = {
    m1: m1 <= 25.0,
    m2: m2 <= 0.20,
    m3a: m3a <= 0.07,
    m5: m5 <= 10000000,
    wasm: wasmRaw <= 1500000 && wasmGzip <= 700000,
  };
  const js = {
    n: n <= 40000,
    t: t <= 60000,
    z: z <= 30000,
    b45: b <= 45000,
    b58: b <= 58000,
  };
  if (!suitesPass) return { verdict: "no-go", kernel, js, reason: "correctness suite is red" };
  if (!Object.values(kernel).every(Boolean)) return { verdict: "no-go", kernel, js, reason: "kernel miss" };
  if (!js.n || !js.t || !js.z || b > 45000) {
    return { verdict: "go-kernel-only", kernel, js, reason: "JS-only miss" };
  }
  return { verdict: "go", kernel, js, reason: "all thresholds pass" };
}

function packPackage(source, destDir, env) {
  const out = execFileSync("npm", ["pack", "--offline", "--json", "--ignore-scripts", "--pack-destination", destDir],
    { cwd: source, env, encoding: "utf8" });
  const info = JSON.parse(out)[0];
  return join(destDir, info.filename);
}

function installTarball(tarball, consumer, env) {
  mkdirSync(consumer, { recursive: true });
  writeFileSync(join(consumer, "package.json"),
    JSON.stringify({ name: "bench-consumer", version: "0.0.0", private: true, type: "module" }));
  execFileSync("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarball],
    { cwd: consumer, env, stdio: "pipe" });
}

/** Packs and installs one deliverable offline; returns the installed package root. */
function stageInstalled(source, dest, name, env) {
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  const tarball = packPackage(source, dest, env);
  const consumer = join(dest, "consumer");
  installTarball(tarball, consumer, env);
  return join(consumer, "node_modules", name);
}

function summarizeRuns(runs) {
  return Object.fromEntries(Object.entries(runs[0].metrics).map(([name, metric]) => {
    const median = (key) => distribution(runs.map(({ metrics }) => key ? metrics[name][key] : metrics[name])).median;
    return [name, typeof metric === "number" ? median() : { median: median("median"), p95: median("p95"), min: median("min") }];
  }));
}

function seriesFor(label, entry, node) {
  const coldCode = {
    baseline: 'console.log(JSON.stringify({ rss: process.memoryUsage().rss }));',
    "cold-import": [
      'const start = performance.now();',
      'await import(' + entry + ');',
      'console.log(JSON.stringify({ tImport: performance.now() - start }));',
    ].join("\n"),
    "cold-full": [
      'const start = performance.now();',
      'const { Bash } = await import(' + entry + ');',
      'const imported = performance.now();',
      'const bash = new Bash();',
      'const constructed = performance.now();',
      'await bash.exec("echo hi");',
      'const executed = performance.now();',
      'console.log(JSON.stringify({ tImport: imported - start, tNew: constructed - imported,',
      '  tExec: executed - constructed, tCold: executed - start, rss: process.memoryUsage().rss }));',
    ].join("\n"),
  };
  const assertion = [
    'const { Bash, defineCommand } = await import(' + entry + ');',
    'const { exitCode, stdout, stderr } = await new Bash().exec("echo hi");',
    'if (exitCode !== 0 || stdout !== "hi\\n" || stderr !== "") throw new Error("echo smoke test failed");',
    HOST_SETUP,
    'const result = await host.exec("noop");',
    'if (result.exitCode !== 0 || result.stdout !== "" || result.stderr !== "") throw new Error("noop smoke test failed");',
  ].join("\n");
  node(assertion);
  const runs = [];
  for (const seed of [0x6c320001, 0x6c320002, 0x6c320003]) {
    console.error(label + " repetition " + (runs.length + 1));
    runs.push(runSeries(seed, coldCode, entry, node));
  }
  return { summary: summarizeRuns(runs), runs };
}

async function main(args) {
  const [flag, value, ...extra] = args;
  const variant = value ?? "committed";
  if (extra.length || (flag !== undefined && (flag !== "--variant" || !value)) ||
      !["committed", "minified", "unminified", "wasm-opt", "jco"].includes(variant)) {
    throw new Error("usage: node tools/measure-l2.mjs [--variant committed|minified|unminified|wasm-opt|jco]");
  }

  const root = join(repoRoot, "tmp", "bench-l2");
  const dest = join(root, variant);
  const scratch = join(root, "tmp");
  mkdirSync(scratch, { recursive: true });
  mkdirSync(dest, { recursive: true });
  const env = {
    ...process.env, LC_ALL: "C", TZ: "UTC", TMPDIR: scratch,
    npm_config_cache: join(repoRoot, "tmp", "npmcache"), npm_config_offline: "true",
  };
  const wasmOpt = {
    status: "not measured",
    reason: "Binaryen is not pinned for offline core optimization",
  };
  if (variant === "wasm-opt") {
    const result = { variant, ...wasmOpt };
    writeFileSync(join(dest, "results.json"), JSON.stringify(result) + "\n");
    console.log(JSON.stringify(result, null, 2));
    return;
  }
  // jco's child processes inherit the same environment as the benchmark children.
  Object.assign(process.env, env);

  const run = (bin, args) => execFileSync(bin, args, { cwd: repoRoot, env, encoding: "utf8", maxBuffer: 64 << 20 }).trim();
  const node = (source, flags = []) => run(process.execPath, [...flags, "--input-type=module", "--eval", source]);
  const version = (bin) => run(bin, ["--version"]);
  const binaryenManifest = join(toolsDir(), "npm", "node_modules", "binaryen", "package.json");
  const toolVersion = existsSync(binaryenManifest) && JSON.parse(readFileSync(binaryenManifest, "utf8")).version;
  const environment = {
    os: type(), kernel: release(), cpu: cpus()[0]?.model, node: process.version, rustc: version("rustc"),
    wasmTools: version(toolBin("wasm-tools")),
    binaryen: { installed: toolVersion || null, pinned: false },
    commit: run("git", ["rev-parse", "HEAD"]),
    dirty: run("git", ["--no-optional-locks", "status", "--porcelain"]).length !== 0,
  };

  if (variant === "committed" || variant === "jco") {
    // The packed default release, installed offline, with the matching Option D control.
    const packages = variant === "committed"
      ? [["default", repoRoot, "@sentropic/bash"], ["jco", join(repoRoot, "packages", "bash-jco"), "@sentropic/bash-jco"]]
      : [["jco", join(repoRoot, "packages", "bash-jco"), "@sentropic/bash-jco"]];
    const measured = {};
    for (const [label, source, name] of packages) {
      const tree = stageInstalled(source, join(dest, "install-" + label), name, env);
      const entry = JSON.stringify(pathToFileURL(join(tree, "dist", "js", "index.js")).href);
      measured[label] = { tree, ...seriesFor(variant + "/" + label, entry, node) };
      measured[label].sizes = sizes(tree, env);
      measured[label].split = packageSplit(tree, env);
    }
    const result = { variant, environment, wasmOpt, measured };
    // Backwards-compatible top-level aliases for the default package.
    if (measured.default) {
      result.summary = measured.default.summary;
      result.sizes = measured.default.sizes;
      result.split = measured.default.split;
      result.runs = measured.default.runs;
      result.control = { summary: measured.jco.summary, sizes: measured.jco.sizes, split: measured.jco.split };
    } else {
      result.summary = measured.jco.summary;
      result.sizes = measured.jco.sizes;
      result.split = measured.jco.split;
      result.runs = measured.jco.runs;
    }
    writeFileSync(join(dest, "results.json"), JSON.stringify(result) + "\n");
    console.log(JSON.stringify({ variant, wasmOpt, summary: result.summary, split: result.split,
      control: result.control?.summary }, null, 2));
    return;
  }

  let tree = repoRoot;
  if (variant !== "committed") {
    tree = join(dest, "tree");
    const generated = join(dest, "generated");
    rmSync(tree, { recursive: true, force: true });
    stagePackage(repoRoot, tree, env);
    cpSync(join(repoRoot, "dist", "kernel.core.wasm"), join(tree, "dist", "kernel.core.wasm"));
    await transpileComponent(join(tree, "dist", "kernel.component.wasm"), generated, { minify: variant === "minified" });
    cpSync(generated, join(tree, "dist", "js", "kernel"), { recursive: true });
  }
  const entry = JSON.stringify(pathToFileURL(join(tree, "host", "index.js")).href);
  const { summary, runs } = seriesFor(variant, entry, node);
  const result = { variant, environment, wasmOpt, summary, sizes: sizes(tree, env), runs };
  writeFileSync(join(dest, "results.json"), JSON.stringify(result) + "\n");
  console.log(JSON.stringify({ variant, wasmOpt, summary, sizes: result.sizes }, null, 2));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) await main(process.argv.slice(2));
