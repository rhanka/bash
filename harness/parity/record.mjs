// The recorder: runs cases on an adapter and writes canonical snapshots.
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { canonicalize, hashValue } from "./canonical.mjs";
import { runCase } from "./run.mjs";
import { loadCases, validateSnapshot, ValidationError } from "./validate.mjs";

const RUNS = 3;

/**
 * Records one case. Resolves to `{status: 'recorded'|'engine-defect', snapshot}`,
 * `{status: 'unstable'}` or `{status: 'unsupported'}`.
 * @param {object} adapter
 * @param {object} caseObject a validated case
 */
export async function recordCase(adapter, caseObject) {
  const runs = [];
  for (let i = 0; i < RUNS; i++) {
    const r = await runCase(adapter, caseObject);
    if (r.outcome === "unsupported") return { status: "unsupported", message: r.message };
    runs.push(r);
  }
  const forms = runs.map((r) => canonicalize(r));
  if (forms.some((f) => f !== forms[0])) return { status: "unstable" };
  const r = runs[0];
  const snapshot = {
    case: caseObject.id,
    caseHash: hashValue(caseObject),
    engine: { name: adapter.name, version: adapter.version },
  };
  if (r.outcome === "defect") {
    snapshot.status = "engine-defect";
    snapshot.defect = r.defect;
  } else {
    snapshot.status = "recorded";
    Object.assign(snapshot, r.result);
  }
  return { status: snapshot.status, snapshot };
}

/**
 * @param {{adapter: object, casesDir: string, snapshotsDir: string, check?: boolean, filter?: string}} o
 * @returns {Promise<{recorded: string[], unchanged: string[], changed: string[], unstable: string[],
 *   defects: string[], missing: string[], stale: string[], orphans: string[], unsupported: string[],
 *   invalid: {file: string, message: string}[], lines: string[], failed: boolean}>}
 */
export async function recordAll({ adapter, casesDir, snapshotsDir, check = false, filter }) {
  const res = { recorded: [], unchanged: [], changed: [], unstable: [], defects: [], missing: [], stale: [],
    orphans: [], unsupported: [], invalid: [], lines: [], failed: false };
  const { cases, invalid } = loadCases(casesDir);
  res.invalid = invalid;
  for (const inv of invalid) res.lines.push(`invalid ${inv.message}`);
  if (invalid.length) res.failed = true;
  const known = new Set(cases.map((c) => c.name));
  if (!check) mkdirSync(snapshotsDir, { recursive: true });
  for (const c of cases) {
    if (filter && !c.id.includes(filter)) continue;
    const out = join(snapshotsDir, `${c.name}.snap.json`);
    const existing = existsSync(out) ? readFileSync(out, "utf8") : null;
    if (check && existing === null) {
      res.missing.push(c.id);
      res.lines.push(`missing ${c.id}`);
      res.failed = true;
      continue;
    }
    if (check) {
      let prev;
      try {
        prev = validateSnapshot(JSON.parse(existing), out);
      } catch (e) {
        res.changed.push(c.id);
        res.lines.push(`changed ${c.id}`);
        res.failed = true;
        continue;
      }
      if (prev.caseHash !== c.hash) {
        res.stale.push(c.id);
        res.lines.push(`stale ${c.id}`);
        res.failed = true;
        continue;
      }
    }
    const r = await recordCase(adapter, c.obj);
    if (r.status === "unsupported") {
      res.unsupported.push(c.id);
      res.lines.push(`not-covered ${c.id}`);
      continue;
    }
    if (r.status === "unstable") {
      res.unstable.push(c.id);
      res.lines.push(`unstable ${c.id}`);
      res.failed = true;
      continue;
    }
    const text = canonicalize(r.snapshot);
    const same = existing === text;
    let label;
    if (existing !== null && !same) {
      res.changed.push(c.id);
      label = "changed";
    } else if (r.status === "engine-defect") {
      res.defects.push(c.id);
      label = "engine-defect";
    } else if (existing === null) {
      res.recorded.push(c.id);
      label = "recorded";
    } else {
      res.unchanged.push(c.id);
      label = "unchanged";
    }
    if (check && !same) res.failed = true;
    res.lines.push(`${label} ${c.id}`);
    if (!check) writeFileSync(out, text);
  }
  if (existsSync(snapshotsDir)) {
    for (const f of readdirSync(snapshotsDir).sort()) {
      if (!f.endsWith(".snap.json")) continue;
      const name = f.replace(/\.snap\.json$/, "");
      if (!known.has(name)) {
        const id = `${basename(casesDir)}/${name}`;
        res.orphans.push(id);
        res.lines.push(`orphan ${id}`);
        if (check) res.failed = true;
      }
    }
  }
  return res;
}

export { ValidationError };
