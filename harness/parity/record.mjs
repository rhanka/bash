// The recorder: runs cases on an adapter and writes canonical snapshots.
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
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
  const known = new Set(cases.map((c) => c.snapRel));
  if (!check) mkdirSync(snapshotsDir, { recursive: true });
  for (const c of cases) {
    if (filter && !c.id.includes(filter)) continue;
    const out = join(snapshotsDir, `${c.snapRel}.snap.json`);
    const existing = existsSync(out) ? readFileSync(out, "utf8") : null;
    // A case the adapter does not support is not covered, whatever the state of its snapshot (as on replay).
    const notCovered = async () => (await runCase(adapter, c.obj)).outcome === "unsupported";
    const unsupported = async () => {
      res.unsupported.push(c.id);
      res.lines.push(`not-covered ${c.id}`);
    };
    if (check && existing === null) {
      if (await notCovered()) {
        await unsupported();
        continue;
      }
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
        if (await notCovered()) {
          await unsupported();
          continue;
        }
        res.changed.push(c.id);
        res.lines.push(`changed ${c.id}`);
        res.failed = true;
        continue;
      }
      if (prev.caseHash !== c.hash) {
        if (await notCovered()) {
          await unsupported();
          continue;
        }
        res.stale.push(c.id);
        res.lines.push(`stale ${c.id}`);
        res.failed = true;
        continue;
      }
    }
    const r = await recordCase(adapter, c.obj);
    if (r.status === "unsupported") {
      // No snapshot for an unsupported case: a leftover one from a former adapter would be stale.
      if (!check && existing !== null) rmSync(out);
      await unsupported();
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
    if (!check) {
      mkdirSync(dirname(out), { recursive: true });
      writeFileSync(out, text);
    }
  }
  if (existsSync(snapshotsDir)) {
    const walkSnaps = (dir, prefix) => {
      for (const d of readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
        if (d.isDirectory() && prefix === "") walkSnaps(join(dir, d.name), `${d.name}/`);
        else if (d.name.endsWith(".snap.json")) {
          const rel = prefix + d.name.replace(/\.snap\.json$/, "");
          if (known.has(rel)) continue;
          const id = prefix === "" ? `${basename(casesDir)}/${rel}` : rel;
          res.orphans.push(id);
          res.lines.push(`orphan ${id}`);
          if (check) res.failed = true;
        }
      }
    };
    walkSnaps(snapshotsDir, "");
  }
  return res;
}

export { ValidationError };
