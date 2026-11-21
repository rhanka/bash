#!/usr/bin/env node
// Builds the components: core module, component, validation, JavaScript bindings.
// Usage: node tools/build.mjs [--component <name>|all] [--out dist] [--check-bindings]
import { mkdirSync, mkdtempSync, readFileSync, rmSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { join, relative, resolve } from "node:path";
import { repoRoot, components, coreImports, foreignImports, run, tryRun, sha256File, toolBin, walk } from "./lib.mjs";
import { bindingsProblems } from "./bindings.mjs";
import { transpileComponent } from "./transpile.mjs";

const argv = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : dflt;
};
const only = opt("--component", "all");
const out = resolve(repoRoot, opt("--out", "dist"));
const checkBindings = argv.includes("--check-bindings");
const all = components();
const names = only === "all" ? Object.keys(all) : [only];
for (const n of names) if (!all[n]) throw new Error(`unknown component ${n}`);

const wasmTools = toolBin("wasm-tools");

function report(path) {
  const size = statSync(path).size;
  console.log(`artefact ${relative(repoRoot, path)} ${size} ${sha256File(path)}`);
}

// Test-only components are written under dist/test so that the packed tarball cannot include them.
const outDirOf = (def) => (def.test ? join(out, "test") : out);

function buildCore(c, def) {
  const rustflags = [
    "-C link-self-contained=no",
    "-C link-arg=--no-entry",
    "-C link-arg=--skip-wit-component",
    `--remap-path-prefix=${repoRoot}=.`,
  ].join(" ");
  const target = join(out, "target");
  const features = def.features?.length ? ["--features", def.features.join(",")] : [];
  run("cargo", ["build", "--release", "--locked", "--offline", "--target", "wasm32-wasip2", "-p", def.crate, ...features], {
    cwd: repoRoot,
    env: { ...process.env, LC_ALL: "C", TZ: "UTC", RUSTFLAGS: rustflags, CARGO_TARGET_DIR: target, CARGO_ENCODED_RUSTFLAGS: undefined },
  });
  const built = join(target, "wasm32-wasip2", "release", def.crate.replace(/-/g, "_") + ".wasm");
  const core = join(outDirOf(def), `${c}.core.wasm`);
  run("cp", [built, core]);
  const magic = readFileSync(core).subarray(0, 8).toString("hex");
  if (magic !== "0061736d01000000") throw new Error(`${core} is not a core module (magic ${magic})`);
  // A core module of a component with resources imports only the canonical builtins of its own
  // interface (`[export]...` with `[resource-new]`, `[resource-drop]`); componentization resolves them.
  const imports = foreignImports(coreImports(core));
  if (imports.length > 0) throw new Error(`${c} core module has ${imports.length} import(s): ${imports[0]}`);
  return core;
}

function buildComponent(c, def, core) {
  if (def.wasmOpt) throw new Error("wasm-opt is not pinned yet (decided at L2)");
  const tmp = mkdtempSync(join(outDirOf(def), "embed-"));
  try {
    const embedded = join(tmp, "embedded.wasm");
    const witDir = join(repoRoot, def.witDir);
    run(wasmTools, ["component", "embed", witDir, "--world", def.world, core, "-o", embedded]);
    const comp = join(outDirOf(def), `${c}.component.wasm`);
    run(wasmTools, ["component", "new", embedded, "-o", comp]);
    run(wasmTools, ["validate", comp]);
    const magic = readFileSync(comp).subarray(0, 8).toString("hex");
    if (magic !== "0061736d0d000100") throw new Error(`${comp} is not a component (magic ${magic})`);
    return comp;
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

/** Reads the literal CommandSpec records consumed by kernel/build.rs. */
export function scanCommandFiles(dir) {
  const names = new Set();
  const standard = [];
  for (const file of readdirSync(dir).filter((name) => name.endsWith(".rs") && name !== "mod.rs").sort()) {
    const source = readFileSync(join(dir, file), "utf8");
    const spec = source.split("pub const SPEC")[1]?.split("= &[")[1]?.split("];")[0];
    const commands = spec?.split("CommandSpec {").slice(1);
    if (!commands?.length) throw new Error("Missing command specs in " + file);
    for (const entry of commands) {
      const name = entry.match(/\bname:\s*"([^"]+)"/)?.[1];
      const kind = entry.match(/\bkind:\s*CommandKind::(Standard|Network)\b/)?.[1];
      if (!name || !kind || names.has(name)) throw new Error("Invalid or duplicate command spec in " + file);
      names.add(name);
      if (kind === "Standard") standard.push(name);
    }
  }
  return standard.sort();
}

async function main() {
  const f = tryRun("node", [join(repoRoot, "tools", "fetch-tools.mjs"), "--offline"], { cwd: repoRoot });
  if (f.status !== 0) {
    process.stderr.write(f.stdout + f.stderr);
    throw new Error("tools are not verified; run node tools/fetch-tools.mjs");
  }
  if (checkBindings) return verifyBindings();
  mkdirSync(out, { recursive: true });
  // A full build starts from an empty output (the cargo target cache is kept), so no stale file survives.
  if (only === "all") for (const f of readdirSync(out)) if (f !== "target") rmSync(join(out, f), { recursive: true, force: true });
  mkdirSync(join(out, "test"), { recursive: true });
  for (const c of names) {
    const def = all[c];
    const core = buildCore(c, def);
    const comp = buildComponent(c, def, core);
    const jsDir = join(outDirOf(def), "js", c);
    await transpileComponent(comp, jsDir, { minify: c === "kernel" });
    if (c === "kernel") {
      const commands = scanCommandFiles(join(repoRoot, "kernel", "src", "cmd"));
      writeFileSync(join(jsDir, "commands.js"), "export const COMMAND_NAMES = Object.freeze(" + JSON.stringify(commands) + ");\n");
    }
    report(core);
    report(comp);
    for (const p of walk(jsDir)) report(p);
  }
}

function verifyBindings() {
  let bad = false;
  for (const c of names) {
    const b = all[c].bindings;
    if (!b) {
      console.log(`bindings ${c}: none committed (hand-written glue)`);
      continue;
    }
    for (const p of bindingsProblems(repoRoot, c, b, toolBin("wit-bindgen"))) {
      console.log(p);
      bad = true;
    }
  }
  if (bad) process.exit(1);
  console.log("bindings: OK");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) main().catch((e) => {
  console.error(`build failed: ${e.message}`);
  if (e.stderr) console.error(e.stderr);
  process.exit(1);
});
