// Shared helpers for the build tools. Node built-ins only.
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
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

export function toolDir(pin, root = repoRoot) {
  return join(toolsDir(root), `${pin.name}-${pin.version}`);
}

export function toolBin(name, root = repoRoot) {
  const pin = pins(root).find((p) => p.name === name);
  if (!pin) throw new Error(`unknown tool ${name}`);
  return join(toolDir(pin, root), pin.bin);
}

/** Returns an error string when the tool is not verified against its pin, else null. */
export function toolProblem(pin, root = repoRoot) {
  const marker = join(toolDir(pin, root), ".verified");
  if (!existsSync(marker)) return `missing .verified marker for ${pin.name} ${pin.version}`;
  if (readFileSync(marker, "utf8").trim() !== pin.sha256) return `marker hash differs from pin for ${pin.name}`;
  if (!existsSync(join(toolDir(pin, root), pin.bin))) return `missing executable ${pin.bin}`;
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
    if (statSync(p).isDirectory()) out.push(...walk(p, filter));
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
