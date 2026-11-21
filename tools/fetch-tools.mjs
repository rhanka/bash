#!/usr/bin/env node
// Acquires and verifies the pinned build tools. Never executes anything from an archive and never
// updates pins.json. Usage: node tools/fetch-tools.mjs [--offline] [--skip-npm]
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { repoRoot, readJson, sha256, sha256File, toolsDir, toolDir, toolProblem, tryRun, lockResolution, lockProblem, markerText, nonRegularEntries, pinsProblem, tarProblem, treeManifest, npmProblem, NPM_MANIFEST } from "./lib.mjs";

const args = process.argv.slice(2);
const offline = args.includes("--offline");
const skipNpm = args.includes("--skip-npm");
const pinsFile = process.env.SENTROPIC_PINS ? resolve(process.env.SENTROPIC_PINS) : join(repoRoot, "tools", "pins.json");
const dir = toolsDir();
// A non-https pin is accepted only for the test hook (SENTROPIC_PINS); the committed tools/pins.json is
// held to https by gate G8.
const testPins = Boolean(process.env.SENTROPIC_PINS);
let failed = false;

function fail(msg) {
  console.log(msg);
  failed = true;
}

async function fetchNative(pin) {
  const out = toolDir(pin, dir);
  if (toolProblem(pin, dir) === null) {
    console.log(`tool ${pin.name} ${pin.version} verified`);
    return;
  }
  if (offline) return fail(`tool ${pin.name} ${pin.version} ERROR offline and not verified: ${toolProblem(pin, dir)}`);
  if (!pin.url.startsWith("https://") && !(testPins && pin.url.startsWith("file:"))) return fail(`tool ${pin.name} ${pin.version} ERROR url is not https`);
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
  // The archive is listed and refused before anything is extracted.
  const listed = tarProblem(archive);
  if (listed) return fail(`tool ${pin.name} ${pin.version} ERROR ${listed}`);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const r = tryRun("tar", ["-xzf", archive, "-C", out, "--strip-components=1", "--no-same-owner", "--no-same-permissions"]);
  if (r.status !== 0) return fail(`tool ${pin.name} ${pin.version} ERROR extraction failed: ${r.stderr.trim()}`);
  const odd = nonRegularEntries(out);
  if (odd.length) {
    rmSync(out, { recursive: true, force: true });
    return fail(`tool ${pin.name} ${pin.version} ERROR archive holds symbolic links or special files: ${odd.join(", ")}`);
  }
  const exe = join(out, pin.bin);
  if (!existsSync(exe)) {
    rmSync(out, { recursive: true, force: true });
    return fail(`tool ${pin.name} ${pin.version} ERROR missing executable ${pin.bin}`);
  }
  writeFileSync(join(out, ".verified"), markerText(pin, sha256File(exe)));
  const problem = toolProblem(pin, dir);
  if (problem) {
    rmSync(join(out, ".verified"), { force: true });
    return fail(`tool ${pin.name} ${pin.version} ERROR ${problem}`);
  }
  console.log(`tool ${pin.name} ${pin.version} verified`);
}

function fetchNpm() {
  const src = join(repoRoot, "tools");
  const npmDir = join(dir, "npm");
  const lockSrc = readJson(join(src, "package-lock.json"));
  const want = readJson(join(src, "package.json")).devDependencies;
  // The manifest, the lockfile root and the integrity of every package are checked before npm runs.
  const early = lockProblem(lockSrc, readJson(join(src, "package.json")));
  if (early) return fail(`tool npm-tools ERROR ${early}`);
  // npm versions rewrite lockfile metadata differently, so the check compares what was resolved.
  const resolved = lockResolution;
  const same = () =>
    existsSync(join(npmDir, "package-lock.json")) &&
    resolved(readJson(join(npmDir, "package-lock.json"))) === resolved(lockSrc);
  if (same() && npmProblem(npmDir, want) === null) {
    console.log("tool npm-tools verified");
    return;
  }
  if (offline) return fail("tool npm-tools ERROR offline and not installed or lockfile differs");
  // A tree that fails verification is discarded, so nothing planted in it survives the reinstall.
  rmSync(join(npmDir, "node_modules"), { recursive: true, force: true });
  rmSync(join(npmDir, NPM_MANIFEST), { force: true });
  mkdirSync(npmDir, { recursive: true });
  copyFileSync(join(src, "package.json"), join(npmDir, "package.json"));
  copyFileSync(join(src, "package-lock.json"), join(npmDir, "package-lock.json"));
  // `npm ci` fails with TypeScript 7 (npm bug with optional platform packages), so `npm install` is used
  // with the committed lockfile; the lockfile must be unchanged afterwards.
  const r = tryRun("npm", ["install", "--ignore-scripts", "--no-audit", "--no-fund"], { cwd: npmDir });
  if (r.status !== 0) return fail(`tool npm-tools ERROR npm install failed: ${r.stderr.trim().slice(0, 500)}`);
  if (!same()) return fail("tool npm-tools ERROR lockfile changed during install");
  writeFileSync(join(npmDir, NPM_MANIFEST), JSON.stringify(treeManifest(join(npmDir, "node_modules")), null, 1));
  const problem = npmProblem(npmDir, want);
  if (problem) return fail(`tool npm-tools ERROR ${problem}`);
  console.log("tool npm-tools verified");
}

const pins = readJson(pinsFile).tools;
{
  const bad = pinsProblem(pins, { fieldsOnly: testPins });
  if (bad) {
    console.log(bad);
    process.exit(1);
  }
}
for (const pin of pins) await fetchNative(pin);
if (!skipNpm) fetchNpm();
process.exit(failed ? 1 : 0);
