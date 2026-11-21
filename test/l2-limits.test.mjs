import assert from "node:assert/strict";
import { test } from "node:test";
import { defineCommand } from "@sentropic/bash";
import { profileLimits, PROFILES } from "../host/core/limits.js";
import { Bash, bash, enc, dec, internal, module } from "./kt.mjs";
import { Instance } from "../host/core/instance.js";

const input = (cap) => "bash: stdin size limit exceeded (" + cap + " bytes)\n";
const commandCount = (cap) => "bash: too many commands executed (>" + cap + "), increase executionLimits.maxCommandCount\n";
const callDepth = (cap) => "bash: f: maximum recursion depth (" + cap + ") exceeded, increase executionLimits.maxCallDepth\n";
const mib = 1 << 20;

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
  const { exitCode, stdout, stderr } = await shell.exec("echo ok");
  assert.deepEqual({ exitCode, stdout, stderr }, { exitCode: 0, stdout: "ok\n", stderr: "" });
}

async function expectLimit(shell, probe, limit, stderr, script, opts) {
  const r = await shell.exec(script, opts);
  assert.equal(r.exitCode, 126, r.stderr);
  assert.equal(r.stderr, stderr, limit);
  assert.equal(probe.last.status, "limit-exceeded");
  assert.equal(probe.last.limit, limit);
  assert.equal(r.env["?"], undefined);
  await usable(shell);
}

function commandPressure(cap) {
  if ((cap + 1) * 3 <= 100_000) return "echo x\n".repeat(cap + 1);
  // A flat list exceeds the independent lexer cap before this profile's command cap.
  const levels = Math.ceil(Math.log2(cap + 1));
  const script = ["f0() { echo x; }"];
  for (let i = 1; i <= levels; i++) script.push("f" + i + "() { f" + (i - 1) + "; f" + (i - 1) + "; }");
  script.push("f" + levels);
  return script.join("\n");
}

for (const profile of ["micro", "contain", "hardened", "normal"]) {
  test(profile + " enforces its byte and operational limits and recovers", async (t) => {
    const L = profileLimits(profile);
    const probe = statusProbe();
    const recur = defineCommand("recur", (_args, ctx) => ctx.exec("recur"));
    const shell = bash({ executionLimitProfile: profile, customCommands: [recur] }, probe.hooks);
    const cap = Math.min(L.maxInputBytes, L.maxStringLength);
    const inputName = L.maxInputBytes <= L.maxStringLength ? "maxInputBytes" : "maxStringLength";
    await expectLimit(shell, probe, "maxSourceBytes", "bash: script input size limit exceeded (" + L.maxSourceBytes + " bytes)\n", "x".repeat(L.maxSourceBytes + 1));
    await expectLimit(shell, probe, inputName, input(cap), "__stdin", { stdin: "x".repeat(cap + 1) });
    const large = "A=" + "x".repeat(900_000) + "\nB=" + "$A".repeat(Math.ceil((L.maxStringLength + 1) / 900_000));
    await expectLimit(shell, probe, "maxStringLength", "bash: word expansion: string length limit exceeded (" + L.maxStringLength + " bytes)\n", large);
    await expectLimit(shell, probe, "maxOutputSize", "bash: pipeline: total output size exceeded (>" + L.maxOutputSize + " bytes), increase executionLimits.maxOutputSize\n", "__out " + (L.maxOutputSize + 1));
    await expectLimit(shell, probe, "maxLiveBytes", "bash: execution: live byte limit exceeded (" + L.maxLiveBytes + " bytes)\n", "__alloc " + (L.maxLiveBytes + 1));
    await expectLimit(shell, probe, "maxFileSystemBytes", "bash: filesystem size limit exceeded (" + L.maxFileSystemBytes + " bytes)\n", "__fsalloc /big " + (L.maxFileSystemBytes + 1));
    await expectLimit(shell, probe, "maxCommandCount", commandCount(L.maxCommandCount), commandPressure(L.maxCommandCount));
    await expectLimit(shell, probe, "maxCallDepth", callDepth(L.maxCallDepth), "f() { f; }; f");
    await expectLimit(shell, probe, "maxWorkUnits", "bash: execution: aggregate work limit exceeded (" + L.maxWorkUnits + ")\n", "__work " + (L.maxWorkUnits + 1));
    // ctx.exec propagates its children's streams and exit code through custom-command results.
    const nested = await shell.exec("recur");
    assert.equal(nested.exitCode, 126);
    assert.equal(nested.stderr, "bash: maximum nested execution depth (" + L.maxExecDepth + ") exceeded\n");
    await usable(shell);
    const array = 'A=a\n' + 'A="$A $A"\n'.repeat(Math.floor(Math.log2(L.maxArrayElements)) + 1) + "echo $A";
    await expectLimit(shell, probe, "maxArrayElements", "bash: array element limit exceeded (" + L.maxArrayElements + ")\n", array);

    // Drive the host clock across the configured deadline without waiting between 10 s and 1 h.
    let ticks = 0;
    const clock = t.mock.method(performance, "now", () => ticks++ < 2 ? 0 : L.maxExecutionTimeMs + 1);
    try {
      const spin = await shell.exec("__spin");
      assert.equal(spin.exitCode, 124);
      assert.equal(spin.stderr, "bash: execution aborted\n");
      assert.equal(probe.last.status, "deadline");
    } finally {
      clock.mock.restore();
    }
    await usable(shell);
  });
}

