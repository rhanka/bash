// Precompiled ABI layouts from flat numeric/op tables (generated at build time).
// The shipped JS decodes tables.js instead of the WIT AST (descriptors.js) and has no
// layout compiler for named types; layout() only supports test-fixture descriptors.
import { F, M, N, S, T, TN, limitsIdx, tableNamespace, tableResourceKinds } from "./tables.js";

export const namespace = tableNamespace;
export const resourceKinds = tableResourceKinds;
export const alignTo = (size, align) => Math.ceil(size / align) * align;
export const camelCase = (name) => name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());

const scalar = (kind, size, slot = "i32") => ({ kind, size, align: size || 1, flat: size ? [slot] : [] });
const unit = scalar("unit", 0);
const sequence = { size: 8, align: 4, flat: ["i32", "i32"], dynamic: true };
// Primitive layouts by numeric code: 0 bool, 1 u8, 2 u16, 3 u32, 4 s32, 5 u64, 6 s64, 7 f64, 8 string, 9 unit.
const P = [
  scalar("bool", 1),
  scalar("u8", 1),
  scalar("u16", 2),
  scalar("u32", 4),
  scalar("s32", 4),
  scalar("u64", 8, "i64"),
  scalar("s64", 8, "i64"),
  scalar("f64", 8, "f64"),
  { kind: "string", ...sequence },
  unit,
];
// Test-only string lookups live inside layout() so shipped DCE drops TN/N entirely.

function discriminant(count) {
  if (count <= 0x100) return scalar("u8", 1);
  if (count <= 0x10000) return scalar("u16", 2);
  return scalar("u32", 4);
}

function join(a, b) {
  // This WIT uses i32, i64 and f64 flat slots.
  return !a || a === b ? b : "i64";
}

const cache = new Map();

const capByField = new Map();
for (const [typeIdx, fieldPos, maxBytes] of M) {
  let m = capByField.get(typeIdx);
  if (!m) capByField.set(typeIdx, m = new Map());
  m.set(fieldPos, maxBytes);
}

function buildRecord(entry, idx) {
  // Flat entry: [11, fieldCount, k1, t1, k2, t2, ...]; caps live in M.
  let size = 0;
  let align = 1;
  const flat = [];
  let dynamic = false;
  const members = [];
  const count = entry[1];
  const caps = capByField.get(idx);
  let p = 2;
  for (let i = 0; i < count; i++) {
    const keyIdx = entry[p++];
    const typeIdx = entry[p++];
    const type = getType(typeIdx);
    const offset = alignTo(size, type.align);
    size = offset + type.size;
    align = Math.max(align, type.align);
    flat.push(...type.flat);
    dynamic ||= !!type.dynamic;
    const member = { key: S[keyIdx], type, offset };
    const maxBytes = caps?.get(i);
    if (maxBytes) member.maxBytes = maxBytes;
    members.push(member);
  }
  const out = { kind: "record", size: alignTo(size, align), align, flat, fields: members };
  if (idx === limitsIdx) out.name = "limits";
  if (dynamic) out.dynamic = true;
  return out;
}

function buildSum(kind, entry) {
  // Flat variant entry: [14, caseCount, tag1, t1, tag2, t2, ...].
  const count = entry[1];
  const resolved = [];
  let p = 2;
  for (let i = 0; i < count; i++) {
    const tagIdx = entry[p++];
    const typeIdx = entry[p++];
    resolved.push({ tag: S[tagIdx], type: getType(typeIdx) });
  }
  const tag = discriminant(resolved.length);
  const payload = [];
  let payloadAlign = 1;
  let payloadSize = 0;
  let dynamic = false;
  for (const { type } of resolved) {
    payloadAlign = Math.max(payloadAlign, type.align);
    payloadSize = Math.max(payloadSize, type.size);
    dynamic ||= !!type.dynamic;
    type.flat.forEach((slot, i) => { payload[i] = join(payload[i], slot); });
  }
  const align = Math.max(tag.align, payloadAlign);
  const payloadOffset = alignTo(tag.size, payloadAlign);
  const out = { kind, tag, cases: resolved, size: alignTo(payloadOffset + payloadSize, align), align,
    payloadOffset, flat: ["i32", ...payload] };
  if (dynamic) out.dynamic = true;
  return out;
}

function getType(idx) {
  if (idx < 10) return P[idx];
  const hit = cache.get(idx);
  if (hit) return hit;
  const e = T[idx - 10];
  const kind = e[0];
  let out;
  if (kind === 10) {
    const element = getType(e[1]);
    out = { kind: "list", size: 8, align: 4, flat: ["i32", "i32"], dynamic: true, element };
  } else if (kind === 11) {
    out = buildRecord(e, idx);
  } else if (kind === 12) {
    const count = e[1];
    const labels = [];
    for (let i = 0; i < count; i++) labels.push(S[e[2 + i]]);
    out = { ...discriminant(labels.length), kind: "enum", labels };
  } else if (kind === 13) {
    const count = e[1];
    const labels = [];
    for (let i = 0; i < count; i++) labels.push(S[e[2 + i]]);
    if (labels.length < 1 || labels.length > 32) throw new Error("unsupported flags width");
    const size = labels.length <= 8 ? 1 : labels.length <= 16 ? 2 : 4;
    out = { ...scalar("flags", size), labels, mask: (2 ** labels.length - 1) >>> 0 };
  } else if (kind === 14) {
    out = buildSum("variant", e);
  } else if (kind === 15) {
    out = buildSum("option", e);
  } else if (kind === 16) {
    out = buildSum("result", e);
  } else if (kind === 17 || kind === 18) {
    out = { ...scalar(kind === 17 ? "own" : "borrow", 4), resource: resourceKinds[e[1]] };
  } else {
    throw new Error("unsupported ABI table entry " + kind);
  }
  cache.set(idx, out);
  return out;
}

