import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync, cpSync } from "node:fs";
import { join } from "node:path";
import { repoRoot, scratch } from "./helpers.mjs";
import { toolsDir, toolBin, run, readJson } from "../tools/lib.mjs";
import * as gates from "../tools/gates.mjs";

const fx = (name) => join(repoRoot, "test", "fixtures", name);
const real = () => ({ root: repoRoot, out: join(repoRoot, "dist"), toolsDir: toolsDir(), quick: true });

test("the real tree passes G1, G2, G4, G5, G6, G7, G8", () => {
  for (const [id, , fn] of gates.GATES) {
    if (id === "G3") continue;
    assert.equal(fn(real()), null, id);
  }
});

test("G1 fails on a component with an import", () => {
  const out = scratch("g1");
  run(toolBin("wasm-tools"), ["parse", join(fx("g1-import"), "import.wat"), "-o", join(out, "bad.component.wasm")]);
  assert.match(gates.g1ZeroImport({ ...real(), out }), /has an import/);
});

test("G2 fails on a bare specifier", () => {
  assert.match(gates.g2NoBareImport({ ...real(), root: fx("g2-bare"), out: fx("g2-bare") }), /bare specifier "lodash"/);
});

test("G2 fails when the browser entry reaches a node: import", () => {
  assert.match(gates.g2NoBareImport({ ...real(), root: fx("g2-node"), out: fx("g2-node") }), /reachable from host\/browser\.js/);
});

test("G2 fails on a dynamic import with a non-literal argument", () => {
  assert.match(gates.g2NoBareImport({ ...real(), root: fx("g2-dynamic"), out: fx("g2-dynamic") }), /non-literal/);
});

test("G6 fails on a foreign crate in Cargo.lock", () => {
  assert.match(gates.g6ZeroCrate({ ...real(), root: fx("g6-foreign-crate") }), /non-workspace package serde/);
});

test("G6 fails on alloc::vec outside kernel/src/mem", () => {
  assert.match(gates.g6ZeroCrate({ ...real(), root: fx("g6-alloc") }), /alloc heap type/);
});

test("G7 fails on a dependency and on an install script", () => {
  assert.match(gates.g7Manifests({ ...real(), root: fx("g7-dep"), manifestFile: "manifest.json", skipMake: true }), /dependencies/);
  assert.match(gates.g7Manifests({ ...real(), root: fx("g7-script"), manifestFile: "manifest.json", skipMake: true }), /postinstall/);
});

test("G8 fails on a wrong pinned hash", () => {
  const root = scratch("g8");
  mkdirSync(join(root, "tools"));
  const pins = readJson(join(repoRoot, "tools", "pins.json"));
  pins.tools[0].sha256 = "0".repeat(64);
  writeFileSync(join(root, "tools", "pins.json"), JSON.stringify(pins));
  // The scratch root has no tools of its own: point it at the real tools directory, whether or not
  // SENTROPIC_TOOLS_DIR is set, so the marker exists and only the pinned hash differs.
  const saved = process.env.SENTROPIC_TOOLS_DIR;
  process.env.SENTROPIC_TOOLS_DIR = toolsDir(repoRoot);
  try {
    assert.match(gates.g8ToolPins({ ...real(), root }), /marker hash differs/);
  } finally {
    if (saved === undefined) delete process.env.SENTROPIC_TOOLS_DIR;
    else process.env.SENTROPIC_TOOLS_DIR = saved;
  }
});
