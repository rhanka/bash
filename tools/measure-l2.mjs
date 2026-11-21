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

const [flag, value, ...extra] = process.argv.slice(2);
const variant = value ?? "committed";
if (extra.length || (flag !== undefined && (flag !== "--variant" || !value)) ||
    !["committed", "minified", "unminified", "wasm-opt"].includes(variant)) {
  throw new Error("usage: node tools/measure-l2.mjs [--variant committed|minified|unminified|wasm-opt]");
}
if (variant === "wasm-opt") throw new Error("Binaryen is not pinned for the offline build");

const root = join(repoRoot, "tmp", "bench-l2");
const dest = join(root, variant);
const scratch = join(root, "tmp");
mkdirSync(scratch, { recursive: true });
mkdirSync(dest, { recursive: true });
const env = {
  ...process.env, LC_ALL: "C", TZ: "UTC", TMPDIR: scratch,
  npm_config_cache: join(repoRoot, "tmp", "npmcache"), npm_config_offline: "true",
};
// jco's child processes inherit the same environment as the benchmark children.
Object.assign(process.env, env);

const run = (bin, args) => execFileSync(bin, args, { cwd: repoRoot, env, encoding: "utf8", maxBuffer: 64 << 20 }).trim();
const node = (source) => run(process.execPath, ["--input-type=module", "--eval", source]);
const version = (bin) => run(bin, ["--version"]);
const binaryenManifest = join(toolsDir(), "npm", "node_modules", "binaryen", "package.json");
const toolVersion = existsSync(binaryenManifest) && JSON.parse(readFileSync(binaryenManifest, "utf8")).version;
const environment = {
  os: type(), kernel: release(), cpu: cpus()[0]?.model, node: process.version, rustc: version("rustc"),
  wasmTools: version(toolBin("wasm-tools")),
  binaryen: { installed: toolVersion || null, pinned: false },
  commit: run("git", ["rev-parse", "HEAD"]), dirty: run("git", ["status", "--porcelain"]).length !== 0,
};

let tree = repoRoot;
if (variant !== "committed") {
  tree = join(dest, "tree");
  rmSync(tree, { recursive: true, force: true });
  mkdirSync(join(tree, "dist", "js"), { recursive: true });
  cpSync(join(repoRoot, "host"), join(tree, "host"), { recursive: true });
  cpSync(join(repoRoot, "package.json"), join(tree, "package.json"));
  await transpileComponent(join(repoRoot, "dist", "kernel.component.wasm"), join(tree, "dist", "js", "kernel"), { minify: variant === "minified" });
  cpSync(join(repoRoot, "dist", "js", "kernel", "commands.js"), join(tree, "dist", "js", "kernel", "commands.js"));
}
const entry = JSON.stringify(pathToFileURL(join(tree, "host", "index.js")).href);
const assertion = [
  'const { Bash } = await import(' + entry + ');',
  'const { exitCode, stdout, stderr } = await new Bash().exec("echo hi");',
  'if (exitCode !== 0 || stdout !== "hi\\n" || stderr !== "") throw new Error("echo smoke test failed");',
].join("\n");
node(assertion);

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

const warmCode = [
  'const { Bash, defineCommand } = await import(' + entry + ');',
  'const bash = new Bash();',
  'const host = new Bash({ customCommands: [defineCommand("noop", () => ({ stdout: "", stderr: "", exitCode: 0 }))] });',
  'const result = await host.exec("noop");',
  'if (result.exitCode !== 0 || result.stdout !== "" || result.stderr !== "") throw new Error("noop smoke test failed");',
  'const workloads = [',
  '  ["M2", () => new Bash(), false],',
  '  ["M3a", () => bash.exec("echo hi"), true],',
  '  ["M3aSync", () => bash.execSync("echo hi"), false],',
  '  ["M_host", () => host.exec("noop"), true],',
  '];',
  'const data = {};',
  'for (const [name, fn, async] of workloads) {',
  '  for (let i = 0; i < 20; i++) { if (async) await fn(); else fn(); }',
  '  const samples = [];',
  '  const start = performance.now();',
  '  while (samples.length < 1000 || performance.now() - start < 2000) {',
  '    const from = performance.now();',
  '    if (async) await fn(); else fn();',
  '    samples.push(performance.now() - from);',
  '  }',
  '  data[name] = samples;',
  '}',
  'console.log(JSON.stringify(data));',
].join("\n");

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

