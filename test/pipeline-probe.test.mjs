import { test } from "node:test";
import assert from "node:assert/strict";
import { pipelineProbe } from "@sentropic/bash";

test("the probe instantiates synchronously with an empty import object", () => {
  const probe = pipelineProbe();
  assert.equal(typeof probe.ping, "function");
});

test("ping adds one", () => {
  assert.equal(pipelineProbe().ping(41), 42);
});

test("echoBytes round-trips", () => {
  const probe = pipelineProbe();
  const everyByte = Uint8Array.from({ length: 256 }, (_, i) => i);
  const big = new Uint8Array(70000).map((_, i) => (i * 31 + 7) & 0xff);
  for (const data of [new Uint8Array(0), Uint8Array.of(0xab), big, everyByte]) {
    assert.deepEqual(probe.echoBytes(data), data);
  }
});

test("views are refreshed after the memory grows", () => {
  const probe = pipelineProbe();
  const big = new Uint8Array(70000).map((_, i) => (i * 13 + 5) & 0xff);
  assert.deepEqual(probe.echoBytes(big), big);
  const before = probe.grow(0);
  const after = probe.grow(16);
  assert.equal(after, before + 16);
  assert.deepEqual(probe.echoBytes(big), big);
});
