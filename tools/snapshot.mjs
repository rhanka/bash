#!/usr/bin/env node
// Deterministic Wasm-owned default snapshot (spec/lots/L2c.md section 4).
//
// Usage: node tools/snapshot.mjs <core.wasm> [<name>]
//
// Captures the real default shell setup through the first-party adapter, encodes the memory
// difference as one passive data segment, and appends a `__restore_default() -> i64` export with
// its latch global to the core module, in place. The image is installed before final component
// embed/new; capture and augmentation are byte-deterministic, so G3 stays green.
//
// No reference source, no dependency, no Python. Node built-ins only.
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join, basename } from "node:path";
import { pathToFileURL } from "node:url";
import { repoRoot, toolBin } from "./lib.mjs";

const PAGE = 65_536;
const DEFAULT_CAP = 268_435_456;
const MAX_RANGES = 8192;

// ------------------------------------------------------------------------------------ encoding

function uleb(value, out = []) {
  value >>>= 0;
  do {
    let byte = value & 0x7f;
    value >>>= 7;
    if (value) byte |= 0x80;
    out.push(byte);
  } while (value);
  return out;
}

function sleb32(value) {
  value |= 0;
  const out = [];
  let more = true;
  while (more) {
    let byte = value & 0x7f;
    value >>= 7;
    if ((value === 0 && (byte & 0x40) === 0) || (value === -1 && (byte & 0x40) !== 0)) more = false;
    else byte |= 0x80;
    out.push(byte);
  }
  return out;
}

function vecLen(bytes) {
  return uleb(bytes.length).concat(bytes);
}

class Reader {
  constructor(bytes) {
    this.b = bytes;
    this.p = 0;
  }
  eof() { return this.p >= this.b.length; }
  byte() { return this.b[this.p++]; }
  uleb() {
    let r = 0, s = 0, byte;
    do { byte = this.b[this.p++]; r |= (byte & 0x7f) << s; s += 7; } while (byte & 0x80);
    return r >>> 0;
  }
  sleb() {
    let r = 0, s = 0, byte;
    do { byte = this.b[this.p++]; r |= (byte & 0x7f) << s; s += 7; } while (byte & 0x80);
    if (s < 32 && (byte & 0x40)) r |= -(1 << s);
    return r;
  }
  bytes(n) { return this.b.subarray(this.p, this.p += n); }
  slice(n) { const s = this.p; this.p += n; return s; }
}

// ----------------------------------------------------------------------------- module model

// Sections in canonical order; custom sections (id 0) keep their recorded position.
function parseSections(bytes) {
  if (bytes[0] !== 0 || bytes[1] !== 0x61 || bytes[2] !== 0x73 || bytes[3] !== 0x6d ||
      bytes[4] !== 1 || bytes[5] !== 0 || bytes[6] !== 0 || bytes[7] !== 0) {
    throw new Error("not a wasm module");
  }
  const sections = [];
  const r = new Reader(bytes);
  r.p = 8;
  while (!r.eof()) {
    const id = r.byte();
    const size = r.uleb();
    const start = r.p;
    const end = start + size;
    let name = null;
    if (id === 0) {
      const q = new Reader(bytes.subarray(start, end));
      const len = q.uleb();
      name = Buffer.from(q.bytes(len)).toString("utf8");
    }
    sections.push({ id, name, start, end });
    r.p = end;
  }
  return sections;
}

function sectionBytes(bytes, section) {
  return Buffer.from(bytes.subarray(section.start, section.end));
}

function serialize(id, body, out) {
  out.push(id);
  uleb(body.length, out);
  for (const byte of body) out.push(byte);
}

function sectionBodies(bytes, sections) {
  return sections.map((s) => ({ id: s.id, name: s.name, body: [...sectionBytes(bytes, s)] }));
}

function buildModule(sections) {
  const out = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
  for (const { id, body } of sections) {
    serialize(id, body, out);
  }
  return Buffer.from(out);
}

// Skips one init or function-body expression (instructions up to the matching 0x0b end,
// with LEB immediates). Only the const and reference forms our cores use are supported.
function skipExpr(r) {
  for (;;) {
    const op = r.byte();
    if (op === 0x0b) return;
    if (op === 0x41 || op === 0x42) { r.sleb(); continue; } // i32/i64.const
    if (op === 0x43) { r.bytes(4); continue; } // f32.const
    if (op === 0x44) { r.bytes(8); continue; } // f64.const
    if (op === 0x23 || op === 0x24 || op === 0xd0) { r.uleb(); continue; } // global/ref.null
    if (op === 0xd2) { r.uleb(); r.uleb(); continue; } // ref.func in element init
    throw new Error("unsupported init expression opcode 0x" + op.toString(16));
  }
}

