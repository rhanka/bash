import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { repoRoot, scratch } from "./helpers.mjs";
import { sha256File, toolsDir } from "../tools/lib.mjs";
import * as gates from "../tools/gates.mjs";
import { runParity, summarize } from "../harness/parity/replay.mjs";

const EXPECTED_DEFAULT = [
  "DIVERGENCES.md",
  "LICENSE",
  "README.md",
  "dist/js/browser.js",
  "dist/js/index.js",
  "dist/js/kernel/kernel.core.wasm",
  "dist/js/node/load.js",
  "dist/js/shared.js",
  "dist/kernel.component.wasm",
  "package.json",
  "types/browser.d.ts",
  "types/core.d.ts",
  "types/index.d.ts",
  "wit/sentropic-bash.wit",
];

const EXPECTED_JCO = [
  "DIVERGENCES.md",
  "LICENSE",
  "README.md",
  "dist/js/browser.js",
  "dist/js/index.js",
  "dist/js/jco/jco-adapter.js",
  "dist/js/jco/kernel.js",
  "dist/js/jco/kernel.shims.js",
  "dist/js/kernel/kernel.core.wasm",
  "dist/js/node/load.js",
  "dist/js/shared.js",
  "dist/kernel.component.wasm",
  "package.json",
  "types/browser.d.ts",
  "types/core.d.ts",
  "types/index.d.ts",
  "wit/sentropic-bash.wit",
];

const PACKS = [
  { name: "@sentropic/bash", dir: repoRoot, expected: EXPECTED_DEFAULT },
  { name: "@sentropic/bash-jco", dir: join(repoRoot, "packages", "bash-jco"), expected: EXPECTED_JCO },
];

const env = () => ({
  ...process.env,
  npm_config_cache: join(repoRoot, "tmp", "npmcache"),
  npm_config_offline: "true",
  TMPDIR: join(repoRoot, "tmp"),
  LC_ALL: "C",
  TZ: "UTC",
});

/** Packs a package dir offline and returns the tarball path and file list. */
function pack(cwd) {
  const dest = scratch("pack");
  const p = spawnSync("npm", ["pack", "--offline", "--json", "--ignore-scripts", "--pack-destination", dest],
    { cwd, encoding: "utf8", env: env() });
  assert.equal(p.status, 0, p.stderr);
  const info = JSON.parse(p.stdout)[0];
  return { tarball: join(dest, info.filename), files: info.files.map((f) => f.path).sort() };
}

/** Installs a tarball alone in an empty consumer and returns the consumer dir. */
function install(tarball) {
  const consumer = join(scratch("pack"), "consumer");
  mkdirSync(consumer, { recursive: true });
  writeFileSync(join(consumer, "package.json"),
    JSON.stringify({ name: "consumer", version: "0.0.0", private: true, type: "module" }));
  const i = spawnSync("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarball],
    { cwd: consumer, encoding: "utf8", env: env() });
  assert.equal(i.status, 0, i.stderr);
  return consumer;
}

const pkgRootOf = (consumer, name) => join(consumer, "node_modules", name);

/** One .wasm per shipped glue directory and no long base64 literal in shipped JS (G5). */
function checkShippedGlue(pkgRoot) {
  const wasm = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir)) {
      const p = join(dir, entry);
      if (statSync(p).isDirectory()) walk(p);
      else if (p.endsWith(".wasm")) wasm.push(p);
      else if (p.endsWith(".js")) {
        const m = readFileSync(p, "utf8").match(/[A-Za-z0-9+/]{1000,}={0,2}/);
        assert.equal(m, null, p + " embeds a base64 literal");
      }
    }
  };
  walk(join(pkgRoot, "dist", "js"));
  const byDir = new Map();
  for (const w of wasm) {
    const dir = join(w, "..");
    byDir.set(dir, (byDir.get(dir) ?? 0) + 1);
  }
  for (const [dir, count] of byDir) assert.equal(count, 1, dir + " holds " + count + " loader cores");
  assert.ok(wasm.length > 0, "no loader core shipped");
}

