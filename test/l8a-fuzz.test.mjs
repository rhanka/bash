import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import { bash } from "./kt.mjs";

const quote = (s) => `'${s.replaceAll("'", "'\\''")}'`;

describe("L8a Gate 7: deterministic seeded regex/glob fuzz slice", () => {
  it("runs >= 1500 seeded regex and glob cases through Wasm with 0 traps in < 2 s", async () => {
    const sh = bash({ executionLimitProfile: "normal" });
    let state = 0x53454e544c38415an;
    const nextU32 = () => {
      state = (state * 6364136223846793005n + 1442695040888963407n) & 0xffffffffffffffffn;
      return Number((state >> 32n) & 0xffffffffn);
    };
    const pick = (arr) => arr[nextU32() % arr.length];

    const regexTokens = [
      "a", "b", "c", ".", "[ab]", "[^a]", "[[:digit:]]", "\\w", "\\d",
      "(a|b)", "(ab)", "a*", "b+", "a?", "^", "$", "a{1,2}", "[a-c]",
      "(", ")", "[", "\\", "|", "[z-a]", "[[:bogus:]]", "a(?=b)",
    ];
    const regexModes = ["ere", "bre-grep", "bre-sed", "bre-expr", "perl", "fixed"];
    const globTokens = [
      "a", "b", "*", "?", "/", ".", "[a-b]", "[^a]", "[!a]", "[[:digit:]]",
      "@(a|b)", "*(a)", "+(b)", "?(a|b)", "!(a)", "!(a|b)", "\\*", "[", "[z-a]",
    ];
    const hayAlphabet = ["a", "b", "c", "0", "1", " ", "_", ".", "/", "!", "é"];

    const t0 = performance.now();
    // Batch commands into scripts of 50 statements each so 1500 cases finish well under 2 s.
    let totalCases = 0;
    const batchLines = [];
    const flush = async () => {
      if (batchLines.length === 0) return;
      const script = batchLines.join("\n");
      batchLines.length = 0;
      const res = await sh.exec(script, { stdin: "abc 01\na/b.c\né!\n" });
      assert.ok(
        res.exitCode === 0 || res.exitCode === 1 || res.exitCode === 2,
        `unexpected exitCode ${res.exitCode}: ${res.stderr}`,
      );
    };

    // 900 regex fuzz cases
    for (let i = 0; i < 900; i++) {
      const n = 1 + (nextU32() % 4);
      let pat = "";
      for (let j = 0; j < n; j++) pat += pick(regexTokens);
      const mode = regexModes[i % regexModes.length];
      const flags = (nextU32() & 1) ? "i" : "-";
      batchLines.push(`__regex ${mode} ${flags} ${quote(pat)}`);
      totalCases++;
      if (batchLines.length >= 60) await flush();
    }
    await flush();

    // 600 glob/extglob fuzz cases
    for (let i = 0; i < 600; i++) {
      const n = 1 + (nextU32() % 3);
      let pat = "";
      for (let j = 0; j < n; j++) pat += pick(globTokens);
      const slen = nextU32() % 10;
      let subj = "";
      for (let j = 0; j < slen; j++) subj += pick(hayAlphabet);
      const flags = ((nextU32() & 1) ? "e" : "") + ((nextU32() & 1) ? "p" : "");
      const op = (i % 3 === 0) ? "match" : (i % 3 === 1 ? "strip-prefix" : "strip-suffix");
      batchLines.push(`__glob ${op} ${flags || "-"} ${quote(pat)} ${quote(subj)}`);
      totalCases++;
      if (batchLines.length >= 60) await flush();
    }
    await flush();

    const dt = performance.now() - t0;
    assert.ok(totalCases >= 1500, `expected >= 1500 cases, ran ${totalCases}`);
    assert.ok(dt < 2000, `fuzz slice took ${dt.toFixed(1)} ms (limit 2000 ms)`);

    // Verify Bash instance stayed healthy after all 1500 fuzz cases.
    const check = await sh.exec("echo healthy");
    assert.equal(check.exitCode, 0);
    assert.equal(check.stdout, "healthy\n");
  });
});