// Splits a vec-prefixed section body into raw entries. `skip` advances the reader past one entry.
function splitVec(body, skip) {
  const r = new Reader(Buffer.from(body));
  const count = r.uleb();
  const entries = [];
  while (entries.length < count) {
    const start = r.p;
    skip(r);
    entries.push(body.slice(start, r.p));
  }
  if (r.p !== body.length) throw new Error("trailing bytes in section body");
  return { count, entries };
}

const skipUleb = (r) => { r.uleb(); };
const skipName = (r) => { r.bytes(r.uleb()); };

function skipType(r) {
  if (r.byte() !== 0x60) throw new Error("unsupported type form");
  for (let i = r.uleb(); i > 0; i--) r.byte();
  for (let i = r.uleb(); i > 0; i--) r.byte();
}

function skipImport(r) {
  skipName(r);
  skipName(r);
  const kind = r.byte();
  r.uleb();
  if (kind === 0x00 || kind === 0x01 || kind === 0x02 || kind === 0x03) return;
  throw new Error("unsupported import kind " + kind);
}

function skipGlobal(r) {
  r.byte(); // valtype
  r.byte(); // mutability
  skipExpr(r);
}

function skipExport(r) {
  skipName(r);
  r.byte(); // kind
  r.uleb(); // index
}

function skipData(r) {
  const flags = r.uleb();
  if (flags !== 1) {
    if (flags !== 0 && flags !== 2) throw new Error("unsupported data flags " + flags);
    if (flags === 2) r.uleb();
    skipExpr(r);
  }
  r.bytes(r.uleb());
}

// Appends one raw entry to a vec-prefixed section body, fixing the count prefix.
function appendEntry(body, entry) {
  const r = new Reader(Buffer.from(body));
  const count = r.uleb();
  const rest = body.slice(r.p);
  return [...uleb(count + 1), ...rest, ...entry];
}

// ----------------------------------------------------------------------------- inspection

function byId(sections, id, name = null) {
  return sections.find((s) => s.id === id && (name === null || s.name === name));
}

function readName(r) {
  return Buffer.from(r.bytes(r.uleb())).toString("utf8");
}

// Initial linear memory: zero pages with the active data segments applied.
function initialImage(sections) {
  const mem = byId(sections, 5);
  if (!mem) throw new Error("core has no memory section");
  const r = new Reader(Buffer.from(mem.body));
  const count = r.uleb();
  if (count !== 1) throw new Error("core must have exactly one memory");
  const flags = r.uleb();
  if (flags !== 0) throw new Error("core memory must have no maximum");
  const pages = r.uleb();
  const image = new Uint8Array(pages * PAGE);
  const data = byId(sections, 11);
  const segments = data ? splitVec(data.body, skipData).entries : [];
  for (const entry of segments) {
    const q = new Reader(Buffer.from(entry));
    const segFlags = q.uleb();
    if (segFlags !== 0 && segFlags !== 2) continue; // passive segments are not initial memory
    if (segFlags === 2) q.uleb();
    const op = q.byte();
    if (op !== 0x41) throw new Error("active segment without an i32.const offset");
    const offset = q.sleb();
    if (q.byte() !== 0x0b) throw new Error("bad active segment init");
    const bytes = q.bytes(q.uleb());
    if (offset < 0 || offset + bytes.length > image.length) throw new Error("active segment out of bounds");
    image.set(bytes, offset);
  }
  return { image, pages };
}

function globalInits(sections) {
  const globals = byId(sections, 6);
  if (!globals) return [];
  return splitVec(globals.body, skipGlobal).entries.map((entry) => {
    const r = new Reader(Buffer.from(entry));
    const type = r.byte();
    const mut = r.byte();
    const values = [];
    for (;;) {
      const op = r.byte();
      if (op === 0x0b) break;
      if (op === 0x41) values.push(["i32", r.sleb()]);
      else if (op === 0x42) { r.sleb(); values.push(["i64", 0]); }
      else throw new Error("unsupported global init");
    }
    return { type, mut, values };
  });
}