// Test-only descriptor path. Everything in here (including TN/N) is unreachable from the
// shipped entries, so the bundler drops it with layout() via DCE.
export function layout(ref) {
  // Lazy per-process caches, built only when a test calls layout().
  const cache = layout.cache ??= { prim: null, named: null, fixtures: new Map() };
  if (!cache.prim) {
    cache.prim = new Map([
      ["bool", 0], ["u8", 1], ["u16", 2], ["u32", 3], ["s32", 4],
      ["u64", 5], ["s64", 6], ["f64", 7], ["string", 8],
    ]);
    cache.named = new Map(N.map(([nameIdx, typeIdx]) => [TN[nameIdx], typeIdx]));
  }
  const tuple = (fields, recordName) => {
    let size = 0;
    let align = 1;
    const flat = [];
    let dynamic = false;
    const members = fields.map(([name, r]) => {
      const type = layout(r);
      const offset = alignTo(size, type.align);
      size = offset + type.size;
      align = Math.max(align, type.align);
      flat.push(...type.flat);
      dynamic ||= !!type.dynamic;
      let maxBytes;
      if (recordName === "exec-opts" && name === "script") maxBytes = 3 * (1 << 20);
      if (recordName === "write-req" && name === "data") maxBytes = 1 << 20;
      if ((recordName === "[method]fiber.write-stdin" || recordName === "[method]fiber.push-call-data") &&
        name === "chunk") maxBytes = 1 << 20;
      return { key: camelCase(name), type, offset, maxBytes };
    });
    const out = { kind: "record", name: recordName, size: alignTo(size, align), align, flat, fields: members };
    if (dynamic) out.dynamic = true;
    return out;
  };
  const sum = (kind, variants) => {
    const cases = variants.map(([tag, r]) => ({ tag, type: layout(r) }));
    const tag = discriminant(cases.length);
    const payload = [];
    let payloadAlign = 1;
    let payloadSize = 0;
    let dynamic = false;
    for (const { type } of cases) {
      payloadAlign = Math.max(payloadAlign, type.align);
      payloadSize = Math.max(payloadSize, type.size);
      dynamic ||= !!type.dynamic;
      type.flat.forEach((slot, i) => { payload[i] = join(payload[i], slot); });
    }
    const align = Math.max(tag.align, payloadAlign);
    const payloadOffset = alignTo(tag.size, payloadAlign);
    const out = { kind, tag, cases, size: alignTo(payloadOffset + payloadSize, align), align,
      payloadOffset, flat: ["i32", ...payload] };
    if (dynamic) out.dynamic = true;
    return out;
  };
  const compile = (r, name) => {
    const [kind, first, second] = r;
    switch (kind) {
      case "list": return { kind, size: 8, align: 4, flat: ["i32", "i32"], dynamic: true, element: layout(first) };
      case "record": return tuple(first, name);
      case "enum": return { ...discriminant(first.length), kind, labels: first };
      case "flags": {
        if (first.length < 1 || first.length > 32) throw new Error("unsupported flags width");
        const size = first.length <= 8 ? 1 : first.length <= 16 ? 2 : 4;
        return { ...scalar(kind, size), labels: first.map(camelCase), mask: (2 ** first.length - 1) >>> 0 };
      }
      case "variant": return sum(kind, first);
      case "option": return sum(kind, [["none", null], ["some", first]]);
      case "result": return sum(kind, [["ok", first], ["err", second]]);
      case "own":
      case "borrow": return { ...scalar(kind, 4), resource: first };
      default: throw new Error("unsupported ABI descriptor " + kind);
    }
  };
  if (ref === null) return unit;
  if (typeof ref === "string") {
    const prim = cache.prim.get(ref);
    if (prim !== undefined) return P[prim];
    const idx = cache.named.get(ref);
    if (idx !== undefined) return getType(idx);
    if (cache.fixtures.has(ref)) return cache.fixtures.get(ref);
    throw new Error("unknown ABI type " + ref);
  }
  const key = JSON.stringify(ref);
  let hit = cache.fixtures.get(key);
  if (!hit) {
    hit = compile(ref);
    cache.fixtures.set(key, hit);
  }
  return hit;
}

export const functions = F.map((entry) => {
  // F entries: freestanding [0, nameIdx, paramsIdx, resultIdx, unwrap, stride?];
  // methods/statics [1|2, resIdx, methodIdx, paramsIdx, resultIdx, unwrap, stride?].
  let name, paramsIdx, resultIdx, unwrap, stride;
  if (entry[0] === 0) {
    name = S[entry[1]];
    paramsIdx = entry[2];
    resultIdx = entry[3];
    unwrap = entry[4];
    stride = entry[5];
  } else {
    const kind = entry[0] === 1 ? "method" : "static";
    name = "[" + kind + "]" + resourceKinds[entry[1]] + "." + S[entry[2]];
    paramsIdx = entry[3];
    resultIdx = entry[4];
    unwrap = entry[5];
    stride = entry[6];
  }
  const params = getType(paramsIdx);
  const result = getType(resultIdx);
  const indirectParams = params.flat.length > 16;
  const indirectResult = result.flat.length > 1;
  const exportName = namespace + "#" + name;
  return { name, exportName, postName: result.dynamic ? "cabi_post_" + exportName : undefined,
    params, indirectParams, result, indirectResult,
    unwrap: unwrap === 1,
    streamStride: stride,
    coreParams: indirectParams ? ["i32"] : params.flat,
    coreResults: indirectResult ? ["i32"] : result.flat };
});
