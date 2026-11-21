import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { checkCanonicalLifts, checkLoaderCore, checkWit } from "../../tools/abi-check.mjs";
import { descriptorSource, normalizeWit, readWit } from "../../tools/abi-descriptors.mjs";
import { coreName, postName } from "./support.mjs";
import { repoRoot, run, toolBin } from "../../tools/lib.mjs";

const componentPath = join(repoRoot, "dist/kernel.component.wasm");
const corePath = join(repoRoot, "dist/js/kernel/kernel.core.wasm");
const skeleton = (path) => run(toolBin("wasm-tools"), ["print", "--skeleton", path]);

test("WIT normalization refuses a reordered record field or variant arm", () => {
  const current = readWit(join(repoRoot, "wit"));
  const component = readWit(componentPath);
  checkWit(current, component);
  assert.equal(descriptorSource(normalizeWit(current)), readFileSync(join(repoRoot, "host/core/abi/descriptors.js"), "utf8"));
  for (const [name, kind, key] of [["env-var", "record", "fields"], ["fs-op", "variant", "cases"]]) {
    const fixture = structuredClone(current);
    const id = fixture.interfaces[0].types[name];
    const fields = fixture.types[id].kind[kind][key];
    [fields[0], fields[1]] = [fields[1], fields[0]];
    assert.throws(() => checkWit(fixture, component), /ABI WIT\.types\..*differs/);
  }
});

test("the core check refuses a changed export signature, and the component check refuses a different post-return", () => {
  const core = skeleton(corePath);
  checkLoaderCore(core);
  const line = core.split("\n").find((line) => line.includes('(export "' + coreName("configure") + '"'));
  const wrong = core.split("\n").find((line) => line.includes('(export "' + coreName("usage") + '"'));
  const signatureDrift = core.replace(line, wrong.replace(coreName("usage"), coreName("configure")));
  assert.throws(() => checkLoaderCore(signatureDrift), /ABI core export.*#configure\.params\.0 differs/);

  const component = skeleton(componentPath);
  checkCanonicalLifts(component);
  const lift = component.split("\n").find((line) => line.includes("(canon lift") && line.includes(coreName("raise-ceiling")));
  const wrongPost = component.replace(lift, lift.replace(postName("raise-ceiling"), postName("configure")));
  assert.throws(() => checkCanonicalLifts(wrongPost), /#raise-ceiling\.post-return differs/);
});

