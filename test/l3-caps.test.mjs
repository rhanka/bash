import assert from "node:assert/strict";
import { test } from "node:test";
import { Bash } from "@sentropic/bash";
import { profileLimits } from "../host/core/limits.js";
import { bash } from "./kt.mjs";

function statusProbe() {
  const last = {};
  const hooks = {
    onEngine({ Fiber }) {
      for (const name of ["run", "resume", "cancel"]) {
        const original = Fiber.prototype[name];
        Fiber.prototype[name] = function (...args) {
          const result = original.apply(this, args);
          const value = name === "cancel" ? result : result.tag === "done" ? result.val : undefined;
          if (value) Object.assign(last, { status: value.status, limit: value.limitName });
          return result;
        };
      }
    },
  };
  return { last, hooks };
}

async function usable(shell) {
  const r = await shell.exec("echo ok");
  assert.deepEqual({ exitCode: r.exitCode, stdout: r.stdout, stderr: r.stderr }, { exitCode: 0, stdout: "ok\n", stderr: "" });
}

test("two script-size gates: maxSourceBytes and 1_000_000 UTF-16 parser cap", async () => {
  for (const cap of [12, 65536]) {
    const probe = statusProbe();
    const shell = bash({ executionLimitProfile: "normal", executionLimits: { maxSourceBytes: cap } }, probe.hooks);
    const over = await shell.exec("a".repeat(cap + 1));
    assert.equal(over.exitCode, 126);
    assert.equal(over.stderr, `bash: script input size limit exceeded (${cap} bytes)\n`);
    assert.equal(probe.last.status, "limit-exceeded");
    assert.equal(probe.last.limit, "maxSourceBytes");
    assert.equal(over.env["?"], undefined);

    const exact = await shell.exec("#" + "a".repeat(cap - 1));
    assert.equal(exact.exitCode, 0);
    await usable(shell);
  }

  // micro profile at 4 MiB + 1 bytes
  const mib4 = 4 << 20;
  const microProbe = statusProbe();
  const micro = bash({ executionLimitProfile: "micro" }, microProbe.hooks);
  const microOver = await micro.exec("x".repeat(mib4 + 1));
  assert.equal(microOver.exitCode, 126);
  assert.equal(microOver.stderr, `bash: script input size limit exceeded (${mib4} bytes)\n`);
  assert.equal(microProbe.last.limit, "maxSourceBytes");
  await usable(micro);

  // Post-normalization 1_000_000 UTF-16 code-unit parser cap under normal
  const normal = new Bash({ executionLimitProfile: "normal" });
  const asciiOver = await normal.exec("a=" + "x".repeat(1_000_000));
  assert.equal(asciiOver.exitCode, 2);
  assert.equal(asciiOver.stderr, "bash: syntax error: Parse error at 1:1: Input too large: 1000002 bytes exceeds limit of 1000000\n");

  const astralOver = await normal.exec("😀".repeat(500_001));
  assert.equal(astralOver.exitCode, 2);
  assert.equal(astralOver.stderr, "bash: syntax error: Parse error at 1:1: Input too large: 1000002 bytes exceeds limit of 1000000\n");
  await usable(normal);
});

test("top-level and cumulative token caps", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });
  const top = await shell.exec("a ".repeat(100_001));
  assert.equal(top.exitCode, 2);
  assert.equal(top.stderr, "bash: syntax error: Parse error at 1:1: Too many tokens: 100002 exceeds limit of 100000\n");

  const cum = await shell.exec("a ".repeat(60_000) + ": $(" + "b ".repeat(50_000) + ")");
  assert.equal(cum.exitCode, 2);
  assert.equal(cum.stderr, "bash: syntax error: Parse error at 1:1: Too many tokens: cumulative count exceeds limit of 100000\n");
  await usable(shell);
});

