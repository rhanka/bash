// Negative tests for the second hardening round of PR #9: the G2 scanner (layer A), the runtime enforcement
// (layer B: --disallow-code-generation-from-strings and the module resolve hook) and the supply-chain minors.
import { test } from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { repoRoot, scratch } from "./helpers.mjs";
import { toolsDir, pinsProblem, treeManifest } from "../tools/lib.mjs";
import * as gates from "../tools/gates.mjs";
import { CODEGEN_FLAG, codegenAllowed, runTests } from "../tools/run-tests.mjs";

const real = () => ({ root: repoRoot, out: join(repoRoot, "dist"), toolsDir: toolsDir(), quick: true });
function tree(prefix, files) {
  const root = scratch(prefix);
  for (const [p, text] of Object.entries(files)) {
    mkdirSync(join(root, p, ".."), { recursive: true });
    writeFileSync(join(root, p), text);
  }
  return root;
}
const g2 = (files) => {
  const root = tree("hard", files);
  return gates.g2NoBareImport({ ...real(), root, out: root });
};
const host = (text, name = "host/browser.js") => g2({ [name]: text });
const BARE = /bare specifier "node:child_process"/;

// ------------------------------------------------------------------ A. G2 scanner

test("G2: // comments, quotes and regexes end on \\r, U+2028 and U+2029", () => {
  for (const lt of ["\r", " ", " "]) {
    assert.match(host(`//${lt}import('node:child_process');\n`), BARE, JSON.stringify(lt));
    assert.match(host(`const s = 'x${lt}'; import('node:child_process');`), /unterminated string/, JSON.stringify(lt));
    assert.match(host(`const r = /x${lt}/; import('node:child_process');`), /unterminated regular expression/, JSON.stringify(lt));
  }
});

test("G2: rejects <!-- and --> in code, even when a string would hide them in a naive scan", () => {
  assert.match(host('<!-- "\nrequire("child_process");\n// "', "host/y.cjs"), /HTML-like comment marker/);
  assert.match(host("const a = 1; --> import('node:child_process');\n"), /HTML-like comment marker/);
  assert.equal(host('const s = "<!-- -->";'), null);
});

test("G2: a regex after the ) of an if/while/for header is a regex; after an expression ) it is a division", () => {
  assert.match(host('if (1) /"/.test("/*"); import("node:child_process"); /* " */'), BARE);
  assert.match(host('while (a) /"/.test(b); import("node:child_process"); /* " */'), BARE);
  assert.match(host('for (;;) /"/.test(b); import("node:child_process"); /* " */'), BARE);
  assert.match(host('const x = (a + b) / 2; const y = "/*"; import("node:child_process"); /* " */'), BARE);
  assert.equal(g2({ "js/p/p.js": "const x = (a + b) / 2, y = x / 3; if (y) /z/.test(x);" }), null);
});

test("G2: a keyword after ., ?. or # does not start a regex; ++ and -- are not binary operators", () => {
  assert.match(host('const x = { in: 2 }.in / 1 + "/" + "/*"; import("node:child_process"); /* " */'), BARE);
  assert.match(host('const x = o?.in / 1 + "/" + "/*"; import("node:child_process"); /* " */'), BARE);
  assert.match(host('class A { #in = 1; f() { return this.#in / 1 + "/" + "/*"; } } import("node:child_process"); /* " */'), BARE);
  assert.match(host('let a = 4; a++ / 1 + "/" + "/*"; import("node:child_process"); /* " */'), BARE);
  assert.match(host('let a = 4; a-- / 1 + "/" + "/*"; import("node:child_process"); /* " */'), BARE);
  assert.match(host('const x = { in: 2 }.in / (eval("1"), 1) / 1;'), /eval\(\)/);
  assert.match(host('const x = { in: 2 }.in / (0, eval)("1") / 1;'), /banned bare identifier|eval/);
});

