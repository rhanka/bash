#!/usr/bin/env node
// Acquires and verifies the pinned build tools. Never executes anything from an archive and never
// updates pins.json. Usage: node tools/fetch-tools.mjs [--offline] [--skip-npm]
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repoRoot, readJson, sha256, toolsDir, toolDir, toolProblem, tryRun } from "./lib.mjs";

const args = process.argv.slice(2);
const offline = args.includes("--offline");
const skipNpm = args.includes("--skip-npm");
const pinsFile = process.env.SENTROPIC_PINS ? resolve(process.env.SENTROPIC_PINS) : join(repoRoot, "tools", "pins.json");
const dir = toolsDir();
let failed = false;

function fail(msg) {
  console.log(msg);
  failed = true;
}

async function fetchNative(pin) {
  const out = toolDir(pin);
  if (toolProblem(pin) === null) {
    console.log(`tool ${pin.name} ${pin.version} verified`);
    return;
  }
  if (offline) return fail(`tool ${pin.name} ${pin.version} ERROR offline and not verified: ${toolProblem(pin)}`);
  let buf;
  try {
    if (pin.url.startsWith("file:")) buf = readFileSync(fileURLToPath(pin.url));
    else {
      const res = await fetch(pin.url);
      if (!res.ok) return fail(`tool ${pin.name} ${pin.version} ERROR http ${res.status}`);
      buf = Buffer.from(await res.arrayBuffer());
    }
  } catch (e) {
    return fail(`tool ${pin.name} ${pin.version} ERROR download failed: ${e.message}`);
  }
  const got = sha256(buf);
  if (got !== pin.sha256) return fail(`tool ${pin.name} ${pin.version} ERROR hash mismatch: expected ${pin.sha256}, got ${got}`);
  const dl = join(dir, "dl");
  mkdirSync(dl, { recursive: true });
  const archive = join(dl, `${pin.name}-${pin.version}.tar.gz`);
  writeFileSync(archive, buf);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const r = tryRun("tar", ["-xzf", archive, "-C", out, "--strip-components=1", "--no-same-owner"]);
  if (r.status !== 0) return fail(`tool ${pin.name} ${pin.version} ERROR extraction failed: ${r.stderr.trim()}`);
  writeFileSync(join(out, ".verified"), pin.sha256 + "\n");
  const problem = toolProblem(pin);
  if (problem) {
    rmSync(join(out, ".verified"), { force: true });
    return fail(`tool ${pin.name} ${pin.version} ERROR ${problem}`);
  }
  console.log(`tool ${pin.name} ${pin.version} verified`);
}

function npmInstalled(npmDir, want) {
  for (const [name, version] of Object.entries(want)) {
    try {
      if (readJson(join(npmDir, "node_modules", ...name.split("/"), "package.json")).version !== version) return false;
    } catch {
      return false;
    }
  }
  return true;
}

function fetchNpm() {
  const src = join(repoRoot, "tools");
  const npmDir = join(dir, "npm");
  const lockSrc = readFileSync(join(src, "package-lock.json"));
  const want = readJson(join(src, "package.json")).devDependencies;
  const same = () => existsSync(join(npmDir, "package-lock.json")) && readFileSync(join(npmDir, "package-lock.json")).equals(lockSrc);
  if (same() && npmInstalled(npmDir, want)) {
    console.log("tool npm-tools verified");
    return;
  }
  if (offline) return fail("tool npm-tools ERROR offline and not installed or lockfile differs");
  mkdirSync(npmDir, { recursive: true });
  copyFileSync(join(src, "package.json"), join(npmDir, "package.json"));
  copyFileSync(join(src, "package-lock.json"), join(npmDir, "package-lock.json"));
  // `npm ci` fails with TypeScript 7 (npm bug with optional platform packages), so `npm install` is used
  // with the committed lockfile; the lockfile must be unchanged afterwards.
  const r = tryRun("npm", ["install", "--ignore-scripts", "--no-audit", "--no-fund"], { cwd: npmDir });
  if (r.status !== 0) return fail(`tool npm-tools ERROR npm install failed: ${r.stderr.trim().slice(0, 500)}`);
  if (!same()) return fail("tool npm-tools ERROR lockfile changed during install");
  if (!npmInstalled(npmDir, want)) return fail("tool npm-tools ERROR installed versions differ from the manifest");
  console.log("tool npm-tools verified");
}

const pins = readJson(pinsFile).tools;
for (const pin of pins) await fetchNative(pin);
if (!skipNpm) fetchNpm();
process.exit(failed ? 1 : 0);
