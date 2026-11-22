import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { bash } from "./kt.mjs";

const fixture = JSON.parse(readFileSync(new URL("./fixtures/l8b-js-numbers.json", import.meta.url), "utf8"));

async function runBatched(sh, lines, batchSize = 500) {
  const out = [];
  for (let i = 0; i < lines.length; i += batchSize) {
    const r = await sh.exec(lines.slice(i, i + batchSize).join("\n"));
    assert.equal(r.exitCode, 0, r.stderr);
    assert.equal(r.stderr, "");
    const got = r.stdout.slice(0, -1).split("\n");
    out.push(...got);
  }
  return out;
}

test("l8b-js-numbers.json vectors (>= 600) match Wasm __num js/fixed/exp/prec with 0 mismatches", async () => {
  assert.ok(fixture.vectors.length >= 600, `expected >= 600 vectors, got ${fixture.vectors.length}`);
  const sh = bash({ executionLimitProfile: "normal" });
  await sh.exec("echo warm");

  const jsCmds = [];
  const f0Cmds = [];
  const f6Cmds = [];
  const e6Cmds = [];
  const p6Cmds = [];
  for (const v of fixture.vectors) {
    jsCmds.push(`__num js ${v.bits} bits`);
    f0Cmds.push(`__num fixed ${v.bits} 0 bits`);
    f6Cmds.push(`__num fixed ${v.bits} 6 bits`);
    e6Cmds.push(`__num exp ${v.bits} 6 bits`);
    p6Cmds.push(`__num prec ${v.bits} 6 bits`);
  }

  const [gotJs, gotF0, gotF6, gotE6, gotP6] = await Promise.all([
    runBatched(sh, jsCmds),
    runBatched(sh, f0Cmds),
    runBatched(sh, f6Cmds),
    runBatched(sh, e6Cmds),
    runBatched(sh, p6Cmds),
  ]);

  for (let i = 0; i < fixture.vectors.length; i++) {
    const v = fixture.vectors[i];
    assert.equal(gotJs[i], v.js, `js mismatch at ${v.bits}`);
    assert.equal(gotF0[i], v.fixed0, `fixed0 mismatch at ${v.bits}`);
    assert.equal(gotF6[i], v.fixed6, `fixed6 mismatch at ${v.bits}`);
    assert.equal(gotE6[i], v.exp6, `exp6 mismatch at ${v.bits}`);
    assert.equal(gotP6[i], v.prec6, `prec6 mismatch at ${v.bits}`);
  }
});

test("10 000+ seeded f64 bit patterns match Node 22 String(x) in < 1.5 s via Wasm __num js", async () => {
  const sh = bash({ executionLimitProfile: "normal" });
  await sh.exec("echo warm");

  const buf = new ArrayBuffer(8);
  const dv = new DataView(buf);
  let s = 0x5345_4e54_4c38_4231n;
  const mask = 0xffff_ffff_ffff_ffffn;

  const expected = [];
  const cmds = [];
  const COUNT = 10_000;
  for (let i = 0; i < COUNT; i++) {
    s ^= (s << 13n) & mask;
    s ^= s >> 7n;
    s ^= (s << 17n) & mask;
    let bits = s;
    dv.setBigUint64(0, bits, false);
    let val = dv.getFloat64(0, false);
    if (Number.isNaN(val)) {
      bits = 0x7ff8_0000_0000_0000n;
      val = NaN;
    }
    expected.push(String(val));
    cmds.push(`__num js 0x${bits.toString(16).padStart(16, "0")} bits`);
  }

  const t0 = performance.now();
  const got = await runBatched(sh, cmds, 1000);
  const dt = performance.now() - t0;

  assert.equal(got.length, COUNT);
  for (let i = 0; i < COUNT; i++) {
    assert.equal(got[i], expected[i], `mismatch at index ${i}`);
  }
  assert.ok(dt < 1500, `10000 __num js took ${dt.toFixed(1)} ms (budget < 1500 ms)`);
});

test("__num arith, printf-int, f64-prefix, f64-strict, and error exits", async () => {
  const sh = bash({ executionLimitProfile: "contain" });

  assert.equal((await sh.exec("__num arith '64#@_'")).stdout, "4031\n");
  const badBase = await sh.exec("__num arith '08'");
  assert.equal(badBase.exitCode, 1);
  assert.equal(badBase.stderr, "bash: value too great for base\n");

  const badNum = await sh.exec("__num arith 'abc'");
  assert.equal(badNum.exitCode, 1);
  assert.equal(badNum.stderr, "bash: invalid number\n");

  assert.equal((await sh.exec("__num printf-int '3abc'")).stdout, "3|1\n");
  assert.equal((await sh.exec("__num f64-prefix '  3.5abc'")).stdout, "3.5\n");
  assert.equal((await sh.exec("__num f64-strict '  3.5abc'")).exitCode, 1);
  assert.equal((await sh.exec("__num fixed 1.5 101")).exitCode, 2);
});