test("micro also reports the input byte limit when it is tighter than the string limit", async () => {
  const probe = statusProbe();
  const shell = bash({ executionLimitProfile: "micro", executionLimits: { maxStringLength: 32 * mib } }, probe.hooks);
  const cap = profileLimits("micro").maxInputBytes;
  await expectLimit(shell, probe, "maxInputBytes", input(cap), "__stdin", { stdin: "x".repeat(cap + 1) });
});

test("oversized stdin preserves the live byte failure recorded at spawn", async () => {
  const probe = statusProbe();
  const shell = bash({ executionLimits: { maxLiveBytes: 8_192, maxInputBytes: 10 } }, probe.hooks);
  await expectLimit(shell, probe, "maxLiveBytes", "bash: execution: live byte limit exceeded (8192 bytes)\n",
    "echo " + "a".repeat(9_000), { stdin: "z".repeat(11) });
});

test("a blank script with oversized stdin returns the input limit", async () => {
  const probe = statusProbe();
  const shell = bash({ executionLimits: { maxInputBytes: 10 } }, probe.hooks);
  await expectLimit(shell, probe, "maxInputBytes", input(10), " \t\n\u00a0", { stdin: "z".repeat(11) });
});

test("the kernel and the constructor reject a hardened override that exceeds its ceiling", () => {
  const options = { executionLimitProfile: "hardened", executionLimits: { maxOutputSize: 20 * mib } };
  assert.throws(() => new Bash(options), RangeError);
  const { engine } = new Instance(module, internal());
  engine.configure(BigInt(PROFILES.hardened.ceiling));
  const root = engine.Vfs.create(128n << 20n);
  assert.throws(() => engine.Shell.create(root, {
    limits: { profile: "hardened", overrides: [{ name: "maxOutputSize", value: 20n << 20n }] },
    grants: {}, customCommands: [], env: [], defaultLayout: false, trace: false, coverage: false,
  }), { code: "invalid-config" });
});

test("deep shell recursion saves its call frames on budgeted heap memory", async () => {
  const shell = new Bash({ executionLimits: { maxCallDepth: 10_000, maxCommandCount: 20_000 } });
  const r = await shell.exec("f() { f; }; f");
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, callDepth(10_000));
  await usable(shell);
});

test("a recursion diagnostic survives a full, zero, or nearly spent output budget", async () => {
  const name = "f".repeat(1024);
  const suffix = ": maximum recursion depth (1) exceeded, increase executionLimits.maxCallDepth\n";
  for (const maxOutputSize of [0, 6, 20]) {
    const shell = new Bash({ executionLimits: { maxCallDepth: 1, maxOutputSize } });
    const prefix = maxOutputSize === 0 ? "" : "echo begin; ";
    const r = await shell.exec(`${prefix}${name}() { ${name}; }; ${name}`);
    assert.equal(r.exitCode, 126);
    assert.equal(r.stdout, prefix ? "begin\n" : "");
    assert.equal(r.stderr, "bash: " + name.slice(0, 128) + suffix);
    assert.equal((await shell.exec("A=ok")).env.A, "ok");
  }
  const shell = bash({ executionLimits: { maxCallDepth: 1, maxOutputSize: 6 } });
  const r = await shell.exec("__err 6; f() { f; }; f");
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, "aaaaaa" + callDepth(1));
  await usable(shell);
});

