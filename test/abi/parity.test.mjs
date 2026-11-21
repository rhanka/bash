import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { runParity, summarize, writeReport } from "../../harness/parity/replay.mjs";
import adapter from "../../harness/parity/sentropic-adapter.mjs";
import { INTERNAL } from "../../host/core/instance.js";
import { repoRoot } from "../helpers.mjs";
import { adapterPair } from "./support.mjs";

test("the default binding and guarded oracle each match all 30 committed L2 snapshots", async (t) => {
  const pair = await adapterPair("kernel");
  const oracle = {
    ...adapter,
    create(init) {
      return adapter.create({ ...init, engine: { ...init.engine, [INTERNAL]: { kernel: pair.jcoHooks } } });
    },
  };
  const cases = join(repoRoot, "test/cases/l2");
  for (const [name, engine] of [["first-party", adapter], ["jco", oracle]]) {
    const { report, outcomes } = await runParity({ adapter: engine, casesDir: cases, snapshotsDir: cases,
      exclusions: join(repoRoot, "test/parity/exclusions.json") });
    t.diagnostic(name + " " + summarize(report));
    writeReport(report, join(repoRoot, "tmp/parity-report-l2-" + name + ".json"));
    assert.deepEqual(outcomes.filter(({ status }) => status !== "passed"), []);
    assert.equal(report.cases, 30);
    assert.equal(report.passed, 30);
  }
});

