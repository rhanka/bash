// Canonical ABI value validation, staging and wasm32 memory access.
import { utf8Length } from "../bytes.js";
import { PROFILES } from "../limits.js";

const U32_MAX = 0xffffffff;
const MIB = 1 << 20;
const TRANSFER_BUDGET = 64 * MIB;
const typedArray = Object.getPrototypeOf(Uint8Array.prototype);
const arrayKind = Object.getOwnPropertyDescriptor(typedArray, Symbol.toStringTag).get;
const arrayLength = Object.getOwnPropertyDescriptor(typedArray, "length").get;
const arrayValues = typedArray.values;
const memoryMethods = {
  bool: "Uint8", u8: "Uint8", u16: "Uint16", u32: "Uint32", s32: "Int32",
  u64: "BigUint64", s64: "BigInt64", f64: "Float64", own: "Uint32", borrow: "Uint32",
};
const integerBounds = {
  u8: [0, 0xff], u16: [0, 0xffff], u32: [0, U32_MAX], s32: [-0x80000000, 0x7fffffff],
  u64: [0n, (1n << 64n) - 1n], s64: [-(1n << 63n), (1n << 63n) - 1n],
};

export const trap = (detail) => new WebAssembly.RuntimeError("canonical ABI: " + detail);
const badType = (path, expected) => new TypeError(path + " must be " + expected);
const badRange = (path) => new RangeError(path + " is outside the ABI bounds");

export function unsigned32(value) {
  if (!Number.isInteger(value) || value < -0x80000000 || value > U32_MAX) throw trap("invalid i32 value");
  return value >>> 0;
}

function integer(kind, value, path) {
  const [min, max] = integerBounds[kind];
  if (typeof value !== typeof min) throw badType(path, typeof min === "bigint" ? "a BigInt" : "a number");
  if (value < min || value > max || (typeof value === "number" && !Number.isInteger(value))) throw badRange(path);
  return value;
}

function object(value, path) {
  if (!value || typeof value !== "object" || Array.isArray(value) || ArrayBuffer.isView(value)) {
    throw badType(path, "an object");
  }
}

function field(value, key, path, optional) {
  if (Object.hasOwn(value, key)) return value[key];
  if (!optional) throw badType(path, "present");
}

function indexOf(labels, value, path) {
  const index = labels.indexOf(value);
  if (index === -1) throw badType(path, "a known tag");
  return index;
}

function memoryMethod(type) {
  return memoryMethods[type.kind] ?? (type.size === 1 ? "Uint8" : type.size === 2 ? "Uint16" : "Uint32");
}

function listArray(type) {
  if (type.element.kind === "u8") return Uint8Array;
  if (type.element.kind === "u16") return Uint16Array;
}

export class Codec {
  constructor({ core, borrow, own, lowerBudget = TRANSFER_BUDGET, liftBudget = TRANSFER_BUDGET, oneshotBytes }) {
    this.core = core;
    this.borrow = borrow;
    this.own = own;
    this.lowerBudget = lowerBudget;
    this.liftBudget = liftBudget;
    this.oneshotBytes = oneshotBytes;
  }

  charge(size, path, maxBytes) {
    if (size > this.lowerBudget || size > (maxBytes ?? U32_MAX)) throw badRange(path);
    this.lowerBudget -= size;
  }