test("nesting depth cap (200 succeeds, 201 fails) across all nested constructs", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });
  const cases = [
    ["group", (n) => "{ ".repeat(n) + "echo deep;" + " }".repeat(n), "deep\n"],
    ["subshell", (n) => "( ".repeat(n) + "echo deep" + " )".repeat(n), "deep\n"],
    ["if", (n) => "if true; then ".repeat(n) + "echo deep;" + " fi;".repeat(n), "deep\n"],
    ["cond", (n) => "[[ " + "( ".repeat(n) + "a" + " )".repeat(n) + " ]]", ""],
    ["arith", (n) => "echo $(( " + "(".repeat(n) + "1" + ")".repeat(n) + " ))", "1\n"],
  ];
  for (const [label, gen, expectedOut] of cases) {
    const ok = await shell.exec(gen(200));
    assert.equal(ok.exitCode, 0, `${label} depth 200 failed: ${ok.stderr}`);
    assert.equal(ok.stdout, expectedOut, `${label} depth 200 stdout`);

    const err = await shell.exec(gen(201));
    assert.equal(err.exitCode, 2, `${label} depth 201 should fail`);
    assert.match(err.stderr, /Maximum parser nesting depth exceeded \(200\)/, label);
    await usable(shell);
  }
});

test("top-level statement iteration cap (> 10_000)", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });
  const r = await shell.exec("a\n".repeat(10_001));
  assert.equal(r.exitCode, 2);
  assert.equal(r.stderr, "bash: syntax error: Parse error at 10001:1: Parser stuck: too many iterations (>10000)\n");
  await usable(shell);
});

test("maxHeredocSize enforcement on overrides 5 and 12345", async () => {
  for (const cap of [5, 12345]) {
    const shell = new Bash({ executionLimitProfile: "normal", executionLimits: { maxHeredocSize: cap } });
    // Exact cap bytes (note trailing newline counts in body bytes)
    const exact = await shell.exec(`echo ok <<EOF\n${"x".repeat(cap - 1)}\nEOF`);
    assert.equal(exact.exitCode, 0, exact.stderr);
    assert.equal(exact.stdout, "ok\n");

    // Cap + 1 bytes on line 2
    const over = await shell.exec(`echo ok <<EOF\n${"x".repeat(cap)}\nEOF`);
    assert.equal(over.exitCode, 2);
    assert.equal(over.stderr, `bash: line 2: Heredoc size limit exceeded (${cap} bytes)\n`);
    await usable(shell);
  }
});

test("maxLoopIterations across profiles and loop kinds", async () => {
  for (const [profile, expected] of [
    ["micro", 25_000],
    ["contain", 100_000],
    ["hardened", 10_000],
    ["normal", 100_000],
  ]) {
    assert.equal(profileLimits(profile).maxLoopIterations, expected);
  }

  const probe = statusProbe();
  const small = bash({ executionLimitProfile: "contain", executionLimits: { maxLoopIterations: 3 } }, probe.hooks);
  const loopCases = [
    ["for", "for x in 1 2 3 4; do echo $x; done", "1\n2\n3\n"],
    ["while", "while echo w; do echo b; done", "w\nb\nw\nb\nw\nb\n"],
    ["until", "until ! echo u; do echo b; done", "u\nb\nu\nb\nu\nb\n"],
  ];
  for (const [kind, script, partialOut] of loopCases) {
    const r = await small.exec(script);
    assert.equal(r.exitCode, 126, kind);
    assert.equal(r.stdout, partialOut, kind);
    assert.equal(r.stderr, `bash: ${kind} loop: too many iterations (3), increase executionLimits.maxLoopIterations\n`);
    assert.equal(probe.last.status, "limit-exceeded");
    assert.equal(probe.last.limit, "maxLoopIterations");
    assert.equal(r.env["?"], undefined);
    await usable(small);
  }

  // Verify hardened profile default (10_000) with assignment body (raise maxCommandCount/maxWorkUnits so loop limit trips first)
  const hardProbe = statusProbe();
  const hard = bash({
    executionLimitProfile: "hardened",
    executionLimits: { maxCommandCount: 50_000, maxWorkUnits: 10_000_000 },
  }, hardProbe.hooks);
  const r = await hard.exec("while true; do x=1; done");
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, "bash: while loop: too many iterations (10000), increase executionLimits.maxLoopIterations\n");
  assert.equal(hardProbe.last.status, "limit-exceeded");
  assert.equal(hardProbe.last.limit, "maxLoopIterations");
  assert.equal(r.env["?"], undefined);
  await usable(hard);
});

