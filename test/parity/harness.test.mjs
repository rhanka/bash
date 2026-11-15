// Self-tests of the parity harness (lot L10a, spec section 8), run on the mock adapter.
import { test } from "node:test";
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { repoRoot, scratch } from "../helpers.mjs";
import { canonicalize } from "../../harness/parity/canonical.mjs";
import { recordAll } from "../../harness/parity/record.mjs";
import { evaluateCase, runParity } from "../../harness/parity/replay.mjs";
import { loadCases } from "../../harness/parity/validate.mjs";
import mock, { createMockAdapter } from "../../harness/parity/mock-adapter.mjs";

const SAMPLE_CASES = join(repoRoot, "test/cases/l10a");
const SAMPLE_SNAPS = join(repoRoot, "test/snapshots/l10a");
const ADAPTER = join(repoRoot, "harness/parity/mock-adapter.mjs");
const NO_EXCLUSIONS = { cases: {}, divergences: {} };

const cleanEnv = () => {
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  return env;
};
const node = (args) => spawnSync(process.execPath, args, { cwd: repoRoot, env: cleanEnv(), encoding: "utf8" });
const cli = (cases, out, ...extra) =>
  node(["tools/parity-record.mjs", "--adapter", ADAPTER, "--cases", cases, "--out", out, ...extra]);

function writeCase(dir, name, spec) {
  const lot = dir.split("/").pop();
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `${name}.case.json`), canonicalize({ id: `${lot}/${name}`, title: name, ...spec }));
}