test("G2: fails closed on a slash it cannot classify", () => {
  assert.match(host('const x = { a: 1 } / 1 + "/" + "/*"; import("node:child_process"); /* " */'), /fail closed/);
  assert.match(host("function f() {}\n/re/.test(x);"), /fail closed/);
  assert.match(host("const r = /unterminated\n;"), /fail closed/);
  assert.match(host("/* never closed import 'x';"), /unterminated block comment/);
  assert.match(host('const s = "never closed'), /unterminated string/);
  assert.match(host("const t = `never closed"), /unterminated template/);
});

test("G2: a literal cannot bridge two statements", () => {
  assert.match(host('const x = "export /*";\nimport "node:child_process";\nconst y = "*/ from \'./ok.js\'";'), BARE);
  assert.match(host('const x = "import /*";\nexport * from "node:child_process";\nconst y = "*/ \'./ok.js\'";'), BARE);
});

test("G2: ES2022 quoted export names are parsed, or the gate fails closed", () => {
  assert.match(host('export * as "p" from "node:child_process";'), BARE);
  assert.match(host('export { execSync as "a\\"}" } from "node:child_process";'), BARE);
  assert.match(host('import { "x" as y } from "node:child_process";'), BARE);
  assert.match(host('export * as "p\\u0071" from "node:child_process";'), BARE);
  assert.match(host('export * from "node:child\\u005fprocess";'), /fail closed/);
  assert.match(host('import "node:child\\u005fprocess";'), /fail closed/);
  assert.match(host('import x from "node:child\\u005fprocess";'), /fail closed/);
  assert.match(host('export { a } from "node:child\\u005fprocess";'), /fail closed/);
});

test("G2: fails closed on an import keyword that no import regex accounted for; import.meta is fine", () => {
  assert.match(host('import.source("x");'), /fail closed/);
  assert.match(host('import.defer("x");'), /fail closed/);
  assert.match(host('import "x" '), /bare specifier|fail closed/);
  assert.equal(host("export const u = import.meta.url;"), null);
});

test("G2: DYNAMIC_ANY runs on code: a non-literal import() is found even next to literals and comments", () => {
  assert.match(host('const s = "/*"; import(x); /* " */'), /non-literal/);
  assert.match(host('const s = `${"/*"}`; import(x); /* ` */'), /non-literal/);
  assert.match(host('import("./a.js", { with: {} });', "host/browser.js"), /non-literal/);
});

test("G2: a backslash in an import specifier is refused", () => {
  assert.match(host('import "./a\\u002ejs";'), /fail closed/);
});

test("G2: no backslash escape inside a bracket-key string or a quoted property key; no quoted constructor", () => {
  assert.match(host('(() => {})["\\x63onstructor"]("return 1")();', "host/a.js"), /backslash escape inside a bracket-key/);
  assert.match(g2({ "js/p/p.js": '(() => {})["\\x63onstructor"]("return 1")();' }), /backslash escape inside a bracket-key/);
  assert.match(g2({ "js/p/p.js": 'const { "constructor": c } = () => {}; c("return 1")();' }), /quoted "constructor" key/);
  assert.match(g2({ "js/p/p.js": "const { 'constructor': c } = () => {};" }), /quoted "constructor" key/);
  assert.match(g2({ "js/p/p.js": 'const { a, "\\x63onstructor": c } = () => {};' }), /backslash escape inside a quoted property key/);
  assert.equal(g2({ "js/p/p.js": 'const { "a": c } = o; const x = o["b"]; const m = "x\\n";' }), null);
});

