import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, linkSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
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
  assert.match(r.stdout, /non-regular entry/);
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

function rawArchive(name, build, tarArgs = []) {
  const dir = scratch(name);
  const src = join(dir, "src");
  mkdirSync(join(src, "toolpkg"), { recursive: true });
  writeFileSync(join(src, "toolpkg", "tool"), "#!/bin/sh\n");
  build(src, dir);
  const archive = join(dir, "toolpkg.tar.gz");
  assert.equal(spawnSync("tar", ["-czf", archive, ...tarArgs, "-C", src, "."]).status, 0);
  return { dir, archive };
}

function fixtureFrom(archive, dir, extra = {}) {
  const pin = { name: "faketool", version: "1.0.0", url: pathToFileURL(archive).href, sha256: sha256File(archive), bin: "tool", ...extra };
  const pinsFile = join(dir, "pins.json");
  writeFileSync(pinsFile, JSON.stringify({ tools: [pin] }));
  const tools = join(dir, "tools");
  return { dir, pinsFile, tools, marker: join(tools, "faketool-1.0.0", ".verified") };
}

test("an archive holding a hard link, a fifo or a path with .. is refused before extraction", () => {
  const cases = {
    hardlink: (src) => linkSync(join(src, "toolpkg", "tool"), join(src, "toolpkg", "hard")),
    fifo: (src) => assert.equal(spawnSync("mkfifo", [join(src, "toolpkg", "fifo")]).status, 0),
  };
  for (const [name, build] of Object.entries(cases)) {
    const a = rawArchive(`fetch-${name}`, build);
    const f = fixtureFrom(a.archive, a.dir);
    const r = run(f);
    assert.notEqual(r.status, 0, name);
    assert.match(r.stdout, /non-regular entry/, name);
    assert.equal(existsSync(join(f.tools, "faketool-1.0.0")), false, `${name}: nothing extracted`);
  }
  const a = rawArchive("fetch-dotdot", () => {}, ["--transform", "s|^\\./toolpkg|../escape|"]);
  const f = fixtureFrom(a.archive, a.dir);
  const r = run(f);
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /unsafe path/);
  assert.equal(existsSync(join(a.dir, "tools", "escape")), false);
});

test("an archive with an absolute path is refused before extraction", () => {
  const a = rawArchive("fetch-abs", () => {}, ["-P", "--transform", "s|^\\./toolpkg|/abs-escape|"]);
  const f = fixtureFrom(a.archive, a.dir);
  const r = run(f);
  assert.notEqual(r.status, 0);
  assert.match(r.stdout, /unsafe path/);
  assert.equal(existsSync(f.marker), false);
});

test("a pin with an unsafe name, version or bin is refused even under the test hook", () => {
  const a = rawArchive("fetch-pin", () => {});
  for (const bad of [{ name: "../evil" }, { version: "1/../2" }, { bin: "../tool" }, { bin: "/bin/sh" }]) {
    const f = fixtureFrom(a.archive, a.dir, bad);
    const r = run(f);
    assert.notEqual(r.status, 0, JSON.stringify(bad));
    assert.match(r.stdout, /unsafe/);
  }
});

test("build.mjs scrubs SENTROPIC_PINS before it verifies the tools", () => {
  const a = rawArchive("fetch-scrub", () => {});
  const f = fixtureFrom(a.archive, a.dir);
  // The ambient pin file names an unverified tool in an empty tools dir; if it leaked into the offline
  // verification the build would stop, but the real pins are verified in the real tools dir.
  const r = spawnSync("node", [join(repoRoot, "tools", "build.mjs"), "--check-bindings"], {
    encoding: "utf8",
    env: { ...process.env, SENTROPIC_PINS: f.pinsFile },
  });
  assert.equal(r.status, 0, r.stdout + r.stderr);
});
