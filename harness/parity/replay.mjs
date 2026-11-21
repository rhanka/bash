// The replay runner: replays snapshots on an adapter and reports parity.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { test } from "node:test";
import { canonicalize } from "./canonical.mjs";
import { compare, formatDiffs } from "./compare.mjs";
import { runCase } from "./run.mjs";
import { loadCases, validateSnapshot, ValidationError } from "./validate.mjs";

const DEFAULT_REPORT = "tmp/parity-report.json";

async function resolveAdapter(adapter) {
  if (typeof adapter === "string") return (await import(pathToFileURL(resolve(adapter)).href)).default;
  return adapter;
}

function resolveExclusions(ex) {
  const raw = typeof ex === "string" ? JSON.parse(readFileSync(ex, "utf8")) : ex ?? {};
  for (const k of Object.keys(raw)) {
    if (k !== "divergences" && k !== "cases") throw new Error(`exclusions: unknown key "${k}"`);
  }
  return { divergences: ownMap(raw.divergences), cases: ownMap(raw.cases) };
}

/** Own entries only, so `constructor`, `toString` and `__proto__` are ordinary names. */
function ownMap(o) {
  return new Map(Object.entries(o ?? {}));
}

/**
 * Evaluates one case against its snapshot.
 * @returns {Promise<{id: string, lot: string, tags: string[], status: 'passed'|'failed'|'excluded'|'not-covered',
 *                    reason?: string, message?: string}>}
 */
export async function evaluateCase({ adapter, entry, snapshotsDir, exclusions: rawExclusions }) {
  const exclusions = rawExclusions.cases instanceof Map ? rawExclusions : resolveExclusions(rawExclusions);
  const { id, obj } = entry;
  const base = { id, lot: entry.lot ?? id.split("/")[0], tags: obj.tags ?? [] };
  const exclReason = exclusions.cases.get(id) ?? (obj.divergence ? exclusions.divergences.get(obj.divergence) : undefined);
  if (exclReason !== undefined) return { ...base, status: "excluded", reason: exclReason };
  const snapFile = join(snapshotsDir, `${entry.snapRel ?? entry.name}.snap.json`);
  // A case the adapter does not support is reported as not covered, whatever the state of its snapshot.
  const notCovered = async () => (await runCase(adapter, obj)).outcome === "unsupported";
  if (!existsSync(snapFile)) {
    if (await notCovered()) return { ...base, status: "not-covered", reason: "not covered" };
    return { ...base, status: "failed", message: `missing snapshot for ${id}` };
  }
  let snap;
  try {
    snap = validateSnapshot(JSON.parse(readFileSync(snapFile, "utf8")), snapFile);
  } catch (e) {
    return { ...base, status: "failed", message: `invalid snapshot for ${id}: ${e.message}` };
  }
  if (snap.caseHash !== entry.hash || snap.case !== id) {
    if (await notCovered()) return { ...base, status: "not-covered", reason: "not covered" };
    return { ...base, status: "failed", message: `stale snapshot for ${id}: the case changed since it was recorded` };
  }
  if (snap.status === "engine-defect") return { ...base, status: "excluded", reason: "upstream-defect" };
  const r = await runCase(adapter, obj);
  if (r.outcome === "unsupported") return { ...base, status: "not-covered", reason: "not covered" };
  if (r.outcome === "defect") {
    return { ...base, status: "failed", message: `${id}: engine defect at step ${r.defect.step}: ${r.defect.message}` };
  }
  const expected = {};
  const actual = {};
  for (const k of ["steps", "fs", "trace", "coverage"]) {
    if (Object.hasOwn(snap, k)) expected[k] = snap[k];
    if (Object.hasOwn(r.result, k)) actual[k] = r.result[k];
  }
  const diffs = compare(expected, actual);
  if (diffs.length) return { ...base, status: "failed", message: `${id}: ${formatDiffs(diffs)}`, paths: diffs.map((d) => d.path) };
  return { ...base, status: "passed" };
}

