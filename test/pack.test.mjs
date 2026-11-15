import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { repoRoot, scratch } from "./helpers.mjs";

const EXPECTED = [
  "DIVERGENCES.md",
  "LICENSE",
  "README.md",
  "dist/js/kernel/interfaces/exports/sentropic-bash-engine.d.ts",
  "dist/js/kernel/interfaces/exports/sentropic-bash-types.d.ts",
  "dist/js/kernel/interfaces/sentropic-bash-engine.d.ts",
  "dist/js/kernel/interfaces/sentropic-bash-types.d.ts",
  "dist/js/kernel/kernel.core.wasm",
  "dist/js/kernel/kernel.d.ts",
  "dist/js/kernel/kernel.js",
  "dist/js/kernel/kernel.shims.js",
  "dist/js/probe/probe.core.wasm",
  "dist/js/probe/probe.d.ts",
  "dist/js/probe/probe.js",
  "dist/kernel.component.wasm",
  "dist/probe.component.wasm",
  "host/browser-probe.js",
  "host/browser.js",
  "host/core/bytes.js",
  "host/core/driver.js",
  "host/core/errors.js",
  "host/core/facade.js",
  "host/core/fs.js",
  "host/core/holder.js",
  "host/core/instance.js",
  "host/core/limits.js",
  "host/core/membrane.js",
  "host/core/paths.js",
  "host/index.js",
  "host/node/load.js",
  "host/node/probe.js",
  "package.json",
  "types/browser.d.ts",
  "types/core.d.ts",
  "types/index.d.ts",
  "types/probe.d.ts",
  "wit/sentropic-bash.wit",
];

test("the tarball holds exactly the expected files and installs offline", () => {
  const dest = scratch("pack");
  const p = spawnSync("npm", ["pack", "--json", "--ignore-scripts", "--pack-destination", dest], { cwd: repoRoot, encoding: "utf8" });
  assert.equal(p.status, 0, p.stderr);
  const info = JSON.parse(p.stdout)[0];
  assert.deepEqual(info.files.map((f) => f.path).sort(), EXPECTED);

  const tarball = join(dest, info.filename);
  const x = spawnSync("tar", ["-xzOf", tarball, "package/package.json"], { encoding: "utf8" });
  const packed = JSON.parse(x.stdout);
  for (const k of ["dependencies", "peerDependencies", "optionalDependencies"]) assert.deepEqual(Object.keys(packed[k] ?? {}), []);
  for (const s of ["preinstall", "install", "postinstall", "prepare"]) assert.equal(packed.scripts?.[s], undefined);

  const consumer = join(dest, "consumer");
  mkdirSync(consumer);
  writeFileSync(join(consumer, "package.json"), JSON.stringify({ name: "consumer", version: "0.0.0", private: true, type: "module" }));
  const i = spawnSync("npm", ["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarball], { cwd: consumer, encoding: "utf8" });
  assert.equal(i.status, 0, i.stderr);
  writeFileSync(
    join(consumer, "main.mjs"),
    'import { Bash, pipelineProbe } from "@sentropic/bash";\nconsole.log(pipelineProbe().ping(1));\nconst r = await new Bash().exec("");\nconsole.log(r.exitCode, JSON.stringify(r.stdout), JSON.stringify(r.stderr));\n',
  );
  const r = spawnSync("node", ["main.mjs"], { cwd: consumer, encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(r.stdout.trim().split("\n"), ["2", '0 "" ""']);
  assert.ok(readFileSync(join(consumer, "node_modules", "@sentropic", "bash", "package.json")));
});
