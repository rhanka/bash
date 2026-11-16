import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync, cpSync, appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot, scratch } from "./helpers.mjs";
import { toolsDir, toolBin, run, readJson, toolProblem, markerText, sha256File, lockProblem, pinsProblem } from "../tools/lib.mjs";
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
  run(toolBin("wasm-tools", toolsDir()), ["parse", join(fx("g1-import"), "import.wat"), "-o", join(out, "probe.component.wasm")]);
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
  assert.match(gates.g6ZeroCrate({ ...real(), root: fx("g6-alloc") }), /uses the alloc crate/);
});

test("G7 fails on a dependency and on an install script", () => {
  assert.match(gates.g7Manifests({ ...real(), root: fx("g7-dep"), manifestFile: "manifest.json", skipMake: true }), /dependencies/);
  assert.match(gates.g7Manifests({ ...real(), root: fx("g7-script"), manifestFile: "manifest.json", skipMake: true }), /postinstall/);
});


/** Writes files (path -> text) under a fresh scratch root and returns the root. */
function tree(prefix, files) {
  const root = scratch(prefix);
  mkdirSync(join(root, "tools"));
  cpSync(join(repoRoot, "tools", "pins.json"), join(root, "tools", "pins.json"));
  cpSync(join(repoRoot, "tools", "components.json"), join(root, "tools", "components.json"));
  for (const [p, text] of Object.entries(files)) {
    mkdirSync(join(root, p, ".."), { recursive: true });
    writeFileSync(join(root, p), text);
  }
  return root;
}
const g2 = (files) => {
  const root = tree("g2", files);
  return gates.g2NoBareImport({ ...real(), root, out: root });
};

test("G2 fails on a template-literal import()", () => {
  assert.match(g2({ "host/browser.js": "export const f = (x) => import(`./${x}.js`);\n" }), /non-literal/);
  assert.match(g2({ "host/browser.js": "export const f = () => import(`./a.js`);\n" }), /non-literal/);
});

test("G2 fails on minified static imports and exports", () => {
  assert.match(g2({ "host/browser.js": 'import{x}from"lodash";export{x};' }), /bare specifier "lodash"/);
  assert.match(g2({ "host/browser.js": 'export*from"node:fs";' }), /bare specifier "node:fs"/);
  assert.match(g2({ "host/browser.js": 'import/**/{x}/**/from/**/"lodash";' }), /bare specifier "lodash"/);
});

test("G2 scans .mjs and .cjs files", () => {
  assert.match(g2({ "host/x.mjs": 'import "lodash";' }), /bare specifier/);
  assert.match(g2({ "host/y.cjs": 'import "lodash";' }), /bare specifier/);
});

test("G2 fails on require(, eval(, the Function constructor and bracket lookups on globals", () => {
  assert.match(g2({ "host/browser.js": 'const fs = require("fs");' }), /require\(\)/);
  assert.match(g2({ "host/browser.js": 'new Function("return 1")();' }), /Function constructor/);
  assert.match(g2({ "host/browser.js": "eval('1');" }), /eval\(\)/);
  assert.match(g2({ "host/browser.js": 'const r = globalThis["req"+"uire"];' }), /bracket lookup/);
  assert.match(g2({ "host/browser.js": 'const r = process?.["getB"+"uiltinModule"];' }), /bracket lookup/);
});

test("G2 fails on getBuiltinModule and createRequire outside host/node/", () => {
  assert.match(g2({ "host/index.js": "const m = process?.getBuiltinModule('fs');" }), /getBuiltinModule outside host\/node/);
  assert.match(g2({ "host/index.js": "createRequire(import.meta.url);" }), /createRequire outside host\/node/);
});

test("G2 fails on a relative import whose target does not exist", () => {
  assert.match(g2({ "host/browser.js": 'import "./missing.js";' }), /does not exist/);
});

test("G2 accepts a relative import of an existing file and node: inside host/node/", () => {
  assert.equal(g2({ "host/browser.js": 'import "./a.js";', "host/a.js": "export const a = 1;", "host/node/n.js": 'import "node:fs";' }), null);
});

