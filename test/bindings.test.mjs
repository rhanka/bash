import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { scratch } from "./helpers.mjs";
import { bindingsProblems } from "../tools/bindings.mjs";

function setup(committed) {
  const root = scratch("bindings");
  const bin = join(root, "fake-bindgen");
  // A stand-in for wit-bindgen: writes gen.rs into the --out-dir argument.
  writeFileSync(bin, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = "--out-dir" ] && out="$2"; shift; done\nprintf "generated\\n" > "$out/gen.rs"\n');
  chmodSync(bin, 0o755);
  mkdirSync(join(root, "wit"));
  mkdirSync(join(root, "kernel", "src", "bindings"), { recursive: true });
  for (const [f, text] of Object.entries(committed)) writeFileSync(join(root, "kernel", "src", "bindings", f), text);
  return { root, bin, b: { witDir: "wit", world: "w", dir: "kernel/src/bindings", args: [] } };
}

test("bindings equal to the generated output pass", () => {
  const s = setup({ "gen.rs": "generated\n" });
  assert.deepEqual(bindingsProblems(s.root, "k", s.b, s.bin), []);
});

test("a modified, a missing and an extra hand-written bindings file are reported", () => {
  const s = setup({ "gen.rs": "edited\n", "extra.rs": "pub fn x() {}\n" });
  const p = bindingsProblems(s.root, "k", s.b, s.bin).join("\n");
  assert.match(p, /gen\.rs differs/);
  assert.match(p, /extra\.rs is committed but not generated/);
  assert.match(bindingsProblems(setup({}).root ? s.root : "", "k", { ...s.b, dir: "kernel/src/none" }, s.bin).join("\n"), /gen\.rs is not committed/);
});
