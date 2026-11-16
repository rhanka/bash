import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { repoRoot, scratch } from "./helpers.mjs";
import { sha256File } from "../tools/lib.mjs";

const script = join(repoRoot, "tools", "fetch-tools.mjs");

function fixture(hashOverride, { symlink = false, url } = {}) {
  const dir = scratch("fetch");
  const src = join(dir, "src", "toolpkg");
  mkdirSync(src, { recursive: true });
  writeFileSync(join(src, "tool"), "#!/bin/sh\n");
  if (symlink) symlinkSync("/etc/passwd", join(src, "link"));
  const archive = join(dir, "toolpkg.tar.gz");
  const t = spawnSync("tar", ["-czf", archive, "-C", join(dir, "src"), "toolpkg"]);
  assert.equal(t.status, 0);
  const pin = { name: "faketool", version: "1.0.0", url: url ?? pathToFileURL(archive).href, sha256: hashOverride ?? sha256File(archive), bin: "tool" };
  const pinsFile = join(dir, "pins.json");
  writeFileSync(pinsFile, JSON.stringify({ tools: [pin] }));
  const tools = join(dir, "tools");
  return { dir, pinsFile, tools, marker: join(tools, "faketool-1.0.0", ".verified") };
}

const run = (f, ...args) =>
  spawnSync("node", [script, "--skip-npm", ...args], {
    encoding: "utf8",
    env: { ...process.env, SENTROPIC_PINS: f.pinsFile, SENTROPIC_TOOLS_DIR: f.tools },
  });

test("an altered pin hash fails and leaves no marker", () => {
  const f = fixture("f".repeat(64));
  const r = run(f);
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /hash mismatch/);
  assert.equal(existsSync(f.marker), false);
});

test("--offline with a missing tool fails", () => {
  const f = fixture();
  const r = run(f, "--offline");
  assert.notEqual(r.status, 0);
  assert.equal(existsSync(f.marker), false);
});

test("a matching hash is verified, and then --offline succeeds", () => {
  const f = fixture();
  assert.equal(run(f).status, 0);
  assert.equal(existsSync(f.marker), true);
  assert.equal(run(f, "--offline").status, 0);
});

test("an archive holding a symbolic link is refused and leaves no marker", () => {
  const f = fixture(undefined, { symlink: true });
  const r = run(f);
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /symbolic links/);
  assert.equal(existsSync(f.marker), false);
});

test("a tampered cached binary is not trusted by --offline", () => {
  const f = fixture();
  assert.equal(run(f).status, 0);
  writeFileSync(join(f.tools, "faketool-1.0.0", "tool"), "#!/bin/sh\necho tampered\n");
  const r = run(f, "--offline");
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /does not match the hash recorded/);
});

test("a committed pin that is not https is refused outside the test hook", () => {
  const f = fixture(undefined, { url: "http://example.invalid/x.tar.gz" });
  // SENTROPIC_PINS is the test hook; without it the pins file is the repository one, so exercise the
  // https rule through the per-pin check: an http URL is refused even with the hook.
  const r = run(f);
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /url is not https/);
});
