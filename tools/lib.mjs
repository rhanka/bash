// Shared helpers for the build tools. Node built-ins only.
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync, readdirSync, existsSync, lstatSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export function toolsDir(root = repoRoot) {
  return resolve(process.env.SENTROPIC_TOOLS_DIR || join(root, "tmp", "tools"));
}

export function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

export function sha256(data) {
  return createHash("sha256").update(data).digest("hex");
}

export function sha256File(path) {
  return sha256(readFileSync(path));
}

export function pins(root = repoRoot) {
  return readJson(join(root, "tools", "pins.json")).tools;
}

export function toolDir(pin, tdir = toolsDir()) {
  return join(tdir, `${pin.name}-${pin.version}`);
}

export function toolBin(name, tdir = toolsDir(), root = repoRoot) {
  const pin = pins(root).find((p) => p.name === name);
  if (!pin) throw new Error(`unknown tool ${name}`);
  return join(toolDir(pin, tdir), pin.bin);
}

/** The .verified marker: the archive hash, then the hash of the extracted executable. */
export function markerText(pin, binHash) {
  return `${pin.sha256}\nbin ${binHash}\n`;
}

/** Returns an error string when the tool is not verified against its pin, else null. The executable is
 * re-hashed on every call, so a cache restore or a later edit of the binary is detected. */
export function toolProblem(pin, tdir = toolsDir()) {
  const dir = toolDir(pin, tdir);
  const marker = join(dir, ".verified");
  if (!existsSync(marker)) return `missing .verified marker for ${pin.name} ${pin.version}`;
  const [archive, bin] = readFileSync(marker, "utf8").trim().split("\n");
  if (archive !== pin.sha256) return `marker hash differs from pin for ${pin.name}`;
  const exe = join(dir, pin.bin);
  if (!existsSync(exe) || !lstatSync(exe).isFile()) return `missing executable ${pin.bin}`;
  if (bin !== `bin ${sha256File(exe)}`) return `executable ${pin.bin} does not match the hash recorded at verification for ${pin.name}`;
  return null;
}

/** Returns an error string when a pin is not an https URL with a sha256, else null. */
export function pinsProblem(list) {
  for (const p of list) {
    if (!/^https:\/\/[^/\s]+\//.test(p.url ?? "")) return `pin ${p.name} does not use an https:// URL`;
    if (!/^[0-9a-f]{64}$/.test(p.sha256 ?? "")) return `pin ${p.name} has no valid sha256`;
  }
  return null;
}

export function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], ...opts });
}

/** Runs a command and returns { status, stdout, stderr } without throwing. */
export function tryRun(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  return { status: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? "" };
}

export function walk(dir, filter = () => true) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir).sort()) {
    const p = join(dir, name);
    const st = lstatSync(p);
    if (st.isSymbolicLink()) continue;
    if (st.isDirectory()) out.push(...walk(p, filter));
    else if (filter(p)) out.push(p);
  }
  return out;
}

export function components(root = repoRoot) {
  return readJson(join(root, "tools", "components.json")).components;
}

// What an npm lockfile resolved: package path, version, resolved URL and integrity, sorted. npm versions
// rewrite other lockfile metadata differently, so lockfiles are compared on this projection.
export function lockResolution(lock) {
  return JSON.stringify(
    Object.entries(lock.packages ?? {})
      .filter(([key]) => key !== "")
      .map(([key, p]) => [key, p.version, p.resolved, p.integrity])
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
  );
}

/** Symbolic links and special files under a directory (paths relative to it). */
export function nonRegularEntries(dir) {
  const bad = [];
  const visit = (d) => {
    for (const name of readdirSync(d).sort()) {
      const p = join(d, name);
      const st = lstatSync(p);
      if (st.isDirectory()) visit(p);
      else if (!st.isFile()) bad.push(p.slice(dir.length + 1));
    }
  };
  visit(dir);
  return bad;
}

/** Checks the tools npm manifest against its lockfile before anything is installed. Returns an error
 * string or null: root devDependencies must equal the manifest, no runtime dependency, every package
 * must carry a sha512 integrity and an https resolved URL. */
export function lockProblem(lock, manifest) {
  const root = lock.packages?.[""];
  if (!root) return "lockfile has no root package entry";
  const eq = (x, y) => JSON.stringify(Object.entries(x ?? {}).sort()) === JSON.stringify(Object.entries(y ?? {}).sort());
  if (!eq(root.devDependencies, manifest.devDependencies)) return "lockfile root devDependencies differ from tools/package.json";
  if (Object.keys(manifest.dependencies ?? {}).length || Object.keys(root.dependencies ?? {}).length) return "tools manifest has runtime dependencies";
  for (const [key, p] of Object.entries(lock.packages)) {
    if (key === "") continue;
    if (p.link) return `lockfile package ${key} is a link`;
    if (!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(p.integrity ?? "")) return `lockfile package ${key} has no sha512 integrity`;
    if (!/^https:\/\//.test(p.resolved ?? "")) return `lockfile package ${key} is not resolved over https`;
  }
  return null;
}

/** Returns the import lines of wasm-tools text output: only lines that start with `(import`. */
export function importLines(text) {
  return text.split("\n").filter((l) => /^\s*\(import\b/.test(l)).map((l) => l.trim());
}

/** Returns the `(import` lines of a core module as printed by wasm-tools (empty when it has none). */
export function coreImports(path, tdir = toolsDir(), root = repoRoot) {
  return importLines(run(toolBin("wasm-tools", tdir, root), ["print", path], { maxBuffer: 1 << 28 }));
}
