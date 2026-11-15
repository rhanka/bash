// The eight gates of tools/check.mjs. Each gate takes a context { root, out, toolsDir, quick } and returns
// null on success, "SKIPPED (...)" when skipped, or a failure reason string.
import { cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { components, lockProblem, nonRegularEntries, lockResolution, pins, pinsProblem, readJson, run, sha256File, toolBin, toolProblem, tryRun, walk } from "./lib.mjs";
import { bindingsProblems } from "./bindings.mjs";

const wasmToolsOf = (ctx) => toolBin("wasm-tools", ctx.toolsDir, ctx.root);

/** One entry per component declared in tools/components.json: { name, def, path } (path may not exist). */
function declared(ctx) {
  return Object.entries(components(ctx.root)).map(([name, def]) => ({ name, def, path: join(ctx.out, `${name}.component.wasm`) }));
}

// ---------------------------------------------------------------------------------------------- G1

export function g1ZeroImport(ctx) {
  const list = declared(ctx);
  if (list.length === 0) return "no component is declared in tools/components.json";
  for (const { name, path } of list) {
    if (!existsSync(path)) return `${name}: ${relative(ctx.root, path)} was not built`;
    const r = tryRun(wasmToolsOf(ctx), ["component", "wit", path]);
    if (r.status !== 0) return `${relative(ctx.root, path)}: wasm-tools component wit failed: ${r.stderr.trim()}`;
    const line = r.stdout.split("\n").find((l) => /^\s*import\b/.test(l));
    if (line) return `${relative(ctx.root, path)} has an import: ${line.trim()}`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G2

// Whitespace between tokens, including both comment forms, so a comment cannot split a token pair.
const W = String.raw`(?:\s|/\*[\s\S]*?\*/|//[^\n]*)*`;
// A braced name list: quoted names (ES2022) may hold any character, including } ( ) and comment markers.
const NAMES = String.raw`\{(?:"[^"]*"|'[^']*'|[^}"'])*\}`;
const CLAUSE = String.raw`(?:${NAMES}|[\w$*,\s])*?`;
const STATIC_IMPORT = new RegExp(String.raw`\b(?:import|export)${W}(?:${CLAUSE}${W}\bfrom${W})?(['"])([^'"\n]+)\1`, "g");
const DYNAMIC_LITERAL = new RegExp(String.raw`\bimport${W}\(${W}(['"])([^'"` + "`" + String.raw`\n$\\]*)\1${W}\)`, "g");
const DYNAMIC_ANY = new RegExp(String.raw`\bimport${W}\(`, "g");

/** Replaces JS comments by a space, leaving string and template literals and regex literals intact. */
export function stripComments(src) {
  let out = "";
  let i = 0;
  let prev = ""; // last significant character, to tell a regex literal from a division
  const n = src.length;
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === "/" && d === "*") {
      const e = src.indexOf("*/", i + 2);
      i = e < 0 ? n : e + 2;
      out += " ";
    } else if (c === "/" && d === "/") {
      const e = src.indexOf("\n", i);
      i = e < 0 ? n : e;
      out += " ";
    } else if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < n && src[j] !== c) j += src[j] === "\\" ? 2 : 1;
      out += src.slice(i, j + 1);
      i = j + 1;
      prev = c;
    } else if (c === "/" && (prev === "" || "(,=:[!&|?{};+-*%<>~^".includes(prev))) {
      let j = i + 1;
      let cls = false;
      while (j < n && src[j] !== "\n" && (cls || src[j] !== "/")) {
        if (src[j] === "\\") j++;
        else if (src[j] === "[") cls = true;
        else if (src[j] === "]") cls = false;
        j++;
      }
      out += src.slice(i, j + 1);
      i = j + 1;
      prev = "/";
    } else {
      out += c;
      i++;
      if (!/\s/.test(c)) prev = c;
    }
  }
  return out;
}

/** Returns { specifiers, nonLiteralDynamic }. Only a single or double quoted literal is a literal import(). */
export function scanImports(raw) {
  const src = stripComments(raw);
  const specifiers = [];
  for (const m of src.matchAll(STATIC_IMPORT)) specifiers.push(m[2]);
  for (const m of src.matchAll(DYNAMIC_LITERAL)) specifiers.push(m[2]);
  const nonLiteralDynamic = [...src.matchAll(DYNAMIC_ANY)].length > [...src.matchAll(DYNAMIC_LITERAL)].length;
  return { specifiers, nonLiteralDynamic, src };
}

