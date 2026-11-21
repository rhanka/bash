// Limit profiles (EVOL Annex B), mirrored from kernel/src/limits.rs (a test compares the two tables).
// Column order of the rows: normal, hardened, contain, micro.
import { utf8Length } from "./bytes.js";

const MIB = 1024 * 1024;

/** Checks canonical-ABI string payloads before lowering, including fixed-size list records. */
export function stringLimit(strings, maxStringLength, headroom, overhead = 0) {
  let total = overhead;
  for (const s of strings) {
    const n = utf8Length(s);
    if (n > maxStringLength) return "maxStringLength";
    total += n;
  }
  if (total > Math.floor(headroom / 2)) return "headroom";
}

const ROWS = [
  ["maxSourceBytes", [64 * MIB, 8 * MIB, 8 * MIB, 4 * MIB]],
  ["maxExecDepth", [64, 64, 64, 64]],
  ["maxCallDepth", [100, 100, 100, 100]],
  ["maxSubstitutionDepth", [50, 50, 50, 50]],
  ["maxSourceDepth", [100, 100, 100, 100]],
  ["maxCommandCount", [100000, 10000, 100000, 25000]],
  ["maxLoopIterations", [100000, 10000, 100000, 25000]],
  ["maxAwkIterations", [100000, 10000, 100000, 10000]],
  ["maxSedIterations", [100000, 10000, 100000, 10000]],
  ["maxJqIterations", [10000000, 10000, 10000000, 10000]],
  ["maxWorkUnits", [100000000, 100000, 100000000, 1000000]],
  ["maxQueryTokens", [100000, 25000, 100000, 25000]],
  ["maxQueryDepth", [1000, 256, 1000, 256]],
  ["maxQueryElements", [1000000, 100000, 1000000, 100000]],
  ["maxAwkParserTokens", [100000, 25000, 100000, 25000]],
  ["maxAwkParserDepth", [256, 128, 256, 128]],
  ["maxAwkParserOperations", [1000000, 100000, 1000000, 100000]],
  ["maxCsvRows", [1000000, 100000, 1000000, 100000]],
  ["maxCsvCells", [10000000, 1000000, 10000000, 1000000]],
  ["maxTraversalEntries", [1000000, 100000, 1000000, 100000]],
  ["maxTraversalDepth", [1000, 256, 1000, 256]],
  ["maxTraversalWork", [1000000, 100000, 1000000, 100000]],
  ["maxGlobOperations", [1000000, 100000, 1000000, 100000]],
  ["maxArrayElements", [1000000, 100000, 1000000, 100000]],
  ["maxBraceExpansionResults", [100000, 10000, 100000, 10000]],
  ["maxFileDescriptors", [4096, 1024, 4096, 1024]],
  ["maxLiveBytes", [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB]],
  ["maxInputBytes", [512 * MIB, 32 * MIB, 64 * MIB, 16 * MIB]],
  ["maxFileSystemBytes", [1024 * MIB, 128 * MIB, 128 * MIB, 32 * MIB]],
  ["maxOutputSize", [256 * MIB, 10 * MIB, 32 * MIB, 8 * MIB]],
  ["maxStringLength", [64 * MIB, 10 * MIB, 16 * MIB, 4 * MIB]],
  ["maxHeredocSize", [64 * MIB, 10 * MIB, 16 * MIB, 4 * MIB]],
  ["maxArchiveBytes", [1024 * MIB, 128 * MIB, 128 * MIB, 32 * MIB]],
  ["maxArchiveCompressedBytes", [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB]],
  ["maxArchiveEntryBytes", [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB]],
  ["maxArchiveEntries", [1000000, 100000, 100000, 100000]],
  ["maxExecutionTimeMs", [3600000, 30000, 60000, 10000]],
  ["maxExtensionCleanupTimeMs", [100, 25, 100, 25]],
  ["maxDatabaseBytes", [0, 0, 0, 0]],
  ["maxDatabaseResultBytes", [0, 0, 0, 0]],
  ["maxWorkerMessageBytes", [0, 0, 0, 0]],
  ["maxSqliteTimeoutMs", [0, 0, 0, 0]],
  ["maxPythonTimeoutMs", [0, 0, 0, 0]],
  ["maxJsTimeoutMs", [0, 0, 0, 0]],
  ["maxJsBridgeRequests", [0, 0, 0, 0]],
];

/** Names of the limits accepted as overrides, in table order. */
export const LIMIT_NAMES = ROWS.map((r) => r[0]);

const COLUMN = { normal: 0, hardened: 1, contain: 2, micro: 3 };

/** Headroom, ceiling and `exec-oneshot` cap of each profile, in bytes. */
export const PROFILES = {
  micro: { headroom: 8 * MIB, ceiling: 64 * MIB, oneshot: 4 * MIB },
  contain: { headroom: 32 * MIB, ceiling: 256 * MIB, oneshot: 16 * MIB },
  hardened: { headroom: 54 * MIB, ceiling: 256 * MIB, oneshot: 27 * MIB },
  normal: { headroom: 64 * MIB, ceiling: 1856 * MIB, oneshot: 32 * MIB },
};

export const PROFILE_NAMES = Object.keys(PROFILES);

/** The default profile of every environment. */
export const DEFAULT_PROFILE = "contain";

/** The values of one profile as a null-prototype object. */
export function profileLimits(profile) {
  const c = COLUMN[profile];
  const out = Object.create(null);
  for (const [name, v] of ROWS) out[name] = v[c];
  return out;
}

/** The table rows (for the consistency test). */
export function tableRows() {
  return ROWS.map(([n, v]) => [n, [...v]]);
}

/** Sum of the byte caps that must fit the ceiling: filesystem + live + output + headroom. */
export function byteSum(limits, profile) {
  return limits.maxFileSystemBytes + limits.maxLiveBytes + limits.maxOutputSize + PROFILES[profile].headroom;
}

/**
 * Resolves a profile and overrides. Throws TypeError for an unknown name or a value that is not a
 * non-negative integer, RangeError for a value above 2^53 - 1 or a set of byte caps that exceeds `ceiling`.
 */
export function resolveLimits(profile, overrides, ceiling) {
  if (!Object.hasOwn(PROFILES, profile)) throw new TypeError(`executionLimitProfile: unknown profile ${String(profile)}`);
  const limits = profileLimits(profile);
  if (overrides !== undefined) {
    if (overrides === null || typeof overrides !== "object") throw new TypeError("executionLimits must be an object");
    for (const [name, value] of Object.entries(overrides)) {
      if (!(name in limits)) throw new TypeError(`executionLimits: unknown limit ${name}`);
      if (typeof value !== "number" || !Number.isInteger(value) || value < 0) throw new TypeError(`executionLimits.${name} must be a non-negative integer`);
      if (!Number.isSafeInteger(value)) throw new RangeError(`executionLimits.${name} is above 2^53 - 1`);
      limits[name] = value;
    }
  }
  if (byteSum(limits, profile) > ceiling) {
    throw new RangeError("executionLimits: maxFileSystemBytes + maxLiveBytes + maxOutputSize + headroom exceeds the memory ceiling");
  }
  return limits;
}
