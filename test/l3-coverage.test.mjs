import assert from "node:assert/strict";
import { test } from "node:test";
import { Instance } from "../host/core/instance.js";
import { enc, internal, module } from "./kt.mjs";

test("L3 VM emits bash:cmd:* coverage hits in execution order for all 8 executed command kinds", () => {
  const { engine } = new Instance(module, internal());
  engine.configure(256n << 20n);
  const names = engine.coverageFeatures();
  const fs = engine.Vfs.create(128n << 20n);
  const shell = engine.Shell.create(fs, {
    limits: { profile: "contain", overrides: [] },
    grants: {},
    customCommands: [],
    env: [],
    defaultLayout: false,
    trace: false,
    coverage: true,
  });

  const script = [
    "f() { echo hi; }",
    "if true; then",
    "  for x in a; do",
    "    while false; do echo w; done",
    "    until true; do echo u; done",
    "    ( { f; } )",
    "  done",
    "fi",
  ].join("\n");

  const fiber = engine.Fiber.spawn(shell, undefined, {
    script: enc(script),
    args: [],
    env: [],
    replaceEnv: false,
    rawScript: false,
  });
  let step;
  do {
    step = fiber.run(10_000n);
  } while (step.tag === "out-of-fuel");
  assert.equal(step.tag, "done");
  assert.equal(step.val.exitCode, 0);

  const hits = [];
  for (;;) {
    const chunk = fiber.takeCoverage(64);
    if (chunk.length === 0) break;
    for (const id of chunk) hits.push(names[id]);
  }
  fiber[Symbol.dispose]();

  const cmdHits = hits.filter((h) => h.startsWith("bash:cmd:"));
  assert.deepEqual(cmdHits, [
    "bash:cmd:FunctionDef",
    "bash:cmd:If",
    "bash:cmd:SimpleCommand", // `true`
    "bash:cmd:For",
    "bash:cmd:While",
    "bash:cmd:SimpleCommand", // `false`
    "bash:cmd:Until",
    "bash:cmd:SimpleCommand", // `true`
    "bash:cmd:Subshell",
    "bash:cmd:Group",
    "bash:cmd:SimpleCommand", // `f`
    "bash:cmd:Group",         // body of `f`
    "bash:cmd:SimpleCommand", // `echo hi`
  ]);
});