test("G1, G4 and G5 fail when a declared component is missing from dist", () => {
  const out = scratch("empty-dist");
  assert.match(gates.g1ZeroImport({ ...real(), out }), /probe: .*was not built/);
  assert.match(gates.g4ComponentValidity({ ...real(), out }), /probe: .*was not built/);
  assert.match(gates.g5OneCoreModule({ ...real(), out }), /probe: no generated JavaScript/);
  mkdirSync(join(out, "js"));
  assert.match(gates.g5OneCoreModule({ ...real(), out }), /probe: no generated JavaScript/);
});

test("G4 fails when the component exports more than the committed world", () => {
  const root = tree("g4-world", {
    "tools/components.json": JSON.stringify({ components: { probe: { crate: "x", witDir: "probe/wit", world: "probe" } } }),
    "probe/wit/probe.wit": "package sentropic:bash-probe@0.0.0;\nworld probe { export ping: func(x: u32) -> u32; }\n",
  });
  // The built probe also exports echo-bytes and grow: width subtyping lets it target the smaller world.
  assert.match(gates.g4ComponentValidity({ ...real(), root }), /differs from the committed WIT world/);
});

test("G4 fails on a committed wit/ package that does not parse", () => {
  const root = tree("g4-wit", {
    "tools/components.json": readFileSync(join(repoRoot, "tools", "components.json"), "utf8"),
    "wit/broken.wit": "package a:b@0.0.0; interface i { nonsense }\n",
  });
  assert.match(gates.g4ComponentValidity({ ...real(), root }), /wit\/: wasm-tools component wit failed/);
});

test("G4 compares functions inside exported interfaces", () => {
  const doc = (fns) => ({
    worlds: [{ name: "root", imports: {}, exports: { engine: { interface: { id: 0 } } } }],
    interfaces: [{ name: "engine", types: {}, functions: Object.fromEntries(fns.map((f) => [f, { name: f, kind: "freestanding", params: [], result: "u32" }])) }],
    types: [],
  });
  const a = JSON.stringify(gates.normalizeWorld(doc(["run"]), "root"));
  assert.equal(a, JSON.stringify(gates.normalizeWorld(doc(["run"]), "root")));
  assert.notEqual(a, JSON.stringify(gates.normalizeWorld(doc(["run", "extra"]), "root")));
});

test("G5 fails on two .wasm files and on an embedded base64 literal", () => {
  const two = tree("g5-two", { "js/probe/a.wasm": "x", "js/probe/b.wasm": "y" });
  assert.match(gates.g5OneCoreModule({ ...real(), out: two }), /expected exactly one .wasm, found 2/);
  const b64 = tree("g5-b64", { "js/probe/a.wasm": "x", "js/probe/a.js": "const d = '" + "A".repeat(1200) + "';" });
  assert.match(gates.g5OneCoreModule({ ...real(), out: b64 }), /base64 literal/);
});

const g6root = (rs, lock) => {
  const root = scratch("g6");
  cpSync(fx("g6-alloc"), root, { recursive: true });
  mkdirSync(join(root, "tools"));
  cpSync(join(repoRoot, "tools", "components.json"), join(root, "tools", "components.json"));
  mkdirSync(join(root, "probe", "src"), { recursive: true });
  writeFileSync(join(root, "kernel", "src", "lib.rs"), rs);
  if (lock) writeFileSync(join(root, "Cargo.lock"), lock);
  return root;
};

test("G6 fails on qualified alloc paths outside kernel/src/mem", () => {
  for (const rs of [
    "pub fn f() -> alloc::vec::Vec<u8> { alloc::vec![] }",
    "pub fn f() -> ::alloc::string::String { todo!() }",
    "pub fn f() { let _ = alloc::format!(\"x\"); }",
    "use alloc::{borrow::{Cow}, vec::Vec};",
    "pub fn f() -> alloc::sync::Arc<u8> { todo!() }",
  ]) assert.match(gates.g6ZeroCrate({ ...real(), root: g6root(rs) }), /uses the alloc crate/, rs);
});