  // This phase may run caller getters. Every staged value used by the writer is independent of them.
  stage(type, value, path, maxBytes) {
    switch (type.kind) {
      case "unit":
        if (value !== undefined) throw badType(path, "undefined");
        return;
      case "bool":
        if (typeof value !== "boolean") throw badType(path, "a boolean");
        return Number(value);
      case "u8": case "u16": case "u32": case "s32": case "u64": case "s64":
        return integer(type.kind, value, path);
      case "f64":
        if (typeof value !== "number") throw badType(path, "a number");
        return value;
      case "string": {
        if (typeof value !== "string") throw badType(path, "a string");
        if (value.length > this.lowerBudget) throw badRange(path);
        this.charge(utf8Length(value), path, maxBytes);
        return new TextEncoder().encode(value);
      }
      case "list": {
        const ArrayType = listArray(type);
        if (ArrayType) {
          if (arrayKind.call(value) !== ArrayType.name) throw badType(path, ArrayType.name);
          // Validate the typed-array view itself, even when its reported length is zero.
          arrayValues.call(value);
          this.charge(arrayLength.call(value) * type.element.size, path, maxBytes);
          return new ArrayType(value);
        }
        if (!Array.isArray(value)) throw badType(path, "an array");
        const length = integer("u32", value.length, path + ".length");
        this.charge(length * Math.max(1, type.element.size), path, maxBytes);
        const staged = new Array(length);
        for (let i = 0; i < length; i++) {
          const label = path + "[" + i + "]";
          staged[i] = this.stage(type.element, field(value, i, label), label);
        }
        return staged;
      }
      case "record": {
        object(value, path);
        const stageField = ({ key, type: child, maxBytes: cap }) => {
          const label = path + "." + key;
          const staged = this.stage(child, field(value, key, label, child.kind === "option"), label, cap);
          if (this.oneshotBytes !== undefined && type.name === "limits" && key === "profile") {
            this.lowerBudget = PROFILES[child.labels[staged]].oneshot - this.oneshotBytes;
          }
          return staged;
        };
        if (this.oneshotBytes !== undefined && type.name === "oneshot-req") {
          // Select the profile budget before copying files. Keep the staged tuple in WIT order.
          const config = type.fields.find(({ key }) => key === "config");
          const staged = stageField(config);
          return type.fields.map((member) => member === config ? staged : stageField(member));
        }
        return type.fields.map(stageField);
      }
      case "enum": return indexOf(type.labels, value, path);
      case "flags": {
        object(value, path);
        let flags = 0;
        for (const key of Reflect.ownKeys(value)) {
          const bit = type.labels.indexOf(key);
          if (bit === -1) throw badType(path, "an object with known flag names");
          const enabled = value[key];
          if (typeof enabled !== "boolean") throw badType(path + "." + key, "a boolean");
          if (enabled) flags += 2 ** bit;
        }
        return flags;
      }
      case "option":
        return value === undefined ? [0] : [1, this.stage(type.cases[1].type, value, path, maxBytes)];
      case "variant":
      case "result": {
        object(value, path);
        const tag = field(value, "tag", path + ".tag");
        const index = type.cases.findIndex((c) => c.tag === tag);
        if (index === -1) throw badType(path + ".tag", "a known tag");
        const child = type.cases[index].type;
        if (child.kind === "unit") {
          if (Object.hasOwn(value, "val")) throw badType(path, "a tag without a payload");
          return [index];
        }
        return [index, this.stage(child, field(value, "val", path + ".val"), path + ".val")];
      }
      case "borrow": return this.borrow(type.resource, value, path);
      default: throw new Error("cannot lower ABI type " + type.kind);
    }
  }

  stageParams(type, values, name, receiver) {
    return type.fields.map(({ key, type: child, maxBytes }, i) => {
      const value = receiver && i === 0 ? receiver.value : values[i - Number(Boolean(receiver))];
      return this.stage(child, value, name + "." + key, maxBytes);
    });
  }

  // Recheck the current buffer after any call or allocation that could grow memory.
  view(ptr, count, stride, align) {
    const buffer = this.core.memory.buffer;
    if (!Number.isInteger(ptr) || ptr < 0 || ptr > U32_MAX || ptr % align !== 0 ||
        !Number.isInteger(count) || count < 0 || count > U32_MAX || ptr > buffer.byteLength ||
        (stride && count > Math.floor((buffer.byteLength - ptr) / stride))) {
      throw trap("misaligned or out-of-bounds memory span");
    }
    if (this.data?.buffer !== buffer) this.data = new DataView(buffer);
    return this.data;
  }

  allocate(size, align) {
    if (!size) return 0;
    if (!Number.isSafeInteger(size) || size < 0 || size > U32_MAX) throw trap("invalid allocation size");
    const ptr = unsigned32(this.core.cabi_realloc(0, 0, align, size));
    if (ptr === 0) throw trap("allocator returned null");
    this.view(ptr, 1, size, align);
    return ptr;
  }

  lowerSequence(type, value) {
    const length = value.length;
    const element = type.element;
    const stride = element?.size ?? 1;
    const ptr = this.allocate(length * stride, element?.align ?? 1);
    if (type.kind === "string" || element.kind === "u8") {
      const { buffer } = this.view(ptr, length, 1, 1);
      new Uint8Array(buffer, ptr, length).set(value);
    } else {
      this.view(ptr, length, stride, element.align);
      for (let i = 0; i < length; i++) this.write(element, value[i], ptr + i * stride);
    }
    return [ptr, length];
  }

  write(type, value, ptr) {
    const data = this.view(ptr, 1, type.size, type.align);
    switch (type.kind) {
      case "unit": return;
      case "string":
      case "list": {
        const [address, length] = this.lowerSequence(type, value);
        const current = this.view(ptr, 1, type.size, type.align);
        current.setUint32(ptr, address, true);
        current.setUint32(ptr + 4, length, true);
        return;
      }
      case "record":
        type.fields.forEach(({ type: child, offset }, i) => this.write(child, value[i], ptr + offset));
        return;
      case "variant": case "option": case "result": {
        const [index, payload] = value;
        data["set" + memoryMethod(type.tag)](ptr, index, true);
        const child = type.cases[index].type;
        if (child.kind !== "unit") this.write(child, payload, ptr + type.payloadOffset);
        return;
      }
      default:
        data["set" + memoryMethod(type)](ptr, value, true);
    }
  }

