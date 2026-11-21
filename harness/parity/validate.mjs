// Validation and loading of case files and snapshot files. Errors name the file and the key.
import { readdirSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";
import { hashValue } from "./canonical.mjs";

export const NAME_RE = /^[a-z0-9][a-z0-9-]*$/;
const CASE_KEYS = ["id", "title", "tags", "files", "env", "cwd", "profile", "engine", "capture", "steps", "divergence"];
const STEP_KEYS = ["script", "stdin", "stdinKind", "env", "replaceEnv", "cwd", "args", "rawScript"];
const SNAP_KEYS = ["case", "caseHash", "engine", "status", "steps", "fs", "trace", "coverage", "defect"];

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isStr = (v) => typeof v === "string";

export class ValidationError extends Error {
  constructor(file, message) {
    super(`${file}: ${message}`);
    this.name = "ValidationError";
  }
}

// RFC 4648: groups of four characters, padding only at the end.
const BASE64_RE = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

function isContent(v) {
  if (isStr(v)) return true;
  return isObj(v) && Object.keys(v).length === 1 && isStr(v.base64) && BASE64_RE.test(v.base64);
}

function strRecord(v) {
  return isObj(v) && Object.values(v).every(isStr);
}

function validateStep(file, step, i) {
  const at = `steps[${i}]`;
  if (!isObj(step)) throw new ValidationError(file, `"${at}" must be an object`);
  for (const k of Object.keys(step)) {
    if (!STEP_KEYS.includes(k)) throw new ValidationError(file, `unknown key "${at}.${k}"`);
  }
  if (!Object.hasOwn(step, "script")) throw new ValidationError(file, `missing key "${at}.script"`);
  if (!isContent(step.script)) throw new ValidationError(file, `"${at}.script" must be text or {base64}`);
  if (Object.hasOwn(step, "stdin") && !isContent(step.stdin)) throw new ValidationError(file, `"${at}.stdin" must be text or {base64}`);
  if (Object.hasOwn(step, "stdinKind") && step.stdinKind !== "text" && step.stdinKind !== "bytes") {
    throw new ValidationError(file, `"${at}.stdinKind" must be "text" or "bytes"`);
  }
  if (Object.hasOwn(step, "env") && !strRecord(step.env)) throw new ValidationError(file, `"${at}.env" must map names to strings`);
  if (Object.hasOwn(step, "replaceEnv") && typeof step.replaceEnv !== "boolean") throw new ValidationError(file, `"${at}.replaceEnv" must be a boolean`);
  if (Object.hasOwn(step, "rawScript") && typeof step.rawScript !== "boolean") throw new ValidationError(file, `"${at}.rawScript" must be a boolean`);
  if (Object.hasOwn(step, "cwd") && !isStr(step.cwd)) throw new ValidationError(file, `"${at}.cwd" must be a string`);
  if (Object.hasOwn(step, "args") && !(Array.isArray(step.args) && step.args.every(isStr))) {
    throw new ValidationError(file, `"${at}.args" must be an array of strings`);
  }
}

/** Validates a parsed case object located at `file` whose expected id is `expectedId`. */
export function validateCase(obj, file, expectedId) {
  if (!isObj(obj)) throw new ValidationError(file, "case must be a JSON object");
  for (const k of Object.keys(obj)) {
    if (!CASE_KEYS.includes(k)) throw new ValidationError(file, `unknown key "${k}"`);
  }
  const name = basename(file).replace(/\.case\.json$/, "");
  if (!NAME_RE.test(name)) throw new ValidationError(file, `invalid name "${name}" for key "id" (the file name must match ${NAME_RE})`);
  for (const k of ["id", "title", "steps"]) {
    if (!(k in obj)) throw new ValidationError(file, `missing key "${k}"`);
  }
  if (!isStr(obj.id)) throw new ValidationError(file, `"id" must be a string`);
  if (obj.id !== expectedId) throw new ValidationError(file, `"id" is "${obj.id}" but the location gives "${expectedId}"`);
  if (!isStr(obj.title)) throw new ValidationError(file, `"title" must be a string`);
  if (Object.hasOwn(obj, "tags") && !(Array.isArray(obj.tags) && obj.tags.every(isStr))) throw new ValidationError(file, `"tags" must be an array of strings`);
  if (Object.hasOwn(obj, "files")) {
    if (!isObj(obj.files)) throw new ValidationError(file, `"files" must be an object`);
    for (const [p, f] of Object.entries(obj.files)) {
      const ok = isContent(f) || (isObj(f) && Object.keys(f).every((k) => k === "content" || k === "mode") &&
        Object.hasOwn(f, "content") && isContent(f.content) && (!Object.hasOwn(f, "mode") || Number.isInteger(f.mode)));
      if (!ok) throw new ValidationError(file, `"files[${JSON.stringify(p)}]" must be content or {content, mode}`);
    }
  }
  if (Object.hasOwn(obj, "env") && !strRecord(obj.env)) throw new ValidationError(file, `"env" must map names to strings`);
  if (Object.hasOwn(obj, "cwd") && !isStr(obj.cwd)) throw new ValidationError(file, `"cwd" must be a string`);
  if (Object.hasOwn(obj, "profile") && !isStr(obj.profile)) throw new ValidationError(file, `"profile" must be a string`);
  if (Object.hasOwn(obj, "engine") && !isObj(obj.engine)) throw new ValidationError(file, `"engine" must be an object`);
  if (Object.hasOwn(obj, "capture")) {
    if (!isObj(obj.capture)) throw new ValidationError(file, `"capture" must be an object`);
    for (const [k, v] of Object.entries(obj.capture)) {
      if (!["trace", "coverage", "fs"].includes(k)) throw new ValidationError(file, `unknown key "capture.${k}"`);
      if (typeof v !== "boolean") throw new ValidationError(file, `"capture.${k}" must be a boolean`);
    }
  }
  if (!Array.isArray(obj.steps) || obj.steps.length < 1) throw new ValidationError(file, `"steps" must be an array with at least one step`);
  obj.steps.forEach((s, i) => validateStep(file, s, i));
  if (Object.hasOwn(obj, "divergence") && obj.divergence !== null && !isStr(obj.divergence)) throw new ValidationError(file, `"divergence" must be a string or null`);
  return obj;
}

/** Validates the top-level shape of a snapshot. */
export function validateSnapshot(obj, file) {
  if (!isObj(obj)) throw new ValidationError(file, "snapshot must be a JSON object");
  for (const k of Object.keys(obj)) {
    if (!SNAP_KEYS.includes(k)) throw new ValidationError(file, `unknown key "${k}"`);
  }
  for (const k of ["case", "caseHash", "engine", "status"]) {
    if (!(k in obj)) throw new ValidationError(file, `missing key "${k}"`);
  }
  if (!isObj(obj.engine) || !isStr(obj.engine.name) || !isStr(obj.engine.version)) {
    throw new ValidationError(file, `"engine" must be {name, version} strings`);
  }
  if (obj.status === "recorded") {
    if (!Array.isArray(obj.steps)) throw new ValidationError(file, `missing key "steps"`);
  } else if (obj.status === "engine-defect") {
    if (!isObj(obj.defect)) throw new ValidationError(file, `missing key "defect"`);
    if (!Number.isInteger(obj.defect.step) || !isStr(obj.defect.message)) {
      throw new ValidationError(file, `"defect" must be {step: integer, message: string}`);
    }
  } else {
    throw new ValidationError(file, `"status" must be "recorded" or "engine-defect"`);
  }
  return obj;
}

/** Case with defaults applied, as handed to the engine. */
export function withDefaults(c) {
  return {
    ...c,
    tags: c.tags ?? [],
    files: c.files ?? {},
    profile: c.profile ?? "normal",
    engine: c.engine ?? {},
    capture: { trace: false, coverage: false, fs: true, ...(c.capture ?? {}) },
    divergence: c.divergence ?? null,
  };
}

function loadDir(dir, lot, prefix, out) {
  for (const f of readdirSync(dir).sort()) {
    if (!f.endsWith(".case.json")) continue;
    const file = join(dir, f);
    const name = f.replace(/\.case\.json$/, "");
    const id = `${lot}/${name}`;
    try {
      let obj;
      try {
        obj = JSON.parse(readFileSync(file, "utf8"));
      } catch (e) {
        throw new ValidationError(file, `not valid JSON (${e.message})`);
      }
      validateCase(obj, file, id);
      out.cases.push({ id: obj.id, lot, name, snapRel: prefix + name, file, obj, hash: hashValue(obj) });
    } catch (e) {
      if (!(e instanceof ValidationError)) throw e;
      out.invalid.push({ file, id, lot, message: e.message });
    }
  }
}

/**
 * Loads every `<name>.case.json` of a directory (lot = the directory's base name) and of each of its
 * subdirectories (lot = the subdirectory name, snapshot at `<lot>/<name>.snap.json`).
 * @returns {{cases: {id: string, lot: string, name: string, snapRel: string, file: string, obj: object, hash: string}[],
 *            invalid: {file: string, id: string, lot: string, message: string}[]}}
 */
export function loadCases(casesDir) {
  const out = { cases: [], invalid: [] };
  loadDir(casesDir, basename(casesDir), "", out);
  for (const d of readdirSync(casesDir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    if (d.isDirectory() && !d.name.startsWith(".")) loadDir(join(casesDir, d.name), d.name, `${d.name}/`, out);
  }
  return out;
}
