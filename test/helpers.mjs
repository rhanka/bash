import { mkdirSync, mkdtempSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/** A fresh scratch directory under the git-ignored tmp/ of the repository. */
export function scratch(prefix) {
  const base = join(repoRoot, "tmp");
  mkdirSync(base, { recursive: true });
  return mkdtempSync(join(base, `${prefix}-`));
}
