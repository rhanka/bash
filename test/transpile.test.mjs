import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { scratch } from "./helpers.mjs";
import { writeFiles } from "../tools/transpile.mjs";

test("generated files are written inside the output directory", () => {
  const out = scratch("transpile");
  writeFiles(out, { "a.js": "x", "sub/b.js": "y" });
  assert.equal(readFileSync(join(out, "sub", "b.js"), "utf8"), "y");
});

test("a generated file name that escapes the output directory is refused", () => {
  const out = scratch("transpile-escape");
  assert.throws(() => writeFiles(out, { "../escape.js": "x" }), /escapes the output directory/);
  assert.throws(() => writeFiles(out, { "/abs-escape.js": "x" }), /escapes the output directory/);
  assert.equal(existsSync(join(out, "..", "escape.js")), false);
});