test("G6 accepts alloc inside kernel/src/mem and a path that only contains the word", () => {
  const root = g6root("pub mod mem; pub fn f() { crate::mem::alloc::g(); }");
  mkdirSync(join(root, "kernel", "src", "mem"));
  writeFileSync(join(root, "kernel", "src", "mem", "mod.rs"), "extern crate alloc;\nuse alloc::vec::Vec;\n");
  assert.equal(gates.g6ZeroCrate({ ...real(), root }), null);
});

test("G6 fails on a Cargo.lock package with a source", () => {
  const lock = 'version = 4\n\n[[package]]\nname = "fixture-kernel"\nversion = "0.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n';
  assert.match(gates.g6ZeroCrate({ ...real(), root: g6root("", lock) }), /has a package with a source/);
});

test("G3 fails when dist/ differs from the reproducible build", { timeout: 300000 }, () => {
  const out = scratch("g3-dist");
  cpSync(join(repoRoot, "dist"), out, { recursive: true, filter: (s) => !s.includes("/target") });
  appendFileSync(join(out, "js", "probe", "probe.js"), "\n// tampered\n");
  assert.match(gates.g3TwoBuildIdentity({ ...real(), quick: false, out }), /differs from the reproducible build/);
});

test("G8 fails on a wrong pinned hash", () => {
  const root = tree("g8", {});
  const pins = readJson(join(repoRoot, "tools", "pins.json"));
  pins.tools[0].sha256 = "0".repeat(64);
  writeFileSync(join(root, "tools", "pins.json"), JSON.stringify(pins));
  assert.match(gates.g8ToolPins({ ...real(), root }), /marker hash differs/);
});

test("G8 fails on a pin that is not an https URL", () => {
  const root = tree("g8-url", {});
  const pins = readJson(join(repoRoot, "tools", "pins.json"));
  pins.tools[0].url = "http://example.invalid/x.tar.gz";
  writeFileSync(join(root, "tools", "pins.json"), JSON.stringify(pins));
  assert.match(gates.g8ToolPins({ ...real(), root }), /does not use an https/);
  assert.match(pinsProblem([{ name: "t", url: "file:///x.tar.gz", sha256: "0".repeat(64) }]), /https/);
});

test("a tampered cached binary is detected by toolProblem and by G8", () => {
  const tdir = scratch("tools-tamper");
  const pin = { name: "fake", version: "1.0.0", sha256: "a".repeat(64), bin: "tool" };
  const dir = join(tdir, "fake-1.0.0");
  mkdirSync(dir);
  writeFileSync(join(dir, "tool"), "original");
  writeFileSync(join(dir, ".verified"), markerText(pin, sha256File(join(dir, "tool"))));
  assert.equal(toolProblem(pin, tdir), null);
  writeFileSync(join(dir, "tool"), "tampered");
  assert.match(toolProblem(pin, tdir), /does not match the hash recorded/);
  // The same through G8, with the scratch tools directory carried by the context.
  const root = tree("g8-tamper", { "tools/pins.json": JSON.stringify({ tools: [{ ...pin, url: "https://example.invalid/x.tar.gz" }] }) });
  assert.match(gates.g8ToolPins({ ...real(), root, toolsDir: tdir }), /does not match the hash recorded/);
});

test("lockProblem rejects a missing integrity, an http URL and a manifest mismatch", () => {
  const manifest = { devDependencies: { a: "1.0.0" } };
  const pkg = { version: "1.0.0", resolved: "https://registry.npmjs.org/a/-/a-1.0.0.tgz", integrity: "sha512-AAAA" };
  const lock = (p, dev = { a: "1.0.0" }) => ({ packages: { "": { devDependencies: dev }, "node_modules/a": p } });
  assert.equal(lockProblem(lock(pkg), manifest), null);
  assert.match(lockProblem(lock({ ...pkg, integrity: undefined }), manifest), /no sha512 integrity/);
  assert.match(lockProblem(lock({ ...pkg, resolved: "http://x/a.tgz" }), manifest), /not resolved over https/);
  assert.match(lockProblem(lock(pkg, { a: "2.0.0" }), manifest), /devDependencies differ/);
});
