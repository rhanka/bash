import { test } from "node:test";
import assert from "node:assert/strict";
import { Codec } from "../../host/core/abi/codec.js";
import { alignTo, layout } from "../../host/core/abi/layout.js";
import { binary, bytes, layoutCases, testConfig } from "./support.mjs";

const U64_MAX = (1n << 64n) - 1n;
const S64_MIN = -(1n << 63n);
const S64_MAX = (1n << 63n) - 1n;
const fsErr = { tag: "err", val: { code: "not-found", path: bytes("/missing") } };
const fsOk = { tag: "ok", val: { tag: "flag", val: false } };
const env = [{ name: bytes("X"), value: binary, exported: true }];
const headers = [{ name: "x-sandbox", value: binary }];
const command = { name: "é", args: [binary, bytes("")], cwd: bytes("/"), env, fds: [{ fd: 0, len: 4n }],
  inputLen: 4n, xpgEcho: false };
const fetch = { method: "POST", url: "https://example.invalid/é", headers, inputLen: 4n, followRedirects: true,
  maxRedirects: 0xffffffff, timeoutMs: 0, maxResponseBytes: U64_MAX };
const response = { status: 0xffff, statusText: "é", url: fetch.url, headers,
  redirectChain: [{ url: fetch.url, status: 0, statusText: "", headers: [] }] };
const outcome = { exitCode: -0x80000000, status: "limit-exceeded", limitName: "maxInputBytes", env, stdoutKind: "bytes" };

const hostReqs = [
  { tag: "fs-batch", val: { mountId: 0xffffffff, ops: [{ tag: "all-paths" }, { tag: "exists", val: binary }] } },
  { tag: "lazy-file", val: binary },
  { tag: "custom-command", val: command },
  { tag: "fetch", val: fetch },
  { tag: "sleep", val: U64_MAX },
  { tag: "clock" },
  { tag: "entropy", val: 0xffffffff },
  { tag: "trace-clock" },
];
const hostOutcomes = [
  { tag: "fs", val: [fsOk, fsErr] },
  { tag: "data", val: binary },
  { tag: "command-exit", val: -0x80000000 },
  { tag: "fetched", val: response },
  { tag: "clock", val: { wallMs: S64_MIN, monotonicNs: U64_MAX } },
  { tag: "completed" },
  { tag: "denied" },
  { tag: "limit-exceeded", val: "é" },
  { tag: "net-failed", val: { code: "response-too-large", detail: binary } },
  { tag: "failed", val: binary },
];

function roundtrip(ref, values) {
  const type = layout(ref);
  const memory = new WebAssembly.Memory({ initial: 1 });
  let next = 8;
  const core = {
    memory,
    cabi_realloc(_old, _size, align, length) {
      const ptr = alignTo(next, align);
      next = ptr + length;
      return ptr;
    },
  };
  for (const [value, expected = value] of values) {
    const codec = new Codec({ core });
    const staged = codec.stage(type, value, "fixture");
    const ptr = codec.allocate(type.size, type.align);
    codec.write(type, staged, ptr);
    assert.deepEqual(codec.read(type, ptr), expected);
    const flat = [];
    codec.flatten(type, staged, flat);
    assert.equal(flat.length, type.flat.length);
    assert.deepEqual(codec.liftFlat(type, flat), expected);
  }
}

test("the codec preserves scalar extrema, UTF-8 with BOM, typed arrays and nested layout", () => {
  for (const [type, values] of [
    ["u8", [0, 0xff]],
    ["u16", [0, 0xffff]],
    ["u32", [0, 0xffffffff]],
    ["s32", [-0x80000000, 0x7fffffff]],
    ["u64", [0n, U64_MAX]],
    ["s64", [S64_MIN, S64_MAX]],
    ["f64", [-Number.MAX_VALUE, Number.MAX_VALUE, -Infinity, Infinity, -0, Number.MIN_VALUE, NaN]],
    ["bool", [false, true]],
    ["profile", ["micro", "normal"]],
  ]) roundtrip(type, values.map((value) => [value]));
  roundtrip("string", [["\ufeffé\ud800😀", "\ufeffé\ufffd😀"], [""]]);
  roundtrip("bytes", [[binary], [new Uint8Array()]]);
  roundtrip(["list", "u16"], [[new Uint16Array([0, 0x7f80, 0xffff])], [new Uint16Array()]]);
  roundtrip(["list", ["list", "bytes"]], [[[[binary], []]], [[]]]);
  roundtrip("shell-config", [[testConfig(), {
    ...testConfig(),
    grants: { hostFs: false, network: false, realClock: false, realSleep: false, entropy: false, customCommands: true },
    commands: undefined, cwd: undefined, processInfo: undefined,
  }]]);
  roundtrip("trace-event", [[{ category: "builtin", name: "é", durationMs: -0,
    details: [{ key: "float", value: { tag: "number", val: -0 } }, { key: "binary", value: { tag: "text", val: binary } }] }]]);
  roundtrip(["variant", [["float", "f64"], ["signed", "s32"], ["wide", "u64"], ["unit", null]]],
    [[{ tag: "float", val: -0 }], [{ tag: "signed", val: -1 }], [{ tag: "wide", val: U64_MAX }], [{ tag: "unit" }]]);
  const codec = new Codec({ core: { memory: new WebAssembly.Memory({ initial: 1 }) } });
  assert.equal(codec.liftFlat(layout("u32"), [-1]), 0xffffffff);
  assert.equal(codec.liftFlat(layout("u64"), [-1n]), U64_MAX);
  assert.throws(() => codec.liftFlat(layout("bool"), [2]), WebAssembly.RuntimeError);
});

test("the codec covers all 8 host requests, all 10 outcomes, both result arms and both option arms", () => {
  layoutCases("host-req", hostReqs);
  layoutCases("host-outcome", hostOutcomes);
  roundtrip("host-req", hostReqs.map((value) => [value]));
  roundtrip("host-outcome", hostOutcomes.map((value) => [value]));
  roundtrip("host-req", [[{ tag: "fetch", val: { ...fetch, maxRedirects: undefined } }]]);
  roundtrip("step", [
    [{ tag: "out-of-fuel" }],
    [{ tag: "done", val: outcome }],
    [{ tag: "host-call", val: { epoch: U64_MAX, req: hostReqs[2] } }],
  ]);
  roundtrip("fs-result", [[fsOk], [fsErr]]);
  roundtrip(["result", null, "fs-error"], [[{ tag: "ok" }], [fsErr]]);
  roundtrip(["option", "bytes"], [[undefined], [new Uint8Array()]]);
});

test("array lengths and staged encodings consume budget before allocating host containers", () => {
  const codec = new Codec({ core: { memory: new WebAssembly.Memory({ initial: 1 }) }, lowerBudget: 12, liftBudget: 12 });
  const list = ["list", "u32"];
  assert.throws(() => codec.stage(layout(list), new Array(4), "list"), RangeError);
  assert.throws(() => codec.stage(layout("string"), "😀".repeat(4), "string"), RangeError);
  assert.throws(() => codec.liftFlat(layout(list), [16, 4]), /transfer budget/);
  assert.throws(() => codec.liftFlat(layout(["list", "u16"]), [1, 0]), /misaligned/);
  assert.throws(() => codec.liftFlat(layout("bytes"), [65537, 0]), /out-of-bounds/);
  assert.deepEqual(codec.liftFlat(layout("bytes"), [65536, 0]), new Uint8Array());
});

