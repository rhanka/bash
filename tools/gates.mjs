// The eight gates of tools/check.mjs. Each gate takes a context { root, out, toolsDir, quick } and returns
// null on success, "SKIPPED (...)" when skipped, or a failure reason string.
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { components, pins, readJson, run, sha256File, toolBin, toolProblem, tryRun, walk } from "./lib.mjs";

const wasmToolsOf = (ctx) => toolBin("wasm-tools", ctx.root);

function builtComponents(ctx) {
  if (!existsSync(ctx.out)) return [];
  return readdirSync(ctx.out)
    .filter((f) => f.endsWith(".component.wasm"))
    .sort()
    .map((f) => join(ctx.out, f));
}

// ---------------------------------------------------------------------------------------------- G1

export function g1ZeroImport(ctx) {
  const list = builtComponents(ctx);
  if (list.length === 0) return "no built component found";
  for (const c of list) {
    const r = tryRun(wasmToolsOf(ctx), ["component", "wit", c]);
    if (r.status !== 0) return `${relative(ctx.root, c)}: wasm-tools component wit failed: ${r.stderr.trim()}`;
    const line = r.stdout.split("\n").find((l) => /^\s*import\b/.test(l));
    if (line) return `${relative(ctx.root, c)} has an import: ${line.trim()}`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G2

const STATIC_IMPORT = /\b(?:import|export)\s+(?:[^'"();]*?\s+from\s+)?(['"])([^'"\n]+)\1/g;
const DYNAMIC_LITERAL = /\bimport\s*\(\s*(['"`])([^'"`\n]*)\1\s*\)/g;
const DYNAMIC_ANY = /\bimport\s*\(/g;

/** Returns { specifiers, nonLiteralDynamic }. */
export function scanImports(src) {
  const specifiers = [];
  for (const m of src.matchAll(STATIC_IMPORT)) specifiers.push(m[2]);
  for (const m of src.matchAll(DYNAMIC_LITERAL)) specifiers.push(m[2]);
  const nonLiteralDynamic = [...src.matchAll(DYNAMIC_ANY)].length > [...src.matchAll(DYNAMIC_LITERAL)].length;
  return { specifiers, nonLiteralDynamic };
}

const isRelative = (s) => s.startsWith("./") || s.startsWith("../");

export function g2NoBareImport(ctx) {
  const hostDir = join(ctx.root, "host");
  const files = [...walk(join(ctx.out, "js"), (p) => p.endsWith(".js")), ...walk(hostDir, (p) => p.endsWith(".js"))];
  for (const f of files) {
    const rel = relative(ctx.root, f);
    const { specifiers, nonLiteralDynamic } = scanImports(readFileSync(f, "utf8"));
    if (nonLiteralDynamic) return `${rel}: import() with a non-literal argument`;
    const inNodeDir = f.startsWith(join(hostDir, "node") + "/");
    for (const s of specifiers) {
      if (isRelative(s)) continue;
      if (s.startsWith("node:") && inNodeDir) continue;
      return `${rel}: bare specifier "${s}"`;
    }
  }
  // The browser entry must not reach any node-only construct.
  const entry = join(hostDir, "browser.js");
  if (!existsSync(entry)) return null;
  const seen = new Set();
  const queue = [entry];
  while (queue.length) {
    const f = queue.pop();
    if (seen.has(f)) continue;
    seen.add(f);
    const src = readFileSync(f, "utf8");
    const rel = relative(ctx.root, f);
    if (/process\s*\.\s*getBuiltinModule|\bcreateRequire\b/.test(src)) return `${rel}: reachable from host/browser.js and uses a node-only API`;
    for (const s of scanImports(src).specifiers) {
      if (s.startsWith("node:")) return `${rel}: reachable from host/browser.js and imports ${s}`;
      if (isRelative(s)) {
        const target = resolve(dirname(f), s);
        if (existsSync(target)) queue.push(target);
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G3

function trackedAndNew(root) {
  const r = run("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], { cwd: root });
  return r.split("\0").filter((p) => p && existsSync(join(root, p)));
}

export function g3TwoBuildIdentity(ctx) {
  if (ctx.quick) return "SKIPPED (quick)";
  const base = join(ctx.root, "tmp");
  const dirs = [join(base, "g3-a", "copy-one"), join(base, "g3-b", "nested", "copy-two")];
  const hashes = [];
  try {
    for (const d of dirs) {
      rmSync(d, { recursive: true, force: true });
      for (const p of trackedAndNew(ctx.root)) {
        mkdirSync(dirname(join(d, p)), { recursive: true });
        cpSync(join(ctx.root, p), join(d, p));
      }
      const r = tryRun("node", [join(d, "tools", "build.mjs")], {
        cwd: d,
        env: { ...process.env, SENTROPIC_TOOLS_DIR: ctx.toolsDir },
      });
      if (r.status !== 0) return `build failed in ${relative(ctx.root, d)}: ${(r.stderr || r.stdout).trim().slice(0, 400)}`;
      const dist = join(d, "dist");
      const map = new Map();
      for (const p of walk(dist, (f) => !relative(dist, f).startsWith("target/"))) {
        const rel = relative(dist, p);
        if (rel.endsWith(".core.wasm") || rel.endsWith(".component.wasm") || rel.startsWith("js/")) map.set(rel, sha256File(p));
      }
      hashes.push(map);
    }
  } finally {
    for (const d of ["g3-a", "g3-b"]) rmSync(join(base, d), { recursive: true, force: true });
  }
  const [a, b] = hashes;
  if (a.size === 0) return "no artefact produced";
  for (const k of new Set([...a.keys(), ...b.keys()])) {
    if (a.get(k) !== b.get(k)) return `${k} differs between the two builds (${a.get(k)} vs ${b.get(k)})`;
  }
  ctx.detail = [...a].map(([k, v]) => `  ${v}  ${k}`);
  return null;
}

// ---------------------------------------------------------------------------------------------- G4

function worldKeys(json, worldName) {
  const w = json.worlds.find((x) => x.name === worldName) ?? (json.worlds.length === 1 ? json.worlds[0] : null);
  if (!w) return null;
  const names = (o) => Object.keys(o ?? {}).sort();
  return { imports: names(w.imports), exports: names(w.exports) };
}

export function g4ComponentValidity(ctx) {
  const defs = components(ctx.root);
  const list = builtComponents(ctx);
  if (list.length === 0) return "no built component found";
  const wt = wasmToolsOf(ctx);
  for (const c of list) {
    const name = c.split("/").pop().replace(/\.component\.wasm$/, "");
    const def = defs[name];
    if (!def) return `${name}: not listed in tools/components.json`;
    const v = tryRun(wt, ["validate", c]);
    if (v.status !== 0) return `${name}: wasm-tools validate failed: ${v.stderr.trim()}`;
    const t = tryRun(wt, ["component", "targets", "-w", def.world, join(ctx.root, def.witDir), c]);
    if (t.status !== 0) return `${name}: component does not conform to world ${def.world}: ${t.stderr.trim()}`;
    const got = tryRun(wt, ["component", "wit", "--json", c]);
    const want = tryRun(wt, ["component", "wit", "--json", join(ctx.root, def.witDir)]);
    if (got.status !== 0 || want.status !== 0) return `${name}: cannot print WIT as JSON`;
    const a = worldKeys(JSON.parse(got.stdout), "root");
    const b = worldKeys(JSON.parse(want.stdout), def.world);
    if (!a || !b) return `${name}: world ${def.world} not found`;
    if (JSON.stringify(a) !== JSON.stringify(b)) return `${name}: printed WIT ${JSON.stringify(a)} differs from committed ${JSON.stringify(b)}`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G5

export function g5OneCoreModule(ctx) {
  const jsRoot = join(ctx.out, "js");
  if (!existsSync(jsRoot)) return "no generated JavaScript found";
  for (const d of readdirSync(jsRoot).sort()) {
    const dir = join(jsRoot, d);
    if (!statSync(dir).isDirectory()) continue;
    const wasm = walk(dir, (p) => p.endsWith(".wasm"));
    if (wasm.length !== 1) return `${d}: expected exactly one .wasm, found ${wasm.length}`;
    for (const js of walk(dir, (p) => p.endsWith(".js"))) {
      const m = readFileSync(js, "utf8").match(/[A-Za-z0-9+/]{1000,}={0,2}/);
      if (m) return `${relative(ctx.root, js)} embeds a base64 literal of ${m[0].length} characters`;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G6

const HEAP_USES = [
  /\buse\s+alloc::(?:\{[^}]*\b(?:vec|string|collections|boxed|rc|fmt)\b[^}]*\}|(?:vec|string|collections|boxed|rc|fmt)\b)/,
  /\balloc::borrow::ToOwned\b/,
];
const ALLOC_CRATE = /\bextern\s+crate\s+alloc\b|\buse\s+alloc\b|\balloc::/;

function workspaceNames(root) {
  const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
  const members = (manifest.match(/members\s*=\s*\[([^\]]*)\]/)?.[1] ?? "").match(/"([^"]+)"/g)?.map((s) => s.slice(1, -1)) ?? [];
  const names = new Set();
  for (const m of members) {
    const f = join(root, m, "Cargo.toml");
    if (!existsSync(f)) continue;
    const n = readFileSync(f, "utf8").match(/^\s*name\s*=\s*"([^"]+)"/m);
    if (n) names.add(n[1]);
  }
  return names;
}

function sourcesUnder(dir) {
  return walk(dir, (p) => p.endsWith(".rs"));
}

export function g6ZeroCrate(ctx) {
  const lockPath = join(ctx.root, "Cargo.lock");
  if (!existsSync(lockPath)) return "Cargo.lock is missing";
  const members = workspaceNames(ctx.root);
  for (const m of readFileSync(lockPath, "utf8").matchAll(/\[\[package\]\]\s*\nname\s*=\s*"([^"]+)"/g)) {
    if (!members.has(m[1])) return `Cargo.lock contains the non-workspace package ${m[1]}`;
  }
  const kernelSrc = join(ctx.root, "kernel", "src");
  for (const f of sourcesUnder(kernelSrc)) {
    const rel = relative(kernelSrc, f);
    if (rel.startsWith("mem/") || rel.startsWith("bindings/")) continue;
    const src = readFileSync(f, "utf8");
    if (HEAP_USES.some((re) => re.test(src))) return `kernel/src/${rel} uses an alloc heap type outside kernel/src/mem/ and kernel/src/bindings/`;
  }
  const probeSrc = join(ctx.root, "probe", "src");
  for (const f of sourcesUnder(probeSrc)) {
    if (ALLOC_CRATE.test(readFileSync(f, "utf8"))) return `probe/src/${relative(probeSrc, f)} uses the alloc crate`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G7

export function checkRootManifest(root) {
  const f = join(root, "package.json");
  if (!existsSync(f)) return "package.json is missing";
  const p = readJson(f);
  for (const k of ["dependencies", "peerDependencies", "optionalDependencies"]) {
    if (Object.keys(p[k] ?? {}).length) return `package.json has ${k} entries`;
  }
  for (const s of ["preinstall", "install", "postinstall", "prepare"]) {
    if (p.scripts && s in p.scripts) return `package.json has a ${s} script`;
  }
  return null;
}

export function g7Manifests(ctx) {
  const own = checkRootManifest(ctx.root);
  if (own) return own;
  if (ctx.skipMake) return null;
  for (const target of ["check-attribution", "check-no-python", "check-no-deps"]) {
    const r = tryRun("make", ["--no-print-directory", "-C", ctx.root, target]);
    if (r.status !== 0) return `make ${target} failed: ${(r.stdout + r.stderr).trim().slice(0, 400)}`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G8

function findNodeModules(dir, root, found) {
  for (const name of readdirSync(dir)) {
    if (name === ".git" || name === "tmp" || name === "target") continue;
    const p = join(dir, name);
    let st;
    try {
      st = statSync(p);
    } catch {
      continue;
    }
    if (!st.isDirectory()) continue;
    if (name === "node_modules") found.push(relative(root, p));
    else findNodeModules(p, root, found);
  }
  return found;
}

export function g8ToolPins(ctx) {
  for (const pin of pins(ctx.root)) {
    const problem = toolProblem(pin, ctx.root);
    if (problem) return problem;
  }
  const staged = join(ctx.toolsDir, "npm", "package-lock.json");
  const committed = join(ctx.root, "tools", "package-lock.json");
  if (!existsSync(staged)) return "staged npm lockfile is missing";
  if (!readFileSync(staged).equals(readFileSync(committed))) return "staged npm lockfile differs from tools/package-lock.json";
  const nm = findNodeModules(ctx.root, ctx.root, []);
  if (nm.length) return `node_modules inside the repository: ${nm.join(", ")}`;
  return null;
}

export const GATES = [
  ["G1", "zero-import", g1ZeroImport],
  ["G2", "no bare import", g2NoBareImport],
  ["G3", "two-build identity", g3TwoBuildIdentity],
  ["G4", "component validity", g4ComponentValidity],
  ["G5", "one core module", g5OneCoreModule],
  ["G6", "zero crate, heap discipline", g6ZeroCrate],
  ["G7", "manifests", g7Manifests],
  ["G8", "tool pins", g8ToolPins],
];