  cast(value, from, to) {
    if (from === to) return value;
    if (from === "i32" && to === "i64") return BigInt(unsigned32(value));
    if (from === "i64" && to === "i32") return Number(BigInt.asUintN(32, value));
    const bits = this.bits ??= new DataView(new ArrayBuffer(8));
    if (from === "f64" && to === "i64") {
      bits.setFloat64(0, value, true);
      return bits.getBigInt64(0, true);
    }
    if (from === "i64" && to === "f64") {
      bits.setBigInt64(0, value, true);
      return bits.getFloat64(0, true);
    }
    throw trap("unsupported flat bitcast");
  }

  flatten(type, value, out) {
    switch (type.kind) {
      case "unit": return;
      case "list":
      case "string": out.push(...this.lowerSequence(type, value)); return;
      case "record":
        type.fields.forEach(({ type: child }, i) => this.flatten(child, value[i], out));
        return;
      case "variant": case "option": case "result": {
        const [index, payload] = value;
        const child = type.cases[index].type;
        const slots = [];
        this.flatten(child, payload, slots);
        out.push(index);
        for (let i = 1; i < type.flat.length; i++) {
          const to = type.flat[i];
          out.push(i <= slots.length ? this.cast(slots[i - 1], child.flat[i - 1], to) : to === "i64" ? 0n : 0);
        }
        return;
      }
      default: out.push(value);
    }
  }

  liftSequence(type, ptr, count, maxBytes = U32_MAX) {
    const element = type.element;
    const stride = element?.size ?? 1;
    const data = this.view(ptr, count, stride, element?.align ?? 1);
    const size = count * Math.max(1, stride);
    if (size > this.liftBudget || size > maxBytes) throw trap("result exceeds the transfer budget");
    this.liftBudget -= size;
    if (type.kind === "string" || element.kind === "u8") {
      const bytes = new Uint8Array(data.buffer, ptr, count);
      return type.kind === "string" ? new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes)
        : new Uint8Array(bytes);
    }
    if (element.kind === "u16") {
      const out = new Uint16Array(count);
      for (let i = 0; i < count; i++) out[i] = data.getUint16(ptr + i * 2, true);
      return out;
    }
    const out = new Array(count);
    for (let i = 0; i < count; i++) out[i] = this.read(element, ptr + i * stride);
    return out;
  }

  liftScalar(type, raw) {
    switch (type.kind) {
      case "bool":
        if (raw !== 0 && raw !== 1) throw trap("invalid bool discriminant");
        return raw === 1;
      case "enum":
        if (raw >= type.labels.length) throw trap("invalid enum discriminant");
        return type.labels[raw];
      case "flags": {
        if ((raw & ~type.mask) !== 0) throw trap("unknown flag bits");
        return Object.fromEntries(type.labels.map((key, i) => [key, Boolean(raw & (1 << i))]));
      }
      case "own": return this.own(type.resource, raw);
      case "borrow": throw trap("unexpected borrowed return");
      default: return raw;
    }
  }

  liftCase(type, index, read) {
    const arm = type.cases[index];
    if (!arm) throw trap("invalid " + type.kind + " discriminant");
    if (type.kind === "option") return index === 0 ? undefined : read(arm.type);
    return arm.type.kind === "unit" ? { tag: arm.tag } : { tag: arm.tag, val: read(arm.type) };
  }

  read(type, ptr, maxBytes) {
    const data = this.view(ptr, 1, type.size, type.align);
    switch (type.kind) {
      case "unit": return;
      case "list": case "string":
        return this.liftSequence(type, data.getUint32(ptr, true), data.getUint32(ptr + 4, true), maxBytes);
      case "record":
        return Object.fromEntries(type.fields.map(({ key, type: child, offset, maxBytes: cap }) =>
          [key, this.read(child, ptr + offset, cap)]));
      case "variant": case "option": case "result":
        return this.liftCase(type, data["get" + memoryMethod(type.tag)](ptr, true),
          (child) => this.read(child, ptr + type.payloadOffset, maxBytes));
      default: return this.liftScalar(type, data["get" + memoryMethod(type)](ptr, true));
    }
  }

  liftFlat(type, slots, start = 0) {
    const raw = slots[start];
    switch (type.kind) {
      case "unit": return;
      case "list":
      case "string": return this.liftSequence(type, unsigned32(raw), unsigned32(slots[start + 1]));
      case "record": {
        let pos = start;
        return Object.fromEntries(type.fields.map(({ key, type: child }) => {
          const value = this.liftFlat(child, slots, pos);
          pos += child.flat.length;
          return [key, value];
        }));
      }
      case "variant": case "option": case "result":
        return this.liftCase(type, unsigned32(raw), (child) =>
          this.liftFlat(child, child.flat.map((slot, i) => this.cast(slots[start + i + 1], type.flat[i + 1], slot))));
      case "u64": return BigInt.asUintN(64, raw);
      case "s64": return BigInt.asIntN(64, raw);
      case "s32": return unsigned32(raw) | 0;
      case "u8": return unsigned32(raw) & 0xff;
      case "u16": return unsigned32(raw) & 0xffff;
      case "f64": return raw;
      default: return this.liftScalar(type, unsigned32(raw));
    }
  }
}

