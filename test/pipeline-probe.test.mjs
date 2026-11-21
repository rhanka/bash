import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { instantiate } from "../dist/test/js/probe/probe.js";

const module = new WebAssembly.Module(readFileSync(new URL("../dist/test/js/probe/probe.core.wasm", import.meta.url)));
const pipelineProbe = () => instantiate(() => module, {});

test("the probe loads on demand and instantiates with an empty import object", async () => {
  const probe = await pipelineProbe();
  assert.equal(typeof probe.ping, "function");
});

test("ping adds one", async () => {
  assert.equal((await pipelineProbe()).ping(41), 42);
});

test("echoBytes round-trips", async () => {
  const probe = await pipelineProbe();
  const everyByte = Uint8Array.from({ length: 256 }, (_, i) => i);
  const big = new Uint8Array(70000).map((_, i) => (i * 31 + 7) & 0xff);
  for (const data of [new Uint8Array(0), Uint8Array.of(0xab), big, everyByte]) {
    assert.deepEqual(probe.echoBytes(data), data);
  }
});

test("views are refreshed after the memory grows", async () => {
  const probe = await pipelineProbe();
  const big = new Uint8Array(70000).map((_, i) => (i * 13 + 5) & 0xff);
  assert.deepEqual(probe.echoBytes(big), big);
  const before = probe.grow(0);
  const after = probe.grow(16);
  assert.equal(after, before + 16);
  assert.deepEqual(probe.echoBytes(big), big);
});
