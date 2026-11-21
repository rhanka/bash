// Exit gates 1-9 and 14 of lot L1a-2: the wasm allocator, driven from node on the real wasm build.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { repoRoot } from "./helpers.mjs";
import { assertUnderTime } from "./timing.mjs";
import { buildMemModule, instantiate } from "./memwasm.mjs";
import { walk } from "../tools/lib.mjs";

const module = buildMemModule();
const GIB = 1 << 30;
const fresh = (ceiling = GIB) => {
  const x = instantiate(module).exports;
  assert.equal(x.t_configure(ceiling), 0);
  return x;
};
const NAMES = { 1: "W1 uniform", 2: "W2 bimodal", 3: "W3 growing buffers", 4: "W4 sawtooth" };
const ratios = {};

test("gates 1 and 2: footprint bound and full return, per workload", () => {
  for (const w of [1, 2, 3, 4]) {
    const x = fresh();
    const base = x.t_used();
    assert.equal(x.t_run(w, 0), 0, `${NAMES[w]} ran without a refusal`);
    const peak = x.t_peak();
    const hw = x.t_footprint_hw();
    const ratio = hw / peak;
    ratios[w] = ratio;
    console.log(`# ${NAMES[w]}: peak live ${peak} B, footprint high water ${hw} B, ratio ${ratio.toFixed(3)}`);
    assert.ok(peak >= 8 << 20, `${NAMES[w]} peak ${peak} is at least 8 MiB`);
    if (w === 3) assert.equal(peak, 8 * (8 << 20), "all eight buffers reach exactly 8 MiB");
    if (w === 4) assert.ok(ratio <= 2.0, `W4 ratio ${ratio}`);
    else assert.ok(ratio <= 1.25, `${NAMES[w]} ratio ${ratio} must be at most 1.25`);
    // Gate 2.
    assert.equal(x.t_used(), base, `${NAMES[w]}: usage back to its start value`);
    assert.equal(x.t_full_return(), 1, `${NAMES[w]}: 99% of the footprint minus the reserve fits without growth`);
  }
});

test("gate 3: reuse, ten runs of W1 leave the footprint high water unchanged after the first", () => {
  const x = fresh();
  assert.equal(x.t_run(1, 1), 0);
  const first = x.t_footprint_hw();
  for (let seed = 2; seed <= 10; seed++) assert.equal(x.t_run(1, seed), 0);
  console.log(`# W1 x10: footprint high water ${first} -> ${x.t_footprint_hw()}`);
  assert.equal(x.t_footprint_hw(), first);
});

test("gate 4: the ceiling bounds the footprint", () => {
  const C = 32 << 20;
  const x = fresh(C);
  assert.equal(x.t_run(1, 0), 0, "W1 fits");
  assert.ok(x.t_footprint_hw() <= C);
  assert.notEqual(x.t_run(2, 0), 0, "W2 needs more than C and is refused, not trapped");
  assert.ok(x.t_footprint_hw() <= C, `footprint ${x.t_footprint_hw()} <= ${C}`);
  assert.equal(x.t_used(), 0, "a refused workload frees what it got");
  const n = x.t_fill(4096);
  assert.ok(n > 1000);
  assert.ok(x.t_footprint() <= C);
  assert.equal(x.t_alloc(4096, 8), 0, "the allocation that needs growth beyond C returns null");
  assert.equal(x.t_raise(2 * C), 0);
  assert.notEqual(x.t_alloc(4096, 8), 0, "after raise_ceiling it succeeds");
  assert.ok(x.t_footprint() <= 2 * C);
});

test("gate 5: null, never trap", () => {
  const x = fresh(0xffffffff);
  assert.equal(x.t_alloc(0x7fffffff, 8), 0, "usize::MAX / 2 bytes");
  assert.equal(x.t_alloc(0xffffffff, 8), 0);
  assert.equal(x.t_alloc(8, 32), 0, "alignment 32");
  // Reach the engine's memory maximum (1 GiB in this build): requests are refused without a trap.
  const held = [];
  for (;;) {
    const p = x.t_alloc(128 << 20, 8);
    if (p === 0) break;
    held.push(p);
    assert.ok(held.length < 100, "the engine maximum must stop the loop");
  }
  assert.ok(held.length >= 4, `got ${held.length} blocks of 128 MiB`);
  assert.ok(x.t_footprint() <= GIB);
  assert.equal(x.t_alloc(128 << 20, 8), 0);
  for (const p of held) x.t_free(p, 128 << 20, 8);
  assert.notEqual(x.t_alloc(100, 8), 0, "still usable");
  assert.notEqual(x.t_alloc(64 << 20, 8), 0, "and the freed memory is reusable");
});

