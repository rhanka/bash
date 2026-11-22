import assert from "node:assert/strict";
import { test } from "node:test";
import { Bash, defineCommand } from "../host/index.js";
import { filterTraceEvent } from "../harness/parity/run.mjs";
import * as kt from "./kt.mjs";

test("multi-slice buffer drain delivers >12000 coverage hits and >1000 trace events with zero drops under tight maxLiveBytes", async () => {
  const hits = [];
  const events = [];
  const bash = kt.bash({
    executionLimits: { maxLiveBytes: 128 * 1024 },
    coverage: { hit: (f) => hits.push(f) },
    trace: (ev) => events.push(ev),
  });

  const r = await bash.exec("__cov 12000 bash:builtin:echo\n__trace flood 1000");
  assert.equal(r.exitCode, 0);
  assert.equal(r.stderr, "");

  // 2 SimpleCommands + 4 word_globs + 12000 echo hits
  assert.equal(hits.filter((h) => h === "bash:builtin:echo").length, 12000);
  assert.equal(hits.length, 12006);

  assert.equal(events.length, 1000);
  for (let i = 0; i < 1000; i++) {
    assert.equal(events[i].category, "test");
    assert.equal(events[i].name, "item");
    assert.equal(events[i].durationMs, 0);
    assert.equal(Object.getPrototypeOf(events[i].details), null);
    assert.equal(events[i].details.seq, i);
  }
});

test("buffers drain before custom command callbacks and nested ctx.exec", async () => {
  const timeline = [];
  let hitsBeforeCustom = [];

  const mycmd = defineCommand("mycmd", async (_args, ctx) => {
    hitsBeforeCustom = timeline.slice();
    ctx.coverage.hit("custom:mid");
    ctx.trace({ category: "custom", name: "mid", durationMs: 1 });
    await ctx.exec("echo child");
    return { stdout: "", stderr: "", exitCode: 0 };
  });

  const bash = new Bash({
    customCommands: [mycmd],
    coverage: { hit: (f) => timeline.push(`cov:${f}`) },
    trace: (ev) => timeline.push(`trace:${ev.category}:${ev.name}`),
  });

  const r = await bash.exec("echo -n before; mycmd; echo -e after");
  assert.equal(r.exitCode, 0);

  assert.deepEqual(hitsBeforeCustom, [
    "cov:bash:cmd:SimpleCommand",
    "cov:bash:expansion:word_glob",
    "cov:bash:expansion:word_glob",
    "cov:bash:builtin:echo",
    "cov:cmd:flag:echo:-n",
    "cov:bash:cmd:SimpleCommand",
  ]);

  const midCov = timeline.indexOf("cov:custom:mid");
  const midTrace = timeline.indexOf("trace:custom:mid");
  const childEcho = timeline.indexOf("cov:bash:builtin:echo", hitsBeforeCustom.length);
  const afterFlag = timeline.indexOf("cov:cmd:flag:echo:-e");
  assert.ok(midCov >= hitsBeforeCustom.length);
  assert.ok(midTrace > midCov);
  assert.ok(childEcho > midTrace);
  assert.ok(afterFlag > childEcho);
});

test("buffers drain on cancel before exec resolves", async () => {
  const hits = [];
  const ac = new AbortController();
  const slow = defineCommand("slow_sleep", async (_args, ctx) => {
    ac.abort();
    await new Promise((resolve) => {
      ctx.signal.addEventListener("abort", resolve, { once: true });
    });
    return { stdout: "", stderr: "", exitCode: 0 };
  });

  const bash = new Bash({
    customCommands: [slow],
    coverage: { hit: (f) => hits.push(f) },
  });

  const r = await bash.exec("echo -n hi; slow_sleep", { signal: ac.signal });
  assert.equal(r.exitCode, 124);
  assert.deepEqual(hits.slice(0, 6), [
    "bash:cmd:SimpleCommand",
    "bash:expansion:word_glob",
    "bash:expansion:word_glob",
    "bash:builtin:echo",
    "cmd:flag:echo:-n",
    "bash:cmd:SimpleCommand",
  ]);
});

test("emit_find_trace satisfies Annex D.6 schema, path normalization, and S10 null-prototype details", async () => {
  const events = [];
  const bash = kt.bash({ trace: (ev) => events.push(ev) });

  const r = await bash.exec('__trace find /dir/ 4 2 1 3 5 1 0\n__trace find / 0 0 0 0 0 0 0\n__trace find "" 1 0 0 0 0 0 0');
  assert.equal(r.exitCode, 0);
  assert.equal(events.length, 6);

  for (const ev of events) {
    assert.equal(Object.getPrototypeOf(ev.details), null);
  }

  const summary = filterTraceEvent(events[0]);
  assert.equal(summary.category, "find");
  assert.equal(summary.name, "summary");
  assert.deepEqual(Object.keys(summary.details), [
    "readdirCalls",
    "readdirTimeMs",
    "statCalls",
    "statTimeMs",
    "evalCalls",
    "evalTimeMs",
    "nodeCount",
    "batchCount",
    "batchTimeMs",
    "earlyPrunes",
    "otherTimeMs",
  ]);
  assert.equal(summary.details.readdirCalls, 2);
  assert.equal(summary.details.statCalls, 1);
  assert.equal(summary.details.evalCalls, 3);
  assert.equal(summary.details.nodeCount, 5);
  assert.equal(summary.details.batchCount, 1);
  assert.equal(summary.details.earlyPrunes, 0);
  for (const k of ["readdirTimeMs", "statTimeMs", "evalTimeMs", "batchTimeMs", "otherTimeMs"]) {
    assert.equal(summary.details[k], null);
    assert.ok(Number.isFinite(events[0].details[k]));
  }

  const sp1 = filterTraceEvent(events[1]);
  assert.equal(sp1.category, "find");
  assert.equal(sp1.name, "searchPath");
  assert.deepEqual({ ...sp1.details }, { path: "/dir", resultsFound: 4 });

  const sp2 = filterTraceEvent(events[3]);
  assert.deepEqual({ ...sp2.details }, { path: "/", resultsFound: 0 });

  const sp3 = filterTraceEvent(events[5]);
  assert.deepEqual({ ...sp3.details }, { path: ".", resultsFound: 1 });
});