test("timed-pipeline shape for time and time -p", async () => {
  const shell = new Bash();
  const r1 = await shell.exec("time echo hi");
  assert.equal(r1.exitCode, 0);
  assert.equal(r1.stdout, "hi\n");
  assert.match(r1.stderr, /^\nreal\t0m0\.000s\nuser\t0m0\.000s\nsys\t0m0\.000s\n$/);

  const r2 = await shell.exec("time -p echo hi");
  assert.equal(r2.exitCode, 0);
  assert.equal(r2.stdout, "hi\n");
  assert.match(r2.stderr, /^real 0\.00\nuser 0\.00\nsys 0\.00\n$/);

  const r3 = await shell.exec("time");
  assert.equal(r3.exitCode, 0);
  assert.equal(r3.stdout, "");
  assert.match(r3.stderr, /^\nreal\t0m0\.000s\nuser\t0m0\.000s\nsys\t0m0\.000s\n$/);
});

test("adversarial deep nesting in lexer, word/param/brace, arith, and cond never traps Wasm", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });
  const depthErrCases = [
    ["lexer cmd subst 5000", "echo " + "$(".repeat(5000)],
    ["lexer arith subst 5000", "echo " + "$((".repeat(5000)],
    ["lexer dq param 5000", "echo " + '"${x:-'.repeat(5000)],
    ["lexer legacy arith dq 5000", "echo " + '$["'.repeat(5000)],
    ["word brace 5000", "echo " + "{,".repeat(5000) + "}".repeat(5000)],
    ["word param bangs 5000", "echo ${" + "!".repeat(5000) + "}"],
    ["word param nested 5000", "echo " + "${x:-".repeat(5000) + "a" + "}".repeat(5000)],
    ["arith subscript 5000", "(( " + "a[".repeat(5000) + "0" + "]".repeat(5000) + " ))"],
    ["arith nested dparen 500", "(( " + "$((".repeat(500) + "1" + "))".repeat(500) + " ))"],
    ["arith nested bracket 500", "(( " + "$[".repeat(500) + "1" + "]".repeat(500) + " ))"],
    ["arith ternary consequent 500", "(( " + "1?".repeat(500) + "2" + ":3".repeat(500) + " ))"],
  ];
  for (const [label, script] of depthErrCases) {
    const r = await shell.exec(script);
    assert.equal(r.exitCode, 2, `${label} expected exit 2, got ${r.exitCode}: ${r.stderr}`);
    assert.match(r.stderr, /Maximum parser nesting depth exceeded \(200\)/, label);
    await usable(shell);
  }

  // Iterative chains stay O(1) stack and do not trap Wasm.
  const iterativeCases = [
    ["arith unary 50k", "if false; then echo $((" + "!".repeat(50_000) + "1)); fi; echo ok"],
    ["arith assign 5k", "if false; then (( " + "a=".repeat(5_000) + "1 )); fi; echo ok"],
    ["arith pow 5k", "if false; then (( " + "2**".repeat(5_000) + "2 )); fi; echo ok"],
    ["arith ternary alt 5k", "if false; then (( " + "1?2:".repeat(5_000) + "3 )); fi; echo ok"],
    ["cond not 50k", "if false; then [[ " + "! ".repeat(50_000) + "a ]]; fi; echo ok"],
    ["unclosed brace 5k", "if false; then echo " + "{,".repeat(5_000) + "; fi; echo ok"],
  ];
  for (const [label, script] of iterativeCases) {
    const r = await shell.exec(script);
    assert.equal(r.exitCode, 0, `${label} failed: ${r.stderr}`);
    assert.equal(r.stdout, "ok\n", label);
  }
  await usable(shell);
});