/** An adapter over an installed Bash class, mirroring the committed parity adapter mapping. */
function installedAdapter(Bash) {
  const fileData = (value) => typeof value === "string" ? value : Buffer.from(value.base64, "base64");
  return {
    name: "installed",
    async create(init) {
      const options = { executionLimitProfile: init.profile ?? "normal" };
      if (init.env !== undefined) options.env = init.env;
      if (init.cwd !== undefined) options.cwd = init.cwd;
      if (init.files && Object.keys(init.files).length > 0) {
        options.files = Object.fromEntries(Object.entries(init.files).map(([path, value]) => {
          const data = typeof value === "object" && "content" in value
            ? { ...value, content: fileData(value.content) }
            : fileData(value);
          return [path, data];
        }));
      }
      const bash = new Bash(options);
      return {
        async exec(step) {
          const { script, stdin, stdinKind, ...opts } = step;
          if (stdin && typeof stdin === "object") {
            opts.stdin = Buffer.from(stdin.base64, "base64").toString("latin1");
            opts.stdinKind = "bytes";
          } else if (stdin !== undefined) {
            opts.stdin = stdin;
            opts.stdinKind = stdinKind;
          }
          const text = typeof script === "string" ? script : Buffer.from(script.base64, "base64").toString("utf8");
          const { stdout, stderr, exitCode, env = {}, stdoutKind } = await bash.exec(text, opts);
          return { stdout, stderr, exitCode, env, ...(stdoutKind ? { stdoutKind } : {}) };
        },
        async readFs() {
          const entries = new Map();
          for (const path of bash.fs.getAllPaths()) {
            const stat = await bash.fs.lstat(path);
            if (stat.isFile) {
              entries.set(path, { kind: "file", mode: stat.mode, content: await bash.fs.readFileBuffer(path) });
            } else if (stat.isDirectory) entries.set(path, { kind: "directory", mode: stat.mode });
            else if (stat.isSymbolicLink) entries.set(path, { kind: "symlink", target: await bash.fs.readlink(path) });
          }
          return entries;
        },
        takeTrace: () => [],
        takeCoverage: () => [],
        dispose() {},
      };
    },
  };
}

