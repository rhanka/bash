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
    // De Morgan semantics for nested `!(!(a)|b)` = a & !b.
    assert.equal((await sh.exec(`__glob match e ${quote("!(!(a)|b)")} ${quote("a")}`)).stdout, "match\n");
    assert.equal((await sh.exec(`__glob match e ${quote("!(!(a)|b)")} ${quote("b")}`)).stdout, "no\n");
    assert.equal((await sh.exec(`__glob match e ${quote("!(!(a)|b)")} ${quote("c")}`)).stdout, "no\n");
  });

  it("PR #22 regressions (Wasm): nested !(...) DP, nullable backref loops, stacked quantifiers", async () => {
    const sh = bash({ executionLimitProfile: "normal" });
    // Warm up instance.
    await sh.exec(`__glob match e ${quote("!(a)")} ${quote("b")}`);

    // 1. Nested negation chain Q_15 = !(!(a) Q_14) (depth 31 <= 100) on 40-byte subject:
    // exponential O(N^15) without memoization; polynomial and < 50 ms with memoized bitset DP.
    const buildQ = (k) => "!(!(a)".repeat(k) + "!(a)" + ")".repeat(k);
    const q15 = buildQ(15);
    const tNot0 = performance.now();
    const notRes = await sh.exec(`__glob match e ${quote(q15)} ${quote("ab".repeat(20))}`);
    const dtNot = performance.now() - tNot0;
    assert.equal(notRes.exitCode, 0, `nested !(...) failed: ${notRes.stderr}`);
    assert.equal(notRes.stdout, "no\n");
    assert.ok(dtNot < 50, `nested !(...) took ${dtNot.toFixed(2)} ms (limit 50 ms)`);

    // Nested `+(+(...!(a)...))` (depth 40 <= 100) and unclosed `@(` repeated 40 times.
    const plus40 = "+(".repeat(40) + "!(a)" + ")".repeat(40);
    const tPlus0 = performance.now();
    const plusRes = await sh.exec(`__glob match e ${quote(plus40)} ${quote("aa")}`);
    const dtPlus = performance.now() - tPlus0;
    assert.equal(plusRes.exitCode, 0);
    assert.equal(plusRes.stdout, "match\n");
    assert.ok(dtPlus < 50, `nested +(...!(a)...) took ${dtPlus.toFixed(2)} ms (limit 50 ms)`);

    const unclosed40 = "@(".repeat(40);
    const unclosedRes = await sh.exec(`__glob match e ${quote(unclosed40)} ${quote(unclosed40)}`);
    assert.equal(unclosedRes.exitCode, 0);
    assert.equal(unclosedRes.stdout, "match\n");

    // strip-suffix-greedy and replace-all with nested `!(...)` share one DP memo table.
    const q8 = buildQ(8);
    const tSr0 = performance.now();
    const ssRes = await sh.exec(`__glob strip-suffix-greedy e ${quote(q8)} ${quote("ab".repeat(12))}`);
    const repRes = await sh.exec(`__glob replace-all e ${quote(q8)} ${quote("ab".repeat(12))} ${quote("X")}`);
    const dtSr = performance.now() - tSr0;
    assert.equal(ssRes.exitCode, 0);
    assert.equal(repRes.exitCode, 0);
    assert.ok(dtSr < 50, `nested !(...) strip/replace took ${dtSr.toFixed(2)} ms (limit 50 ms)`);

    // 2. Bounded backtracker on nullable capture loops `(a*)*\1` and `((a?)+)*\1`.
    for (const [pat, stdin, wantExit, wantOut] of [
      ["(a*)*\\1", "\n", 0, "\n"],
      ["(a*)*\\1", "a\n", 0, "a\n"],
      ["(a*)*\\1", "aa\n", 0, "aa\n"],
      ["^(a*)*\\1$", "b\n", 1, ""],
      ["^((a?)+)*\\1$", "aa\n", 0, "aa\n"],
      ["^((a?)+)*\\1$", "b\n", 1, ""],
    ]) {
      const t0 = performance.now();
      const res = await sh.exec(`__regex ere b ${quote(pat)}`, { stdin });
      const dt = performance.now() - t0;
      assert.equal(res.exitCode, wantExit, `${pat} on ${JSON.stringify(stdin)} stderr=${res.stderr}`);
      assert.equal(res.stdout, wantOut);
      assert.ok(dt < 50, `${pat} took ${dt.toFixed(2)} ms (limit 50 ms)`);
    }

    // 3. Adversarial stacked postfix quantifiers (25,000 `*` and 250 `{1}`) reject cleanly without Wasm trap.
    for (const pat of ["a" + "*".repeat(25_000), "a" + "{1}".repeat(250)]) {
      const t0 = performance.now();
      const res = await sh.exec(`__regex ere D ${quote(pat)}`, { stdin: "a\n" });
      const dt = performance.now() - t0;
      assert.equal(res.exitCode, 2, `expected exit 2 for stacked quantifiers`);
      assert.ok(
        res.stderr.includes("NestingTooDeep { limit: 200 }"),
        `stderr "${res.stderr}" should report NestingTooDeep`,
      );
      assert.ok(dt < 20, `stacked quantifiers rejection took ${dt.toFixed(2)} ms (limit 20 ms)`);
    }

    // Exact boundary depth 200 compiles and matches cleanly on wasm32 without stack trap.
    const boundaryStar = "a" + "*".repeat(200);
    const bStarRes = await sh.exec(`__regex ere - ${quote(boundaryStar)}`, { stdin: "aaa\n" });
    assert.equal(bStarRes.exitCode, 0);
    assert.equal(bStarRes.stdout, "aaa\n");

    const boundaryGroup = "(?:".repeat(200) + "a" + ")".repeat(200);
    const bGrpRes = await sh.exec(`__regex perl - ${quote(boundaryGroup)}`, { stdin: "a\n" });
    assert.equal(bGrpRes.exitCode, 0);
    assert.equal(bGrpRes.stdout, "a\n");

    const zeroNested = "a{0}{1000}{1000}";
    const zeroRes = await sh.exec(`__regex ere - ${quote(zeroNested)}`, { stdin: "a\n" });
    assert.equal(zeroRes.exitCode, 0);

    // 4. find_all with `\K` match-start reset advances cleanly without stalling.
    const keepRes = await sh.exec(`__regex perl o ${quote("a\\Kb|c")}`, { stdin: "abc\n" });
    assert.equal(keepRes.exitCode, 0);
    assert.equal(keepRes.stdout, "b\nc\n");

    // 5. Nested self-referencing backreference group in outer loop must not trap on Wasm (`a > b_end`).
    const selfRefEre = await sh.exec(`__regex ere bo ${quote("((a\\2?)b)+")}`, { stdin: "abaa\nabab\n" });
    assert.equal(selfRefEre.exitCode, 0);
    assert.equal(selfRefEre.stdout, "ab\nabab\n");

    const selfRefEreMiss = await sh.exec(`__regex ere b ${quote("^((a\\2?)b)+$")}`, { stdin: "abaa\n" });
    assert.equal(selfRefEreMiss.exitCode, 1);

    const selfRefPerl = await sh.exec(`__regex perl bo ${quote("(?:(a\\1?)b)+")}`, { stdin: "abaa\n" });
    assert.equal(selfRefPerl.exitCode, 0);
    assert.equal(selfRefPerl.stdout, "ab\n");

    // Instance stays healthy after all adversarial checks.
    const ok = await sh.exec("echo ok");
    assert.equal(ok.exitCode, 0);
    assert.equal(ok.stdout, "ok\n");
  });
});