function importEntries(sections) {
  const imports = byId(sections, 2);
  if (!imports) return [];
  let funcs = 0;
  return splitVec(imports.body, skipImport).entries.map((entry) => {
    const r = new Reader(Buffer.from(entry));
    const mod = readName(r);
    const field = readName(r);
    const kind = r.byte();
    const index = r.uleb();
    // For function imports `index` is the type index; the callable function index counts only
    // function-kind imports in order.
    const funcIndex = kind === 0 ? funcs++ : -1;
    return { mod, field, kind, index, funcIndex };
  });
}

function exportEntries(sections) {
  const exports = byId(sections, 7);
  if (!exports) return [];
  return splitVec(exports.body, skipExport).entries.map((entry) => {
    const r = new Reader(Buffer.from(entry));
    const name = readName(r);
    const kind = r.byte();
    const index = r.uleb();
    return { name, kind, index };
  });
}

function typeEntries(sections) {
  const types = byId(sections, 1);
  if (!types) return [];
  return splitVec(types.body, skipType).entries;
}

// Fails capture when the core can mutate its table: unmodeled mutable state. Scans the decoded
// text (via the pinned wasm-tools) rather than raw bytes, so immediates cannot false-positive.
function checkNoTableMutation(coreBytes, wasmTools, scratch) {
  writeFileSync(join(scratch, "core.wasm"), coreBytes);
  const text = execFileSync(wasmTools, ["print", "--skeleton", join(scratch, "core.wasm")], {
    encoding: "utf8", maxBuffer: 256 << 20,
  });
  const bad = text.split("\n").find((line) =>
    /(^|[\s(])table\.(set|grow|fill|copy|init)($|[\s)])/.test(line) ||
    /(^|[\s(])elem\.drop($|[\s)])/.test(line));
  if (bad) throw new Error("core can mutate its table: " + bad.trim());
  return text;
}

// ----------------------------------------------------------------------------- capture

// Runs the production default setup through the first-party adapter and records the live state.
// The configuration is the validated ordinary `contain` default: 268,435,456 B ceiling,
// 134,217,728 B volume, default layout, default env/grants/process info and the full command set.
async function captureOnce(root, coreBytes) {
  const { createFacade } = await import(pathToFileURL(join(root, "host", "core", "facade.js")).href);
  const { INTERNAL } = await import(pathToFileURL(join(root, "host", "core", "instance.js")).href);
  const mod = new WebAssembly.Module(coreBytes);
  const loader = {
    syncModule: () => mod,
    asyncModule: () => Promise.resolve(mod),
    precompile: () => Promise.resolve(),
  };
  let seen = null;
  let engine = null;
  const { Bash } = createFacade(loader);
  const bash = new Bash({
    [INTERNAL]: {
      inspect: (state) => { seen = state; },
      onEngine: (e) => { engine = e; },
      snapshotCapture: true,
    },
  });
  if (!seen || !engine) throw new Error("capture setup did not link");
  // All canonical post-returns ran inside the constructor above. Quiescence first.
  const handles = seen.handles();
  if (handles.length !== 2) throw new Error("capture setup left " + handles.length + " live handles");
  const kinds = handles.map((h) => h.kind).sort();
  if (kinds[0] !== "shell" || kinds[1] !== "vfs") throw new Error("capture setup kept unexpected resources");
  for (const h of handles) {
    if (!h.hostOwned || h.borrows !== 0) throw new Error("capture setup left a borrow or guest handle");
  }
  const probe = seen.exports.__snapshot_probe();
  if (probe !== 0b111n && probe !== 7) throw new Error("capture setup is not quiescent (probe " + probe + ")");
  const usage = engine.usage();
  const memory = new Uint8Array(seen.exports.memory.buffer.slice(0));
  const pages = memory.length / PAGE;
  if (!Number.isInteger(pages)) throw new Error("live memory is not a whole number of pages");
  if (memory.length > DEFAULT_CAP) throw new Error("capture exceeds the 268,435,456 B default cap");
  const rep = {};
  for (const h of handles) rep[h.kind] = h.rep >>> 0;
  void bash;
  return { usage, memory, pages, rep };
}

function diffRanges(initial, live) {
  const ranges = [];
  let start = -1;
  const end = Math.max(initial.length, live.length);
  for (let i = 0; i < end; i++) {
    const a = i < initial.length ? initial[i] : 0;
    const b = i < live.length ? live[i] : 0;
    if (a !== b) {
      if (start < 0) start = i;
    } else if (start >= 0) {
      ranges.push({ dst: start, bytes: live.slice(start, i) });
      start = -1;
    }
  }
  if (start >= 0) ranges.push({ dst: start, bytes: live.slice(start, end) });
  return ranges;
}