// Constructs that reach a module, a code evaluator or a node API by a route the import scan cannot see.
const FORBIDDEN = [
  [new RegExp(String.raw`\brequire${W}\(`), "require()"],
  [new RegExp(String.raw`\beval${W}\(`), "eval()"],
  [/\bFunction\b/, "the Function constructor"],
  [new RegExp(String.raw`\b(?:globalThis|global|self|window|process)${W}(?:\?\.)?${W}\[`), "a bracket lookup on a global object"],
];
const NODE_ONLY = [
  [/\bgetBuiltinModule\b/, "getBuiltinModule"],
  [/\bcreateRequire\b/, "createRequire"],
];

const isRelative = (s) => s.startsWith("./") || s.startsWith("../");
const SCRIPT = (p) => /\.(?:js|mjs|cjs)$/.test(p);
const isRegularFile = (p) => existsSync(p) && lstatSync(p).isFile();
const inside = (dir, p) => p.startsWith(dir + sep);

/** Symbolic links and special files under any of the directories, as a failure reason or null. */
function oddEntries(ctx, dirs) {
  for (const d of dirs) {
    if (!existsSync(d)) continue;
    const odd = nonRegularEntries(d);
    if (odd.length) return `${relative(ctx.root, d)}: symbolic link or special file: ${odd.join(", ")}`;
  }
  return null;
}