// Counters keep their keys (lots, tags, exclusion reasons) in Maps while counting, so user-chosen names such
// as "constructor" or "__proto__" never touch Object.prototype; they become plain own properties at the end.
function emptyCounts() {
  return { cases: 0, passed: 0, failed: 0, excluded: new Map(), notCovered: 0 };
}

function bucket(map, key) {
  if (!map.has(key)) map.set(key, emptyCounts());
  return map.get(key);
}

function add(counts, o) {
  counts.cases++;
  if (o.status === "passed") counts.passed++;
  else if (o.status === "failed") counts.failed++;
  else if (o.status === "not-covered") counts.notCovered++;
  else counts.excluded.set(o.reason, (counts.excluded.get(o.reason) ?? 0) + 1);
}

const plainEntries = (map, f = (v) => v) => Object.fromEntries([...map].map(([k, v]) => [k, f(v)]));
const finishCounts = (c) => ({ ...c, excluded: plainEntries(c.excluded) });

/**
 * Builds the parity report (no timestamp) from case outcomes. Invalid case files count as failed cases,
 * named by their id-shaped `<lot>/<name>`.
 * @param {object[]} outcomes
 * @param {{id: string, lot: string}[]} [invalid]
 */
export function buildReport(outcomes, invalid = []) {
  const all = [
    ...outcomes,
    ...invalid.map((i) => ({ id: i.id, lot: i.lot, tags: [], status: "failed" })),
  ];
  const report = emptyCounts();
  const byLot = new Map();
  const byTag = new Map();
  const failedCases = [];
  for (const o of all) {
    add(report, o);
    add(bucket(byLot, o.lot), o);
    for (const t of new Set(o.tags)) add(bucket(byTag, t), o);
    if (o.status === "failed") failedCases.push(o.id);
  }
  failedCases.sort();
  return { ...finishCounts(report), failedCases, byLot: plainEntries(byLot, finishCounts), byTag: plainEntries(byTag, finishCounts) };
}

export function writeReport(report, reportPath) {
  mkdirSync(dirname(resolve(reportPath)), { recursive: true });
  writeFileSync(reportPath, canonicalize(report));
}

export function summarize(report) {
  const excluded = Object.values(report.excluded).reduce((a, b) => a + b, 0);
  return `parity: ${report.cases} cases, ${report.passed} passed, ${report.failed} failed, ${excluded} excluded, ${report.notCovered} not covered`;
}

/** Evaluates every case of a directory (programmatic form of the replay runner). */
export async function runParity({ adapter, casesDir, snapshotsDir, exclusions }) {
  const ad = await resolveAdapter(adapter);
  const ex = resolveExclusions(exclusions);
  const { cases, invalid } = loadCases(casesDir);
  const outcomes = [];
  for (const entry of cases) outcomes.push(await evaluateCase({ adapter: ad, entry, snapshotsDir, exclusions: ex }));
  return { outcomes, invalid, report: buildReport(outcomes, invalid) };
}

/**
 * Registers one node:test test per case, named by the case id, and one final test writing the report.
 * `adapter` is an adapter object or the path of an adapter module.
 */
export function defineParityTests({ adapter, casesDir, snapshotsDir, exclusions, reportPath = DEFAULT_REPORT }) {
  const ex = resolveExclusions(exclusions);
  const { cases, invalid } = loadCases(casesDir);
  const outcomes = [];
  let loaded;
  for (const inv of invalid) {
    test(`invalid case file ${inv.file}`, () => {
      throw new ValidationError(inv.file, inv.message.slice(inv.file.length + 2));
    });
  }
  for (const entry of cases) {
    test(entry.id, async (t) => {
      loaded ??= await resolveAdapter(adapter);
      const o = await evaluateCase({ adapter: loaded, entry, snapshotsDir, exclusions: ex });
      outcomes.push(o);
      if (o.status === "failed") throw new Error(o.message);
      if (o.status === "excluded") t.skip(`excluded: ${o.reason}`);
      else if (o.status === "not-covered") t.skip("not covered");
    });
  }
  test("parity report", () => {
    const report = buildReport(outcomes, invalid);
    writeReport(report, reportPath);
    console.log(summarize(report));
  });
}
