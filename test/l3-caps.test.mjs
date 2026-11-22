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
  assert.match(top.stderr, /^bash: syntax error: Parse error at 1:1: Too many tokens: 10000[12] exceeds limit of 100000\n$/);

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
