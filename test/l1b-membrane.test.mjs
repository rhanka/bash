// Gates 9-14 and 16 of lot L1b: concurrency, re-entry, default deny, membrane, trust, pollution, sharing, trap.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Bash, InMemoryFs, T, bash, countKernelCalls, internal } from "./kt.mjs";
import { bytesOutput, defineCommand, stdoutAsBytes } from "@sentropic/bash";
import { repoRoot } from "./helpers.mjs";
import { assertUnderTime } from "./timing.mjs";

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
    return out(`${r.exitCode}:${r.stdout}:${/live byte limit exceeded/.test(r.stderr)}\n`);
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
  assertUnderTime("Virtual 5 s sleep", performance.now() - t0, 1000);
  assert.equal(r.stdout, "5000\n");
  assert.deepEqual(seen, []);
  const entropy = bash({}, { onHostCall: (r) => seen.push(r.tag) });
  const first = await entropy.exec("__entropy 16");
  assert.equal(first.stdoutKind, "bytes");
  assert.equal(Buffer.from(first.stdout, "latin1").toString("hex"), "ad4df30bae771bdc76606e02b9eef064");
  assert.equal((await bash().exec("__entropy 16")).stdout, first.stdout);
  assert.notEqual((await entropy.exec("__entropy 16")).stdout, first.stdout);
  assert.deepEqual(seen, []);
});

test("gate 10: a granted host call reaches the driver and is served through the membrane", async () => {
  const seen = [];
  const fs = {
    stat: async (p) => ({ isFile: true, isDirectory: false, isSymbolicLink: false, mode: 0o644, size: 1, mtime: new Date(0) }),
  };
  const b = bash({ fs }, { onHostCall: (r) => seen.push(r.tag) });
  const r = await b.exec("__hostfs 1 /p");
  assert.equal(r.stdout, "ok\n");
  assert.deepEqual(seen, ["fs-batch"]);
  assert.equal((await b.exec("__hostfs 7 /p")).stderr, "denied\n");
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
  // Double the CI limit here so it stays below the command's 500 ms sleep.
  assertUnderTime("Abort while a command ignores cancellation", took, 30 + 100 + 50 + 30, 2);
  const calls = state.calls;
  await sleep(600);
  assert.equal(afterWrite, "ExecutionAbortedError");
  assert.equal(state.calls, calls, "a late resume is not delivered");
  assert.equal(await b.fs.exists("/late"), false);
});

test("abort and deadline report a diagnostic when the kernel has no stderr to drain", async () => {
  const pending = defineCommand("pending", () => new Promise(() => {}));
  const reasons = [];
  const b = bash({ customCommands: [pending], executionLimits: { maxExecutionTimeMs: 20 } }, {
    onEngine(engine) {
      const proto = engine.Fiber.prototype;
      const cancel = proto.cancel;
      proto.cancel = function (reason) {
        reasons.push(reason);
        return cancel.call(this, reason);
      };
      // Model a kernel that has no room to allocate the cancellation diagnostic.
      proto.takeStderr = () => new Uint8Array(0);
    },
  });
  const aborted = await b.exec("pending", { signal: AbortSignal.abort() });
  const deadline = await b.exec("pending");
  assert.deepEqual(reasons, ["aborted", "deadline"]);
  for (const r of [aborted, deadline]) {
    assert.equal(r.exitCode, 124);
    assert.equal(r.stderr, "bash: execution aborted\n");
  }
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
  assert.equal(r.stderr, "failed at <path> and <path>\n");
});

test("shell assignments compose with trusted env writes and are absent for untrusted commands", async () => {
  const { state, hooks } = countKernelCalls();
  const trusted = defineCommand("trusted", async (_args, ctx) => {
    ctx.env.set("MAP_WRITE", "map");
    await ctx.assignShellVariable("OLD", "assigned");
    await ctx.assignShellVariable("NEW", "new");
    assert.equal(ctx.env.get("OLD"), "assigned");
    assert.equal(ctx.exportedEnv.OLD, "assigned");
    assert.equal(Object.hasOwn(ctx.exportedEnv, "NEW"), false);
  });
  const untrusted = defineCommand("untrusted", async (_args, ctx) => {
    if (ctx.assignShellVariable) await ctx.assignShellVariable("LEAK", "injected");
  }, { trusted: false });
  const show = defineCommand("show", async (_args, ctx) => out(JSON.stringify([
    ctx.env.get("MAP_WRITE"), ctx.env.get("OLD"), ctx.env.get("NEW"), ctx.env.get("LEAK"),
    ctx.exportedEnv.OLD, Object.hasOwn(ctx.exportedEnv, "NEW"),
  ])));
  const b = bash({ env: { OLD: "exported" }, customCommands: [trusted, untrusted, show] }, hooks);
  const r = await b.exec("trusted\nuntrusted\nshow");
  assert.equal(r.stdout, '["map","assigned","new",null,"assigned",false]');
  assert.deepEqual([r.env.MAP_WRITE, r.env.OLD, r.env.NEW, r.env.LEAK], ["map", "assigned", "new", undefined]);
  assert.equal(state.names.filter((name) => name === "Fiber.assignVariable").length, 2);
});