test("gate 5: the final wasm32 page is usable with arithmetic checks enabled", () => {
  const x = instantiate(buildMemModule({ addressSpaceProbe: true })).exports;
  assert.equal(x.t_configure(0xffffffff), 0);
  const held = [];
  for (const size of [128 << 20, 1 << 20, 32 << 10]) {
    for (;;) {
      const p = x.t_alloc(size, 16) >>> 0;
      if (p === 0) break;
      held.push([p, size]);
      assert.ok(held.length < 512);
    }
  }
  assert.equal(x.memory.buffer.byteLength, 2 ** 32);
  assert.equal(x.t_footprint() >>> 0, x.t_footprint_hw() >>> 0);
  assert.equal(x.t_alloc(32 << 10, 16), 0, "one more growth is refused");
  for (const [p, size] of held) x.t_free(p, size, 16);
  assert.equal(x.t_used(), 0);
  const p = x.t_alloc(4096, 16);
  assert.notEqual(p, 0, "top-page coalescing leaves the heap reusable");
  x.t_free(p, 4096, 16);
});

test("gate 6: alignment and content preservation", () => {
  const x = fresh();
  for (const align of [1, 2, 4, 8, 16]) {
    for (const size of [1, 3, 16, 100, 5000]) {
      const p = x.t_alloc(size, align);
      assert.notEqual(p, 0);
      assert.equal(p % align, 0, `align ${align} size ${size}`);
      x.t_free(p, size, align);
    }
  }
  const sizes = [1, 15, 16, 17, 4095, 4096, 65536, 1048576];
  const mem = () => new Uint8Array(x.memory.buffer);
  for (const from of sizes) {
    for (const to of sizes) {
      const p = x.t_alloc(from, 8);
      assert.notEqual(p, 0);
      mem().fill((from * 7) & 255, p, p + from);
      mem()[p] = 0xab;
      const q = x.t_realloc(p, from, 8, to);
      assert.notEqual(q, 0, `${from} -> ${to}`);
      assert.equal(q % 8, 0);
      const keep = Math.min(from, to);
      const m = mem();
      assert.equal(m[q], 0xab);
      for (let i = 1; i < keep; i += Math.max(1, keep >> 8)) assert.equal(m[q + i], (from * 7) & 255, `${from} -> ${to} at ${i}`);
      assert.equal(m[q + keep - 1], keep === 1 ? 0xab : (from * 7) & 255);
      x.t_free(q, to, 8);
    }
  }
  assert.equal(x.t_used(), 0);
});

test("gate 7: the reserve serves requests inside reserve_scope only", () => {
  const x = fresh(4 << 20);
  assert.ok(x.t_fill(1024) > 3000, "the heap fills up to the ceiling");
  assert.equal(x.t_alloc(4096, 8), 0, "an ordinary 4 KiB request is refused");
  // bits: 0 refused, 1 served in scope, 2 nested served, 3 served after nested returned, 4 refused again
  assert.equal(x.t_reserve_probe(4096), 0b11111);
  assert.ok(x.t_footprint() <= 4 << 20);
});

test("gate 7: ordinary allocations cannot fragment the reserve", () => {
  const ceiling = 8 * 65536;
  const x = fresh(ceiling);
  const small = Array.from({ length: 4096 }, () => x.t_alloc(16, 8));
  assert.ok(small.every((p) => p !== 0));
  for (let i = 0; i < small.length; i += 2) x.t_free(small[i], 16, 8);
  assert.ok(x.t_fill(4096) > 0);
  assert.equal(x.t_footprint(), ceiling);
  for (let i = 0; i < 3; i++) {
    assert.equal(x.t_alloc(4096, 8), 0);
    assert.equal(x.t_reserve_probe(4096), 0b11111, `scope ${i}, with nesting`);
    assert.equal(x.t_footprint(), ceiling);
  }
  x.t_unfill();
  for (let i = 1; i < small.length; i += 2) x.t_free(small[i], 16, 8);
  assert.equal(x.t_used(), 0);
  assert.equal(x.t_full_return(), 1);
});

