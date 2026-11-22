import assert from "node:assert/strict";
import { test } from "node:test";
import { Bash } from "@sentropic/bash";

function makeRng(seed) {
  let state = seed;
  const mask = (1n << 64n) - 1n;
  return () => {
    let x = state;
    x ^= x >> 12n;
    x = (x ^ (x << 25n)) & mask;
    x ^= x >> 27n;
    state = x;
    return Number(((x * 0x2545_F491_4F6C_DD1Dn) & mask) >> 32n);
  };
}

const FRAGMENTS = [
  "if ", "then ", "elif ", "else ", "fi ", "for ", "in ", "do ", "done ",
  "while ", "until ", "case ", "esac ", "function ", "time ", "-p ",
  "{ ", "} ", "( ", ") ", "(( ", ")) ", "[[ ", "]] ",
  "; ", ";; ", ";& ", ";;& ", "| ", "|& ", "&& ", "|| ", "& ", "! ", "\n",
  "<<EOF\nbody\nEOF\n", "<<-'E'\n\tbody\n\tE\n", "<<<hi ", "{fd}>x ", "2>&1 ",
  "${x:-a} ", "${#x} ", "${!x[@]} ", "${x//a/b} ", "$(echo 1) ", "`echo 2` ",
  "$((1+2)) ", "$[3*4] ", "<(echo p) ", "$'\\x41\\n' ", '$"hi" ',
  "{a,b}{1..3} ", "a[1]=x ", "a=(1 2) ", "echo ", "'sq' ", '"dq" ', "# c\n",
  "\\\n", "=~ ", "== ", "-f ", "!(a|b) ", "~user/x ",
];

const SPECIAL_CHARS = "(){}[];|&<>\"'`$\\#\n\0";

test("deterministic seeded parser fuzz slice (>= 2500 inputs, 0 traps, < 50ms/input, < 3s total)", async () => {
  const next = makeRng(0x5345_4E54_4C33_465An);
  const shell = new Bash({
    executionLimitProfile: "hardened",
    executionLimits: { maxCommandCount: 50, maxLoopIterations: 20 },
  });

  const inputs = [];
  // 1500 grammar-mutated bash fragments
  for (let i = 0; i < 1500; i++) {
    const count = (next() % 14) + 1;
    let s = "";
    for (let j = 0; j < count; j++) {
      s += FRAGMENTS[next() % FRAGMENTS.length];
    }
    if (s.length > 0 && next() % 3 === 0) {
      const idx = next() % s.length;
      const ch = SPECIAL_CHARS[next() % SPECIAL_CHARS.length];
      s = s.slice(0, idx) + ch + s.slice(idx + 1);
    }
    inputs.push(s);
  }

  // 1000 arbitrary byte / UTF-8 / surrogate / deep-nesting edge slices
  for (let i = 0; i < 1000; i++) {
    const len = (next() % 72) + 1;
    let s = "";
    for (let j = 0; j < len; j++) {
      const mode = next() % 6;
      if (mode === 0) s += SPECIAL_CHARS[next() % SPECIAL_CHARS.length];
      else if (mode === 1) s += String.fromCharCode(next() & 0xff);
      else if (mode === 2) s += String.fromCharCode(0xd800 + (next() & 0x3ff)); // lone high surrogate
      else if (mode === 3) s += "😀";
      else s += String.fromCharCode(97 + (next() % 26));
    }
    if (i % 25 === 0) s = "(".repeat(205) + "a" + ")".repeat(205);
    else if (i % 25 === 1) s = "{ ".repeat(205) + "a;" + " }".repeat(205);
    else if (i % 25 === 2) s = "${".repeat(40) + "x";
    inputs.push(s);
  }

  assert.ok(inputs.length >= 2500);

  const t0 = performance.now();
  let maxMs = 0;
  for (let i = 0; i < inputs.length; i++) {
    const start = performance.now();
    const res = shell.execSync(inputs[i]);
    const dt = performance.now() - start;
    if (dt > maxMs) maxMs = dt;
    assert.equal(typeof res.exitCode, "number", `input ${i} returned invalid exitCode`);
    assert.ok(dt < 50, `input ${i} took ${dt.toFixed(2)}ms (>= 50ms): ${JSON.stringify(inputs[i].slice(0, 80))}`);
  }
  const totalMs = performance.now() - t0;
  assert.ok(totalMs < 3000, `fuzz slice took ${totalMs.toFixed(1)}ms (>= 3000ms)`);

  const final = await shell.exec("echo ok");
  assert.equal(final.exitCode, 0);
  assert.equal(final.stdout, "ok\n");
  assert.equal(final.stderr, "");
});