test("G2 host/: bans non-literal computed member access", () => {
  for (const t of [
    '(() => {})[("constructor")]("return 1")();',
    '(() => {})["constr" + "uctor"];',
    "const v = o[k];",
    "const v = o?.[k];",
    "const v = o[0x1];",
    "const v = o[1.5];",
    "const v = f()[k];",
    "const v = o[`a`];",
    "const v = o[ 'a' + 'b' ];",
  ]) assert.match(host(t, "host/a.js"), /non-literal computed member access/, t);
  for (const t of ["const v = o[0];", "const v = o[ 12 ];", 'const v = o["a"];', "const v = o['a b'];", "const w = o[1][2];", "const a = [1, 2]; const [b, c] = a; f(a, [1]); const o2 = { a: [1], ...x }; return [x][0];"]) {
    assert.equal(host(t, "host/a.js"), null, t);
  }
});

test("G2 host/: bans computed object and destructuring keys", () => {
  assert.match(host("const o = { [k]: 1 };", "host/a.js"), /computed object or destructuring key/);
  assert.match(host("const o = { a: 1, [k]: 1 };", "host/a.js"), /computed object or destructuring key/);
  assert.match(host("const { [k]: v } = o;", "host/a.js"), /computed object or destructuring key/);
  assert.match(host("const { a, [k]: v } = o;", "host/a.js"), /computed object or destructuring key/);
  assert.match(host("class A { [k]() {} }", "host/a.js"), /computed/);
});

test("G2 host/: bans the bare identifiers globalThis, global, self and window", () => {
  for (const id of ["globalThis", "global", "self", "window"]) {
    assert.match(host(`const g = ${id}; g["ev" + "al"]("1");`, "host/a.js"), /global object identifier/, id);
    assert.match(host(`(${id})["ev" + "al"]("1");`, "host/a.js"), /global object identifier|bracket lookup/, id);
  }
  assert.equal(host("const re = /x/; const f = re.global; const o = { a: 1 }; o.self = 2;", "host/a.js"), null);
});

test("G2 host/: bans process outside host/node/", () => {
  assert.match(host('const p = process; p["getB" + "uiltinModule"]("fs");', "host/a.js"), /process outside host\/node/);
  assert.match(host("const e = process.env;", "host/browser.js"), /process outside host\/node/);
  assert.equal(host("const e = process.env;", "host/node/n.js"), null);
  assert.match(host('const p = process; p["getB" + "uiltinModule"]("fs");', "host/node/n.js"), /non-literal computed member access/);
});

test("G2: the host/ rules do not apply to generated dist/js", () => {
  assert.equal(g2({ "js/p/p.js": "const w = globalThis.WebAssembly; const r = registers[i]; const o = { [k]: 1 }; const p = process;" }), null);
});

test("G2: Reflect.get allow-list is anchored", () => {
  const ok = 'if (!Reflect.get(cstate, "callingSyncExport")) {}';
  assert.equal(g2({ "js/p/p.js": ok }), null);
  assert.match(g2({ "js/p/p.js": 'if (!o.Reflect.get(cstate, "callingSyncExport")) {}' }), /banned bare identifier/);
  assert.match(g2({ "js/p/p.js": 'if (!$Reflect.get(cstate, "callingSyncExport")) {}' }), /banned bare identifier|backslash|uses/);
});

test("G2: the real tree still passes with the stricter scanner", () => {
  assert.equal(gates.g2NoBareImport(real()), null);
});

// ------------------------------------------------------------------ B. runtime enforcement

const nodeRun = (args, opts = {}) => spawnSync(process.execPath, args, { encoding: "utf8", ...opts });

