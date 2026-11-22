import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { defineParityTests } from "../../harness/parity/replay.mjs";
import adapter from "../../harness/parity/sentropic-adapter.mjs";

const path = (relative) => fileURLToPath(new URL(relative, import.meta.url));
const reportPath = path("../../tmp/parity-report-l1d.json");

defineParityTests({
  adapter,
  casesDir: path("../cases/l1d"),
  snapshotsDir: path("../cases/l1d"),
  exclusions: path("./exclusions.json"),
  reportPath,
});

test("l1d parity has zero failures, zero exclusions, and zero notCovered", () => {
  const report = JSON.parse(readFileSync(reportPath, "utf8"));
  assert.equal(report.cases, 16);
  assert.equal(report.passed, 16);
  assert.equal(report.failed, 0);
  assert.equal(report.notCovered, 0);
  assert.deepEqual(report.excluded, {});
});
