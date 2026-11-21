// Gates 17, 20 and 1 (contract), 21 (types) of lot L1b.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { repoRoot, scratch } from "./helpers.mjs";
import { toolsDir } from "../tools/lib.mjs";
import { g1ZeroImport, g2NoBareImport, g4ComponentValidity } from "../tools/gates.mjs";
import { INTERNAL, module } from "./kt.mjs";
import { instantiate } from "../dist/test/js/kernel-test/kernel-test.js";
import { shims } from "../dist/test/js/kernel-test/kernel-test.shims.js";

test("gate 17: a default Bash never touches the host disk", () => {
  const dir = scratch("sentinel");
  const sentinel = join(dir, "sentinel.txt");
  const r = spawnSync("node", [join(repoRoot, "test", "fixtures", "host-disk-child.mjs"), sentinel], { cwd: repoRoot, encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr);
  const got = JSON.parse(r.stdout);
  assert.equal(got.exists, false);
  assert.equal(got.read, "rejected");
  assert.equal(got.exit, 127, "the release kernel has no test command");
  assert.equal(got.calls.length, 1, JSON.stringify(got.calls));
  assert.equal(got.calls[0][0], "readFileSync");
  assert.match(got.calls[0][1], /dist\/js\/kernel\/kernel\.core\.wasm$/);
});

test("gate 20: the staging queue replays construction, registerCommand and synchronous writes", async () => {
  const browser = await import("../host/browser.js");
  const { Bash, defineCommand } = browser;
  const t0 = performance.now();
  const provider = () => new Promise((r) => setTimeout(() => r(module), 50));
  const hello = defineCommand("hello", async () => ({ stdout: "hello\n", stderr: "", exitCode: 0 }));
  const later = defineCommand("later", async () => ({ stdout: "later\n", stderr: "", exitCode: 0 }));
  const b = new Bash({
    cwd: "/work",
    env: { A: "1" },
    files: { "/work/a.txt": "from files" },
    customCommands: [hello],
    [INTERNAL]: { kernel: { instantiate, shims }, moduleProvider: provider },
  });
  b.registerCommand(later);
  b.fs.writeFileSync("/work/b.txt", "from sync write");
  b.fs.mkdirSync("/work/d", { recursive: true });
  assert.equal(b.getCwd(), "/work");
  assert.equal(b.getEnv().A, "1");
  assert.equal(Object.keys(b.getEnv())[10], "A");
  assert.throws(() => b.fs.getAllPaths(), TypeError);
  assert.throws(() => b.execSync(""), { name: "AsyncCapabilityError" });
  assert.equal(b.fs.resolvePath("/a", "../b"), "/b");
  const r = await b.exec("hello\nlater\n__cat /work/a.txt\n__cat /work/b.txt");
  assert.ok(performance.now() - t0 >= 45, "the module resolved after about 50 ms");
  assert.equal(r.stdout, "hello\nlater\nfrom filesfrom sync write");
  assert.equal(r.exitCode, 0);
  assert.equal(await b.fs.exists("/work/d"), true);
  assert.ok(b.fs.getAllPaths().includes("/work/b.txt"));
});

test("gate 20: the browser entry's import graph has no node: import (G2)", () => {
  const ctx = { root: repoRoot, out: join(repoRoot, "dist"), toolsDir: toolsDir() };
  assert.equal(g2NoBareImport(ctx), null);
});

test("browser compilation retries after failure and accepts wasm served with another MIME type", async () => {
  const { Bash } = await import("../host/browser.js?precompile-retry");
  const b = new Bash({ files: { "/staged": "replayed" } });
  let fetched = 0;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    fetched++;
    return new Response("invalid wasm", { headers: { "Content-Type": "application/wasm" } });
  };
  try {
    await assert.rejects(b.readFile("/staged"), WebAssembly.CompileError);
    assert.equal(fetched, 1);
    const core = readFileSync(new URL("../dist/js/kernel/kernel.core.wasm", import.meta.url));
    const good = new Response(core, { headers: { "Content-Type": "Application/Wasm" } });
    await Bash.precompile(Promise.resolve(good));
    assert.equal(await b.readFile("/staged"), "replayed");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("gate 1: both components have zero imports and conform to the committed WIT", () => {
  const ctx = { root: repoRoot, out: join(repoRoot, "dist"), toolsDir: toolsDir() };
  assert.ok(existsSync(join(ctx.out, "kernel.component.wasm")));
  assert.ok(existsSync(join(ctx.out, "test", "kernel-test.component.wasm")));
  assert.equal(g1ZeroImport(ctx), null);
  assert.equal(g4ComponentValidity(ctx), null);
});

test("gate 1: the committed bindings match a regeneration", () => {
  const r = spawnSync("node", [join(repoRoot, "tools", "build.mjs"), "--check-bindings", "--component", "kernel"], { cwd: repoRoot, encoding: "utf8" });
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /bindings: OK/);
});

test("gate 21: tsc --noEmit passes on types/ and on a consumer using every declaration", () => {
  const tsc = join(toolsDir(), "npm", "node_modules", ".bin", "tsc");
  assert.ok(existsSync(tsc), "the type check fails closed when tsc is absent");
  const run = (args) => spawnSync(tsc, args, { cwd: repoRoot, encoding: "utf8" });
  const own = run(["--noEmit", "--strict", "--skipLibCheck", "false", "--target", "es2022", "--module", "esnext", "--moduleResolution", "bundler", "types/index.d.ts", "types/browser.d.ts"]);
  assert.equal(own.status, 0, own.stdout + own.stderr);
  const consumer = run(["--noEmit", "-p", "test/fixtures/consumer/tsconfig.json"]);
  assert.equal(consumer.status, 0, consumer.stdout + consumer.stderr);
  // The check is real: a wrong use must fail.
  const bad = run(["--noEmit", "-p", "test/fixtures/consumer-bad/tsconfig.json"]);
  assert.notEqual(bad.status, 0, "a misuse must be rejected");
});

test("the limit tables of the kernel and of the facade are the same", async () => {
  const rs = readFileSync(join(repoRoot, "kernel", "src", "limits.rs"), "utf8");
  const MIB = 1024 * 1024;
  const rustRows = [...rs.matchAll(/^\s*L\("(\w+)", \[([^\]]+)\]\),$/gm)].map((m) => [m[1], m[2].split(",").map((x) => Function("MIB", `return ${x}`)(MIB))]);
  const { tableRows, PROFILES } = await import("../host/core/limits.js");
  assert.deepEqual(tableRows(), rustRows);
  const prof = [...rs.matchAll(/ProfileData \{ headroom: (\d+) \* MIB, ceiling: (\d+) \* MIB, oneshot: (\d+) \* MIB \}/g)].map((m) => ({ headroom: m[1] * MIB, ceiling: m[2] * MIB, oneshot: m[3] * MIB }));
  assert.deepEqual([PROFILES.micro, PROFILES.contain, PROFILES.hardened, PROFILES.normal], prof);
});