/** A scratch lot directory `<scratch>/<lot>` holding a copy of the sample cases. */
function sampleCopy(lot = "lx") {
  const base = scratch("l10a");
  const cases = join(base, lot);
  cpSync(SAMPLE_CASES, cases, { recursive: true });
  for (const f of readdirSync(cases)) {
    // ids carry the lot of the directory
    const p = join(cases, f);
    const obj = JSON.parse(readFileSync(p, "utf8"));
    obj.id = obj.id.replace(/^l10a\//, `${lot}/`);
    writeFileSync(p, canonicalize(obj));
  }
  return { base, cases, snaps: join(base, "snaps") };
}

// 1. Canonical form
test("canonical form: 100 permutations give identical bytes", () => {
  let seed = 12345;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  const shuffle = (a) => {
    const b = [...a];
    for (let i = b.length - 1; i > 0; i--) {
      const j = Math.floor(rnd() * (i + 1));
      [b[i], b[j]] = [b[j], b[i]];
    }
    return b;
  };
  const leaves = { z: 1, a: "x", m: [3, { q: 1, b: 2 }], "\u00e9": null };
  const build = () => {
    const l3 = {};
    for (const k of shuffle(Object.keys(leaves))) l3[k] = leaves[k];
    const l2 = {};
    for (const k of shuffle(["k1", "k2", "k3", "k0"])) l2[k] = k === "k2" ? l3 : { v: k };
    const l1 = {};
    for (const k of shuffle(["top", "alpha", "mid"])) l1[k] = l2;
    return l1;
  };
  const first = canonicalize(build());
  for (let i = 0; i < 100; i++) assert.equal(canonicalize(build()), first);
  assert.ok(first.endsWith("}\n") && !first.endsWith("\n\n"));
  assert.ok(first.indexOf('"alpha"') < first.indexOf('"mid"') && first.indexOf('"mid"') < first.indexOf('"top"'));
  assert.equal(canonicalize({ s: "\ud800" }), '{\n  "s": "\\ud800"\n}\n');
});

// 2. Record then replay
test("record then replay: scratch recording is byte-identical to the committed snapshots", async () => {
  const out = join(scratch("l10a"), "snaps");
  const r = cli(SAMPLE_CASES, out);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  const committed = readdirSync(SAMPLE_SNAPS).sort();
  assert.deepEqual(readdirSync(out).sort(), committed);
  assert.ok(committed.length >= 12);
  for (const f of committed) assert.equal(readFileSync(join(out, f), "utf8"), readFileSync(join(SAMPLE_SNAPS, f), "utf8"), f);
  const { outcomes } = await runParity({ adapter: mock, casesDir: SAMPLE_CASES, snapshotsDir: out, exclusions: NO_EXCLUSIONS });
  assert.deepEqual(outcomes.filter((o) => o.status !== "passed"), []);
  const check = cli(SAMPLE_CASES, SAMPLE_SNAPS, "--check");
  assert.equal(check.status, 0, check.stdout + check.stderr);
});

// 3. Sensitivity
test("sensitivity: eleven mutations, eleven failures with the right path", async () => {
  const snaps = join(scratch("l10a"), "snaps");
  cpSync(SAMPLE_SNAPS, snaps, { recursive: true });
  const mutations = [
    ["plain-text", "steps[0].stdout", (s) => { s.steps[0].stdout = s.steps[0].stdout.replace("hello", "hellp"); }],
    ["stderr", "steps[0].stderr", (s) => { s.steps[0].stderr = s.steps[0].stderr.replace("oops", "oopS"); }],
    ["nonzero-exit", "steps[0].exitCode", (s) => { s.steps[0].exitCode = 4; }],
    ["env-change", "steps[0].env.A", (s) => { s.steps[0].env.A = "9"; }],
    ["file-lifecycle", 'fs["/a/new.txt"].content', (s) => {
      const b = Buffer.from(s.fs["/a/new.txt"].content.base64, "base64");
      b[0] ^= 1;
      s.fs["/a/new.txt"].content.base64 = b.toString("base64");
    }],
    ["file-lifecycle", 'fs["/zzz"]', (s) => { s.fs["/zzz"] = { kind: "directory", mode: 493 }; }],
    ["file-lifecycle", 'fs["/a/new.txt"]', (s) => { delete s.fs["/a/new.txt"]; }],
    ["coverage-hits", "coverage[1]", (s) => { s.coverage[1] = "bash:builtin:printf"; }],
    ["coverage-hits", "coverage[0]", (s) => { [s.coverage[0], s.coverage[1]] = [s.coverage[1], s.coverage[0]]; }],
    ["trace-time-keys", "trace[1].name", (s) => { s.trace[1].name = "other"; }],
    ["trace-time-keys", "trace[0].details.readdirCalls", (s) => { s.trace[0].details.readdirCalls += 1; }],
  ];
  assert.equal(mutations.length, 11);
  for (const [name, path, mutate] of mutations) {
    const file = join(snaps, `${name}.snap.json`);
    const original = readFileSync(file, "utf8");
    const snap = JSON.parse(original);
    mutate(snap);
    writeFileSync(file, canonicalize(snap));
    const { outcomes } = await runParity({ adapter: mock, casesDir: SAMPLE_CASES, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS });
    const o = outcomes.find((x) => x.id === `l10a/${name}`);
    assert.equal(o.status, "failed", `${name} ${path}`);
    assert.ok(o.message.startsWith(`l10a/${name}: ${path}`), o.message);
    assert.ok(o.paths.includes(path), `${path} not in ${o.paths}`);
    assert.equal(outcomes.filter((x) => x.status === "failed").length, 1);
    writeFileSync(file, original);
  }
});

test("difference report: code unit index and a window with escapes", async () => {
  const snaps = join(scratch("l10a"), "snaps");
  cpSync(SAMPLE_SNAPS, snaps, { recursive: true });
  const file = join(snaps, "all-bytes.snap.json");
  const snap = JSON.parse(readFileSync(file, "utf8"));
  snap.steps[0].stdout = snap.steps[0].stdout.replace("\u0080", "\u0081");
  writeFileSync(file, canonicalize(snap));
  const { outcomes } = await runParity({ adapter: mock, casesDir: SAMPLE_CASES, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS });
  const o = outcomes.find((x) => x.id === "l10a/all-bytes");
  assert.match(o.message, /first difference at code unit 128;/);
  assert.match(o.message, /\\x7f\\x80\\x81/);
  assert.match(o.message, /expected \.\.\./);
});

// 4. Tolerance
test("tolerance: durations and *TimeMs values never reach a snapshot nor fail a replay", async () => {
  const text = readFileSync(join(SAMPLE_SNAPS, "trace-time-keys.snap.json"), "utf8");
  assert.ok(!text.includes("durationMs") && !text.includes("TimeMs"));
  for (const clock of [() => 0, () => 123456.789]) {
    const { outcomes } = await runParity({
      adapter: createMockAdapter({ clock }), casesDir: SAMPLE_CASES, snapshotsDir: SAMPLE_SNAPS, exclusions: NO_EXCLUSIONS,
    });
    assert.deepEqual(outcomes.filter((o) => o.status !== "passed"), []);
  }
});

// 5. Stale and missing
test("stale, missing and orphan snapshots", async () => {
  const { cases, snaps } = sampleCopy();
  assert.equal(cli(cases, snaps).status, 0);
  assert.equal(cli(cases, snaps, "--check").status, 0);
  // stale
  const p = join(cases, "plain-text.case.json");
  const c = JSON.parse(readFileSync(p, "utf8"));
  c.title = "edited without re-recording";
  writeFileSync(p, canonicalize(c));
  let { outcomes } = await runParity({ adapter: mock, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS });
  const stale = outcomes.find((o) => o.id === "lx/plain-text");
  assert.equal(stale.status, "failed");
  assert.match(stale.message, /stale snapshot/);
  const chk = cli(cases, snaps, "--check");
  assert.equal(chk.status, 1);
  assert.match(chk.stdout, /stale lx\/plain-text/);
  // missing
  rmSync(join(snaps, "stderr.snap.json"));
  ({ outcomes } = await runParity({ adapter: mock, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS }));
  assert.match(outcomes.find((o) => o.id === "lx/stderr").message, /missing snapshot/);
  assert.match(cli(cases, snaps, "--check").stdout, /missing lx\/stderr/);
  // orphan
  rmSync(join(cases, "args.case.json"));
  const orphan = cli(cases, snaps, "--check");
  assert.equal(orphan.status, 1);
  assert.match(orphan.stdout, /orphan lx\/args/);
});

// 6. Instability
test("instability: a case with random is unstable and writes nothing", () => {
  const base = scratch("l10a");
  const cases = join(base, "lu");
  writeCase(cases, "rnd", { steps: [{ script: "random" }] });
  const out = join(base, "snaps");
  const r = cli(cases, out);
  assert.equal(r.status, 1);
  assert.match(r.stdout, /^unstable lu\/rnd$/m);
  assert.ok(!existsSync(join(out, "rnd.snap.json")));
});

// 7. Engine defect
test("engine defect: recorded without an absolute path, excluded as upstream-defect on replay", async () => {
  const base = scratch("l10a");
  const cases = join(base, "ld");
  writeCase(cases, "boom", { steps: [{ script: "echo fine" }, { script: "echo x\ncrash" }] });
  const out = join(base, "snaps");
  const r = cli(cases, out);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /^engine-defect ld\/boom$/m);
  const snap = JSON.parse(readFileSync(join(out, "boom.snap.json"), "utf8"));
  assert.equal(snap.status, "engine-defect");
  assert.equal(snap.defect.step, 1);
  assert.ok(snap.defect.message.includes("<path>"));
  assert.ok(!snap.defect.message.includes(process.cwd()));
  assert.ok(!/(^|[\s'"(])\/[\w.-]+/.test(snap.defect.message), snap.defect.message);
  const { outcomes } = await runParity({ adapter: mock, casesDir: cases, snapshotsDir: out, exclusions: NO_EXCLUSIONS });
  assert.equal(outcomes[0].status, "excluded");
  assert.equal(outcomes[0].reason, "upstream-defect");
});

// 8 and 9. Exclusions, unsupported, report
async function specialLot() {
  const s = sampleCopy("lr");
  writeCase(s.cases, "boom", { steps: [{ script: "crash" }] });
  writeCase(s.cases, "unsupported", { engine: { unsupported: true }, steps: [{ script: "echo never" }] });
  const rec = await recordAll({ adapter: mock, casesDir: s.cases, snapshotsDir: s.snaps });
  assert.deepEqual(rec.unsupported, ["lr/unsupported"]);
  assert.equal(rec.failed, false);
  return s;
}

test("exclusions and unsupported cases are skipped, never failed", async () => {
  const s = await specialLot();
  const { outcomes } = await runParity({
    adapter: mock, casesDir: s.cases, snapshotsDir: s.snaps,
    exclusions: { divergences: { V2: "intended difference" }, cases: {} },
  });
  const by = Object.fromEntries(outcomes.map((o) => [o.id, o]));
  assert.deepEqual([by["lr/divergence-tagged"].status, by["lr/divergence-tagged"].reason], ["excluded", "intended difference"]);
  assert.equal(by["lr/unsupported"].status, "not-covered");
  assert.equal(outcomes.filter((o) => o.status === "failed").length, 0);
  const byId = await runParity({ adapter: mock, casesDir: s.cases, snapshotsDir: s.snaps, exclusions: { cases: { "lr/plain-text": "listed" } } });
  assert.equal(byId.outcomes.find((o) => o.id === "lr/plain-text").reason, "listed");
});

test("report: expected counts and byte-identical runs", async () => {
  const s = await specialLot();
  const exclusions = { divergences: { V2: "intended difference" }, cases: {} };
  const files = [];
  for (let i = 0; i < 2; i++) {
    const { report } = await runParity({ adapter: mock, casesDir: s.cases, snapshotsDir: s.snaps, exclusions });
    const { writeReport } = await import("../../harness/parity/replay.mjs");
    const f = join(s.base, `report-${i}.json`);
    writeReport(report, f);
    files.push(f);
    assert.equal(report.cases, 19);
    assert.equal(report.passed, 16);
    assert.equal(report.failed, 0);
    assert.deepEqual(report.excluded, { "intended difference": 1, "upstream-defect": 1 });
    assert.equal(report.notCovered, 1);
    assert.deepEqual(report.failedCases, []);
    assert.equal(report.byLot.lr.cases, 19);
    assert.equal(report.byTag.files.cases, 4);
    assert.equal(report.byTag.divergence.excluded["intended difference"], 1);
  }
  const a = readFileSync(files[0], "utf8");
  assert.equal(a, readFileSync(files[1], "utf8"));
  assert.ok(!/time|date|stamp/i.test(a.replace(/"[^"]*(TimeMs)[^"]*"/g, "")));
});

// 10. Bytes
test("bytes: all 256 values round-trip and the snapshot is valid UTF-8", async () => {
  const file = join(SAMPLE_SNAPS, "all-bytes.snap.json");
  new TextDecoder("utf-8", { fatal: true }).decode(readFileSync(file));
  const snap = JSON.parse(readFileSync(file, "utf8"));
  assert.equal(snap.steps[0].stdoutKind, "bytes");
  const { cases } = loadCases(SAMPLE_CASES);
  const entry = cases.find((c) => c.id === "l10a/all-bytes");
  const o = await evaluateCase({ adapter: mock, entry, snapshotsDir: SAMPLE_SNAPS, exclusions: NO_EXCLUSIONS });
  assert.equal(o.status, "passed");
  const out = snap.steps[0].stdout;
  assert.equal(out.length, 256);
  for (let i = 0; i < 256; i++) assert.equal(out.charCodeAt(i), i);
});

// 11. Validation
test("validation: both tools reject invalid case files naming the file and the key", async () => {
  const base = scratch("l10a");
  const cases = join(base, "lv");
  mkdirSync(cases);
  const put = (file, obj) => writeFileSync(join(cases, file), JSON.stringify(obj));
  put("unknown-key.case.json", { id: "lv/unknown-key", title: "t", steps: [{ script: "echo" }], bogus: 1 });
  put("no-steps.case.json", { id: "lv/no-steps", title: "t" });
  put("wrong-id.case.json", { id: "lv/other", title: "t", steps: [{ script: "echo" }] });
  put("Bad_Name.case.json", { id: "lv/Bad_Name", title: "t", steps: [{ script: "echo" }] });
  const expected = [
    ["unknown-key.case.json", '"bogus"'],
    ["no-steps.case.json", '"steps"'],
    ["wrong-id.case.json", '"id"'],
    ["Bad_Name.case.json", '"id"'],
  ];
  const r = cli(cases, join(base, "snaps"));
  assert.equal(r.status, 1);
  const { invalid, report } = await runParity({ adapter: mock, casesDir: cases, snapshotsDir: join(base, "snaps"), exclusions: NO_EXCLUSIONS });
  assert.equal(invalid.length, 4);
  assert.equal(report.cases, 0);
  for (const [file, key] of expected) {
    assert.ok(r.stdout.split("\n").some((l) => l.includes(file) && l.includes(key)), `${file} ${key}: ${r.stdout}`);
    assert.ok(invalid.some((i) => i.message.includes(file) && i.message.includes(key)), file);
  }
});

test("defineParityTests: stale case fails with its message, invalid file fails, report is written", () => {
  const s = sampleCopy("lw");
  assert.equal(cli(s.cases, s.snaps).status, 0);
  const p = join(s.cases, "plain-text.case.json");
  const c = JSON.parse(readFileSync(p, "utf8"));
  c.title = "edited";
  writeFileSync(p, canonicalize(c));
  writeFileSync(join(s.cases, "broken.case.json"), JSON.stringify({ id: "lw/broken", title: "t" }));
  const reportPath = join(s.base, "report.json");
  const spec = join(s.base, "scratch.test.mjs");
  const href = (f) => pathToFileURL(join(repoRoot, f)).href;
  writeFileSync(spec, `import { defineParityTests } from ${JSON.stringify(href("harness/parity/replay.mjs"))};
defineParityTests({ adapter: ${JSON.stringify(ADAPTER)}, casesDir: ${JSON.stringify(s.cases)}, snapshotsDir: ${JSON.stringify(s.snaps)}, reportPath: ${JSON.stringify(reportPath)} });\n`);
  const r = node(["--test", spec]);
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /stale snapshot for lw\/plain-text/);
  assert.match(r.stdout, /broken\.case\.json/);
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  assert.deepEqual(report.failedCases.sort(), [join(s.cases, "broken.case.json"), "lw/plain-text"].sort());
  assert.equal(report.passed, 16);
});

// 12. Isolation
test("isolation: relative or node: imports only, no network module, no fetch(", () => {
  const files = readdirSync(join(repoRoot, "harness/parity")).map((f) => join("harness/parity", f)).concat("tools/parity-record.mjs");
  const banned = new Set(["node:http", "node:https", "node:net", "node:dns", "node:tls", "node:dgram"]);
  const re = /(?:\bimport\s*(?:[^'"()]*?\bfrom\s*)?|\bexport\s[^'"]*?\bfrom\s*|\bimport\s*\(\s*)(["'`])([^"'`]+)\1/g;
  let seen = 0;
  for (const f of files) {
    const text = readFileSync(join(repoRoot, f), "utf8");
    assert.ok(!text.includes("fetch("), `${f} contains fetch(`);
    for (const m of text.matchAll(re)) {
      seen++;
      const spec = m[2];
      assert.ok(spec.startsWith("./") || spec.startsWith("../") || spec.startsWith("node:"), `${f}: ${spec}`);
      assert.ok(!banned.has(spec), `${f}: ${spec}`);
    }
  }
  assert.ok(seen > 15);
});

// 13. Cost
test("cost: replaying the committed sample takes less than 2 seconds", async () => {
  const t0 = performance.now();
  const { outcomes } = await runParity({ adapter: mock, casesDir: SAMPLE_CASES, snapshotsDir: SAMPLE_SNAPS, exclusions: NO_EXCLUSIONS });
  const ms = performance.now() - t0;
  assert.equal(outcomes.filter((o) => o.status === "passed").length, outcomes.length);
  assert.ok(ms < 2000, `${ms} ms`);
});

// 14. API skeleton
test("api skeleton: nine todo tests", () => {
  const r = node(["--test", "--test-reporter=tap", "test/api/tier-a.test.mjs", "test/api/tier-b.test.mjs"]);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /^# todo 9$/m);
  assert.match(r.stdout, /^# fail 0$/m);
});
