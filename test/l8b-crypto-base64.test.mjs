import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { bash } from "./kt.mjs";
import { latin1FromBytes as hostLatin1 } from "../host/index.js";

test("hash __hash md5/sha1/sha256 match node:crypto including 1 MiB payload", async () => {
  const sh = bash({ executionLimitProfile: "normal" });

  const cases = [
    Buffer.from(""),
    Buffer.from("a"),
    Buffer.from("abc"),
    Buffer.from("a".repeat(56)),
    Buffer.from("a".repeat(112)),
    Buffer.from(Array.from({ length: 256 }, (_, i) => i)),
    Buffer.from("a".repeat(1_000_000)),
  ];

  for (const data of cases) {
    const stdin = hostLatin1(data);
    for (const [cmd, nodeAlg] of [["md5", "md5"], ["sha1", "sha1"], ["sha256", "sha256"]]) {
      const expected = createHash(nodeAlg).update(data).digest("hex") + "  -\n";
      const r = await sh.exec(`__hash ${cmd} -`, { stdin, stdinKind: "bytes" });
      assert.equal(r.exitCode, 0);
      assert.equal(r.stderr, "");
      assert.equal(r.stdout, expected);
    }
  }
});

test("base64 RFC 4648 vectors, wrap columns, strict vs permissive, and 1 MiB round-trip", async () => {
  const sh = bash({ executionLimitProfile: "normal" });

  const rfc = [
    ["", ""],
    ["f", "Zg=="],
    ["fo", "Zm8="],
    ["foo", "Zm9v"],
    ["foob", "Zm9vYg=="],
    ["fooba", "Zm9vYmE="],
    ["foobar", "Zm9vYmFy"],
  ];
  for (const [plain, b64] of rfc) {
    const enc = await sh.exec("__base64 encode 0", { stdin: plain });
    assert.equal(enc.exitCode, 0);
    assert.equal(enc.stdoutKind, undefined);
    assert.equal(enc.stdout, b64);

    const dec = await sh.exec("__base64 decode-strict", { stdin: b64 });
    assert.equal(dec.exitCode, 0);
    assert.equal(dec.stdoutKind, "bytes");
    assert.equal(dec.stdout, plain);
  }

  // Wrap cols
  const w16 = await sh.exec("__base64 encode 16", { stdin: "a".repeat(20) });
  assert.equal(w16.exitCode, 0);
  assert.equal(w16.stdout, "YWFhYWFhYWFhYWFh\nYWFhYWFhYWE=\n");

  // Permissive vs strict
  const perm = await sh.exec("__base64 decode", { stdin: "YW!Jj" });
  assert.equal(perm.exitCode, 0);
  assert.equal(perm.stdout, "abc");

  const strictBad = await sh.exec("__base64 decode-strict", { stdin: "YW!Jj" });
  assert.equal(strictBad.exitCode, 1);
  assert.equal(strictBad.stderr, "base64: invalid input\n");

  // 1 MiB binary round-trip
  const raw = Buffer.alloc(1 << 20);
  for (let i = 0; i < raw.length; i++) raw[i] = (i * 31 + 7) & 0xff;
  const encBig = await sh.exec("__base64 encode 76", { stdin: hostLatin1(raw), stdinKind: "bytes" });
  assert.equal(encBig.exitCode, 0);
  const decBig = await sh.exec("__base64 decode-strict", { stdin: encBig.stdout });
  assert.equal(decBig.exitCode, 0);
  assert.equal(decBig.stdoutKind, "bytes");
  assert.equal(decBig.stdout, hostLatin1(raw));
});
