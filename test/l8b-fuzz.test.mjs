import { test } from "node:test";
import assert from "node:assert/strict";
import { bash } from "./kt.mjs";

function makeRng(seed) {
  let s = seed;
  const mask = 0xffffffffffffffffn;
  return () => {
    s ^= (s << 13n) & mask;
    s ^= s >> 7n;
    s ^= (s << 17n) & mask;
    return s;
  };
}

function sq(str) {
  return "'" + str.replaceAll("'", "'\\''") + "'";
}

test("deterministic seeded fuzz slice (>= 2000 cases, 0 panics/traps, < 2 s)", async () => {
  const sh = bash({ executionLimitProfile: "contain" });
  await sh.exec("echo warm");
  const next = makeRng(0x5345_4e54_4c38_4231n);
  const specs = ["%s", "%d", "%i", "%u", "%o", "%x", "%X", "%f", "%e", "%g", "%c", "%q", "%b", "%(%F %T %Z)T", "%5.2s", "%04d", "%-5x"];
  const tokens = ["", "0", "-1", "42", "0x1f", "010", "08", "'a", "3abc", "1.5", "-0.0", "Infinity", "NaN", "hello\\nworld", "a\\cb", "é😀", "a\tb", "\\x41\\xc3\\x28"];

  const lines = [];
  for (let i = 0; i < 600; i++) {
    const sp = specs[Number(next() % BigInt(specs.length))];
    const arg = tokens[Number(next() % BigInt(tokens.length))];
    lines.push(`__printf ${sq(sp)} ${sq(arg)}`);
  }
  for (let i = 0; i < 500; i++) {
    const sec = BigInt.asIntN(36, next()).toString();
    const fmts = ["%Y-%m-%d %H:%M:%S %Z %z", "%V-%G-%g", "%U-%W-%j", "%c %r %p %P", "%Q%"];
    const fmt = fmts[Number(next() % BigInt(fmts.length))];
    const tzs = ["UTC", "", "EST5", "Etc/GMT+5", "+05:30", "Invalid/Zone"];
    const tz = tzs[Number(next() % BigInt(tzs.length))];
    lines.push(`__strftime ${sec} ${sq(fmt)} ${sq(tz)}`);
  }
  for (let i = 0; i < 500; i++) {
    const arg = tokens[Number(next() % BigInt(tokens.length))];
    const ops = ["arith", "printf-int", "f64-prefix", "f64-strict"];
    const op = ops[Number(next() % BigInt(ops.length))];
    lines.push(`__num ${op} ${sq(arg)}`);
  }
  for (let i = 0; i < 450; i++) {
    const payload = tokens[Number(next() % BigInt(tokens.length))];
    const mode = i % 3 === 0 ? "encode 16" : i % 3 === 1 ? "decode" : "decode-strict";
    lines.push(`__base64 ${mode} <<< ${sq(payload)}`);
  }

  assert.ok(lines.length >= 2000);
  const t0 = performance.now();
  const batchSize = 250;
  for (let i = 0; i < lines.length; i += batchSize) {
    const r = await sh.exec(lines.slice(i, i + batchSize).join("\n"));
    assert.equal(r.fatal, undefined, `unexpected fatal/trap at batch ${i}`);
  }
  const dt = performance.now() - t0;
  assert.ok(dt < 2000, `fuzz suite took ${dt.toFixed(1)} ms (budget < 2000 ms)`);
});