test("the test runner passes --disallow-code-generation-from-strings, and every code-generation syntax throws under it", () => {
  const f = join(tree("codegen", {}), "cg.mjs");
  writeFileSync(
    f,
    `const AsyncFunction = (async () => {}).constructor;
const tries = [
  () => eval("1"),
  () => (0, eval)("1"),
  () => new Function("return 1"),
  () => Function("return 1")(),
  () => globalThis["ev" + "al"]("1"),
  () => (() => {})["constr" + "uctor"]("return 1")(),
  () => new AsyncFunction("return 1"),
  () => new (function* () {}).constructor("yield 1"),
  () => setTimeout("1", 0),
  () => new WebAssembly.Module(new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0])),
];
const out = tries.map((t) => { try { t(); return "ok"; } catch (e) { return e.name; } });
console.log(JSON.stringify(out));
`,
  );
  const r = nodeRun([CODEGEN_FLAG, f]);
  const out = JSON.parse(r.stdout);
  assert.deepEqual(out.slice(0, 9), Array(9).fill("EvalError").map((v, i) => (i === 8 ? out[8] : v)));
  assert.equal(out[9], "ok", "WebAssembly stays available");
  // Without the flag the same code runs: the flag is what enforces it.
  assert.equal(JSON.parse(nodeRun([f]).stdout)[0], "ok");
});

test("runTests runs each test with the flag, and honours the allow-list with a reason", () => {
  const mk = (allow) => {
    const root = tree("cgrun", { "test/a.test.mjs": 'import { test } from "node:test";\ntest("x", () => { new Function("return 1"); });\n' });
    if (allow !== null) {
      mkdirSync(join(root, "tools"));
      writeFileSync(join(root, "tools", "codegen-allow.json"), JSON.stringify({ allow }));
    }
    return root;
  };
  assert.notEqual(runTests(mk(null), { stdio: "ignore" }), 0, "no allow-list: eval is refused");
  assert.notEqual(runTests(mk([]), { stdio: "ignore" }), 0);
  assert.equal(runTests(mk([{ file: "test/a.test.mjs", reason: "fixture" }]), { stdio: "ignore" }), 0);
  assert.throws(() => codegenAllowed(mk([{ file: "test/a.test.mjs" }])), /reason/);
  assert.throws(() => codegenAllowed(mk([{ file: "test/a.test.mjs", reason: " " }])), /reason/);
});

test("the code-generation allow-list of the repository is empty", () => {
  assert.equal(codegenAllowed(repoRoot).size, 0);
});

const hookUrl = pathToFileURL(join(repoRoot, "tools", "test-hooks.mjs")).href;
/** Runs `entry` (path relative to root) under the hook installed for `root`, in a fresh process. */
function underHook(root, entry) {
  const runner = join(scratch("hook-runner"), "run-under-hook.mjs");
  writeFileSync(runner, `import { installHooks } from ${JSON.stringify(hookUrl)};\ninstallHooks({ root: ${JSON.stringify(root)} });\nawait import(${JSON.stringify(pathToFileURL(join(root, entry)).href)});\nconsole.log("loaded");\n`);
  return nodeRun([runner]);
}
const refused = (r, spec) => {
  assert.notEqual(r.status, 0, r.stdout);
  assert.match(r.stderr, /ERR_SENTROPIC_FORBIDDEN_IMPORT|sentropic test hook: forbidden import/);
  if (spec) assert.ok(r.stderr.includes(spec), r.stderr);
};

test("host/browser.js and host/index.js load under the resolve hook", () => {
  for (const entry of ["host/browser.js", "host/index.js"]) {
    const r = underHook(repoRoot, entry);
    assert.equal(r.status, 0, r.stderr);
    assert.match(r.stdout, /loaded/);
  }
});

test("the resolve hook refuses a node: import from the browser graph at runtime, whatever its syntax", () => {
  refused(underHook(tree("hk1", { "host/browser.js": 'import "node:fs";' }), "host/browser.js"), "node:fs");
  refused(underHook(tree("hk2", { "host/browser.js": 'import "./a.js";', "host/a.js": 'import { readFileSync } from "node:fs"; export const r = readFileSync;' }), "host/browser.js"), "node:fs");
  refused(underHook(tree("hk3", { "host/browser.js": 'await import(["node:", "fs"].join(""));' }), "host/browser.js"), "node:fs");
  refused(underHook(tree("hk4", { "host/browser.js": 'import "../dist/js/p/p.js";', "dist/js/p/p.js": 'import "node:os";' }), "host/browser.js"), "node:os");
  // host/node/ is reachable from the browser entry here, so its node: import is refused too.
  refused(underHook(tree("hk5", { "host/browser.js": 'import "./node/n.js";', "host/node/n.js": 'import "node:fs";' }), "host/browser.js"), "node:fs");
});