export function g2NoBareImport(ctx) {
  const hostDir = join(ctx.root, "host");
  const jsDir = join(ctx.out, "js");
  const typesDir = join(ctx.root, "types");
  const nodeDir = join(hostDir, "node") + sep;
  const link = oddEntries(ctx, [hostDir, jsDir, typesDir]);
  if (link) return link;
  // Scripts: every file of host/ and dist/js/. A relative specifier must reach a regular script file in them.
  function checkScript(f, inNodeDir) {
    const rel = relative(ctx.root, f);
    const { specifiers, nonLiteralDynamic, src } = scanImports(readFileSync(f, "utf8"));
    if (nonLiteralDynamic) return `${rel}: import() with a non-literal argument`;
    for (const [re, what] of FORBIDDEN) if (re.test(src)) return `${rel}: uses ${what}`;
    if (!inNodeDir) for (const [re, what] of NODE_ONLY) if (re.test(src)) return `${rel}: uses ${what} outside host/node/`;
    for (const s of specifiers) {
      if (isRelative(s)) {
        const target = resolve(dirname(f), s);
        if (!inside(hostDir, target) && !inside(jsDir, target)) return `${rel}: relative import "${s}" leaves host/ and dist/js/`;
        if (!isRegularFile(target) || !SCRIPT(target)) return `${rel}: relative import "${s}" is not a regular script file`;
        continue;
      }
      if (s.startsWith("node:") && inNodeDir) continue;
      return `${rel}: bare specifier "${s}"`;
    }
    return null;
  }
  for (const f of [...walk(jsDir, SCRIPT), ...walk(hostDir, SCRIPT)]) {
    const r = checkScript(f, f.startsWith(nodeDir));
    if (r) return r;
  }
  // Declarations: the same specifier rules; a relative one must reach a declaration file inside types/.
  for (const f of walk(typesDir, (p) => p.endsWith(".ts"))) {
    const rel = relative(ctx.root, f);
    const { specifiers, nonLiteralDynamic } = scanImports(readFileSync(f, "utf8"));
    if (nonLiteralDynamic) return `${rel}: import() with a non-literal argument`;
    for (const s of specifiers) {
      if (!isRelative(s)) return `${rel}: bare specifier "${s}"`;
      const t = resolve(dirname(f), s);
      const ok = [t, t.replace(/\.js$/, ".d.ts")].some((c) => inside(typesDir, c) && isRegularFile(c));
      if (!ok) return `${rel}: relative import "${s}" does not resolve to a regular file in types/`;
    }
  }
  // The browser entry must not reach host/node/, and every file it reaches obeys the full script rules.
  const entry = join(hostDir, "browser.js");
  if (!existsSync(entry)) return null;
  const seen = new Set();
  const queue = [entry];
  while (queue.length) {
    const f = queue.pop();
    if (seen.has(f)) continue;
    seen.add(f);
    const rel = relative(ctx.root, f);
    if (f.startsWith(nodeDir)) return `${rel}: reachable from host/browser.js but lives in host/node/`;
    const r = checkScript(f, false);
    if (r) return `reachable from host/browser.js: ${r}`;
    for (const s of scanImports(readFileSync(f, "utf8")).specifiers) queue.push(resolve(dirname(f), s));
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G3

function trackedAndNew(root) {
  const r = run("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], { cwd: root });
  return r.split("\0").filter((p) => p && existsSync(join(root, p)));
}

/** Map of relative path -> sha256 for the reproducible artefacts of a dist directory. */
function distHashes(dist) {
  const map = new Map();
  for (const p of walk(dist, (f) => !relative(dist, f).startsWith("target/"))) {
    const rel = relative(dist, p);
    if (rel.endsWith(".core.wasm") || rel.endsWith(".component.wasm") || rel.startsWith("js/")) map.set(rel, sha256File(p));
  }
  return map;
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
      hashes.push(distHashes(join(d, "dist")));
    }
  } finally {
    for (const d of ["g3-a", "g3-b"]) rmSync(join(base, d), { recursive: true, force: true });
  }
  const [a, b] = hashes;
  if (a.size === 0) return "no artefact produced";
  for (const k of new Set([...a.keys(), ...b.keys()])) {
    if (a.get(k) !== b.get(k)) return `${k} differs between the two builds (${a.get(k)} vs ${b.get(k)})`;
  }
  // The shipped dist/ must be the artefact the two reproducible builds produce.
  const shipped = distHashes(ctx.out);
  for (const k of new Set([...a.keys(), ...shipped.keys()])) {
    if (a.get(k) !== shipped.get(k)) return `${k} in ${relative(ctx.root, ctx.out)} differs from the reproducible build (${shipped.get(k)} vs ${a.get(k)})`;
  }
  ctx.detail = [...a].map(([k, v]) => `  ${v}  ${k}`);
  return null;
}

// ---------------------------------------------------------------------------------------------- G4

const DROP = new Set(["docs", "package", "stability", "owner"]);

/** Expands type and interface ids of a wasm-tools WIT JSON document into a self-contained structure, so
 * two documents compare by what they declare (functions, types, interfaces) and not by their ids. */
export function normalizeWorld(json, worldName) {
  const w = json.worlds.find((x) => x.name === worldName) ?? (json.worlds.length === 1 ? json.worlds[0] : null);
  if (!w) return null;
  const type = (id) => walkValue(json.types[id]);
  const iface = (id) => walkValue(json.interfaces[id]);
  function walkValue(v, key = "") {
    if (Array.isArray(v)) return v.map((x) => walkValue(x, key));
    if (v && typeof v === "object") {
      const o = {};
      for (const k of Object.keys(v).sort()) {
        if (DROP.has(k)) continue;
        o[k] = k === "interface" && typeof v[k]?.id === "number" ? iface(v[k].id) : walkValue(v[k], k);
      }
      return o;
    }
    if (typeof v === "number" && key !== "length") return type(v);
    return v;
  }
  const side = (m) => Object.fromEntries(Object.keys(m ?? {}).sort().map((k) => [k, walkValue(m[k], k)]));
  return { imports: side(w.imports), exports: side(w.exports) };
}

export function g4ComponentValidity(ctx) {
  const list = declared(ctx);
  if (list.length === 0) return "no component is declared in tools/components.json";
  const wt = wasmToolsOf(ctx);
  // Annex A: the committed WIT package must parse and resolve.
  const annexA = join(ctx.root, "wit");
  if (existsSync(annexA)) {
    const r = tryRun(wt, ["component", "wit", annexA]);
    if (r.status !== 0) return `wit/: wasm-tools component wit failed: ${r.stderr.trim()}`;
  }
  for (const { name, def, path } of list) {
    if (!existsSync(path)) return `${name}: ${relative(ctx.root, path)} was not built`;
    const v = tryRun(wt, ["validate", path]);
    if (v.status !== 0) return `${name}: wasm-tools validate failed: ${v.stderr.trim()}`;
    const t = tryRun(wt, ["component", "targets", "-w", def.world, join(ctx.root, def.witDir), path]);
    if (t.status !== 0) return `${name}: component does not conform to world ${def.world}: ${t.stderr.trim()}`;
    const got = tryRun(wt, ["component", "wit", "--json", path]);
    const want = tryRun(wt, ["component", "wit", "--json", join(ctx.root, def.witDir)]);
    if (got.status !== 0 || want.status !== 0) return `${name}: cannot print WIT as JSON`;
    const a = normalizeWorld(JSON.parse(got.stdout), "root");
    const b = normalizeWorld(JSON.parse(want.stdout), def.world);
    if (!a || !b) return `${name}: world ${def.world} not found`;
    if (JSON.stringify(a) !== JSON.stringify(b)) return `${name}: the component's interface differs from the committed WIT world ${def.world}`;
  }
  const extra = readdirOrEmpty(ctx.out).filter((f) => f.endsWith(".component.wasm") && !(f.replace(/\.component\.wasm$/, "") in components(ctx.root)));
  if (extra.length) return `${extra.join(", ")}: not listed in tools/components.json`;
  return null;
}

function readdirOrEmpty(dir) {
  return existsSync(dir) ? readdirSync(dir) : [];
}

// ---------------------------------------------------------------------------------------------- G5

export function g5OneCoreModule(ctx) {
  const list = declared(ctx);
  if (list.length === 0) return "no component is declared in tools/components.json";
  for (const { name } of list) {
    const dir = join(ctx.out, "js", name);
    if (!existsSync(dir)) return `${name}: no generated JavaScript in ${relative(ctx.root, dir)}`;
    const wasm = walk(dir, (p) => p.endsWith(".wasm"));
    // jco may add the tiny `<name>.coreN.wasm` shim and fixup modules of wit-component (resource
    // destructor table, a few hundred bytes of generated code). Only one real core module is allowed.
    const real = wasm.filter((p) => !/\.core\d+\.wasm$/.test(p) || statSync(p).size > 1024);
    if (real.length !== 1) return `${name}: expected exactly one core module, found ${real.length} of ${wasm.length} .wasm files`;
    for (const js of walk(dir, (p) => p.endsWith(".js"))) {
      const m = readFileSync(js, "utf8").match(/[A-Za-z0-9+/]{1000,}={0,2}/);
      if (m) return `${relative(ctx.root, js)} embeds a base64 literal of ${m[0].length} characters`;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G6

// Any path into the alloc crate: `alloc::...`, `::alloc::...`, `use alloc`, `extern crate alloc`.
const ALLOC_PATH = /(?<![\w:])alloc\s*::|(?<![\w:])::\s*alloc\b|\buse\s+alloc\b|\bextern\s+crate\s+alloc\b/m;
const ALLOC_CRATE = ALLOC_PATH;

const withoutLineComments = (src) =>
  src
    .split("\n")
    .filter((l) => !l.trimStart().startsWith("//"))
    .join("\n");

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
  const lock = readFileSync(lockPath, "utf8");
  for (const m of lock.matchAll(/\[\[package\]\]\s*\nname\s*=\s*"([^"]+)"/g)) {
    if (!members.has(m[1])) return `Cargo.lock contains the non-workspace package ${m[1]}`;
  }
  if (/^\s*source\s*=/m.test(lock)) return "Cargo.lock has a package with a source (registry or git)";
  const kernelSrc = join(ctx.root, "kernel", "src");
  const probeSrc = join(ctx.root, "probe", "src");
  const link = oddEntries(ctx, [kernelSrc, probeSrc]);
  if (link) return link;
  // Only the declared, verified bindings directories are exempt from the heap rule, besides mem/.
  const bindingDirs = Object.values(components(ctx.root))
    .filter((d) => d.bindings)
    .map((d) => join(ctx.root, d.bindings.dir) + sep);
  for (const f of sourcesUnder(kernelSrc)) {
    const rel = relative(kernelSrc, f);
    if (rel.startsWith("mem/") || bindingDirs.some((d) => f.startsWith(d))) continue;
    if (ALLOC_PATH.test(withoutLineComments(readFileSync(f, "utf8")))) return `kernel/src/${rel} uses the alloc crate outside kernel/src/mem/ and the declared bindings directories`;
  }
  for (const f of sourcesUnder(probeSrc)) {
    if (ALLOC_CRATE.test(withoutLineComments(readFileSync(f, "utf8")))) return `probe/src/${relative(probeSrc, f)} uses the alloc crate`;
  }
  // kernel/src/bindings/ is exempt from the heap rule only because it is exactly the generated output.
  for (const [name, def] of Object.entries(components(ctx.root))) {
    if (!def.bindings) continue;
    const problems = bindingsProblems(ctx.root, name, def.bindings, toolBin("wit-bindgen", ctx.toolsDir, ctx.root));
    if (problems.length) return problems[0];
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G7

export function checkRootManifest(root, file = "package.json") {
  const f = join(root, file);
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
  const own = checkRootManifest(ctx.root, ctx.manifestFile);
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
      st = lstatSync(p);
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
  const list = pins(ctx.root);
  const bad = pinsProblem(list);
  if (bad) return bad;
  for (const pin of list) {
    const problem = toolProblem(pin, ctx.toolsDir);
    if (problem) return problem;
  }
  const committed = join(ctx.root, "tools", "package-lock.json");
  const early = lockProblem(readJson(committed), readJson(join(ctx.root, "tools", "package.json")));
  if (early) return early;
  const staged = join(ctx.toolsDir, "npm", "package-lock.json");
  if (!existsSync(staged)) return "staged npm lockfile is missing";
  if (lockResolution(readJson(staged)) !== lockResolution(readJson(committed))) return "staged npm lockfile resolves differently from tools/package-lock.json";
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