test("oversized host env and assignment data complete with a limit before the kernel operation", async () => {
  const cases = [
    ["setCallEnv", "maxStringLength", async (ctx) => ctx.env.set("é".repeat(51), "v")],
    ["assignVariable", "maxStringLength", async (ctx) => ctx.assignShellVariable("KEY", "é".repeat(51))],
    ["assignVariable", "maxStringLength", async (ctx) => ctx.assignShellVariable("KEY", "v", "é".repeat(51))],
    ["assignVariable", "headroom", async (ctx) => ctx.assignShellVariable("KEY", "x".repeat(4 << 20))],
    ["setCallEnv", "headroom", async (ctx) => ctx.env.set("KEY", "x".repeat(4 << 20))],
  ];
  for (const [method, limit, update] of cases) {
    const { state, hooks } = countKernelCalls();
    const cmd = defineCommand("change", async (_args, ctx) => { await update(ctx); });
    const b = bash({ executionLimitProfile: "micro", executionLimits: { maxStringLength: limit === "headroom" ? 8 << 20 : 100 }, customCommands: [cmd] }, hooks);
    const r = await b.exec("change\n__out 1");
    assert.equal(r.exitCode, 126);
    assert.match(r.stderr, new RegExp(limit));
    assert.equal(r.stdout, "");
    assert.ok(!state.names.includes(`Fiber.${method}`));
    assert.equal((await b.exec("__out 1")).stdout, "a");
  }
});

test("lazy command failures redact complete paths from all host roots", async () => {
  const paths = ["C:/private/file", "D:\\work\\file", "file:///C:/users/key", "/etc/a", "/workspace/a", "/app/a", "/nix/a", "/proc/a", "/run/a", `${repoRoot}/build/inner/file`];
  const b = bash({ customCommands: [{ name: "lazy", async load() { throw new Error(`load failed: ${paths.join(" ")}`); } }] });
  const r = await b.exec("lazy");
  assert.equal(r.exitCode, 1);
  assert.equal(r.stderr, `load failed: ${paths.map(() => "<path>").join(" ")}\n`);
});

test("byte stdout survives mixed fiber output and rows returned from ctx.exec", async () => {
  const binary = Uint8Array.of(0xc3, 0xa9, 0x80, 0xff);
  const bytes = defineCommand("bytes", async () => bytesOutput(binary, "é"));
  const child = defineCommand("child", async (_args, ctx) => ctx.exec("__out 1\nbytes"));
  const b = bash({ customCommands: [bytes, child] });
  const r = await b.exec("__out 1\nchild\n__out 1");
  assert.equal(r.stdoutKind, "bytes");
  assert.deepEqual([...Buffer.from(stdoutAsBytes(r), "latin1")], [97, 97, ...binary, 97]);
  assert.equal(r.stderr, "é");
  assert.equal((await b.exec("__out 1")).stdoutKind, undefined);
});

test("coverage and command listings are revoked, and ctx.signal preserves the caller abort reason", async () => {
  const ac = new AbortController();
  const reason = new Error("caller cancelled");
  const hits = [];
  const coverage = { hit(feature) { assert.equal(this, coverage); hits.push(feature); } };
  let ctx;
  let cancelled;
  const command = defineCommand("observe", async (_args, context) => {
    ctx = context;
    ctx.coverage.hit("live");
    assert.deepEqual(ctx.getRegisteredCommands(), ["observe"]);
    cancelled = new Promise((resolve) => ctx.signal.addEventListener("abort", () => resolve(ctx.signal.reason), { once: true }));
    ac.abort(reason);
    return out("");
  });
  const result = await bash({ customCommands: [command], coverage }).exec("observe", { signal: ac.signal });
  assert.equal(result.exitCode, 124);
  assert.equal(await cancelled, reason);
  assert.notEqual(ctx.coverage, coverage);
  assert.throws(() => ctx.coverage.hit("late"), { name: "ExecutionAbortedError" });
  assert.throws(() => ctx.getRegisteredCommands(), { name: "ExecutionAbortedError" });
  assert.deepEqual(hits, ["live"]);
});

