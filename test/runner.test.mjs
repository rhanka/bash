import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import { scratch } from "./helpers.mjs";
import { testFiles } from "../tools/run-tests.mjs";

test("the runner finds tests in nested subdirectories", () => {
  const root = scratch("runner");
  mkdirSync(join(root, "test", "parity", "deep"), { recursive: true });
  for (const f of ["a.test.mjs", "parity/b.test.mjs", "parity/deep/c.test.mjs", "parity/helper.mjs"]) writeFileSync(join(root, "test", f), "");
  assert.deepEqual(
    testFiles(root).map((p) => relative(join(root, "test"), p)),
    ["a.test.mjs", "parity/b.test.mjs", "parity/deep/c.test.mjs"],
  );
});
