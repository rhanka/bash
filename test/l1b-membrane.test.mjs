// Gates 9-14 and 16 of lot L1b: concurrency, re-entry, default deny, membrane, trust, pollution, sharing, trap.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Bash, InMemoryFs, T, bash, countKernelCalls, internal } from "./kt.mjs";
import { defineCommand } from "@sentropic/bash";
import { repoRoot } from "./helpers.mjs";

const out = (stdout, exitCode = 0, stderr = "") => ({ stdout, stderr, exitCode });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

test("gate 9: overlapping execs each get their own output", async () => {
  const slow = defineCommand("slow", async (args) => {
    await sleep(Number(args[0]));
    return out(`slow${args[0]}\n`);
  });
  const b = bash({ customCommands: [slow] });
  const [x, y] = await Promise.all([b.exec("slow 60\n__out 1"), b.exec("slow 10\n__out 2")]);
  assert.equal(x.stdout, "slow60\na");
  assert.equal(y.stdout, "slow10\naa");
});

test("gate 9: ctx.exec runs a child fiber charged to the parent's budget", async () => {
  const child = defineCommand("child", async (args, ctx) => {
    const r = await ctx.exec(args.join(" "), { cwd: ctx.cwd });
    return out(`${r.exitCode}:${r.stdout}:${/maxLiveBytes/.test(r.stderr)}\n`);
  });
  const b = bash({ customCommands: [child], executionLimits: { maxLiveBytes: 1_000_000 } });
  assert.equal((await b.exec("child __out 3")).stdout, "0:aaa:false\n");
  assert.equal((await b.exec("__alloc 600000\nchild __alloc 600000")).stdout, "126::true\n");
  assert.equal((await b.exec("__alloc 300000\nchild __alloc 300000")).stdout, "0::false\n");
});

test("gate 9: origCommand spawns a child that bypasses the custom command", async () => {
  const cat = defineCommand("shadow", async (_args, ctx) => {
    const r = await ctx.origCommand(["x"]);
    return out(`${r.exitCode}|${r.stderr}`);
  });
  const r = await bash({ customCommands: [cat] }).exec("shadow");
  assert.equal(r.stdout, "127|bash: shadow: command not found\n");
});

test("gate 10: default deny, with a recording driver that sees no host call", async () => {
  const seen = [];
  const b = bash({}, { onHostCall: (r) => seen.push(r.tag) });
  for (const cmd of ["__hostfs 1 /x", "__lazy /x", "__fetch http://example.test/"]) {
    const r = await b.exec(cmd);
    assert.equal(r.exitCode, 126, cmd);
    assert.equal(r.stderr, "denied\n");
  }
  assert.deepEqual(seen, []);
  const t0 = performance.now();
  const r = await b.exec("__sleep 5000\n__clock");
  assert.ok(performance.now() - t0 < 1000);
  assert.equal(r.stdout, "5000\n");
  assert.deepEqual(seen, []);
  const e1 = await bash().exec("__entropy 16");
  const e2 = await bash().exec("__entropy 16");
  assert.equal(e1.stdout, "16\n");
  assert.equal(e1.stdout, e2.stdout);
});

test("gate 10: a granted host call reaches the driver and is served through the membrane", async () => {
  const seen = [];
  const fs = {
    stat: async (p) => ({ isFile: true, isDirectory: false, isSymbolicLink: false, mode: 0o644, size: 1, mtime: new Date(0) }),
  };
  const b = bash({ fs }, { onHostCall: (r) => seen.push(r.tag) });
  const r = await b.exec("__hostfs 7 /p");
  assert.equal(r.stdout, "ok\n");
  assert.deepEqual(seen, ["fs-batch"]);
  let slept = null;
  const s = bash({ sleep: async (ms) => (slept = ms) });
  await s.exec("__sleep 25");
  assert.equal(slept, 25);
});

test("gate 11: a kept ctx.fs is revoked once the command resolved", async () => {
  let kept;
  const keep = defineCommand("keep", async (_a, ctx) => {
    kept = ctx.fs;
    return out("");
  });
  const b = bash({ customCommands: [keep] });
  await b.exec("keep");
  await assert.rejects(kept.writeFile("/x", "y"), { name: "ExecutionAbortedError" });
  assert.equal(await b.fs.exists("/x"), false);
});

test("gate 11: a command that ignores abort cannot delay the exec nor write afterwards", async () => {
  let afterWrite;
  const stubborn = defineCommand("stubborn", async (_a, ctx) => {
    await sleep(500);
    afterWrite = await ctx.fs.writeFile("/late", "x").then(() => "wrote", (e) => e.name);
    return out("late");
  });
  const { state, hooks } = countKernelCalls();
  const b = bash({ customCommands: [stubborn] }, hooks);
  const ac = new AbortController();
  setTimeout(() => ac.abort(), 30);
  const t0 = performance.now();
  const r = await b.exec("__out 2\nstubborn", { signal: ac.signal });
  const took = performance.now() - t0;
  assert.equal(r.exitCode, 124);
  assert.equal(r.stdout, "aa");
  assert.ok(took < 30 + 100 + 50 + 30, `took ${took}`);
  const calls = state.calls;
  await sleep(600);
  assert.equal(afterWrite, "ExecutionAbortedError");
  assert.equal(state.calls, calls, "a late resume is not delivered");
  assert.equal(await b.fs.exists("/late"), false);
});

