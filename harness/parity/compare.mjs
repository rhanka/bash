// Structural comparison of a snapshot against a fresh result, with first-difference reporting.

const MAX_WINDOW = 40;

function esc(u) {
  if (u === 0x0a) return "\\n";
  if (u === 0x0d) return "\\r";
  if (u === 0x09) return "\\t";
  if (u === 0x5c) return "\\\\";
  if (u >= 0x20 && u <= 0x7e) return String.fromCharCode(u);
  return u <= 0xff ? `\\x${u.toString(16).padStart(2, "0")}` : `\\u${u.toString(16).padStart(4, "0")}`;
}

function windowOf(units, idx) {
  const from = Math.max(0, idx - MAX_WINDOW);
  const to = Math.min(units.length, idx + MAX_WINDOW + 1);
  return (from > 0 ? "..." : "") + units.slice(from, to).map(esc).join("") + (to < units.length ? "..." : "");
}

function strUnits(s) {
  const out = [];
  for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i));
  return out;
}

function firstDiff(a, b) {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i;
  return a.length === b.length ? -1 : n;
}

function describeUnits(path, unit, exp, act) {
  const i = firstDiff(exp, act);
  return `${path}: first difference at ${unit} ${i}; expected ${windowOf(exp, i)} (length ${exp.length}); actual ${windowOf(act, i)} (length ${act.length})`;
}

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isBase64Obj = (v) => isObj(v) && typeof v.base64 === "string" && Object.keys(v).length === 1;
const short = (v) => {
  const s = JSON.stringify(v);
  return s === undefined ? "undefined" : s.length > 80 ? s.slice(0, 77) + "..." : s;
};

function keyPath(base, key) {
  if (base === "") return key;
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(key) ? `${base}.${key}` : `${base}[${JSON.stringify(key)}]`;
}

/**
 * Differences between `expected` (snapshot) and `actual`, in walk order. Each is `{path, message}`.
 * Paths look like `steps[1].stderr`, `fs["/a/b"].content`, `coverage[17]`.
 */
export function compare(expected, actual, path = "", out = []) {
  if (typeof expected === "string" && typeof actual === "string") {
    if (expected !== actual) out.push({ path, message: describeUnits(path, "code unit", strUnits(expected), strUnits(actual)) });
  } else if (isBase64Obj(expected) && isBase64Obj(actual)) {
    if (expected.base64 !== actual.base64) {
      const e = [...Buffer.from(expected.base64, "base64")];
      const a = [...Buffer.from(actual.base64, "base64")];
      out.push({ path, message: describeUnits(path, "byte", e, a) });
    }
  } else if (Array.isArray(expected) && Array.isArray(actual)) {
    const n = Math.max(expected.length, actual.length);
    for (let i = 0; i < n; i++) {
      const p = `${path}[${i}]`;
      if (i >= expected.length) out.push({ path: p, message: `${p}: unexpected extra element ${short(actual[i])}` });
      else if (i >= actual.length) out.push({ path: p, message: `${p}: missing element, expected ${short(expected[i])}` });
      else compare(expected[i], actual[i], p, out);
    }
  } else if (isObj(expected) && isObj(actual)) {
    const keys = [...new Set([...Object.keys(expected), ...Object.keys(actual)])].sort();
    for (const k of keys) {
      const p = keyPath(path, k);
      if (!(k in actual)) out.push({ path: p, message: `${p}: missing in actual, expected ${short(expected[k])}` });
      else if (!(k in expected)) out.push({ path: p, message: `${p}: unexpected in actual, got ${short(actual[k])}` });
      else compare(expected[k], actual[k], p, out);
    }
  } else if (expected !== actual) {
    out.push({ path, message: `${path}: expected ${short(expected)}; actual ${short(actual)}` });
  }
  return out;
}

/** Formats a difference list: the first with its window, the others by path only. */
export function formatDiffs(diffs) {
  if (diffs.length === 0) return "";
  const rest = diffs.slice(1).map((d) => d.path);
  return diffs[0].message + (rest.length ? `\nalso differing: ${rest.join(", ")}` : "");
}
