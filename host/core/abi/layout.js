// wasm32 Canonical ABI memory and flat layouts for the committed WIT graph.
import { descriptors } from "./descriptors.js";

export const namespace = descriptors.namespace;
export const alignTo = (size, align) => Math.ceil(size / align) * align;
export const camelCase = (name) => name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());

const scalar = (kind, size, slot = "i32") => ({ kind, size, align: size || 1, flat: size ? [slot] : [] });
const MIB = 1 << 20;
const unit = scalar("unit", 0);
const sequence = { size: 8, align: 4, flat: ["i32", "i32"], dynamic: true };
const primitives = new Map([
  ["bool", scalar("bool", 1)],
  ["u8", scalar("u8", 1)],
  ["u16", scalar("u16", 2)],
  ["u32", scalar("u32", 4)],
  ["s32", scalar("s32", 4)],
  ["u64", scalar("u64", 8, "i64")],
  ["s64", scalar("s64", 8, "i64")],
  ["f64", scalar("f64", 8, "f64")],
  ["string", { kind: "string", ...sequence }],
]);
const definitions = new Map(descriptors.types);
const compiled = new Map();

function freeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) freeze(child);
    Object.freeze(value);
  }
  return value;
}

function discriminant(count) {
  if (count <= 0x100) return scalar("u8", 1);
  if (count <= 0x10000) return scalar("u16", 2);
  return scalar("u32", 4);
}

function join(a, b) {
  // This WIT uses i32, i64 and f64 flat slots.
  return !a || a === b ? b : "i64";
}

function tuple(fields, recordName) {
  let size = 0;
  let align = 1;
  const flat = [];
  const members = fields.map(([name, ref]) => {
    const type = layout(ref);
    const offset = alignTo(size, type.align);
    size = offset + type.size;
    align = Math.max(align, type.align);
    flat.push(...type.flat);

    // Streaming byte payloads and the script have their own transfer limits.
    let maxBytes;
    if (recordName === "exec-opts" && name === "script") maxBytes = 3 * MIB;
    if (recordName === "write-req" && name === "data") maxBytes = MIB;
    if ((recordName === "[method]fiber.write-stdin" || recordName === "[method]fiber.push-call-data") &&
        name === "chunk") maxBytes = MIB;

    return { key: camelCase(name), type, offset, maxBytes };
  });
  return { kind: "record", name: recordName, size: alignTo(size, align), align, flat, fields: members,
    dynamic: members.some(({ type }) => type.dynamic) };
}

function sum(kind, variants) {
  const cases = variants.map(([tag, ref]) => ({ tag, type: layout(ref) }));
  const tag = discriminant(cases.length);
  const payload = [];
  let payloadAlign = 1;
  let payloadSize = 0;
  for (const { type } of cases) {
    payloadAlign = Math.max(payloadAlign, type.align);
    payloadSize = Math.max(payloadSize, type.size);
    type.flat.forEach((slot, i) => { payload[i] = join(payload[i], slot); });
  }
  const align = Math.max(tag.align, payloadAlign);
  const payloadOffset = alignTo(tag.size, payloadAlign);
  return { kind, tag, cases, size: alignTo(payloadOffset + payloadSize, align), align,
    payloadOffset, flat: ["i32", ...payload], dynamic: cases.some(({ type }) => type.dynamic) };
}

function compile(ref, name) {
  const [kind, first, second] = ref;
  switch (kind) {
    case "list": return { kind, ...sequence, element: layout(first) };
    case "record": return tuple(first, name);
    case "enum": return { ...discriminant(first.length), kind, labels: first };
    case "flags": {
      // The WIT graph has a single flags type with fewer than eight members.
      // A wider flags type needs an explicit layout change and conformance cases.
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
}

export function layout(ref) {
  if (ref === null) return freeze(unit);
  if (compiled.has(ref)) return compiled.get(ref);
  const type = typeof ref === "string"
    ? primitives.get(ref) ?? compile(definitions.get(ref), ref)
    : compile(ref);
  // Only WIT's named types and primitives are cached. Fixture descriptors can have a caller lifetime.
  if (typeof ref === "string") compiled.set(ref, freeze(type));
  return freeze(type);
}

export const resourceKinds = freeze(descriptors.engineTypes
  .filter(([, type]) => type === "resource").map(([name]) => name));

const streamedResults = {
  "[method]fiber.read-call-input": 1,
  "[method]fiber.take-stdout": 1,
  "[method]fiber.take-stderr": 1,
  "[method]fiber.take-coverage": 2,
};

export const functions = freeze(descriptors.functions.map(([name, fields, resultRef]) => {
  const params = tuple(fields, name);
  const result = layout(resultRef);
  const indirectParams = params.flat.length > 16;
  const indirectResult = result.flat.length > 1;
  const exportName = namespace + "#" + name;
  return { name, exportName, postName: result.dynamic ? "cabi_post_" + exportName : undefined,
    params, indirectParams, result, indirectResult,
    unwrap: Array.isArray(resultRef) && resultRef[0] === "result",
    streamStride: streamedResults[name],
    coreParams: indirectParams ? ["i32"] : params.flat,
    coreResults: indirectResult ? ["i32"] : result.flat };
}));

