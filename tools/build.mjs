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
  if (names.includes("kernel")) {
    const { bundlePackage } = await import("./bundle.mjs");
    await bundleDefault(bundlePackage);
    await stageJcoPackage(bundlePackage);
  }
}

// Shared entry sources and their core-URL rewrites for the shipped layout.
function entryInputs() {
  return {
    indexEntry: join(repoRoot, "host", "index.js"),
    browserEntry: join(repoRoot, "host", "browser.js"),
    loadEntry: join(repoRoot, "host", "node", "load.js"),
  };
}

const CORE_URLS = {
  browser: [["../dist/js/kernel/kernel.core.wasm", "./kernel/kernel.core.wasm"]],
  load: [
    ["../../dist/js/kernel/kernel.core.wasm", "../kernel/kernel.core.wasm"],
    ["new URL(\"../..\", import.meta.url)", "new URL(\"../../..\", import.meta.url)"],
  ],
};

/** Packs the default binding: one minified browser-safe chunk plus binding-specific entries. */
async function bundleDefault(bundlePackage) {
  const jsDir = join(out, "js");
  const result = await bundlePackage({
    ...entryInputs(),
    outShared: join(jsDir, "shared.js"),
    outIndex: join(jsDir, "index.js"),
    outBrowser: join(jsDir, "browser.js"),
    outLoad: join(jsDir, "node", "load.js"),
    urls: { browser: CORE_URLS.browser, load: CORE_URLS.load },
  });
  for (const p of walk(jsDir, (f) => !f.endsWith(".wasm"))) report(p);
  console.log(`bundle default: ${result.modules} modules in ${relative(repoRoot, result.shared)}`);
}

/** Stages the independently installable `@sentropic/bash-jco` package (Option D). */
async function stageJcoPackage(bundlePackage) {
  const pkg = join(repoRoot, "packages", "bash-jco");
  const dist = join(pkg, "dist");
  const jcoJs = join(dist, "js");
  rmSync(dist, { recursive: true, force: true });
  mkdirSync(join(jcoJs, "jco"), { recursive: true });
  const oracle = join(out, "test", "js", "oracle", "kernel");
  // The guarded Option D glue and its resource shims, byte for byte from the verified oracle.
  mkdirSync(join(jcoJs, "kernel"), { recursive: true });
  copyFileSync(join(oracle, "kernel.js"), join(jcoJs, "jco", "kernel.js"));
  copyFileSync(join(oracle, "kernel.shims.js"), join(jcoJs, "jco", "kernel.shims.js"));
  // The same component and loader core bytes as the default package.
  copyFileSync(join(out, "js", "kernel", "kernel.core.wasm"), join(jcoJs, "kernel", "kernel.core.wasm"));
  copyFileSync(join(out, "kernel.component.wasm"), join(dist, "kernel.component.wasm"));
  // Declarations staged directly from the root types tree at pack time; no separately
  // maintained .d.ts files.
  mkdirSync(join(pkg, "types"), { recursive: true });
  for (const f of ["index.d.ts", "browser.d.ts", "core.d.ts"]) {
    copyFileSync(join(repoRoot, "types", f), join(pkg, "types", f));
  }
  mkdirSync(join(pkg, "wit"), { recursive: true });
  copyFileSync(join(repoRoot, "wit", "sentropic-bash.wit"), join(pkg, "wit", "sentropic-bash.wit"));
  for (const f of ["LICENSE", "DIVERGENCES.md"]) {
    copyFileSync(join(repoRoot, f), join(pkg, f));
  }
  // The adapter path as written in host/core/instance.js, independent of --out.
  const adapterFrom = join(repoRoot, "dist", "js", "kernel", "kernel.js");
  const adapterTo = join(jcoJs, "jco", "jco-adapter.js");
  writeFileSync(adapterTo,
    "// Generated by tools/build.mjs: binds instance.js to the pinned guarded jco glue.\n" +
    "import { instantiate as jcoInstantiate } from \"./kernel.js\";\n" +
    "import { shims } from \"./kernel.shims.js\";\n" +
    "export function instantiate(getCoreModule, imports, instantiateCore, options) {\n" +
    "  return jcoInstantiate((path) => {\n" +
    "    if (path in shims) return new WebAssembly.Module(shims[path]);\n" +
    "    return getCoreModule(path);\n" +
    "  }, imports, instantiateCore, options);\n}\n");
  const result = await bundlePackage({
    ...entryInputs(),
    outShared: join(jcoJs, "shared.js"),
    outIndex: join(jcoJs, "index.js"),
    outBrowser: join(jcoJs, "browser.js"),
    outLoad: join(jcoJs, "node", "load.js"),
    alias: { from: adapterFrom, to: adapterTo },
    externals: new Map([[adapterTo, "./jco/jco-adapter.js"]]),
    urls: { browser: CORE_URLS.browser, load: CORE_URLS.load },
  });
  // The Option D loader never restores: drop the default-binding claim so the predicate
  // cannot select the snapshot path even if the adapter ever exposed a restore entry.
  for (const file of [join(jcoJs, "browser.js"), join(jcoJs, "node", "load.js")]) {
    const { readFileSync: read, writeFileSync: write } = await import("node:fs");
    const code = read(file, "utf8");
    if (!code.includes("defaultBinding:!0")) throw new Error("jco loader lacks the defaultBinding claim to drop in " + file);
    write(file, code.replaceAll("defaultBinding:!0", "defaultBinding:!1"));
  }
  for (const p of walk(dist)) report(p);
  for (const p of walk(join(pkg, "types"))) report(p);
  console.log(`bundle jco: ${result.modules} modules in ${relative(repoRoot, result.shared)}`);
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