test("gate 12: trust decides whether a command's environment change is visible", async () => {
  const set = (trusted) => defineCommand("setx", async (_a, ctx) => {
    try {
      ctx.env.set("X", "1");
    } catch {
      /* frozen for an untrusted command */
    }
    return out("");
  }, { trusted });
  const show = defineCommand("show", async (_a, ctx) => out(`[${ctx.env.get("X") ?? ""}]`));
  const un = bash({ customCommands: [set(false), show] });
  assert.equal((await un.exec("setx\nshow")).stdout, "[]");
  const tr = bash({ customCommands: [set(true), show] });
  assert.equal((await tr.exec("setx\nshow")).stdout, "[1]");
  const boom = defineCommand("boom", async () => {
    throw new Error(`failed at ${repoRoot}/secret/file and /Users/someone/x`);
  });
  const r = await bash({ customCommands: [boom] }).exec("boom");
  assert.equal(r.exitCode, 1);
  assert.ok(!r.stderr.includes(repoRoot), r.stderr);
  assert.ok(!r.stderr.includes("/Users/someone"), r.stderr);
  assert.match(r.stderr, /<path>/);
});

test("gate 13: prototype pollution names round-trip as data", async () => {
  const names = ["__proto__", "constructor", "prototype", "toString"];
  const env = JSON.parse(`{${names.map((n, i) => `"${n}": "v${i}"`).join(",")}}`);
  const files = JSON.parse(`{${names.map((n, i) => `"/${n}": "f${i}"`).join(",")}}`);
  const seen = [];
  const cmds = names.map((n) => defineCommand(n, async (args, ctx) => (seen.push([n, args, ctx.env.get("__proto__")]), out(`${n}:${args.join(",")}\n`))));
  const b = bash({ env, files, customCommands: cmds });
  const e = b.getEnv();
  for (const [i, n] of names.entries()) assert.equal(Object.getOwnPropertyDescriptor(e, n)?.value, `v${i}`, n);
  for (const [i, n] of names.entries()) assert.equal(await b.readFile(`/${n}`), `f${i}`);
  assert.deepEqual(await b.fs.readdir("/"), [...names].sort());
  const r = await b.exec(names.map((n) => `${n} ${names.join(" ")}`).join("\n"), { env: JSON.parse('{"__proto__":"x"}') });
  assert.equal(r.exitCode, 0);
  assert.equal(r.stdout, names.map((n) => `${n}:${names.join(",")}\n`).join(""));
  assert.equal(Object.getOwnPropertyDescriptor(r.env, "__proto__")?.value, "x");
  assert.equal(({}).polluted, undefined);
  assert.equal(Object.getPrototypeOf(r.env), null);
  assert.equal(seen[0][2], "x");
});

test("gate 14: Bash objects sharing one InMemoryFs share files only", async () => {
  const fs = new InMemoryFs(undefined, { hooks: internal(), profile: "contain", maxFileSystemBytes: 1 << 20 });
  const mine = defineCommand("mine", async () => out("mine"));
  const a = new Bash(T({ fs, customCommands: [mine] }));
  const b = new Bash(T({ fs, executionLimitProfile: "micro", executionLimits: { maxLiveBytes: 100_000 } }));
  assert.equal((await a.exec("__write /shared hello")).exitCode, 0);
  assert.equal((await b.exec("__cat /shared")).stdout, "hello");
  assert.equal((await a.exec("mine")).stdout, "mine");
  const r = await b.exec("mine");
  assert.equal(r.exitCode, 127);
  assert.equal(r.stderr, "bash: mine: command not found\n");
  assert.equal((await b.exec("__alloc 200000")).exitCode, 126);
  assert.equal((await a.exec("__alloc 200000")).exitCode, 0);
  assert.equal((await b.exec("__out 3")).stdout, "aaa");
  assert.notDeepEqual(Object.keys(a.getEnv()), []);
});

test("gate 16: a trap poisons the instance, and only that instance", async () => {
  const gate = (() => {
    let release;
    const p = new Promise((r) => (release = r));
    return { p, release };
  })();
  const wait = defineCommand("wait", async () => {
    await gate.p;
    return out("never seen");
  });
  const b = bash({ customCommands: [wait] });
  const other = bash();
  const pending = b.exec("__out 1\nwait");
  await sleep(30);
  const trapped = await b.exec("__trap");
  assert.equal(trapped.exitCode, 1);
  assert.equal(trapped.stderr, "bash: internal error: sandbox instance failed\n");
  assert.equal(trapped.fatal, true);
  assert.equal(Object.keys(trapped).includes("fatal"), false);
  const concurrent = await pending;
  assert.equal(concurrent.exitCode, 1);
  assert.equal(concurrent.stderr, "bash: internal error: sandbox instance failed\n");
  assert.equal(concurrent.fatal, true);
  gate.release();
  await assert.rejects(b.exec(""), { name: "KernelPoisonedError" });
  await assert.rejects(b.readFile("/x"), { name: "KernelPoisonedError" });
  await assert.rejects(b.fs.exists("/x"), { name: "KernelPoisonedError" });
  assert.throws(() => b.getCwd(), { name: "KernelPoisonedError" });
  assert.throws(() => b.execSync(""), { name: "KernelPoisonedError" });
  assert.equal((await other.exec("__out 2")).stdout, "aa");
});
