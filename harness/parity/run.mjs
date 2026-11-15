// Runs one case on one adapter and produces the comparable result shape shared by the recorder and the
// replay runner: { steps, fs?, trace?, coverage? }.
import { canonicalize } from "./canonical.mjs";
import { withDefaults } from "./validate.mjs";

/** Replaces absolute paths of the machine by `<path>`. */
export function sanitizePaths(text) {
  return String(text)
    .replace(/[A-Za-z]:\\[^\s'"`<>|]*/g, "<path>")
    .replace(/(?<![\w<])(?:\/[^\s/'"`:;,()<>]+)+/g, "<path>");
}

/** Drops `durationMs` and every `*TimeMs` value; events always carry a `details` object. */
export function filterTraceEvent(ev) {
  const details = {};
  for (const [k, v] of Object.entries(ev.details ?? {})) {
    if (k === "durationMs" || k.endsWith("TimeMs")) continue;
    details[k] = v;
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
  const out = {};
  const paths = new Set([...before.keys(), ...after.keys()]);
  for (const p of [...paths].sort()) {
    const a = before.has(p) ? serializeEntry(before.get(p)) : null;
    const b = after.has(p) ? serializeEntry(after.get(p)) : null;
    if (canonicalize(a) !== canonicalize(b)) out[p] = b;
  }
  return out;
}

/**
 * @returns {Promise<{outcome: 'ok', result: object} | {outcome: 'defect', defect: {step: number, message: string}}
 *                  | {outcome: 'unsupported', message: string}>}
 */
export async function runCase(adapter, caseObj) {
  const c = withDefaults(caseObj);
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
    return { outcome: "ok", result };
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
