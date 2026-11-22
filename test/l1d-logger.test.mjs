import assert from "node:assert/strict";
import { test } from "node:test";
import { Bash, defineCommand } from "../host/index.js";

function makeLogger() {
  const calls = [];
  const logger = {
    info(message, data) {
      calls.push(["info", message, data]);
    },
    debug(message, data) {
      calls.push(["debug", message, data]);
    },
  };
  return { calls, logger };
}

test("logger follows Annex D.1 sequence on exec and execSync", async () => {
  await Bash.precompile();
  for (const sync of [false, true]) {
    const run = async (cmd, options, execOpts) => {
      const { calls, logger } = makeLogger();
      const bash = new Bash({ ...options, logger });
      const result = sync ? bash.execSync(cmd, execOpts) : await bash.exec(cmd, execOpts);
      return { calls, result };
    };

    for (const blank of ["", "   \n\t\r ", "\u00a0"]) {
      const { calls, result } = await run(blank);
      assert.equal(result.exitCode, 0);
      assert.deepEqual(calls, [], `blank script ${JSON.stringify(blank)} (${sync ? "sync" : "async"})`);
    }

    const comment = await run("# comment\n");
    assert.equal(comment.result.exitCode, 0);
    assert.deepEqual(comment.calls, [
      ["info", "exec", { command: "# comment\n" }],
      ["info", "exit", { exitCode: 0 }],
    ]);

    const stdoutOnly = await run("echo out");
    assert.deepEqual(stdoutOnly.calls, [
      ["info", "exec", { command: "echo out" }],
      ["debug", "stdout", { output: "out\n" }],
      ["info", "exit", { exitCode: 0 }],
    ]);

    const stderrOnly = await run("nosuchcmd");
    assert.equal(stderrOnly.result.exitCode, 127);
    assert.deepEqual(stderrOnly.calls, [
      ["info", "exec", { command: "nosuchcmd" }],
      ["info", "stderr", { output: "bash: nosuchcmd: command not found\n" }],
      ["info", "exit", { exitCode: 127 }],
    ]);

    const both = await run("echo ok; nosuchcmd");
    assert.equal(both.result.exitCode, 127);
    assert.deepEqual(both.calls, [
      ["info", "exec", { command: "echo ok; nosuchcmd" }],
      ["debug", "stdout", { output: "ok\n" }],
      ["info", "stderr", { output: "bash: nosuchcmd: command not found\n" }],
      ["info", "exit", { exitCode: 127 }],
    ]);

    const syntaxErr = await run("'");
    assert.equal(syntaxErr.result.exitCode, 2);
    assert.deepEqual(syntaxErr.calls, [
      ["info", "exec", { command: "'" }],
      ["info", "stderr", { output: syntaxErr.result.stderr }],
      ["info", "exit", { exitCode: 2 }],
    ]);

    const srcLimit = await run("echo 123456", { executionLimits: { maxSourceBytes: 5 } });
    assert.equal(srcLimit.result.exitCode, 126);
    assert.deepEqual(srcLimit.calls, [
      ["info", "exec", { command: "echo 123456" }],
      ["info", "stderr", { output: srcLimit.result.stderr }],
      ["info", "exit", { exitCode: 126 }],
    ]);

    const cmdLimit = await run("echo 1; echo 2", { executionLimits: { maxCommandCount: 1 } });
    assert.equal(cmdLimit.result.exitCode, 126);
    assert.deepEqual(cmdLimit.calls, [
      ["info", "exec", { command: "echo 1; echo 2" }],
      ["debug", "stdout", { output: "1\n" }],
      ["info", "stderr", { output: cmdLimit.result.stderr }],
      ["info", "exit", { exitCode: 126 }],
    ]);

    const ac = new AbortController();
    ac.abort();
    const aborted = await run("echo hi", {}, { signal: ac.signal });
    assert.equal(aborted.result.exitCode, 124);
    assert.deepEqual(aborted.calls, [
      ["info", "exec", { command: "echo hi" }],
      ["info", "stderr", { output: "bash: execution aborted\n" }],
      ["info", "exit", { exitCode: 124 }],
    ]);
  }
});

test("logger interleaves pre/post around nested ctx.exec and skips blank/bypass calls", async () => {
  const { calls, logger } = makeLogger();
  let seenAtStart = -1;
  let seenAfterChild = -1;

  const mycmd = defineCommand("mycmd", async (_args, ctx) => {
    seenAtStart = calls.length;
    await ctx.exec("   ");
    const sub = await ctx.exec("echo sub");
    seenAfterChild = calls.length;
    return { stdout: `parent:${sub.stdout}`, stderr: "warn\n", exitCode: 3 };
  });

  const echoOverride = defineCommand("echo", async (args, ctx) => {
    return ctx.origCommand(args);
  });

  const bash = new Bash({ logger, customCommands: [mycmd, echoOverride] });
  const r = await bash.exec("mycmd");
  assert.equal(r.exitCode, 3);
  assert.equal(seenAtStart, 1);
  assert.equal(seenAfterChild, 4);
  assert.deepEqual(calls, [
    ["info", "exec", { command: "mycmd" }],
    ["info", "exec", { command: "echo sub" }],
    ["debug", "stdout", { output: "sub\n" }],
    ["info", "exit", { exitCode: 0 }],
    ["debug", "stdout", { output: "parent:sub\n" }],
    ["info", "stderr", { output: "warn\n" }],
    ["info", "exit", { exitCode: 3 }],
  ]);

  calls.length = 0;
  const orig = await bash.exec("echo direct");
  assert.equal(orig.stdout, "direct\n");
  assert.deepEqual(calls, [
    ["info", "exec", { command: "echo direct" }],
    ["debug", "stdout", { output: "direct\n" }],
    ["info", "exit", { exitCode: 0 }],
  ]);
});
