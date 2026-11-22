import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { Bash } from "../host/index.js";
import { repoRoot } from "./helpers.mjs";
import * as kt from "./kt.mjs";

test("S2 + S11: trace-clock bypasses onHostCall and never leaks into script-visible state", async () => {
  const runWithClock = async (stamps) => {
    let idx = 0;
    const hostCalls = [];
    const events = [];
    const bash = kt.bash(
      { trace: (ev) => events.push(ev) },
      {
        onHostCall: (req) => hostCalls.push(req),
        traceClockNs: () => stamps[idx++],
      },
    );
    const r = await bash.exec("__clock\n__trace span alpha\necho $PWD");
    return { r, hostCalls, events, reads: idx };
  };

  const r1 = await runWithClock([0n, 2_000_000n]);
  const r2 = await runWithClock([100_000_000_000n, 100_055_500_000n]);

  assert.equal(r1.r.stdout, "0\nalpha\n/home/user\n");
  assert.equal(r2.r.stdout, r1.r.stdout);
  assert.equal(r1.r.stderr, "");
  assert.equal(r2.r.stderr, "");
  assert.equal(r1.r.exitCode, 0);
  assert.equal(r2.r.exitCode, 0);
  assert.deepEqual(r1.r.env, r2.r.env);

  assert.deepEqual(r1.hostCalls, []);
  assert.deepEqual(r2.hostCalls, []);
  assert.equal(r1.reads, 2);
  assert.equal(r2.reads, 2);

  assert.equal(r1.events.length, 1);
  assert.equal(r1.events[0].durationMs, 2);
  assert.equal(r1.events[0].details.label, "alpha");
  assert.equal(r2.events.length, 1);
  assert.equal(r2.events[0].durationMs, 55.5);

  // When trace is disabled, __trace span issues zero TraceClock host calls.
  let disabledReads = 0;
  const plain = kt.bash({}, { traceClockNs: () => { disabledReads++; return 1n; } });
  const rPlain = await plain.exec("__trace span beta");
  assert.equal(rPlain.stdout, "beta\n");
  assert.equal(disabledReads, 0);
});

test("execSync succeeds when logger, trace, and coverage are configured", async () => {
  await Bash.precompile();
  const logs = [];
  const hits = [];
  const events = [];
  const bash = new Bash({
    logger: {
      info: (m, d) => logs.push(["info", m, d]),
      debug: (m, d) => logs.push(["debug", m, d]),
    },
    coverage: { hit: (f) => hits.push(f) },
    trace: (ev) => events.push(ev),
  });

  const r = bash.execSync("echo -n sync");
  assert.equal(r.exitCode, 0);
  assert.equal(r.stdout, "sync");
  assert.deepEqual(logs, [
    ["info", "exec", { command: "echo -n sync" }],
    ["debug", "stdout", { output: "sync" }],
    ["info", "exit", { exitCode: 0 }],
  ]);
  assert.ok(hits.includes("cmd:flag:echo:-n"));
  assert.deepEqual(events, []);

  // Synchronous trace events via trace-clock on kernel-test
  const syncEvents = [];
  let ns = 10_000_000n;
  const ksync = kt.bash(
    { trace: (ev) => syncEvents.push(ev) },
    { traceClockNs: () => { const v = ns; ns += 5_000_000n; return v; } },
  );
  const tr = ksync.execSync("__trace span sync");
  assert.equal(tr.stdout, "sync\n");
  assert.equal(syncEvents.length, 1);
  assert.equal(syncEvents[0].durationMs, 5);
  assert.equal(syncEvents[0].details.label, "sync");
});

test("exec-oneshot rejects trace or coverage with invalid-config", async () => {
  const { instantiate } = await import("../dist/js/kernel/kernel.js");
  const core = new WebAssembly.Module(readFileSync(join(repoRoot, "dist", "js", "kernel", "kernel.core.wasm")));
  const { engine, _util } = instantiate(() => core, {});
  try {
    engine.configure(64n << 20n);
    const baseConfig = {
      limits: { profile: "contain", overrides: [] },
      grants: {},
      customCommands: [],
      env: [],
      defaultLayout: false,
      trace: false,
      coverage: false,
    };
    const opts = {
      script: new TextEncoder().encode("echo hi"),
      args: [],
      env: [],
      replaceEnv: false,
      rawScript: false,
    };
    for (const flags of [{ trace: true, coverage: false }, { trace: false, coverage: true }]) {
      assert.throws(
        () => engine.execOneshot({ files: [], config: { ...baseConfig, ...flags }, opts, stdin: new Uint8Array(), fuel: 100n }),
        (err) => err?.code === "invalid-config",
      );
    }
  } finally {
    _util.stop();
  }
});

test("release kernel omits __cov and __trace test commands", () => {
  const release = [
    readFileSync(join(repoRoot, "dist", "kernel.core.wasm")),
    readFileSync(join(repoRoot, "dist", "kernel.component.wasm")),
  ];
  const testCore = readFileSync(join(repoRoot, "dist", "test", "kernel-test.core.wasm"));
  for (const name of ["__cov", "__trace"]) {
    assert.ok(testCore.includes(name), `${name} must exist in kernel-test`);
    for (const r of release) {
      assert.ok(!r.includes(name), `${name} must not exist in release kernel`);
    }
  }
});