test("test allocation hooks refuse empty, invalid and exhausted requests safely", () => {
  const x = fresh(2 * 65536);
  assert.equal(x.t_full_return(), 0, "an empty heap has nothing to return");
  assert.equal(x.t_alloc(0, 8), 0);
  assert.equal(x.t_pairs(1, 0), 0);
  assert.equal(x.t_pairs(1, 0xffffffff), 0);
  assert.equal(x.t_fill(0), 0);
  assert.equal(x.t_reserve_probe(0), 0);
  x.t_free(0, 64, 8);
  assert.equal(x.t_realloc(0, 64, 8, 128), 0);
  const p = x.t_alloc(64, 8);
  assert.notEqual(p, 0);
  new Uint8Array(x.memory.buffer)[p] = 42;
  assert.equal(x.t_realloc(p, 64, 8, 0), 0);
  assert.equal(x.t_realloc(p, 0, 8, 128), 0);
  assert.equal(x.t_realloc(p, 64, 3, 128), 0);
  assert.equal(x.t_realloc(p, 64, 8, 0xffffffff), 0);
  assert.equal(new Uint8Array(x.memory.buffer)[p], 42);
  assert.equal(x.t_used(), 64, "the rejected reallocations leave the block live");
  assert.ok(x.t_fill(1024) > 0);
  assert.equal(x.t_pairs(1, 4096), 0);
  x.t_unfill();
  x.t_free(p, 64, 8);
  assert.equal(x.t_used(), 0);
});

test("gate 8: headroom accounting", () => {
  const x = fresh(1 << 24);
  assert.equal(x.t_headroom_cap(), (1 << 24) / 8, "default cap is one eighth of the ceiling");
  assert.equal(x.t_headroom_used(), 0);
  const p = x.t_cabi(0, 0, 8, 1000);
  assert.notEqual(p, 0);
  assert.equal(x.t_headroom_used(), 1000);
  const q = x.t_cabi(p, 1000, 8, 3000);
  assert.equal(x.t_headroom_used(), 3000);
  const r = x.t_cabi(q, 3000, 8, 500);
  assert.equal(x.t_headroom_used(), 500);
  assert.equal(x.t_set_headroom(100), 1, "below current use is refused");
  assert.equal(x.t_headroom_cap(), (1 << 24) / 8, "and changes nothing");
  assert.equal(x.t_set_headroom(10000), 0);
  assert.equal(x.t_headroom_cap(), 10000);
  x.t_transfer_free(r, 500, 8);
  assert.equal(x.t_headroom_used(), 0, "released to exactly its previous value");
  assert.equal(x.t_cabi(0, 0, 8, 0), 8, "zero length returns the alignment");
  assert.throws(() => x.t_cabi(0, 0, 8, 20000), WebAssembly.RuntimeError, "exceeding the account traps");
});

test("gate 9: speed", () => {
  const best = (f) => {
    let min = Infinity;
    for (let i = 0; i < 5; i++) {
      const x = fresh();
      const t = process.hrtime.bigint();
      f(x);
      min = Math.min(min, Number(process.hrtime.bigint() - t) / 1e6);
    }
    return min;
  };
  const w1 = best((x) => assert.equal(x.t_run(1, 0), 0));
  const pairs = best((x) => x.t_pairs(1_000_000, 64));
  assertUnderTime("W1 uniform", w1, 50);
  assertUnderTime("1 000 000 alloc/free pairs of 64 B", pairs, 100);
});

test("gate 14: memory.grow appears only in the allocator file", () => {
  const root = join(repoRoot, "kernel", "src");
  const where = walk(root, (p) => p.endsWith(".rs"))
    .filter((f) => /memory_grow|memory\.grow/.test(readFileSync(f, "utf8")))
    .map((f) => relative(root, f));
  assert.deepEqual(where, ["mem/heap.rs"]);
});
