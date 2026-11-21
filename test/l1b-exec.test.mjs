// Gates 2-7, 15 and 19 of lot L1b: results, environment, chunking, pre-checks, limits, fuel and cancel.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { Bash, T, bash, countKernelCalls, INTERNAL, internal } from "./kt.mjs";
import { assertUnderTime } from "./timing.mjs";

const EXIT_ABORTED = "bash: execution aborted\n";

test("branch exception IDs comply with the scope profile", () => {
  const branch = new URL("../BRANCH.md", import.meta.url);
  if (!existsSync(branch)) return;
  const profile = JSON.parse(readFileSync(new URL("../harness.profile.json", import.meta.url), "utf8"));
  const ids = readFileSync(branch, "utf8").matchAll(/^\s*- \*\*(BR\S+-EX\d+)\*\*/gm);
  for (const [, id] of ids) assert.match(id, new RegExp(profile.exceptionIdPattern));
});

/** The six cases of gate 2 as [name, run(options) => result]. */
const SIX = [
  ["empty script", (b) => b.exec(""), { exitCode: 0, stdout: "", stderr: "" }],
  ["unknown command", (b) => b.exec("nosuch arg"), { exitCode: 127, stderr: "bash: nosuch: command not found\n" }],
  ["script over 1 000 000 code units", (b) => b.exec(" ".repeat(1_000_006)), { exitCode: 2, stderr: "bash: syntax error: Parse error at 1:1: Input too large: 1000006 bytes exceeds limit of 1000000\n" }],
  ["maxSourceBytes", (b) => b.exec("x".repeat(205)), { exitCode: 126, stderr: "bash: script input size limit exceeded (100 bytes)\n" }, { executionLimits: { maxSourceBytes: 100 } }],
  ["abort already set", (b) => b.exec("__out 3", { signal: AbortSignal.abort() }), { exitCode: 124, stderr: EXIT_ABORTED }],
  ["maxOutputSize", (b) => b.exec("__out 16"), { exitCode: 126, stderr: "bash: pipeline: total output size exceeded (>10 bytes), increase executionLimits.maxOutputSize\n" }, { executionLimits: { maxOutputSize: 10 } }],
];

function check(r, want) {
  for (const [k, v] of Object.entries(want)) assert.equal(r[k], v, k);
}

test("gate 2: the six result cases, exact strings", async () => {
  for (const [name, run, want, options] of SIX) {
    const r = await run(bash(options));
    check(r, want);
    assert.ok(r.env, name);
  }
});

test("gate 15: defenseInDepth has no effect on the six cases", async () => {
  for (const di of [true, { enabled: false }]) {
    for (const [, run, want, options] of SIX) check(await run(bash({ ...options, defenseInDepth: di })), want);
  }
});

test("gate 19: execSync returns the same as exec for the six cases", () => {
  for (const [name, , want, options] of SIX) {
    const b = bash(options);
    const script = name === "empty script" ? "" : name === "unknown command" ? "nosuch arg" : name === "script over 1 000 000 code units" ? " ".repeat(1_000_006) : name === "maxSourceBytes" ? "x".repeat(205) : name === "abort already set" ? "__out 3" : "__out 16";
    const r = b.execSync(script, name === "abort already set" ? { signal: AbortSignal.abort() } : undefined);
    check(r, want);
  }
});

test("gate 19: execSync throws AsyncCapabilityError with an asynchronous capability", async () => {
  const { defineCommand } = await import("@sentropic/bash");
  const cmd = defineCommand("c", async () => ({ stdout: "", stderr: "", exitCode: 0 }));
  assert.throws(() => bash({ customCommands: [cmd] }).execSync(""), { name: "AsyncCapabilityError" });
  assert.throws(() => bash({ sleep: async () => {} }).execSync(""), { name: "AsyncCapabilityError" });
  assert.throws(() => bash({ fetch: async () => ({}) }).execSync(""), { name: "AsyncCapabilityError" });
  assert.throws(() => bash({ network: {} }).execSync(""), { name: "AsyncCapabilityError" });
  assert.throws(() => bash({ fs: {} }).execSync(""), { name: "AsyncCapabilityError" });
  const b = bash();
  b.registerCommand(cmd);
  assert.throws(() => b.execSync(""), { name: "AsyncCapabilityError" });
});

