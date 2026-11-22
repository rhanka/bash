// Eligibility of a validated shell configuration for the Wasm-owned default snapshot
// (spec L2c section 4.5): `contain`, the standard ceiling, zero overrides including aliases,
// default layout/cwd, no supplied FS/files/env/commands/customCommands/processInfo, no
// sleep/fetch/network/grant changes, no trace/coverage and ordinary default runtime options.
// A validated host-only logger may be present. Unknown/invalid options keep their synchronous
// constructor errors upstream; this predicate only classifies valid configurations, performs no
// I/O, registers no callbacks, allocates nothing in the guest and never probes arity.
import { PROFILES } from "./limits.js";

/** The ceiling a restorable store must have been captured with (the explicit default cap). */
export const SNAPSHOT_CEILING = 268435456;

if (PROFILES.contain.ceiling !== SNAPSHOT_CEILING) {
  throw new Error("the contain ceiling no longer matches the snapshot default cap");
}

/**
 * @param {object} summary the already-validated, single-read configuration:
 *   `{ profile, overridesEmpty, ceiling, hasHooks, present, loggerPresent }` where `present`
 *   maps each classified option name to whether it was supplied (not `undefined`).
 */
export function isDefaultSnapshotConfig(summary) {
  if (summary.hasHooks) return false;
  if (summary.ceiling !== SNAPSHOT_CEILING) return false;
  if (summary.profile !== "contain") return false;
  if (!summary.overridesEmpty) return false;
  for (const key of Object.keys(summary.present)) {
    if (summary.present[key]) return false;
  }
  void summary.loggerPresent; // a validated host-only logger may be present
  return true;
}

/** The option names the predicate classifies; every other own key is rejected upstream. */
export const PREDICATE_OPTIONS = Object.freeze([
  "files",
  "env",
  "cwd",
  "fs",
  "commands",
  "customCommands",
  "processInfo",
  "sleep",
  "fetch",
  "network",
  "python",
  "javascript",
  "trace",
  "coverage",
  "defenseInDepth",
]);