function manifestEqual(a, b) {
  return a.initialPages === b.initialPages && a.finalPages === b.finalPages &&
    a.ranges.length === b.ranges.length &&
    a.ranges.every((r, i) => r.dst === b.ranges[i].dst && Buffer.from(r.bytes).equals(Buffer.from(b.ranges[i].bytes))) &&
    a.repVfs === b.repVfs && a.repShell === b.repShell;
}

// Captures twice and requires byte-identical results: host time, entropy, process IDs and build
// paths must never enter the image.
async function capture(root, coreBytes, initial) {
  const first = await captureOnce(root, coreBytes);
  const second = await captureOnce(root, coreBytes);
  const rangesOf = ({ memory, pages }) => {
    if (pages < initial.pages) throw new Error("capture lost pages");
    const ranges = diffRanges(initial.image, memory.subarray(0, Math.max(initial.image.length, memory.length)));
    if (ranges.length > MAX_RANGES) throw new Error("capture has " + ranges.length + " ranges (limit " + MAX_RANGES + ")");
    return ranges;
  };
  const a = {
    initialPages: initial.pages,
    finalPages: first.pages,
    ranges: rangesOf(first),
    repVfs: first.rep.vfs,
    repShell: first.rep.shell,
    usedBytes: first.usage.used.toString(),
    ceilingBytes: first.usage.ceiling.toString(),
    highWaterBytes: first.usage.highWater.toString(),
  };
  const b = {
    initialPages: initial.pages,
    finalPages: second.pages,
    ranges: rangesOf(second),
    repVfs: second.rep.vfs,
    repShell: second.rep.shell,
    usedBytes: second.usage.used.toString(),
    ceilingBytes: second.usage.ceiling.toString(),
    highWaterBytes: second.usage.highWater.toString(),
  };
  if (!manifestEqual(a, b)) throw new Error("capture is not deterministic across two independent setups");
  return a;
}

// ----------------------------------------------------------------------------- augment

const I64 = 0x7e;
const I32 = 0x7f;

// Assembles the body of `__restore_default` in order: latch and preflight checks, growth,
// range installs, segment drop, resource registration, handle packing. Segment `seg` holds the
// concatenated ranges; the hand-written preflight/growth helpers and the existing
// `[resource-new]` imports are called by index.
function assembleRestore(args) {
  const { newVfs, newShell, repVfs, repShell } = args;
  const parts = [];
  parts.push(0x01, 0x01, I32);
  const call = (index) => { parts.push(0x10, ...uleb(index)); };
  const i32c = (v) => { parts.push(0x41, ...sleb32(v)); };
  parts.push(0x23, ...uleb(args.latch), 0x45, 0x04, I64);
  i32c(args.initialPages);
  call(args.preflight);
  parts.push(0x45, 0x04, I64, 0x42, 0x00, 0x05);
  i32c(1);
  parts.push(0x24, ...uleb(args.latch));
  i32c(args.growPages);
  call(args.grow);
  parts.push(0x45, 0x04, I64, 0x42, 0x00, 0x05);
  let src = 0;
  for (const { dst, bytes } of args.ranges) {
    i32c(dst); i32c(src); i32c(bytes.length);
    parts.push(0xfc, 0x08, ...uleb(args.seg), 0x00);
    src += bytes.length;
  }
  parts.push(0xfc, 0x09, ...uleb(args.seg)); // data.drop
  i32c(repVfs); // sleb32 keeps the u32 bit pattern of the captured box pointer
  call(newVfs);
  parts.push(0x21, 0x00); // local.set 0 (Vfs handle)
  i32c(repShell);
  call(newShell);
  parts.push(0xad); // i64.extend_i32_u (Shell)
  parts.push(0x42, 0x20); // i64.const 32
  parts.push(0x86); // i64.shl
  parts.push(0x20, 0x00); // local.get 0 (Vfs)
  parts.push(0xad); // i64.extend_i32_u
  parts.push(0x84); // i64.or: Vfs low 32, Shell high 32
  parts.push(0x0b, 0x0b); // close growth-if and preflight-if
  parts.push(0x05); // else of the latch-if: the latch was already set, refuse untouched
  parts.push(0x42, 0x00); // i64.const 0
  parts.push(0x0b); // close the latch-if
  parts.push(0x0b); // end of function body
  return parts;
}