test("gate 3: environment and working directory", async () => {
  const b = bash();
  assert.equal(b.getCwd(), "/home/user");
  const env = b.getEnv();
  assert.deepEqual(Object.keys(env), ["HOME", "PATH", "IFS", "OSTYPE", "MACHTYPE", "HOSTTYPE", "HOSTNAME", "PWD", "OLDPWD", "OPTIND", "SHELLOPTS", "BASHOPTS"]);
  assert.deepEqual(
    { ...env },
    { HOME: "/home/user", PATH: "/usr/bin:/bin", IFS: " \t\n", OSTYPE: "linux-gnu", MACHTYPE: "x86_64-pc-linux-gnu", HOSTTYPE: "x86_64", HOSTNAME: "localhost", PWD: "/home/user", OLDPWD: "/home/user", OPTIND: "1", SHELLOPTS: "braceexpand:hashall:interactive-comments", BASHOPTS: "globskipdots" },
  );
  const c = bash({ cwd: "/x", env: { A: "1" } });
  assert.equal(c.getCwd(), "/x");
  const keys = Object.keys(c.getEnv());
  assert.equal(c.getEnv().HOME, "/");
  assert.equal(c.getEnv().PWD, "/x");
  assert.equal(keys[keys.indexOf("OPTIND") + 1], "A");
  const f = bash({ files: { "/a": "x" } });
  assert.equal(f.getCwd(), "/");
  assert.equal(f.getEnv().HOME, "/");
  const before = { ...b.getEnv() };
  const r = await b.exec("__setenv B 2");
  assert.equal(r.env.B, "2");
  assert.equal(r.env["?"], "0");
  assert.deepEqual({ ...b.getEnv() }, before);
  assert.equal(b.getCwd(), "/home/user");
});

test("gate 4: chunking keeps every transferred list at or below 1 MiB", async () => {
  const { state, hooks } = countKernelCalls();
  const b = bash({}, hooks);
  const stdin = "a".repeat(3 * 1024 * 1024 + 1);
  const r = await b.exec("__stdin", { stdin });
  assert.equal(r.stdout.length, stdin.length);
  assert.equal(r.stdout, stdin);
  assert.ok(state.maxList <= 1_048_576, `largest list ${state.maxList}`);
  const n = bash({ executionLimitProfile: "normal" });
  const big = await n.exec("__out 5000000");
  assert.equal(big.stdout.length, 5_000_000);
  assert.equal(big.exitCode, 0);
  // Bytes stay bytes: a stdin of bytes round-trips through stdinKind "bytes".
  const bytes = String.fromCharCode(...Array.from({ length: 256 }, (_, i) => i));
  const rb = await bash().exec("__stdin", { stdin: bytes, stdinKind: "bytes" });
  assert.equal(rb.stdout, bytes);
});

test("gate 5: pre-checks produce the limit result without any kernel call", async () => {
  const { state, hooks } = countKernelCalls();
  const b = bash({ executionLimits: { maxStringLength: 100 } }, hooks);
  const base = state.calls;
  const r = await b.exec("__out 1", { env: { BIG: "x".repeat(101) } });
  assert.equal(r.exitCode, 126);
  assert.match(r.stderr, /maxStringLength exceeded/);
  assert.equal(r.env["?"], "126");
  const longName = await b.exec("", { env: { ["é".repeat(51)]: "x" } });
  assert.equal(longName.exitCode, 126);
  assert.match(longName.stderr, /maxStringLength exceeded/);
  assert.equal(state.calls, base);
  const c = countKernelCalls();
  const d = bash({}, c.hooks);
  const calls = c.state.calls;
  const mib10 = "x".repeat(10 * 1024 * 1024);
  const r2 = await d.exec("__out 1", { args: [mib10, mib10] });
  assert.equal(r2.exitCode, 126);
  assert.equal(c.state.calls, calls);
});

test("option records refuse arrays and shell configuration rejects oversized inputs before lowering", async () => {
  for (const options of [[], { env: [] }, { files: [] }, { network: [] }, { fs: [] }, { executionLimits: [] }, { processInfo: [] }]) {
    assert.throws(() => new Bash(options), TypeError);
  }
  await assert.rejects(bash().exec("", []), TypeError);
  const { hooks, state } = countKernelCalls();
  const executionLimits = { maxStringLength: 100 };
  for (const options of [
    { env: { ["é".repeat(51)]: "v" } },
    { cwd: "/" + "é".repeat(50) },
    { customCommands: [{ name: "é".repeat(51), execute() {} }] },
  ]) {
    assert.throws(() => bash({ ...options, executionLimits }, hooks), /maxStringLength exceeded/);
  }
  assert.deepEqual(state.names, []);
  const b = bash({ executionLimitProfile: "micro", executionLimits: { maxStringLength: 64 << 20 } }, hooks);
  const before = state.calls;
  const overHalf = "x".repeat((4 << 20) + 1);
  const result = await b.exec("", { env: { [overHalf]: "" } });
  assert.match(result.stderr, /headroom exceeded/);
  assert.equal(state.calls, before);
  assert.throws(() => bash({ executionLimitProfile: "micro", env: { KEY: overHalf }, executionLimits: { maxStringLength: 64 << 20 } }, hooks), /headroom exceeded/);
  assert.equal(state.calls, before);
});

