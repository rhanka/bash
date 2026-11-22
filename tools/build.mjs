#!/usr/bin/env node
// Builds the components: core module, component, validation, JavaScript bindings.
// Usage: node tools/build.mjs [--component <name>|all] [--out dist] [--check-bindings]
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { join, relative, resolve, sep } from "node:path";
import { repoRoot, components, coreImports, foreignImports, run, tryRun, sha256File, toolBin, walk } from "./lib.mjs";
import { bindingsProblems } from "./bindings.mjs";
import { transpileComponent, JCO_VERSION } from "./transpile.mjs";
import { verifyEngineAbi } from "./abi-check.mjs";

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

async function buildEngineBinding(name, component, jsDir) {
  const oracle = join(out, "test", "js", "oracle", name);
  await transpileComponent(component, oracle, { minify: name === "kernel" });

  const scratch = mkdtempSync(join(out, "test", "unbundle-"));
  const coreName = name + ".core.wasm";
  try {
    run(wasmTools, ["component", "unbundle", "--module-dir", scratch, component, "-o", join(scratch, "component.wasm")]);
    const modules = readdirSync(scratch).filter((file) => file.startsWith("unbundled-module"));
    if (modules.length !== 1) throw new Error(name + " does not have exactly one loader core");
    const core = join(scratch, modules[0]);
    verifyEngineAbi(repoRoot, component, core, wasmTools);
    const hash = sha256File(core);
    if (hash !== sha256File(join(oracle, coreName))) throw new Error(name + " oracle loader core hash differs");

    rmSync(jsDir, { recursive: true, force: true });
    mkdirSync(jsDir, { recursive: true });
    copyFileSync(core, join(jsDir, coreName));
    for (const file of walk(oracle, (file) => file.endsWith(".d.ts"))) {
      const target = join(jsDir, relative(oracle, file));
      mkdirSync(join(target, ".."), { recursive: true });
      copyFileSync(file, target);
    }
    stageEngineBinding(jsDir, name, name === "kernel");
    console.log(`ABI oracle ${name} jco-transpile ${JCO_VERSION} Option D: 14 guarded probes, core SHA-256 ${hash}`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** Stages the first-party engine adapter (and, for the kernel, the command list). */
function stageEngineBinding(jsDir, name, withCommands) {
  mkdirSync(jsDir, { recursive: true });
  const coreName = name + ".core.wasm";
  const binding = relative(jsDir, join(repoRoot, "host/core/abi/binding.js")).split(sep).join("/");
  writeFileSync(join(jsDir, name + ".js"),
    "import { instantiateEngine } from " + JSON.stringify(binding) + ";\n\n" +
    "export function instantiate(getCoreModule, _imports, instantiateCore, options) {\n" +
    "  return instantiateEngine(getCoreModule, instantiateCore, options, " + JSON.stringify(coreName) + ");\n}\n");
  if (withCommands) {
    const commands = scanCommandFiles(join(repoRoot, "kernel", "src", "cmd"));
    writeFileSync(join(jsDir, "commands.js"), "export const COMMAND_NAMES = Object.freeze(" + JSON.stringify(commands) + ");\n");
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
  const cores = {};
  for (const c of names) {
    cores[c] = buildCore(c, all[c]);
  }
  // The snapshot image is captured from the exact linked layout and registered as an internal
  // export before final component embed/new (spec L2c section 4.7).
  if (names.includes("kernel")) stageEngineBinding(join(out, "js", "kernel"), "kernel", true);
  const { snapshotCore } = await import("./snapshot.mjs");
  for (const c of names) {
    if (all[c].snapshot) await snapshotCore(cores[c], c);
  }
  for (const c of names) {
    const def = all[c];
    const comp = buildComponent(c, def, cores[c]);
    const jsDir = join(outDirOf(def), "js", c);
    if (c === "kernel" || c === "kernel-test") await buildEngineBinding(c, comp, jsDir);
    else await transpileComponent(comp, jsDir);
    report(cores[c]);
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
