// Self-tests of the parity harness (lot L10a, spec section 8), run on the mock adapter.
import { test } from "node:test";
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { repoRoot, scratch } from "../helpers.mjs";
import { assertUnderTime } from "../timing.mjs";
import { canonicalize } from "../../harness/parity/canonical.mjs";
import { recordAll } from "../../harness/parity/record.mjs";
import { evaluateCase, runParity } from "../../harness/parity/replay.mjs";
import { loadCases, validateCase, validateSnapshot } from "../../harness/parity/validate.mjs";
import { sanitizePaths } from "../../harness/parity/run.mjs";
import { buildReport } from "../../harness/parity/replay.mjs";
import { compare } from "../../harness/parity/compare.mjs";
import { recordCase } from "../../harness/parity/record.mjs";
import { hashValue } from "../../harness/parity/canonical.mjs";
import { scanImports, stripComments } from "../../tools/gates.mjs";
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
  assert.ok(!text.includes("durationMs"));
  // the *TimeMs keys stay, their values are masked
  assert.match(text, /"readdirTimeMs": null/);
  assert.match(text, /"otherTimeMs": null/);
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
  assert.equal(report.cases, 4);
  assert.equal(report.failed, 4);
  assert.deepEqual(report.failedCases, ["lv/Bad_Name", "lv/no-steps", "lv/unknown-key", "lv/wrong-id"]);
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
  assert.deepEqual(report.failedCases.sort(), ["lw/broken", "lw/plain-text"].sort());
  assert.equal(report.passed, 16);
});

// 12. Isolation
const BANNED_MODULE = /^node:(?:http2?|https|net|dns|tls|dgram)(?:\/|$)/;
const BANNED_CONSTRUCT = [
  [/\bfetch\s*\(/, "fetch("],
  [/\bgetBuiltinModule\b/, "getBuiltinModule"],
  [/\bcreateRequire\b/, "createRequire"],
  [/\beval\b/, "eval"],
  [/\bFunction\b/, "Function"],
  [/\brequire\s*\(/, "require("],
];
// The two adapter loaders: a computed import of the adapter module path given on the command line.
const ADAPTER_LOADER = /\bimport\(pathToFileURL\(resolve\((?:adapter|opts\.adapter)\)\)\.href\)/g;

/** Why a source file breaks the isolation rule, as a list of messages. */
function isolationViolations(text, { allowAdapterLoader = false } = {}) {
  const raw = allowAdapterLoader ? text.replace(ADAPTER_LOADER, "null") : text;
  const { specifiers, nonLiteralDynamic, src } = scanImports(raw);
  const out = [];
  if (nonLiteralDynamic) out.push("non-literal import()");
  for (const spec of specifiers) {
    if (!(spec.startsWith("./") || spec.startsWith("../") || spec.startsWith("node:"))) out.push(`import of ${spec}`);
    if (BANNED_MODULE.test(spec)) out.push(`import of ${spec}`);
  }
  for (const [re, what] of BANNED_CONSTRUCT) if (re.test(stripComments(src))) out.push(what);
  return out;
}

function walkFiles(dir) {
  const out = [];
  for (const f of readdirSync(dir).sort()) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) out.push(...walkFiles(p));
    else if (p.endsWith(".mjs")) out.push(p);
  }
  return out;
}

test("isolation: harness/parity and the CLI import only relative or allowed node: modules, no network", () => {
  const files = [...walkFiles(join(repoRoot, "harness/parity")), join(repoRoot, "tools/parity-record.mjs")];
  assert.ok(files.length >= 8);
  let specifiers = 0;
  for (const f of files) {
    const text = readFileSync(f, "utf8");
    assert.deepEqual(isolationViolations(text, { allowAdapterLoader: true }), [], f);
    specifiers += scanImports(text).specifiers.length;
  }
  assert.ok(specifiers > 15);
  // the loader whitelist is not a hole: the same loader written with another expression is refused
  assert.deepEqual(isolationViolations("await import(pathToFileURL(resolve(other)).href);", { allowAdapterLoader: true }), ["non-literal import()"]);
});

