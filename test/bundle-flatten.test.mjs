import { test } from "node:test";
import assert from "node:assert/strict";
import { flattenShared, splitCode } from "../tools/bundle.mjs";

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