test("the resolve hook refuses a bare specifier from host/ and dist/js/ at runtime", () => {
  refused(underHook(tree("hk6", { "host/browser.js": 'import "lodash";' }), "host/browser.js"), "lodash");
  refused(underHook(tree("hk7", { "host/browser.js": 'import "fs";' }), "host/browser.js"), '"fs"');
  refused(underHook(tree("hk8", { "host/index.js": 'await import("child_process");' }), "host/index.js"), "child_process");
  refused(underHook(tree("hk9", { "host/index.js": 'import "../dist/js/p/p.js";', "dist/js/p/p.js": 'import "lodash";' }), "host/index.js"), "lodash");
  refused(underHook(tree("hk10", { "host/index.js": 'await import("https://example.invalid/x.js");' }), "host/index.js"), "https://example.invalid/x.js");
  refused(underHook(tree("hk11", { "host/index.js": 'import "node:fs";' }), "host/index.js"), "node:fs");
});

test("the resolve hook allows node: in host/node/ when it is not reachable from the browser entry", () => {
  const root = tree("hk12", { "host/index.js": 'import "./node/n.js"; import "./b.js";', "host/b.js": "export {};", "host/node/n.js": 'import "node:fs";', "host/browser.js": 'import "./b.js";' });
  const r = underHook(root, "host/index.js");
  assert.equal(r.status, 0, r.stderr);
});

// ------------------------------------------------------------------ C. supply-chain minors

test("pinsProblem requires a string on every field and a valid sha256 even under fieldsOnly", () => {
  const ok = { name: "t", version: "1.0.0", bin: "bin/t", url: "https://x.invalid/t.tgz", sha256: "0".repeat(64) };
  assert.equal(pinsProblem([ok]), null);
  assert.equal(pinsProblem([{ ...ok, url: "file:///x" }], { fieldsOnly: true }), null);
  for (const bad of [{ url: ["https://x/y"] }, { url: 7 }, { url: undefined }, { sha256: ["0".repeat(64)] }, { sha256: "abc" }, { sha256: undefined }, { name: ["t"] }, { version: ["1.0.0"] }, { bin: ["bin/t"] }, { bin: 1 }]) {
    for (const opts of [{}, { fieldsOnly: true }]) assert.notEqual(pinsProblem([{ ...ok, ...bad }], opts), null, JSON.stringify(bad) + JSON.stringify(opts));
  }
});

test("treeManifest rejects a special file and a symbolic link whose target escapes the tree", () => {
  const dir = join(scratch("tm"), "node_modules");
  mkdirSync(join(dir, "a"), { recursive: true });
  writeFileSync(join(dir, "a", "f.js"), "x");
  symlinkSync("../a/f.js", join(dir, "a", "ok-link"));
  assert.deepEqual(Object.keys(treeManifest(dir)), ["a/f.js", "a/ok-link"]);
  for (const [name, target] of [["abs", "/etc/passwd"], ["up", "../../outside"], ["deep", "../../../x"]]) {
    const d = join(scratch("tm"), "node_modules");
    mkdirSync(join(d, "a"), { recursive: true });
    symlinkSync(target, join(d, "a", name));
    assert.throws(() => treeManifest(d), /escapes the tree/, target);
  }
  const d = join(scratch("tm"), "node_modules");
  mkdirSync(join(d, "a"), { recursive: true });
  assert.equal(spawnSync("mkfifo", [join(d, "a", "pipe")]).status, 0);
  assert.throws(() => treeManifest(d), /special file/);
});