test("heredoc errors propagate through skip_newlines and diagnose unclosed substitutions", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });
  const r1 = await shell.exec("cat <<EOF &&\n${#x-y}\nEOF\necho ok\n");
  assert.equal(r1.exitCode, 2);
  assert.match(r1.stderr, /\$\{#x-y\}: bad substitution/);

  const r2 = await shell.exec("cat <<EOF |\n${#x-y}\nEOF\ncat\n");
  assert.equal(r2.exitCode, 2);
  assert.match(r2.stderr, /\$\{#x-y\}: bad substitution/);

  const r3 = await shell.exec("cat <<EOF\n${x\nEOF\n");
  assert.equal(r3.exitCode, 2);
  assert.match(r3.stderr, /unexpected EOF while looking for matching '\}'/);

  const r4 = await shell.exec("cat <<EOF\n$(echo hi\nEOF\n");
  assert.equal(r4.exitCode, 2);
  assert.match(r4.stderr, /unexpected EOF while looking for matching `\)'/);

  const r5 = await shell.exec("cat <<EOF\n`echo hi\nEOF\n");
  assert.equal(r5.exitCode, 2);
  assert.match(r5.stderr, /unexpected EOF while looking for matching ``'/);

  // Quoted heredoc does not parse expansions inside body.
  const r6 = await shell.exec("echo ok <<'EOF'\n${x\n$(echo hi\nEOF\n");
  assert.equal(r6.exitCode, 0, r6.stderr);
  assert.equal(r6.stdout, "ok\n");
  await usable(shell);
});

test("[[ argument does not corrupt cond_depth and for-loop respects readonly variables", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });

  const r1 = await shell.exec("echo [[; ]]");
  assert.equal(r1.exitCode, 127);
  assert.equal(r1.stdout, "[[\n");
  assert.equal(r1.stderr, "bash: ]]: command not found\n");

  const r2 = await shell.exec("echo [[; if false; then (( x = 1 )); cat <(echo hi); fi; echo ok");
  assert.equal(r2.exitCode, 0, r2.stderr);
  assert.equal(r2.stdout, "[[\nok\n");

  const r3 = await shell.exec("for SHELLOPTS in a b; do echo $SHELLOPTS; done; echo after");
  assert.equal(r3.exitCode, 1);
  assert.equal(r3.stdout, "");
  assert.equal(r3.stderr, "bash: SHELLOPTS: readonly variable\n");
  assert.equal(r3.env.SHELLOPTS, "braceexpand:hashall:interactive-comments");

  const r4 = await shell.exec("for SHELLOPTS in; do echo $SHELLOPTS; done");
  assert.equal(r4.exitCode, 0, r4.stderr);
  assert.equal(r4.stdout, "");
  await usable(shell);
});

test("linear-time lookahead in dparen, arith_subst, brace, extglob, and subscript scans (work-bounded and linear scaling in Wasm)", async () => {
  const ABS_CAP_MS = 2_000;

  // Min of 3 runs filters transient OS scheduling/GC pauses while still verifying every run.
  async function measureMinMs(fn, runs = 3) {
    let best = Infinity;
    let last;
    for (let i = 0; i < runs; i++) {
      const t0 = performance.now();
      last = await fn();
      const ms = performance.now() - t0;
      if (ms < best) best = ms;
    }
    return { result: last, ms: best };
  }

  // Warm up the Wasm instance before timing.
  const warmup = new Bash({ executionLimitProfile: "normal" });
  await usable(warmup);

  // 1. Kernel work-counter gate (deterministic, load-independent): every full-size case
  //    must complete well within a tight O(N) maxWorkUnits ceiling (O(N^2) would need >1e7).
  const dparenCases = [
    ["open parens 50k", "(".repeat(50_000), 150_000],
    ["open parens 50k + ))", "(".repeat(50_000) + "))", 150_000],
    ["spaced (( 25k + ))", "(( ".repeat(25_000) + "))", 150_000],
  ];
  for (const [label, script, workCap] of dparenCases) {
    const shell = new Bash({ executionLimitProfile: "normal", executionLimits: { maxWorkUnits: workCap } });
    const t0 = performance.now();
    const r = await shell.exec(script);
    const ms = performance.now() - t0;
    assert.equal(r.exitCode, 2, `${label} expected exit 2, got ${r.exitCode}: ${r.stderr}`);
    assert.match(r.stderr, /Maximum parser nesting depth exceeded \(200\)/, label);
    assert.ok(ms < ABS_CAP_MS, `${label} took ${ms.toFixed(1)}ms (expected < ${ABS_CAP_MS}ms)`);
  }

  const wordCases = [
    ["brace 100k + }", "echo " + "{".repeat(100_000) + "}", "{".repeat(100_000) + "}\n", 20_000],
    ["brace 50k balanced", "if false; then echo " + "{".repeat(50_000) + "}".repeat(50_000) + "; fi; echo ok", "ok\n", 20_000],
    ["brace 50k inner comma", "if false; then echo " + "{".repeat(50_000) + "a,b" + "}".repeat(50_000) + "; fi; echo ok", "ok\n", 20_000],
    ["brace interleaved 10k", "if false; then echo " + "{{a,b}".repeat(10_000) + "; fi; echo ok", "ok\n", 40_000],
    ["extglob 50k + )", "echo " + "@(".repeat(50_000) + ")", "@(".repeat(50_000) + ")\n", 20_000],
    ["extglob interleaved 10k", ": " + "@(@(a)".repeat(10_000), "", 20_000],
    ["subscript open 50k + ]=1", "if false; then " + "a[ ".repeat(50_000) + "]=1; fi; echo ok", "ok\n", 250_000],
  ];
  for (const [label, script, expectedOut, workCap] of wordCases) {
    const shell = new Bash({ executionLimitProfile: "normal", executionLimits: { maxWorkUnits: workCap } });
    const t0 = performance.now();
    const r = await shell.exec(script);
    const ms = performance.now() - t0;
    assert.equal(r.exitCode, 0, `${label} failed: ${r.stderr}`);
    assert.equal(r.stdout, expectedOut, label);
    assert.ok(ms < ABS_CAP_MS, `${label} took ${ms.toFixed(1)}ms (expected < ${ABS_CAP_MS}ms)`);
  }

  // 2. Verify the kernel work counter is actively charged for all 5 lookahead families.
  const workProbeCases = [
    ["dparen", "(( ".repeat(25_000) + "))"],
    ["arith_subst", "echo $(( " + " ".repeat(50_000) + "1))"],
    ["brace", "echo " + "{".repeat(100_000) + "}"],
    ["extglob", "echo " + "@(".repeat(50_000) + ")"],
    ["subscript", "if false; then " + "a[ ".repeat(50_000) + "]=1; fi; echo ok"],
  ];
  for (const [label, script] of workProbeCases) {
    const probe = statusProbe();
    const tight = bash({ executionLimitProfile: "normal", executionLimits: { maxWorkUnits: 500 } }, probe.hooks);
    const r = await tight.exec(script);
    assert.equal(r.exitCode, 126, `${label} should trip maxWorkUnits under tight cap`);
    assert.equal(probe.last.status, "limit-exceeded", label);
    assert.equal(probe.last.limit, "maxWorkUnits", label);
  }

  // 3. Load-independent N vs 4N time scaling check across all 5 families (linear ~4x, quadratic ~16x).
  const scalingCases = [
    ["dparen", (n) => "(( ".repeat(n) + "))", 5_000, 2],
    ["arith_subst", (n) => "echo $(( " + " ".repeat(n) + "1))", 10_000, 0],
    ["brace", (n) => "if false; then echo " + "{".repeat(n) + "a,b" + "}".repeat(n) + "; fi; echo ok", 10_000, 0],
    ["extglob", (n) => "echo " + "@(".repeat(n) + ")", 10_000, 0],
    ["subscript", (n) => "if false; then " + "a[ ".repeat(n) + "]=1; fi; echo ok", 10_000, 0],
  ];
  const shell = new Bash({ executionLimitProfile: "normal" });
  for (const [label, gen, n, expectedExit] of scalingCases) {
    const small = await measureMinMs(() => shell.exec(gen(n)));
    const large = await measureMinMs(() => shell.exec(gen(n * 4)));
    assert.equal(small.result.exitCode, expectedExit, `${label} N=${n}`);
    assert.equal(large.result.exitCode, expectedExit, `${label} 4N=${n * 4}`);
    assert.ok(large.ms < ABS_CAP_MS, `${label} 4N took ${large.ms.toFixed(1)}ms (expected < ${ABS_CAP_MS}ms)`);
    const ratio = large.ms / Math.max(small.ms, 1);
    assert.ok(
      ratio < 10,
      `${label} non-linear scaling: N=${n} took ${small.ms.toFixed(1)}ms, 4N=${n * 4} took ${large.ms.toFixed(1)}ms (ratio ${ratio.toFixed(2)}x, expected < 10x)`,
    );
  }

  await usable(shell);
});

test("dparen prefix-fail memoization does not over-skip on depth/rparen bailouts", async () => {
  const shell = new Bash({ executionLimitProfile: "normal" });
  const dparenSkip = await shell.exec("(".repeat(10) + "1+" + "(".repeat(5) + "1" + ") ".repeat(12) + "))");
  assert.equal(dparenSkip.exitCode, 2);
  assert.match(dparenSkip.stderr, /Expected RPAREN, got EOF/);
  await usable(shell);
});
