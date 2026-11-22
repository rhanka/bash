import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import { bash } from "./kt.mjs";

const quote = (s) => `'${s.replaceAll("'", "'\\''")}'`;

describe("L8a S9 regex/glob ReDoS, compile caps, and bounded backrefs", () => {
  it("Gate 3: classic backtracking traps finish in < 50 ms with linear O(N) scaling", async () => {
    const sh = bash({ executionLimitProfile: "normal" });
    // Warm up instance so Wasm JIT compilation does not skew timings.
    await sh.exec(`__regex ere - ${quote("(a+)+b")}`, { stdin: "aaa!\n" });

    const hay50k = "a".repeat(50_000) + "!\n";
    const hay20k = "a".repeat(20_000) + "!\n";
    const hay5k = "a".repeat(5_000) + "!\n";

    const traps = [
      ["ere", "(a+)+b", hay50k, 1],
      ["ere", "(a|a)*b", hay50k, 1],
      ["ere", "^([a-zA-Z]+)*$", hay50k, 1],
      ["ere", "([a-zA-Z]+)*$", hay50k, 0],
      ["perl", "(.*a){25}b", hay20k, 1],
    ];

    for (const [mode, pat, stdin, wantExit] of traps) {
      const t0 = performance.now();
      const res = await sh.exec(`__regex ${mode} - ${quote(pat)}`, { stdin });
      const dt = performance.now() - t0;
      assert.equal(res.exitCode, wantExit, `${mode} ${pat} unexpected exitCode`);
      assert.ok(dt < 50, `${mode} ${pat} took ${dt.toFixed(2)} ms (limit 50 ms)`);
    }

    // Scaling check: 50_000 vs 5_000 'a's on (a+)+b must be <= 15x.
    const t5Start = performance.now();
    for (let i = 0; i < 5; i++) {
      const r = await sh.exec(`__regex ere - ${quote("(a+)+b")}`, { stdin: hay5k });
      assert.equal(r.exitCode, 1);
    }
    const avg5k = (performance.now() - t5Start) / 5;

    const t50Start = performance.now();
    const r50 = await sh.exec(`__regex ere - ${quote("(a+)+b")}`, { stdin: hay50k });
    const dt50k = performance.now() - t50Start;
    assert.equal(r50.exitCode, 1);
    const ratio = dt50k / Math.max(avg5k, 0.5);
    assert.ok(ratio <= 15, `scaling ratio ${ratio.toFixed(2)}x exceeded 15x (${dt50k.toFixed(2)}ms vs ${avg5k.toFixed(2)}ms)`);
  });

  it("Gate 4: compile-time structural caps reject blow-up patterns in < 20 ms", async () => {
    const sh = bash({ executionLimitProfile: "normal" });
    await sh.exec(`__regex ere D ${quote("a")}`, { stdin: "a\n" });

    const caps = [
      ["a{1000}{1000}", "ProgramTooLarge { limit: 65536 }"],
      ["a{1001}", "RepeatTooLarge { limit: 1000 }"],
      ["(".repeat(201) + "a" + ")".repeat(201), "NestingTooDeep { limit: 200 }"],
      ["(a)".repeat(101), "TooManyCaptures { limit: 100 }"],
    ];

    for (const [pat, tag] of caps) {
      const t0 = performance.now();
      const res = await sh.exec(`__regex ere D ${quote(pat)}`, { stdin: "a\n" });
      const dt = performance.now() - t0;
      assert.equal(res.exitCode, 2, `expected exit 2 for ${tag}`);
      assert.ok(res.stderr.includes(tag), `stderr "${res.stderr}" should include "${tag}"`);
      assert.ok(dt < 20, `compile cap ${tag} took ${dt.toFixed(2)} ms (limit 20 ms)`);
    }

    // DFA state-explosion pattern `a[ab]{20}` across 64 KiB of pseudo-random "ab" bytes.
    let rng = 0x53454e544c38415an;
    const chars = new Array(65_536);
    for (let i = 0; i < 65_536; i++) {
      rng = (rng * 6364136223846793005n + 1n) & 0xffffffffffffffffn;
      chars[i] = ((rng >> 33n) & 1n) === 0n ? "a" : "b";
    }
    const hay64k = chars.join("") + "\n";
    const rHit = await sh.exec(`__regex ere - ${quote("a[ab]{20}")}`, { stdin: hay64k });
    assert.equal(rHit.exitCode, 0);
    const rMiss = await sh.exec(`__regex ere - ${quote("a[ab]{20}c")}`, { stdin: hay64k });
    assert.equal(rMiss.exitCode, 1);
  });

  it("Gate 5: bounded backreferences honor maxWorkUnits and keep Bash usable", async () => {
    const sh = bash({ executionLimitProfile: "normal" });
    const ok = await sh.exec(`__regex ere b ${quote("([a-z]+)-\\1")}`, { stdin: "abc-abc\n" });
    assert.equal(ok.exitCode, 0);
    assert.equal(ok.stdout, "abc-abc\n");

    const no = await sh.exec(`__regex ere b ${quote("([a-z]+)-\\1")}`, { stdin: "abc-def\n" });
    assert.equal(no.exitCode, 1);

    const unsup = await sh.exec(`__regex ere D ${quote("(a)\\1")}`, { stdin: "aa\n" });
    assert.equal(unsup.exitCode, 2);
    assert.ok(unsup.stderr.includes("BackrefUnsupported(1)"));

    // Adversarial backref bounded by maxWorkUnits: 5_000.
    const capped = bash({
      executionLimitProfile: "normal",
      executionLimits: { maxWorkUnits: 5_000 },
    });
    const t0 = performance.now();
    const adv = await capped.exec(`__regex ere b ${quote("([a-z]+)\\1+b")}`, {
      stdin: "a".repeat(500) + "!\n",
    });
    const dt = performance.now() - t0;
    assert.equal(adv.exitCode, 126);
    assert.ok(
      adv.stderr.includes("aggregate work limit exceeded") || adv.stderr.includes("maxWorkUnits"),
      `unexpected stderr: ${adv.stderr}`,
    );
    assert.ok(dt < 50, `adversarial backref took ${dt.toFixed(2)} ms (limit 50 ms)`);

    // Instance remains usable after hitting maxWorkUnits.
    const after = await capped.exec("echo ok");
    assert.equal(after.exitCode, 0);
    assert.equal(after.stdout, "ok\n");
  });

  it("Gate 6 (Wasm): !(...) extglob, strip, replace, and globstar via __glob", async () => {
    const sh = bash({ executionLimitProfile: "normal" });
    assert.equal((await sh.exec(`__glob match e ${quote("!(foo)bar")} ${quote("foobar")}`)).stdout, "no\n");
    assert.equal((await sh.exec(`__glob match e ${quote("!(foo)bar")} ${quote("xbar")}`)).stdout, "match\n");
    assert.equal((await sh.exec(`__glob match e ${quote("!(a*)b")} ${quote("bb")}`)).stdout, "match\n");
    assert.equal((await sh.exec(`__glob strip-prefix-greedy e ${quote("!(z)")} ${quote("abc")}`)).stdout, "\n");
    assert.equal((await sh.exec(`__glob replace-all - ${quote("*")} ${quote("abc")} ${quote("X")}`)).stdout, "X\n");
    const badRange = await sh.exec(`__glob match - ${quote("[z-a]")} ${quote("a")}`);
    assert.equal(badRange.exitCode, 2);
    assert.equal(badRange.stderr, "__glob: InvalidCharacterRange\n");
  });
});