for (const { name, dir, expected } of PACKS) {
  test(name + ": the tarball holds exactly the expected files and installs offline", () => {
    const { tarball, files } = pack(dir);
    assert.deepEqual(files, expected);
    const consumer = install(tarball);
    const pkgRoot = pkgRootOf(consumer, name);
    const packed = JSON.parse(readFileSync(join(pkgRoot, "package.json"), "utf8"));
    for (const k of ["dependencies", "peerDependencies", "optionalDependencies"]) {
      assert.deepEqual(Object.keys(packed[k] ?? {}), []);
    }
    for (const s of ["preinstall", "install", "postinstall", "prepare"]) {
      assert.equal(packed.scripts?.[s], undefined);
    }
    // The component, WIT and loader core match the build byte for byte.
    for (const path of ["dist/kernel.component.wasm", "wit/sentropic-bash.wit", "dist/js/kernel/kernel.core.wasm"]) {
      assert.equal(sha256File(join(pkgRoot, path)), sha256File(join(repoRoot, path)), path);
    }
    // Each installed declaration matches its root types/** file byte for byte.
    for (const f of ["types/index.d.ts", "types/browser.d.ts", "types/core.d.ts"]) {
      assert.equal(readFileSync(join(pkgRoot, f), "utf8"), readFileSync(join(repoRoot, f), "utf8"), f);
    }
    // G2 on the shipped paths and the G5 loader-core/base64 invariants.
    assert.equal(gates.g2NoBareImport({ root: pkgRoot, out: pkgRoot, toolsDir: toolsDir(), quick: true }), null);
    checkShippedGlue(pkgRoot);
  });

  test(name + ": the installed package runs Node ESM, CJS and the browser entry", async () => {
    const { tarball } = pack(dir);
    const consumer = install(tarball);
    const quoted = JSON.stringify(pathToFileURL(join(pkgRootOf(consumer, name), "dist/js/index.js")).href);
    const esm = `import assert from "node:assert/strict";\nimport * as node from ${quoted};\n` +
      `assert.equal((await new node.Bash().exec("echo hi")).stdout, "hi\\n");\n` +
      `assert.equal(node.Bash ? "ok" : "missing", "ok");\n`;
    const r = spawnSync(process.execPath, ["--input-type=module", "--eval", esm],
      { cwd: consumer, encoding: "utf8", env: env() });
    assert.equal(r.status, 0, r.stderr);
    writeFileSync(join(consumer, "main.cjs"),
      `const assert = require("node:assert/strict");\nconst node = require(${JSON.stringify(name)});\n` +
      `assert.equal(typeof node.Bash, "function");\n` +
      `assert.equal(typeof node.InMemoryFs, "function");\n`);
    const q = spawnSync(process.execPath, ["main.cjs"], { cwd: consumer, encoding: "utf8", env: env() });
    assert.equal(q.status, 0, q.stderr);
    const browserMain = `import assert from "node:assert/strict";\n` +
      `import { readFileSync } from "node:fs";\n` +
      `import * as browser from ${JSON.stringify(name + "/browser")};\n` +
      `await browser.Bash.precompile(readFileSync(new URL(${JSON.stringify("./node_modules/" + name + "/dist/js/kernel/kernel.core.wasm")}, import.meta.url)));\n` +
      `assert.equal((await new browser.Bash().exec("echo hi")).stdout, "hi\\n");\n` +
      `assert.equal(new browser.Bash().execSync("echo hi").stdout, "hi\\n");\n`;
    writeFileSync(join(consumer, "browser-main.mjs"), browserMain);
    const b = spawnSync(process.execPath, ["browser-main.mjs"], { cwd: consumer, encoding: "utf8", env: env() });
    assert.equal(b.status, 0, b.stderr);
  });

  test(name + ": tsc --noEmit passes on a consumer of the installed declarations", () => {
    const { tarball } = pack(dir);
    const consumer = install(tarball);
    const tsc = join(toolsDir(), "npm", "node_modules", ".bin", "tsc");
    assert.ok(existsSync(tsc), "the type check fails closed when tsc is absent");
    const rel = "./node_modules/" + name;
    writeFileSync(join(consumer, "tsconfig.json"), JSON.stringify({
      compilerOptions: {
        noEmit: true, strict: true, target: "es2022", module: "esnext", moduleResolution: "bundler",
        lib: ["es2022", "dom"], types: [],
        paths: { [name]: [rel + "/types/index.d.ts"], [name + "/browser"]: [rel + "/types/browser.d.ts"] },
      },
      files: ["main.ts"],
    }));
    writeFileSync(join(consumer, "main.ts"),
      `import { Bash, defineCommand, getCommandNames } from "${name}";\n` +
      `import { Bash as BrowserBash } from "${name}/browser";\n` +
      `const bash: InstanceType<typeof Bash> = new Bash();\n` +
      `const browser: InstanceType<typeof BrowserBash> = new BrowserBash();\n` +
      `void bash; void browser; void defineCommand; void getCommandNames;\n`);
    const t = spawnSync(tsc, ["--noEmit", "-p", "tsconfig.json"], { cwd: consumer, encoding: "utf8", env: env() });
    assert.equal(t.status, 0, t.stdout + t.stderr);
  });

  test(name + ": 30/30 oracle cases replay through the installed package", async (t) => {
    const { tarball } = pack(dir);
    const consumer = install(tarball);
    const mod = await import(pathToFileURL(join(pkgRootOf(consumer, name), "dist/js/index.js")).href);
    const cases = join(repoRoot, "test", "cases", "l2");
    const { report, outcomes } = await runParity({
      adapter: installedAdapter(mod.Bash),
      casesDir: cases,
      snapshotsDir: cases,
      exclusions: join(repoRoot, "test", "parity", "exclusions.json"),
    });
    t.diagnostic(name + " " + summarize(report));
    assert.deepEqual(outcomes.filter(({ status }) => status !== "passed"), []);
    assert.equal(report.cases, 30);
    assert.equal(report.passed, 30);
  }, { timeout: 300000 });

  test(name + ": all four limit profiles execute on the installed package", async () => {
    const { tarball } = pack(dir);
    const consumer = install(tarball);
    const mod = await import(pathToFileURL(join(pkgRootOf(consumer, name), "dist/js/index.js")).href);
    for (const profile of ["micro", "contain", "hardened", "normal"]) {
      const bash = new mod.Bash({ executionLimitProfile: profile });
      assert.equal((await bash.exec("echo hi")).stdout, "hi\n", profile);
    }
  }, { timeout: 120000 });

  test(name + ": host adapters and fs mounts work on the installed package", async () => {
    const { tarball } = pack(dir);
    const consumer = install(tarball);
    writeFileSync(join(consumer, "host-main.mjs"),
      `import assert from "node:assert/strict";\n` +
      `import { readFileSync } from "node:fs";\n` +
      `import * as node from ${JSON.stringify(name)};\n` +
      `assert.equal(typeof node.OverlayFs, "function");\n` +
      `assert.equal(typeof node.ReadWriteFs, "function");\n` +
      `import * as browser from ${JSON.stringify(name + "/browser")};\n` +
      `await browser.Bash.precompile(readFileSync(new URL(${JSON.stringify("./node_modules/" + name + "/dist/js/kernel/kernel.core.wasm")}, import.meta.url)));\n` +
      `for (const entry of [node, browser]) {\n` +
      `  assert.equal("pipelineProbe" in entry, false);\n` +
      `  const fs = new entry.InMemoryFs();\n` +
      `  const bash = new entry.Bash({ fs });\n` +
      `  assert.equal(bash.fs, fs);\n` +
      `  for (const { exitCode, stdout, stderr } of [await bash.exec("echo hi"), bash.execSync("echo hi")]) {\n` +
      `    assert.deepEqual({ exitCode, stdout, stderr }, { exitCode: 0, stdout: "hi\\n", stderr: "" });\n` +
      `  }\n` +
      `}\n`);
    const r = spawnSync(process.execPath, ["host-main.mjs"], { cwd: consumer, encoding: "utf8", env: env() });
    assert.equal(r.status, 0, r.stderr);
  });
}