test("a recursion limit has a host diagnostic if no kernel stderr is available", async () => {
  const shell = bash({ executionLimits: { maxCallDepth: 1 } }, {
    onEngine({ Fiber }) {
      Fiber.prototype.takeStderr = () => new Uint8Array();
    },
  });
  const r = await shell.exec("f() { f; }; f");
  assert.equal(r.exitCode, 126);
  assert.equal(r.stderr, "bash: maxCallDepth exceeded\n");
});

test("an exec overlay can fit in the live budget as one copy", async () => {
  const shell = new Bash({ executionLimits: { maxLiveBytes: 8_192, maxCallDepth: 1 } });
  const payload = "x".repeat(5_000);
  const opts = { env: { PAYLOAD: payload, A: "before" }, replaceEnv: true };
  const r = await shell.exec("A=after; echo ok", opts);
  assert.equal(r.exitCode, 0, r.stderr);
  assert.equal(r.stdout, "ok\n");
  assert.equal(r.env.PAYLOAD, payload);
  assert.equal(r.env.A, "after");
  assert.equal(r.env.PWD, undefined);
  const early = await shell.exec("A=; f() { f; }; f", opts);
  assert.equal(early.exitCode, 126);
  assert.equal(early.stderr, callDepth(1));
  assert.equal(early.env.PAYLOAD, payload);
  assert.equal(early.env.A, "before");
  await usable(shell);
});

test("an exec overlay is charged to the fiber before it can be retained", async () => {
  const shell = new Bash({ executionLimits: { maxLiveBytes: 8_192 } });
  const payload = "x".repeat(9_000);
  // An empty script previously kept an over-budget overlay in scratch for its early result.
  const r = await shell.exec("", { env: { PAYLOAD: payload } });
  assert.equal(r.exitCode, 126);
  assert.match(r.stderr, /maxLiveBytes/);
  assert.equal(r.env.PAYLOAD, payload);
  await usable(shell);
});

test("the parser caps apply before the VM runs a stage", async () => {
  const shell = new Bash();
  const tokens = await shell.exec("echo x\n".repeat(33_334));
  assert.equal(tokens.exitCode, 2);
  assert.equal(tokens.stdout, "");
  assert.equal(tokens.stderr, "bash: syntax error: Parse error at 1:1: Too many tokens: 100002 exceeds limit of 100000\n");
  const nested = await shell.exec("{ ".repeat(201) + "echo x;" + " };".repeat(201));
  assert.equal(nested.exitCode, 2);
  assert.equal(nested.stdout, "");
  assert.match(nested.stderr, /Maximum parser nesting depth exceeded \(200\)/);
  await usable(shell);
});

test("hardened keeps streams and a diagnostic when filesystem, live and output allocations fill the instance", async () => {
  const { engine } = new Instance(module, internal());
  engine.configure(BigInt(PROFILES.hardened.ceiling));
  const root = engine.Vfs.create(128n << 20n);
  const other = engine.Vfs.create(72n << 20n);
  const shell = engine.Shell.create(root, {
    limits: { profile: "hardened", overrides: [] }, grants: {}, customCommands: [], env: [],
    defaultLayout: false, trace: false, coverage: false,
  });
  // A second volume consumes shared instance space while remaining within its own byte budget.
  assert.equal(other.op({ tag: "write-file", val: { path: enc("/fill"), data: enc(""), offset: 70n << 20n, truncate: true } }).tag, "ok");
  const run = (script) => {
    const f = engine.Fiber.spawn(shell, undefined, { script: enc(script), args: [], env: [], replaceEnv: false, rawScript: false });
    return { f, step: f.run(64n) };
  };
  const { f, step } = run("__fsalloc /occupied 125829120; __out 9437184; __alloc 25165824; __alloc 37748736");
  assert.equal(step.tag, "done");
  assert.equal(step.val.exitCode, 126);
  assert.equal(step.val.limitName, "memory-ceiling");
  assert.equal(dec(f.takeStderr(100)), "bash: memory-ceiling exceeded\n");
  assert.equal(dec(f.takeStdout(4)), "aaaa");
  f[Symbol.dispose]();
  const next = run("echo ok");
  assert.equal(next.step.val.exitCode, 0);
  assert.equal(dec(next.f.takeStdout(100)), "ok\n");
});
