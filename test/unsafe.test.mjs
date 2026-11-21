// Gate 12 of lot L1a-1: `unsafe` lives only in kernel/src/mem/, kernel/src/glue/ (and the generated bindings), and every use
// there is preceded by a SAFETY comment.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { repoRoot } from "./helpers.mjs";
import { walk, components } from "../tools/lib.mjs";

const UNSAFE = /\bunsafe\b/;
const isComment = (l) => /^\s*\/\//.test(l);

/** Bindings directories (relative to kernel/src/, with a trailing slash) declared in tools/components.json. */
export function declaredBindingDirs(root = repoRoot) {
  return Object.values(components(root))
    .filter((d) => d.bindings)
    .map((d) => d.bindings.dir.replace(/^kernel\/src\//, "").replace(/\/?$/, "/"));
}

/** Returns the problems found in one source file (`rel` is its path relative to kernel/src/). */
export function unsafeProblems(rel, src, bindingDirs = declaredBindingDirs()) {
  const inMem = rel.startsWith("mem/") || rel.startsWith("glue/");
  const inBindings = bindingDirs.some((d) => rel.startsWith(d));
  const lines = src.split("\n");
  const problems = [];
  lines.forEach((line, i) => {
    if (!UNSAFE.test(line)) return;
    if (!inMem && !inBindings) {
      problems.push(`${rel}:${i + 1}: the word unsafe outside kernel/src/mem/, kernel/src/glue/ and the declared bindings dirs`);
      return;
    }
    // A declaration `unsafe fn` states a caller obligation, it is not an unsafe block.
    if (inBindings || isComment(line)) return;
    // A declaration `unsafe fn` states a caller obligation; an unsafe block on the same line is not one.
    if (/\bunsafe\s+fn\b/.test(line) && !/\bunsafe\s*\{/.test(line.replace(/\bunsafe\s+fn\b/, ""))) return;
    // Same line, or the contiguous comment/attribute block right above.
    let ok = /SAFETY:/.test(line);
    for (let j = i - 1; !ok && j >= 0 && (isComment(lines[j]) || /^\s*#\[/.test(lines[j])); j--) {
      if (/SAFETY:|#\s*Safety/.test(lines[j])) ok = true;
    }
    if (!ok) problems.push(`${rel}:${i + 1}: unsafe without a preceding // SAFETY: comment`);
  });
  return problems;
}

test("the real kernel tree has no stray unsafe and every unsafe in mem has a SAFETY comment", () => {
  const root = join(repoRoot, "kernel", "src");
  const files = walk(root, (p) => p.endsWith(".rs"));
  assert.ok(files.length > 0);
  const problems = files.flatMap((f) => unsafeProblems(relative(root, f), readFileSync(f, "utf8")));
  assert.deepEqual(problems, []);
});

test("the check fails on unsafe outside mem", () => {
  assert.match(unsafeProblems("lib.rs", "fn f() { unsafe { g() } }")[0], /outside kernel\/src\/mem/);
  assert.match(unsafeProblems("parser/lex.rs", "// unsafe is mentioned")[0], /outside/);
  assert.deepEqual(unsafeProblems("bindings/abi.rs", "unsafe fn f() {}", ["bindings/"]), []);
});

test("bindings are exempt only when declared in components.json", () => {
  assert.match(unsafeProblems("bindings/abi.rs", "unsafe fn f() {}", [])[0], /outside/);
  assert.deepEqual(unsafeProblems("bindings/abi.rs", "unsafe fn f() {}"), [], "the kernel component declares kernel/src/bindings");
  assert.deepEqual(declaredBindingDirs(), ["bindings/"]);
  assert.match(unsafeProblems("glue/x.rs", "fn f() {\n    unsafe { g() };\n}")[0], /without a preceding/, "glue is hand-written: not exempt");
});

test("a one-line unsafe fn with an unsafe block needs a SAFETY comment", () => {
  assert.match(unsafeProblems("mem/x.rs", "unsafe fn f() { unsafe { g() } }")[0], /without a preceding/);
  assert.deepEqual(unsafeProblems("mem/x.rs", "// SAFETY: fine\nunsafe fn f() { unsafe { g() } }"), []);
});

test("the kernel denies unsafe operations inside unsafe fn", () => {
  assert.match(readFileSync(join(repoRoot, "kernel", "src", "lib.rs"), "utf8"), /#!\[deny\(unsafe_op_in_unsafe_fn\)\]/);
});

test("the check fails on an unsafe block without a SAFETY comment", () => {
  assert.match(unsafeProblems("mem/x.rs", "fn f() {\n    unsafe { g() };\n}")[0], /without a preceding/);
  assert.match(unsafeProblems("mem/x.rs", "// SAFETY: fine\n\nunsafe { g() };")[0], /without a preceding/, "a blank line breaks the block");
  assert.match(unsafeProblems("mem/x.rs", "// a note\nunsafe impl Sync for X {}")[0], /without a preceding/);
});

test("the check accepts the documented forms", () => {
  assert.deepEqual(unsafeProblems("mem/x.rs", "    // SAFETY: the pointer is valid.\n    unsafe { g() };"), []);
  assert.deepEqual(unsafeProblems("mem/x.rs", "    // SAFETY: first line\n    // second line\n    let x = unsafe { g() };"), []);
  assert.deepEqual(unsafeProblems("mem/x.rs", "unsafe { g() }; // SAFETY: inline"), []);
  assert.deepEqual(unsafeProblems("mem/x.rs", "/// # Safety\n/// caller contract\n#[inline]\npub unsafe fn f() {}"), []);
});
