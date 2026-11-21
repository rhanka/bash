// Canonical JSON: keys sorted by code unit at every level, two-space indent, "\n" line ends,
// one trailing newline. Writing the same value twice yields identical bytes.
import { createHash } from "node:crypto";

function sortValue(v) {
  if (Array.isArray(v)) return v.map(sortValue);
  if (v !== null && typeof v === "object") {
    const out = Object.create(null);
    for (const k of Object.keys(v).sort()) out[k] = sortValue(v[k]);
    return out;
  }
  return v;
}

/** @param {unknown} value @returns {string} */
export function canonicalize(value) {
  return JSON.stringify(sortValue(value), null, 2) + "\n";
}

/** @param {string | Uint8Array} data @returns {string} hex SHA-256 */
export function sha256(data) {
  return createHash("sha256").update(data).digest("hex");
}

/** Hash of the canonical bytes of a value. */
export function hashValue(value) {
  return sha256(Buffer.from(canonicalize(value), "utf8"));
}
