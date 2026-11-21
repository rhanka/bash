// Runs one case on one adapter and produces the comparable result shape shared by the recorder and the
// replay runner: { steps, fs?, trace?, coverage? }.
import { canonicalize } from "./canonical.mjs";
import { withDefaults } from "./validate.mjs";

/** Replaces absolute paths of the machine by `<path>`. */
export function sanitizePaths(text) {
  return String(text)
    .replace(/[A-Za-z]:[\\/][^\s'"`<>|]*/g, "<path>")
    .replace(/(?<![\w<])(?:\/[^\s/'"`:;<>]+)+/g, "<path>");
}

/**
 * Drops `durationMs`; keeps every `*TimeMs` key with its value masked to `null` (the adapter must supply a
 * finite number, so a missing or malformed time key is a difference, not a silent pass).
 */
export function filterTraceEvent(ev) {
  const details = Object.create(null);
  for (const [k, v] of Object.entries(ev.details ?? {})) {
    if (k === "durationMs") continue;
    if (k.endsWith("TimeMs")) {
      if (typeof v !== "number" || !Number.isFinite(v)) throw new Error(`trace detail "${k}" must be a finite number`);
      details[k] = null;
    } else details[k] = v;
  }
  return { category: ev.category, name: ev.name, details };
}

function serializeEntry(e) {
  if (e.kind === "file") return { kind: "file", mode: e.mode, content: { base64: Buffer.from(e.content).toString("base64") } };
  if (e.kind === "directory") return { kind: "directory", mode: e.mode };
  return { kind: "symlink", target: e.target };
}

/** Entries whose serialized form differs between two states; `null` marks a removed path. */
export function diffFs(before, after) {
  const out = Object.create(null);
  const paths = new Set([...before.keys(), ...after.keys()]);
  for (const p of [...paths].sort()) {
    const a = before.has(p) ? serializeEntry(before.get(p)) : null;
    const b = after.has(p) ? serializeEntry(after.get(p)) : null;
    if (canonicalize(a) !== canonicalize(b)) out[p] = b;
  }
  return out;
}

function checkStepResult(r) {
  const bad = (what) => {
    throw new TypeError(`step result: ${what}`);
  };
  if (r === null || typeof r !== "object") bad("must be an object");
  if (typeof r.stdout !== "string") bad("stdout must be a string");
  if (typeof r.stderr !== "string") bad("stderr must be a string");
  if (!Number.isInteger(r.exitCode)) bad("exitCode must be an integer");
  if (r.stdoutKind !== undefined && r.stdoutKind !== "text" && r.stdoutKind !== "bytes") bad('stdoutKind must be "text" or "bytes"');
  if (r.env === null || typeof r.env !== "object" || !Object.values(r.env).every((v) => typeof v === "string")) {
    bad("env must map names to strings");
  }
}

/**
 * @returns {Promise<{outcome: 'ok', result: object} | {outcome: 'defect', defect: {step: number, message: string}}
 *                  | {outcome: 'unsupported', message: string}>}
 */
export async function runCase(adapter, caseObj) {
  // A fresh copy per run: an adapter that mutates its input cannot corrupt the case or the next run.
  const c = withDefaults(structuredClone(caseObj));
  const init = { files: c.files, profile: c.profile, engine: c.engine, capture: c.capture };
  if (c.env !== undefined) init.env = c.env;
  if (c.cwd !== undefined) init.cwd = c.cwd;
  let session;
  try {
    session = await adapter.create(init);
  } catch (e) {
    if (e && e.code === "UNSUPPORTED") return { outcome: "unsupported", message: String(e.message) };
    return { outcome: "defect", defect: { step: -1, message: sanitizePaths(e && e.message !== undefined ? e.message : e) } };
  }
  let stepIndex = 0;
  try {
    const before = c.capture.fs ? await session.readFs() : null;
    const steps = [];
    const trace = [];
    const coverage = [];
    for (; stepIndex < c.steps.length; stepIndex++) {
      const r = await session.exec(c.steps[stepIndex]);
      checkStepResult(r);
      const res = { stdout: r.stdout, stderr: r.stderr, exitCode: r.exitCode, env: { ...r.env } };
      if (r.stdoutKind !== undefined) res.stdoutKind = r.stdoutKind;
      steps.push(res);
      if (c.capture.trace) for (const ev of session.takeTrace()) trace.push(filterTraceEvent(ev));
      if (c.capture.coverage) coverage.push(...session.takeCoverage());
    }
    const result = { steps };
    if (c.capture.fs) result.fs = diffFs(before, await session.readFs());
    if (c.capture.trace) result.trace = trace;
    if (c.capture.coverage) result.coverage = coverage;
    // JSON-normalized, so what is recorded and what is compared on replay are the same value.
    return { outcome: "ok", result: JSON.parse(JSON.stringify(result)) };
  } catch (e) {
    return { outcome: "defect", defect: { step: Math.min(stepIndex, c.steps.length - 1), message: sanitizePaths(e && e.message !== undefined ? e.message : e) } };
  } finally {
    try {
      await session.dispose();
    } catch {
      /* a failing dispose does not change the outcome */
    }
  }
}