function runSeries(seed) {
  const random = { seed };
  const cold = [];
  for (let i = 0; i < 30; i++) {
    const order = shuffled(Object.keys(coldCode), random);
    const samples = Object.fromEntries(order.map((kind) => [kind, JSON.parse(node(coldCode[kind]))]));
    cold.push({ order, samples });
  }
  const warm = JSON.parse(node(warmCode));
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

function browserImports(entry) {
  const reached = new Set();
  const visit = (file) => {
    const path = relative(tree, file);
    if (reached.has(path)) return;
    reached.add(path);
    const src = stripComments(readFileSync(file, "utf8"));
    for (const match of src.matchAll(/(?:^|[;\n])\s*(?:export\s+[^;]*?\s+from\s*|import\s+(?:[^;]*?\s+from\s*)?)["'](\.[^"']+\.js)["']/g)) {
      visit(resolve(dirname(file), match[1]));
    }
  };
  visit(entry);
  return reached;
}

function size(path, from = tree) {
  const bytes = readFileSync(join(from, path));
  return { raw: bytes.length, gzip: gzipSync(bytes, { level: 9 }).length };
}

function sizes() {
  const files = JSON.parse(run("npm", ["pack", "--offline", "--dry-run", "--json", "--ignore-scripts"]))[0].files
    .map(({ path }) => path).filter((path) => /^(host\/|dist\/js\/).*\.js$/.test(path));
  const browser = browserImports(join(tree, "host", "browser.js"));
  const groups = Object.fromEntries(["glue", "registry", "coreEntry", "nodeOnly"].map((name) => [name, {}]));
  for (const path of files) {
    const name = /^dist\/js\/kernel\/kernel(?:\.shims)?\.js$/.test(path) ? "glue" :
      path === "dist/js/kernel/commands.js" ? "registry" : browser.has(path) ? "coreEntry" : "nodeOnly";
    groups[name][path] = size(path);
  }
  const sum = (values) => values.reduce((total, item) => ({
    raw: total.raw + item.raw, gzip: total.gzip + item.gzip,
  }), { raw: 0, gzip: 0 });
  return {
    wasm: {
      loader: size("dist/js/kernel/kernel.core.wasm"),
      core: size("dist/kernel.core.wasm", repoRoot),
      component: size("dist/kernel.component.wasm", repoRoot),
    },
    js: {
      groups: Object.fromEntries(Object.entries(groups).map(([name, files]) => [name, { files, ...sum(Object.values(files)) }])),
      ...sum(files.map((path) => size(path))),
    },
  };
}

const runs = [];
for (const seed of [0x6c320001, 0x6c320002, 0x6c320003]) {
  console.error(variant + " repetition " + (runs.length + 1));
  runs.push(runSeries(seed));
}
const summary = Object.fromEntries(Object.entries(runs[0].metrics).map(([name, metric]) => {
  const median = (key) => distribution(runs.map(({ metrics }) => key ? metrics[name][key] : metrics[name])).median;
  return [name, typeof metric === "number" ? median() : { median: median("median"), p95: median("p95"), min: median("min") }];
}));
const result = { variant, environment, summary, sizes: sizes(), runs };
writeFileSync(join(dest, "results.json"), JSON.stringify(result) + "\n");
console.log(JSON.stringify({ variant, summary, sizes: result.sizes }, null, 2));
