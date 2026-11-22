import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { bundlePackage, collectDecls, flattenShared, rewriteLazyImports, splitCode } from "../tools/bundle.mjs";
import { toolsDir } from "../tools/lib.mjs";
import { scratch } from "./helpers.mjs";

const ROOT = "/repo";
const id = (name) => ROOT + "/" + name;
const rec = (name, src, imports = []) => ({ file: id(name), src, imports });

// Runs the flattener over in-memory module records (no filesystem, no minifier) with
// dependencies listed first, mirroring the topological order bundlePackage computes.
function flatten(entries) {
  const modules = new Map(entries.map(([name, src, imports]) => [id(name), rec(name, src, imports ?? [])]));
  const order = entries.map(([name]) => id(name));
  return flattenShared(modules, order, ROOT);
}

const codeText = (src) => splitCode(src).filter((s) => s.code).map((s) => s.text).join("");
const nonCodeText = (src) => splitCode(src).filter((s) => !s.code).map((s) => s.text).join("");

test("top-level collision alpha-renames the later module and rewires consumers", () => {
  const { code, mapping } = flatten([
    ["a.js", `export const util = 1;`],
    ["b.js", `export const util = 20;`],
    ["c.js", `import { util as aUtil } from "./a.js";\nimport { util as bUtil } from "./b.js";\nexport const sum = aUtil + bUtil;`,
      [{ spec: "./a.js", namespace: null, names: [{ imported: "util", local: "aUtil" }], target: id("a.js") },
        { spec: "./b.js", namespace: null, names: [{ imported: "util", local: "bUtil" }], target: id("b.js") }]],
  ]);
  assert.equal(mapping.get(id("a.js") + "::util"), "util");
  assert.equal(mapping.get(id("b.js") + "::util"), "util$1");
  assert.equal(mapping.get(id("c.js") + "::sum"), "sum");
  assert.match(code, /const util\$1 = 20;/);
  assert.match(code, /const sum = util \+ util\$1;/);
  assert.equal(new Function(code + "\nreturn sum;")(), 21);
});

test("nested shadowing binding is renamed before the import resolves", () => {
  const { code, mapping } = flatten([
    ["p.js", `export const val = 5;`],
    ["q.js", `import { val as v } from "./p.js";\nexport function get(n) {\n  return n.map((val) => val + v);\n}`,
      [{ spec: "./p.js", namespace: null, names: [{ imported: "val", local: "v" }], target: id("p.js") }]],
  ]);
  assert.equal(mapping.get(id("q.js") + "::get"), "get");
  assert.match(code, /\(val\$1\) => val\$1 \+ val/);
  const get = new Function(code + "\nreturn get;")();
  assert.deepEqual(get([1, 2]), [6, 7]);
});

test("member access and shorthand survive an import rename", () => {
  const { code } = flatten([
    ["r.js", `export const key = 7;`],
    ["s.js", `import { key as k } from "./r.js";\nexport function pick(o) {\n  return o.key + k + (o.k ?? 0);\n}\nexport const shape = { k };`,
      [{ spec: "./r.js", namespace: null, names: [{ imported: "key", local: "k" }], target: id("r.js") }]],
  ]);
  assert.match(code, /return o\.key \+ key \+ \(o\.k \?\? 0\);/);
  assert.match(code, /\{ k: key \}/);
  const { pick, shape } = new Function(code + "\nreturn { pick, shape };")();
  assert.equal(pick({ key: 1, k: 2 }), 10);
  assert.deepEqual(shape, { k: 7 });
});

