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

// Deduplicated quads: 23 distinct 4-tuples back the 45 rows, so the shipped table
// carries each number once instead of repeating shared tuples.
const Q = [
  [64 * MIB, 8 * MIB, 8 * MIB, 4 * MIB],
  [64, 64, 64, 64],
  [100, 100, 100, 100],
  [50, 50, 50, 50],
  [100000, 10000, 100000, 25000],
  [100000, 10000, 100000, 10000],
  [10000000, 10000, 10000000, 10000],
  [100000000, 100000, 100000000, 1000000],
  [100000, 25000, 100000, 25000],
  [1000, 256, 1000, 256],
  [1000000, 100000, 1000000, 100000],
  [256, 128, 256, 128],
  [10000000, 1000000, 10000000, 1000000],
  [4096, 1024, 4096, 1024],
  [512 * MIB, 64 * MIB, 64 * MIB, 16 * MIB],
  [512 * MIB, 32 * MIB, 64 * MIB, 16 * MIB],
  [1024 * MIB, 128 * MIB, 128 * MIB, 32 * MIB],
  [256 * MIB, 10 * MIB, 32 * MIB, 8 * MIB],
  [64 * MIB, 10 * MIB, 16 * MIB, 4 * MIB],
  [1000000, 100000, 100000, 100000],
  [3600000, 30000, 60000, 10000],
  [100, 25, 100, 25],
  [0, 0, 0, 0],
];

const ROWS = [
  ["maxSourceBytes", 0],
  ["maxExecDepth", 1],
  ["maxCallDepth", 2],
  ["maxSubstitutionDepth", 3],
  ["maxSourceDepth", 2],
  ["maxCommandCount", 4],
  ["maxLoopIterations", 4],
  ["maxAwkIterations", 5],
  ["maxSedIterations", 5],
  ["maxJqIterations", 6],
  ["maxWorkUnits", 7],
  ["maxQueryTokens", 8],
  ["maxQueryDepth", 9],
  ["maxQueryElements", 10],
  ["maxAwkParserTokens", 8],
  ["maxAwkParserDepth", 11],
  ["maxAwkParserOperations", 10],
  ["maxCsvRows", 10],
  ["maxCsvCells", 12],
  ["maxTraversalEntries", 10],
  ["maxTraversalDepth", 9],
  ["maxTraversalWork", 10],
  ["maxGlobOperations", 10],
  ["maxArrayElements", 10],
  ["maxBraceExpansionResults", 5],
  ["maxFileDescriptors", 13],
  ["maxLiveBytes", 14],
  ["maxInputBytes", 15],
  ["maxFileSystemBytes", 16],
  ["maxOutputSize", 17],
  ["maxStringLength", 18],
  ["maxHeredocSize", 18],
  ["maxArchiveBytes", 16],
  ["maxArchiveCompressedBytes", 14],
  ["maxArchiveEntryBytes", 14],
  ["maxArchiveEntries", 19],
  ["maxExecutionTimeMs", 20],
  ["maxExtensionCleanupTimeMs", 21],
  ["maxDatabaseBytes", 22],
  ["maxDatabaseResultBytes", 22],
  ["maxWorkerMessageBytes", 22],
  ["maxSqliteTimeoutMs", 22],
  ["maxPythonTimeoutMs", 22],
  ["maxJsTimeoutMs", 22],
  ["maxJsBridgeRequests", 22],
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
  for (const [name, qi] of ROWS) out[name] = Q[qi][c];
  return out;
}

/** The table rows (for the consistency test). */
export function tableRows() {
  return ROWS.map(([n, qi]) => [n, [...Q[qi]]]);
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