test("fast host calls yield for a timer abort and check elapsed time on promise resolution", async () => {
  const ac = new AbortController();
  const fast = defineCommand("fast", async () => out(""));
  const b = bash({ customCommands: [fast] });
  setTimeout(() => ac.abort(), 0);
  assert.equal((await b.exec("fast\n".repeat(1500), { signal: ac.signal })).exitCode, 124);

  const late = defineCommand("late", async () => {
    const until = performance.now() + 35;
    while (performance.now() < until) { /* finish in this turn, after the deadline */ }
    return out("too late");
  });
  const timed = await bash({ customCommands: [late], executionLimits: { maxExecutionTimeMs: 20 } }).exec("late");
  assert.equal(timed.exitCode, 124);
  assert.equal(timed.stdout, "");
});

test("a trap in spawn produces one fatal result and subsequent execs reject", async () => {
  const b = bash({}, {
    onEngine(engine) {
      engine.Fiber.spawn = () => { throw new WebAssembly.RuntimeError("spawn trap"); };
    },
  });
  const r = await b.exec("");
  assert.equal(r.fatal, true);
  assert.equal(r.exitCode, 1);
  await assert.rejects(b.exec(""), { name: "KernelPoisonedError" });
});

test("poisoning stops the binding store before other calls can use it", async () => {
  const base = internal().kernel;
  let store;
  let stopped = 0;
  const kernel = {
    ...base,
    instantiate(...args) {
      store = base.instantiate(...args);
      store.engine.Fiber.prototype.run = () => { throw new WebAssembly.RuntimeError("run trap"); };
      return {
        engine: store.engine,
        _util: { stop() { stopped++; store._util.stop(); throw new Error("cleanup failed"); } },
      };
    },
  };
  const b = bash({}, { kernel });
  const r = await b.exec("");
  assert.equal(r.fatal, true);
  assert.equal(r.exitCode, 1);
  assert.equal(stopped, 1);
  assert.throws(() => store.engine.usage(), { name: "KernelPoisonedError" });
  await assert.rejects(b.exec(""), { name: "KernelPoisonedError" });
  assert.equal(stopped, 1);
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
  assert.deepEqual(await b.fs.readdir("/"), [...names, "bin", "dev", "proc", "usr"].sort());
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
  const entered = Promise.withResolvers();
  const gate = Promise.withResolvers();
  let scope;
  const wait = defineCommand("wait", async (_args, ctx) => {
    scope = ctx.fs;
    entered.resolve();
    await gate.promise;
    return out("never seen");
  });
  const b = bash({ customCommands: [wait] });
  const other = new Bash();
  const pending = b.exec("__out 1\nwait");
  await entered.promise;
  const trapped = await b.exec("__trap");
  assert.equal(trapped.exitCode, 1);
  assert.equal(trapped.stderr, "bash: internal error: sandbox instance failed\n");
  assert.equal(trapped.fatal, true);
  assert.equal(Object.keys(trapped).includes("fatal"), false);
  const concurrent = await pending;
  assert.equal(concurrent.exitCode, 1);
  assert.equal(concurrent.stderr, "bash: internal error: sandbox instance failed\n");
  assert.equal(concurrent.fatal, true);
  await assert.rejects(async () => scope.readFile("/x"), { name: "ExecutionAbortedError" });
  gate.resolve();
  await assert.rejects(b.exec(""), { name: "KernelPoisonedError" });
  await assert.rejects(b.readFile("/x"), { name: "KernelPoisonedError" });
  await assert.rejects(b.fs.exists("/x"), { name: "KernelPoisonedError" });
  assert.throws(() => b.getCwd(), { name: "KernelPoisonedError" });
  assert.throws(() => b.execSync(""), { name: "KernelPoisonedError" });
  assert.equal((await other.exec("echo ok")).stdout, "ok\n");
});