test("gate 6: limits are results, and the instance stays usable", async () => {
  const live = bash({ executionLimits: { maxLiveBytes: 1 << 20 } });
  const r1 = await live.exec("__alloc 2000000");
  assert.equal(r1.exitCode, 126);
  assert.match(r1.stderr, /maxLiveBytes exceeded/);
  assert.equal((await live.exec("__out 3")).stdout, "aaa");
  const inp = bash({ executionLimits: { maxInputBytes: 1000 } });
  const r2 = await inp.exec("__stdin", { stdin: "y".repeat(2000) });
  assert.equal(r2.exitCode, 126);
  assert.match(r2.stderr, /maxInputBytes exceeded/);
  assert.equal((await inp.exec("__out 3")).stdout, "aaa");
  assert.throws(() => bash({ executionLimits: { maxLiveBytes: 300 * 1024 * 1024 } }), RangeError);
  assert.throws(() => bash({ executionLimits: { nope: 1 } }), TypeError);
  assert.throws(() => bash({ executionLimits: { maxLiveBytes: -1 } }), TypeError);
  assert.throws(() => bash({ cwd: 3 }), TypeError);
  assert.throws(() => bash({ executionLimitProfile: "x" }), TypeError);
});

test("gate 6: an allocation beyond the memory ceiling is a limit result", async () => {
  const MIB = 1024 * 1024;
  const overrides = { executionLimits: { maxFileSystemBytes: 4 * MIB, maxOutputSize: 8 * MIB, maxLiveBytes: 56 * MIB } };
  const hooks = { ceilingBytes: 100 * MIB };
  const { InMemoryFs } = await import("./kt.mjs");
  const fs = new InMemoryFs(undefined, { hooks: internal(hooks), profile: "contain", maxFileSystemBytes: 4 * MIB });
  const { defineCommand } = await import("@sentropic/bash");
  let release;
  const gate = new Promise((r) => (release = r));
  const hold = defineCommand("hold", async () => {
    await gate;
    return { stdout: "", stderr: "", exitCode: 0 };
  });
  const a = new Bash({ ...overrides, fs, customCommands: [hold], [INTERNAL]: internal(hooks) });
  const b = new Bash({ ...overrides, fs, [INTERNAL]: internal(hooks) });
  const pa = a.exec("__alloc 58000000\nhold");
  await new Promise((r) => setTimeout(r, 50));
  const rb = await b.exec("__alloc 58000000");
  assert.equal(rb.exitCode, 126);
  assert.match(rb.stderr, /memory-ceiling exceeded/);
  release();
  assert.equal((await pa).exitCode, 0);
  assert.equal((await b.exec("__out 3")).stdout, "aaa");
});

test("gate 7: fuel slices, abort and the wall-time limit", async () => {
  const b = bash();
  const ac = new AbortController();
  setTimeout(() => ac.abort(), 20);
  const t0 = performance.now();
  const r = await b.exec("__out 4\n__spin", { signal: ac.signal });
  assert.equal(r.exitCode, 124);
  assert.equal(r.stdout, "aaaa");
  assert.equal(r.stderr, EXIT_ABORTED);
  assertUnderTime("Abort during fuel slices", performance.now() - t0, 200);
  const w = bash({ executionLimits: { maxExecutionTimeMs: 200 } });
  const t1 = performance.now();
  const r2 = await w.exec("__out 2\n__spin");
  assert.equal(r2.exitCode, 124);
  assert.equal(r2.stdout, "aa");
  assertUnderTime("Wall-time limit during fuel slices", performance.now() - t1, 400);
  assert.equal((await w.exec("__out 3")).stdout, "aaa");
});

test("runtime options (EVOL D13, V5)", async () => {
  const names = ["python3", "python", "js-exec", "node"];
  const { getPythonCommandNames, getJavaScriptCommandNames, getCommandNames, getNetworkCommandNames } = await import("@sentropic/bash");
  for (const o of [{ python: true }, { javascript: true }, { javascript: {} }]) {
    const b = bash(o);
    for (const n of names) {
      const r = await b.exec(n);
      assert.equal(r.exitCode, 127, n);
      assert.equal(r.stderr, `bash: ${n}: command not found\n`);
    }
  }
  assert.deepEqual(getPythonCommandNames(), ["python3", "python"]);
  assert.deepEqual(getJavaScriptCommandNames(), ["js-exec", "node"]);
  assert.deepEqual(getCommandNames(), []);
  assert.deepEqual(getNetworkCommandNames(), ["curl"]);
  const pkg = await import("@sentropic/bash");
  assert.deepEqual(Object.keys(pkg).filter((k) => /DefenseInDepth|SecurityViolation/.test(k)), []);
  assert.throws(() => new Bash().transform(), { name: "NotSupportedError" });
  assert.throws(() => new Bash().registerTransformPlugin(), { name: "NotSupportedError" });
});