test("regex after a keyword is a literal, never renameable code", () => {
  for (const src of [`return /foo/.test(s);`, `typeof /foo/ === "object";`, `case /foo/:`]) {
    assert.equal(codeText(src).includes("foo"), false, src);
    assert.match(nonCodeText(src), /\/foo\//, src);
  }
  // Division keeps its operands as code.
  assert.match(codeText(`const q = a / b / c;`), /a \/ b \/ c/);
});

test("strings, comments and template raw text are never code", () => {
  const src = `const s = "foo"; // foo\n/* foo */\nconst t = \`foo\${foo}foo\`; foo;`;
  const occurrences = codeText(src).match(/foo/g) ?? [];
  // Only the interpolation and the trailing reference are code.
  assert.equal(occurrences.length, 2);
});

test("character class with slashes survives the splitter intact", () => {
  const src = `return /^[A-Za-z0-9_@%+=:,./-]+$/.test(s) ? s : q;`;
  const segs = splitCode(src);
  const literal = segs.find((s) => !s.code && s.text.includes("A-Za-z"));
  assert.equal(literal?.text, `/^[A-Za-z0-9_@%+=:,./-]+$/`);
});

test("call arguments are renamed bare, never expanded as shorthand", () => {
  const { code } = flatten([
    ["a.js", `export const CHUNK = 10;`],
    ["b.js", `export const CHUNK = 20;\nexport const len = Math.min(1, 2, CHUNK, 4);`],
  ]);
  assert.match(code, /const CHUNK\$1 = 20;/);
  assert.match(code, /Math\.min\(1, 2, CHUNK\$1, 4\)/);
  assert.equal(new Function(code + "\nreturn len;")(), 1);
});

test("object shorthand survives a dollar-suffixed rename", () => {
  const { code } = flatten([
    ["a.js", `export const CHUNK = 10;`],
    ["b.js", `export const CHUNK = 20;\nexport const o = { CHUNK };`],
  ]);
  assert.match(code, /\{ CHUNK: CHUNK\$1 \}/);
  assert.deepEqual(new Function(code + "\nreturn o;")(), { CHUNK: 20 });
});

test("dollar-prefixed and dollar-suffixed names are distinct identifiers", () => {
  const { code } = flatten([
    ["c.js", `export const CHUNK = 5;\nexport const keep = [CHUNK$9, $CHUNK];\nexport const use = CHUNK + 1;`],
  ]);
  assert.match(code, /\[CHUNK\$9, \$CHUNK\]/);
  assert.match(code, /const use = CHUNK \+ 1;/);
});

test("destructuring, single-param arrow and generator bindings are renamed before import capture", () => {
  const { code } = flatten([
    ["p.js", `export const val = 10;`],
    ["q.js", [
      `import { val as v } from "./p.js";`,
      `export function run(obj, arr) {`,
      `  const { val } = obj;`,
      `  const [x, { k: val2 }] = arr;`,
      `  const f = val => val + v + val2;`,
      `  function* g(...val) { yield val.length + v; }`,
      `  return val + x + f(1) + g(2, 3).next().value;`,
      `}`,
    ].join("\n"), [{ spec: "./p.js", namespace: null, names: [{ imported: "val", local: "v" }], target: id("p.js") }]],
  ]);
  const run = new Function(code + "\nreturn run;")();
  assert.equal(run({ val: 4 }, [5, { k: 6 }]), 4 + 5 + (1 + 10 + 6) + (2 + 10));
});

test("collectDecls ignores property values and member accesses", () => {
  const decls = collectDecls(`const o = { k: val, val: 1 }; return o.val + fn(val);`);
  assert.equal(decls.has("o"), true);
  assert.equal(decls.has("val"), false);
});

test("rewriteLazyImports rewrites single and double quoted import() calls without touching other strings", () => {
  const src = [
    `const msg = "./ext.js";`,
    `const a = () => import('./ext.js');`,
    `const b = () => import( /* keep */ "./ext.js" );`,
    `export { msg, a, b };`,
  ].join("\n");
  const out = rewriteLazyImports(src, new Map([["./ext.js", "./core/ext.js"]]));
  assert.match(out, /const msg = "\.\/ext\.js";/);
  assert.match(out, /import\("\.\/core\/ext\.js"\)/);
  assert.match(out, /import\( \/\* keep \*\/ "\.\/core\/ext\.js" \)/);
});

test("bundlePackage recurses into lazy chunks and rewrites adapters and nested lazy imports", async (t) => {
  if (!existsSync(join(toolsDir(), "npm", "node_modules", "oxc-minify", "index.js"))) {
    t.skip("oxc-minify not staged");
    return;
  }
  const root = scratch("bundle-lazy-recursive");
  for (const dir of ["host/core", "host/node", "dist/js"]) mkdirSync(join(root, dir), { recursive: true });
  writeFileSync(join(root, "host/browser.js"), `import { run, runNode } from "./core/main.js";\nexport { run, runNode };\n`);
  writeFileSync(join(root, "host/node/load.js"), `export const load = () => 1;\n`);
  writeFileSync(join(root, "host/node/adapter.js"), `export async function useNodeLazy() { return (await import("../core/node-lazy.js")).tag; }\n`);
  writeFileSync(join(root, "host/index.js"), [
    `import { load } from "./node/load.js";`,
    `import { run, runNode } from "./core/main.js";`,
    `export { run, runNode };`,
    `export { useNodeLazy } from "./node/adapter.js";`,
    `export const init = load;`,
  ].join("\n"));
  writeFileSync(join(root, "host/core/main.js"), [
    `export async function run() { return (await import("./lazy-a.js")).val(); }`,
    `export function runNode() { return "ok"; }`,
  ].join("\n"));
  writeFileSync(join(root, "host/core/lazy-a.js"), [
    `import { helper } from "./lazy-helper.js";`,
    `export async function val() { return await helper(); }`,
  ].join("\n"));
  writeFileSync(join(root, "host/core/lazy-helper.js"), [
    `export async function helper() { return "a+" + (await import("./lazy-b.js")).leaf; }`,
  ].join("\n"));
  writeFileSync(join(root, "host/core/lazy-b.js"), `export const leaf = "b";\n`);
  writeFileSync(join(root, "host/core/node-lazy.js"), `export const tag = "node-lazy";\n`);

  const outJs = join(root, "dist", "js");
  await bundlePackage({
    root,
    indexEntry: join(root, "host/index.js"),
    browserEntry: join(root, "host/browser.js"),
    loadEntry: join(root, "host/node/load.js"),
    outShared: join(outJs, "shared.js"),
    outIndex: join(outJs, "index.js"),
    outBrowser: join(outJs, "browser.js"),
    outLoad: join(outJs, "node/load.js"),
  });

  const shared = readFileSync(join(outJs, "shared.js"), "utf8");
  const lazyA = readFileSync(join(outJs, "core/lazy-a.js"), "utf8");
  const lazyB = readFileSync(join(outJs, "core/lazy-b.js"), "utf8");
  const adapters = readFileSync(join(outJs, "node/adapters.js"), "utf8");
  assert.match(shared, /import\(["']\.\/core\/lazy-a\.js["']\)/);
  assert.match(lazyA, /import\(["']\.\/lazy-b\.js["']\)/);
  assert.match(adapters, /import\(["']\.\.\/core\/node-lazy\.js["']\)/);
  assert.ok(lazyB.length > 0);
  const browserMod = await import(pathToFileURL(join(outJs, "browser.js")).href);
  const indexMod = await import(pathToFileURL(join(outJs, "index.js")).href);
  assert.equal(await browserMod.run(), "a+b");
  assert.equal(await indexMod.useNodeLazy(), "node-lazy");
});
