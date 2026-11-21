// Verifies that the committed wit-bindgen output of a component is exactly what the pinned tool prints.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { run } from "./lib.mjs";

/** Returns a list of problems (empty when the committed bindings match). The file sets must be equal:
 * an extra hand-written file in the bindings directory is a problem. */
export function bindingsProblems(root, name, b, witBindgen) {
  const problems = [];
  const base = join(root, "tmp");
  mkdirSync(base, { recursive: true });
  const tmp = mkdtempSync(join(base, "bindings-"));
  try {
    run(witBindgen, ["rust", join(root, b.witDir), "--world", b.world, "--out-dir", tmp, ...b.args]);
    const want = readdirSync(tmp).sort();
    const dir = join(root, b.dir);
    const have = existsSync(dir) ? readdirSync(dir).sort() : [];
    for (const f of want) {
      if (!have.includes(f)) problems.push(`bindings ${name}: ${f} is not committed`);
      else if (!readFileSync(join(tmp, f)).equals(readFileSync(join(dir, f)))) problems.push(`bindings ${name}: ${f} differs from the committed file`);
    }
    for (const f of have) if (!want.includes(f)) problems.push(`bindings ${name}: ${f} is committed but not generated`);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  return problems;
}