test("isolation scan: the bypasses of the former regex are caught", () => {
  const cases = [
    ['const m = await import("node:" + "http");', "non-literal import()"],
    ['export*from"node:http";', "import of node:http"],
    ['import x from "node:http2";', "import of node:http2"],
    ['import("node:dns/promises");', "import of node:dns/promises"],
    ['import x from "lodash";', "import of lodash"],
    ["fetch (url);", "fetch("],
    ["process.getBuiltinModule('node:http');", "getBuiltinModule"],
    ['import { createRequire } from "node:module";', "createRequire"],
    ["eval('1');", "eval"],
    ["new Function('return 1')();", "Function"],
  ];
  for (const [src, expected] of cases) assert.ok(isolationViolations(src).includes(expected), `${src} -> ${isolationViolations(src)}`);
  // a comment is not code
  assert.deepEqual(isolationViolations('// import "node:http"\n/* fetch( */ import { x } from "./x.mjs";'), []);
});

// 13. Cost
test("cost: replaying the committed sample stays within its time limit", async () => {
  const t0 = performance.now();
  const { outcomes } = await runParity({ adapter: mock, casesDir: SAMPLE_CASES, snapshotsDir: SAMPLE_SNAPS, exclusions: NO_EXCLUSIONS });
  const ms = performance.now() - t0;
  assert.equal(outcomes.filter((o) => o.status === "passed").length, outcomes.length);
  assertUnderTime("L10a sample replay", ms, 2000);
});