const RESTORE_EXPORT = "__restore_default";
const RESTORE_TYPE = [0x60, 0x00, 0x01, I64];

function findSection(sections, id) {
  return sections.find((s) => s.id === id && !s.name);
}

// Appends the latch global, the restore function and the passive image to the core. The initial
// memory, the existing table indexes and every existing function are left untouched.
function augment(coreBytes, manifest) {
  const bytes = Buffer.from(coreBytes);
  const sections = sectionBodies(bytes, parseSections(bytes));
  const imports = importEntries(sections);
  const newVfs = imports.find((e) => e.field === "[resource-new]vfs" && e.kind === 0);
  const newShell = imports.find((e) => e.field === "[resource-new]shell" && e.kind === 0);
  if (!newVfs || !newShell) throw new Error("core lacks the [resource-new] imports the restore calls");
  const funcImports = imports.filter((e) => e.kind === 0).length;
  const exports = exportEntries(sections);
  const preflight = exports.find((e) => e.name === "__restore_preflight" && e.kind === 0);
  const grow = exports.find((e) => e.name === "__restore_grow" && e.kind === 0);
  if (!preflight || !grow) throw new Error("core lacks the restore helpers; rebuild the kernel");
  const funcSection = findSection(sections, 3);
  const codeSection = findSection(sections, 10);
  const typeSection = findSection(sections, 1);
  const globalSection = findSection(sections, 6);
  const exportSection = findSection(sections, 7);
  const dataSection = findSection(sections, 11);
  if (!funcSection || !codeSection || !typeSection || !globalSection || !exportSection || !dataSection) {
    throw new Error("core misses a section the restore needs");
  }
  const definedFuncs = splitVec(funcSection.body, skipUleb).count;
  const restoreIndex = funcImports + definedFuncs;
  const typeList = typeEntries(sections);
  let typeIndex = typeList.findIndex((entry) =>
    entry.length === RESTORE_TYPE.length && entry.every((b, i) => b === RESTORE_TYPE[i]));
  if (typeIndex < 0) {
    typeIndex = typeList.length;
    typeSection.body = appendEntry(typeSection.body, RESTORE_TYPE);
  }
  const latch = splitVec(globalSection.body, skipGlobal).count;
  globalSection.body = appendEntry(globalSection.body, [I32, 0x01, 0x41, 0x00, 0x0b]);
  const seg = splitVec(dataSection.body, skipData).count;
  const image = manifest.ranges.flatMap((r) => [...r.bytes]);
  dataSection.body = appendEntry(dataSection.body, [0x01, ...vecLen(image)]);
  let countSection = findSection(sections, 12);
  if (countSection) {
    countSection.body = [...uleb(seg + 1)];
  } else {
    // V8 requires the data count before the code section even though the section order
    // lists it last; rustc emits it there too.
    const codeSection = findSection(sections, 10);
    const at = sections.indexOf(codeSection);
    sections.splice(at, 0, { id: 12, name: null, body: [...uleb(seg + 1)] });
    countSection = sections[at];
  }
  const funcBody = assembleRestore({
    latch,
    preflight: preflight.index,
    grow: grow.index,
    newVfs: newVfs.funcIndex,
    newShell: newShell.funcIndex,
    seg,
    initialPages: manifest.initialPages,
    growPages: manifest.finalPages - manifest.initialPages,
    ranges: manifest.ranges,
    repVfs: manifest.repVfs,
    repShell: manifest.repShell,
  });
  funcSection.body = appendEntry(funcSection.body, [...uleb(typeIndex)]);
  codeSection.body = appendEntry(codeSection.body, [...vecLen(funcBody)]);
  const name = [...Buffer.from(RESTORE_EXPORT, "utf8")];
  exportSection.body = appendEntry(exportSection.body, [...uleb(name.length), ...name, 0x00, ...uleb(restoreIndex)]);
  return { augmented: buildModule(sections), latch, seg, restoreIndex, imageBytes: image.length };
}

