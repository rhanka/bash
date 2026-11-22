import { test } from "node:test";
import assert from "node:assert/strict";
import { bash } from "./kt.mjs";

test("prospective printf width/precision limits fail in < 10 ms without OOM and shell remains usable", async () => {
  const sh = bash({ executionLimitProfile: "contain" });
  await sh.exec("echo warm");

  for (const [script, wantErr] of [
    ["__printf '%2147483647s' 'hi'", "bash: format width limit exceeded (16777216 bytes)\n"],
    ["__printf '%.2147483647d' '1'", "bash: format precision limit exceeded (16777216 bytes)\n"],
  ]) {
    const t0 = performance.now();
    const r = await sh.exec(script);
    const dt = performance.now() - t0;
    assert.equal(r.exitCode, 126);
    assert.equal(r.stderr, wantErr);
    assert.equal(r.stdout, "");
    assert.ok(dt < 10, `expected prospective failure in < 10 ms, got ${dt.toFixed(2)} ms`);
  }

  const ok = await sh.exec("echo ok");
  assert.deepEqual({ exitCode: ok.exitCode, stdout: ok.stdout, stderr: ok.stderr }, { exitCode: 0, stdout: "ok\n", stderr: "" });
});

test("printf precision > 100 returns exit 1 with RangeError stderr", async () => {
  const sh = bash({ executionLimitProfile: "contain" });
  for (const [spec, msg] of [
    ["%.101f", "printf: toFixed() digits argument must be between 0 and 100\n"],
    ["%.101e", "printf: toExponential() argument must be between 0 and 100\n"],
    ["%.101g", "printf: toPrecision() argument must be between 1 and 100\n"],
  ]) {
    const r = await sh.exec(`__printf '${spec}' '1.5'`);
    assert.equal(r.exitCode, 1);
    assert.equal(r.stderr, msg);
    assert.equal(r.stdout, "");
  }
});

test("negative dynamic .* precision omits precision in < 10 ms under micro (upstream-defect divergence)", async () => {
  const sh = bash({ executionLimitProfile: "micro" });
  await sh.exec("echo warm");
  const t0 = performance.now();
  const r = await sh.exec("__printf '%.*d' -100000000 7");
  const dt = performance.now() - t0;
  assert.equal(r.exitCode, 0);
  assert.equal(r.stdout, "7");
  assert.equal(r.stderr, "");
  assert.ok(dt < 10, `expected < 10 ms, got ${dt.toFixed(2)} ms`);
});

test("base64 prospective wrapped-line and size limits", async () => {
  const sh = bash({
    executionLimitProfile: "contain",
    executionLimits: { maxArrayElements: 100 },
  });
  const r = await sh.exec("__base64 encode 1", { stdin: "a".repeat(80) });
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, "bash: base64: wrapped line limit exceeded (100)\n");

  const shStr = bash({
    executionLimitProfile: "contain",
    executionLimits: { maxStringLength: 10 },
  });
  const r2 = await shStr.exec("__base64 encode 0", { stdin: "abcdefghij" });
  assert.equal(r2.exitCode, 126);
  assert.equal(r2.stderr, "bash: base64: encoded size limit exceeded (10 bytes)\n");
});

test("S11 timezone non-disclosure: host TZ never leaks into __strftime", async () => {
  const prevTz = process.env.TZ;
  process.env.TZ = "America/New_York";
  try {
    const sh = bash({ executionLimitProfile: "contain" });
    for (const tzEnv of [{}, { TZ: "" }, { TZ: "Invalid/Zone" }]) {
      const r = await sh.exec("__strftime 0 '%Y-%m-%d %H:%M:%S %Z %z'", { env: tzEnv });
      assert.equal(r.exitCode, 0);
      assert.equal(r.stdout, "1970-01-01 00:00:00 UTC +0000\n");
    }
  } finally {
    if (prevTz === undefined) delete process.env.TZ;
    else process.env.TZ = prevTz;
  }
});