// 14. API skeleton
test("api skeleton: nine todo tests", () => {
  const r = node(["--test", "--test-reporter=tap", "test/api/tier-a.test.mjs", "test/api/tier-b.test.mjs"]);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /^# todo 9$/m);
  assert.match(r.stdout, /^# fail 0$/m);
});

// Review fixes (PR #5)

/** A mock adapter whose sessions are rewritten by `wrap(session, init)`. */
function wrapped(wrap) {
  return {
    name: "mock",
    version: "1",
    async create(init) {
      return wrap(await mock.create(init), init);
    },
  };
}

test("check: an unsupported case is not-covered, never missing or stale (and a leftover snapshot is removed)", async () => {
  const s = await specialLot();
  const chk = cli(s.cases, s.snaps, "--check");
  assert.equal(chk.status, 0, chk.stdout + chk.stderr);
  assert.match(chk.stdout, /^not-covered lr\/unsupported$/m);
  assert.doesNotMatch(chk.stdout, /missing|stale/);
  // a stale snapshot of the unsupported case does not fail --check either
  writeFileSync(join(s.snaps, "unsupported.snap.json"), canonicalize({ case: "lr/unsupported", caseHash: "0", engine: { name: "x", version: "1" }, status: "recorded", steps: [] }));
  const stale = cli(s.cases, s.snaps, "--check");
  assert.equal(stale.status, 0, stale.stdout);
  assert.match(stale.stdout, /^not-covered lr\/unsupported$/m);
  // recording removes it
  assert.equal(cli(s.cases, s.snaps).status, 0);
  assert.ok(!existsSync(join(s.snaps, "unsupported.snap.json")));
});

test("trace: a missing *TimeMs key fails the replay; a non-finite one is an engine defect", async () => {
  const base = scratch("l10a");
  const cases = join(base, "lt");
  writeCase(cases, "tr", { capture: { trace: true }, steps: [{ script: "trace find summary readdirCalls=2 readdirTimeMs=0" }] });
  const snaps = join(base, "snaps");
  assert.equal((await recordAll({ adapter: mock, casesDir: cases, snapshotsDir: snaps })).failed, false);
  const dropKey = wrapped((s) => ({ ...s, takeTrace: () => s.takeTrace().map((e) => { delete e.details.readdirTimeMs; return e; }) }));
  const o = (await runParity({ adapter: dropKey, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS })).outcomes[0];
  assert.equal(o.status, "failed");
  assert.match(o.message, /readdirTimeMs: missing in actual/);
  const nan = wrapped((s) => ({ ...s, takeTrace: () => s.takeTrace().map((e) => { e.details.readdirTimeMs = "slow"; return e; }) }));
  const d = (await runParity({ adapter: nan, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS })).outcomes[0];
  assert.equal(d.status, "failed");
  assert.match(d.message, /readdirTimeMs.*finite number/);
});

test("undefined fields and bad step results: record and replay agree, malformed results are defects", async () => {
  const base = scratch("l10a");
  const cases = join(base, "lt");
  writeCase(cases, "tr", { capture: { trace: true }, steps: [{ script: "trace find summary readdirCalls=2" }] });
  const snaps = join(base, "snaps");
  const withUndefined = wrapped((s) => ({ ...s, takeTrace: () => s.takeTrace().map((e) => { e.details.opt = undefined; e.details.nan = NaN; return e; }) }));
  assert.equal((await recordAll({ adapter: withUndefined, casesDir: cases, snapshotsDir: snaps })).failed, false);
  assert.equal(
    (await recordAll({ adapter: withUndefined, casesDir: cases, snapshotsDir: snaps, check: true })).failed, false);
  const o = (await runParity({ adapter: withUndefined, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS })).outcomes[0];
  assert.equal(o.status, "passed", o.message);
  const bad = wrapped((s) => ({ ...s, exec: async (st) => ({ ...(await s.exec(st)), stdout: 42 }) }));
  const r = await recordCase(bad, JSON.parse(readFileSync(join(cases, "tr.case.json"), "utf8")));
  assert.equal(r.status, "engine-defect");
  assert.match(r.snapshot.defect.message, /stdout must be a string/);
});

test("prototype names: constructor, toString and __proto__ are ordinary names", async () => {
  // report: tags, divergence reasons and lots named after Object.prototype members
  const outcomes = ["constructor", "__proto__", "toString"].map((t, i) => ({
    id: `${t}/c${i}`, lot: t, tags: [t], status: i === 0 ? "excluded" : "passed", reason: "__proto__",
  }));
  const report = buildReport(outcomes);
  assert.equal(report.cases, 3);
  assert.equal(report.byTag.constructor.cases, 1);
  assert.equal(Object.getOwnPropertyDescriptor(report.byTag, "__proto__").value.cases, 1);
  assert.equal(Object.getOwnPropertyDescriptor(report.excluded, "__proto__").value, 1);
  assert.equal(typeof Object.cases, "undefined");
  // compare: a missing toString / valueOf key is missing, not present
  assert.deepEqual(compare({ env: { valueOf: "x" } }, { env: {} }).map((d) => d.path), ["env.valueOf"]);
  assert.deepEqual(compare({ env: {} }, { env: { toString: "x" } }).map((d) => d.path), ["env.toString"]);
  // exclusions: a case whose divergence is "toString" or "constructor" is not silently skipped
  const base = scratch("l10a");
  const cases = join(base, "lp");
  writeCase(cases, "a", { tags: ["constructor"], divergence: "toString", steps: [{ script: "echo hi" }] });
  writeCase(cases, "constructor", { steps: [{ script: "echo hi" }] });
  const snaps = join(base, "snaps");
  await recordAll({ adapter: mock, casesDir: cases, snapshotsDir: snaps });
  const res = await runParity({ adapter: mock, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS });
  assert.deepEqual(res.outcomes.map((o) => o.status), ["passed", "passed"]);
  assert.equal(res.report.byTag.constructor.passed, 1);
  // env: a __proto__ key survives the run and the snapshot
  writeFileSync(join(cases, "env.case.json"), `{"id":"lp/env","title":"t","steps":[{"script":"echo hi","env":{"__proto__":"x"}}]}`);
  const rec = await recordAll({ adapter: mock, casesDir: cases, snapshotsDir: snaps });
  assert.equal(rec.failed, false);
  const snap = JSON.parse(readFileSync(join(snaps, "env.snap.json"), "utf8"));
  assert.equal(Object.getOwnPropertyDescriptor(snap.steps[0].env, "__proto__").value, "x");
  const again = await runParity({ adapter: mock, casesDir: cases, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS });
  assert.equal(again.outcomes.find((o) => o.id === "lp/env").status, "passed");
});

test("multi-lot layout: <lot>/<name> cases, per-lot snapshots, invalid files folded into the report", async () => {
  const root = join(scratch("l10a"), "cases");
  writeCase(join(root, "la"), "one", { steps: [{ script: "echo a" }] });
  writeCase(join(root, "lb"), "two", { steps: [{ script: "echo b" }] });
  writeFileSync(join(root, "lb", "broken.case.json"), JSON.stringify({ id: "lb/broken", title: "t" }));
  const snaps = join(root, "..", "snaps");
  const chk = cli(root, snaps);
  assert.equal(chk.status, 1);
  assert.ok(existsSync(join(snaps, "la", "one.snap.json")) && existsSync(join(snaps, "lb", "two.snap.json")));
  assert.equal(cli(root, snaps, "--check").status, 1); // still the invalid file
  rmSync(join(root, "lb", "broken.case.json"));
  assert.equal(cli(root, snaps, "--check").status, 0);
  rmSync(join(snaps, "lb", "two.snap.json"));
  assert.match(cli(root, snaps, "--check").stdout, /missing lb\/two/);
  writeFileSync(join(snaps, "la", "gone.snap.json"), "{}");
  assert.match(cli(root, snaps, "--check").stdout, /orphan la\/gone/);
  assert.equal(cli(root, snaps).status, 0);
  writeFileSync(join(root, "lb", "broken.case.json"), JSON.stringify({ id: "lb/broken", title: "t" }));
  const { report } = await runParity({ adapter: mock, casesDir: root, snapshotsDir: snaps, exclusions: NO_EXCLUSIONS });
  assert.deepEqual([report.cases, report.passed, report.failed], [3, 2, 1]);
  assert.deepEqual(report.failedCases, ["lb/broken"]);
  assert.deepEqual([report.byLot.la.cases, report.byLot.lb.cases, report.byLot.lb.failed], [1, 2, 1]);
  assert.ok(readFileSync(join(repoRoot, "test/parity/l10a.test.mjs"), "utf8").includes("tmp/parity-report-l10a.json"));
});

test("recordCase: an adapter that mutates its input cannot corrupt the case hash", async () => {
  const caseObject = { id: "lm/m", title: "t", files: { "/a": "x" }, steps: [{ script: "echo hi" }] };
  const before = hashValue(caseObject);
  const mutating = wrapped((s, init) => ({
    ...s,
    exec: async (step) => {
      const r = await s.exec(step);
      step.script = "echo mutated";
      init.files["/b"] = "y";
      return r;
    },
  }));
  const r = await recordCase(mutating, caseObject);
  assert.equal(r.status, "recorded");
  assert.equal(r.snapshot.caseHash, before);
  assert.equal(hashValue(caseObject), before);
});

test("paths and validation details: forward-slash and parenthesized paths, base64 groups, engine and defect", () => {
  assert.equal(sanitizePaths("open C:/Users/x/f.txt failed"), "open <path> failed");
  assert.equal(sanitizePaths("open C:\\Users\\x\\f.txt failed"), "open <path> failed");
  assert.equal(sanitizePaths("in /a/b(1)/c,d/e"), "in <path>");
  const file = "x.case.json";
  const step = (script) => ({ id: "lq/x", title: "t", steps: [{ script }] });
  assert.ok(validateCase(step({ base64: "QQ==" }), file, "lq/x"));
  assert.ok(validateCase(step({ base64: "" }), file, "lq/x"));
  for (const bad of ["A", "QQ=", "QQ", "QUJD=", "QUJDRA"]) {
    assert.throws(() => validateCase(step({ base64: bad }), file, "lq/x"), /script/, bad);
  }
  const snap = { case: "c", caseHash: "h", engine: { name: "e", version: "1" }, status: "recorded", steps: [] };
  assert.ok(validateSnapshot(snap, "s"));
  assert.throws(() => validateSnapshot({ ...snap, engine: { name: "e" } }, "s"), /engine/);
  assert.throws(() => validateSnapshot({ ...snap, engine: "e" }, "s"), /engine/);
  const defect = { ...snap, status: "engine-defect", defect: { step: 0, message: "m" } };
  assert.ok(validateSnapshot(defect, "s"));
  assert.throws(() => validateSnapshot({ ...defect, defect: { step: "0", message: "m" } }, "s"), /defect/);
  assert.throws(() => validateSnapshot({ ...defect, defect: { step: 0 } }, "s"), /defect/);
});