// Re-parses the augmented core and checks the augmentation contract: initial memory identical,
// one appended global/export/function/segment, valid helper references.
function verifyAugment(plainBytes, augmented, manifest, parts) {
  const before = sectionBodies(plainBytes, parseSections(plainBytes));
  const after = sectionBodies(augmented, parseSections(augmented));
  const beforeInit = initialImage(before);
  const afterInit = initialImage(after);
  if (beforeInit.pages !== afterInit.pages ||
      !Buffer.from(beforeInit.image).equals(Buffer.from(afterInit.image))) {
    throw new Error("augmentation changed the captured initial memory");
  }
  const exp = exportEntries(after).find((e) => e.name === RESTORE_EXPORT && e.kind === 0);
  if (!exp || exp.index !== parts.restoreIndex) throw new Error("augmentation lost the restore export");
  const globals = splitVec(findSection(after, 6).body, skipGlobal);
  if (globals.count !== splitVec(findSection(before, 6).body, skipGlobal).count + 1) {
    throw new Error("augmentation added the wrong number of globals");
  }
  const dataCount = new Reader(Buffer.from(findSection(after, 12).body)).uleb();
  if (dataCount !== parts.seg + 1) throw new Error("augmentation left a wrong data count");
  if (manifest.finalPages - manifest.initialPages < 0) throw new Error("capture lost pages");
  // Smoke: preflight passes on the fresh store, refuses another page count; the probe is clean.
  const stub = { "[resource-new]vfs": () => 1, "[resource-drop]vfs": () => {},
    "[resource-new]shell": () => 2, "[resource-drop]shell": () => {},
    "[resource-new]fiber": () => 3, "[resource-drop]fiber": () => {} };
  const inst = new WebAssembly.Instance(new WebAssembly.Module(augmented), {
    "[export]sentropic:bash/engine@0.1.0-draft": stub,
  });
  const calls = inst.exports;
  if (calls.__restore_preflight(manifest.initialPages) !== 1) throw new Error("preflight smoke failed");
  if (calls.__restore_preflight(manifest.initialPages + 1) !== 0) throw new Error("preflight smoke passed a wrong page count");
  if (calls.__snapshot_probe() !== 7n) throw new Error("probe smoke failed");
}

// ----------------------------------------------------------------------------- driver

export async function snapshotCore(corePath, name) {
  const root = repoRoot;
  const scratch = mkdtempSync(join(root, "tmp", "snapshot-"));
  try {
    const plainBytes = readFileSync(corePath);
    const sections = sectionBodies(plainBytes, parseSections(plainBytes));
    if (exportEntries(sections).some((e) => e.name === RESTORE_EXPORT)) {
      throw new Error("core already carries a snapshot image; rebuild the plain core first");
    }
    const initial = initialImage(sections);
    const globals = globalInits(sections);
    const mutable = globals.filter((g) => g.mut === 1);
    if (mutable.length !== 1 || mutable[0].type !== I32 ||
        mutable[0].values.length !== 1 || mutable[0].values[0][1] !== 1048576) {
      throw new Error("unmodeled mutable globals: capture requires only the 1048576 stack pointer");
    }
    checkNoTableMutation(plainBytes, toolBin("wasm-tools"), scratch);
    const manifest = await capture(root, plainBytes, initial);
    const parts = augment(plainBytes, manifest);
    verifyAugment(plainBytes, parts.augmented, manifest, parts);
    writeFileSync(corePath, parts.augmented);
    const full = { name, ...manifest,
      ranges: manifest.ranges.map(({ dst, bytes }) => ({ dst, len: bytes.length })),
      imageBytes: parts.imageBytes,
      stackPointer: 1048576,
      coreSha256: null,
    };
    const { sha256 } = await import("./lib.mjs");
    full.coreSha256 = sha256(parts.augmented);
    const manifestPath = join(root, "tmp", "snapshot-" + name + ".json");
    const serializable = { ...full,
      image: Buffer.from(manifest.ranges.flatMap((r) => [...r.bytes])).toString("base64") };
    writeFileSync(manifestPath, JSON.stringify(serializable, null, 2) + "\n");
    console.log("snapshot " + name + ": pages " + manifest.initialPages + " -> " + manifest.finalPages +
      ", " + manifest.ranges.length + " ranges, " + parts.imageBytes + " image bytes," +
      " reps vfs=" + manifest.repVfs + " shell=" + manifest.repShell +
      ", used=" + manifest.usedBytes + " high-water=" + manifest.highWaterBytes);
    return full;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

export { parseSections, sectionBodies, initialImage };

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [corePath, name] = process.argv.slice(2);
  if (!corePath) throw new Error("usage: node tools/snapshot.mjs <core.wasm> [name]");
  await snapshotCore(corePath, name ?? basename(corePath).replace(/\.core\.wasm$/, ""));
}

