import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const cases = {
  async abort() {
    const shell = new Bash({ executionLimitProfile: "contain" });
    const tree = ["f0() { B=$A; }"];
    for (let i = 1; i <= 15; i++) tree.push(`f${i}() { f${i - 1}; f${i - 1}; }`);
    const script = ["A=x", ...Array(22).fill('A="$A$A"'), ...tree, "f15", "echo unreachable"].join("\n");
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 20);
    try {
      const r = await shell.exec(script, { signal: ctrl.signal });
      assert.equal(r.exitCode, 124);
      assert.equal(r.stderr, "bash: execution aborted\n");
      assert.equal(r.stdout, "");
    } finally {
      clearTimeout(timer);
    }
    assert.equal((await shell.exec("echo ok")).stdout, "ok\n");
  },
  async deadline() {
    const shell = new Bash({ executionLimitProfile: "contain", executionLimits: { maxExecutionTimeMs: 30 } });
    // A single lookup scans over two million colon-delimited entries, including empty ones.
    const script = ["PATH=.:", ...Array(21).fill('PATH="$PATH$PATH"'), "missing"].join("\n");
    const r = shell.execSync(script);
    assert.equal(r.exitCode, 124);
    assert.equal(r.stderr, "bash: execution aborted\n");
    assert.equal(shell.execSync("echo ok").stdout, "ok\n");
  },
  async ifs() {
    const shell = new Bash({ executionLimitProfile: "micro" });
    const script = ["A=x B=y", ...Array(20).fill('A="$A$A" B="$B$B"'), "IFS=$A", "echo $B"].join("\n");
    const r = await shell.exec(script);
    assert.equal(r.exitCode, 0, r.stderr);
    assert.equal(r.stdout, "y".repeat(1 << 20) + "\n");
  },
};

for (const [name, run] of Object.entries(cases)) {
  test(`L2 ${name} completes within its wall-clock bound`, () => {
    const source = `import assert from "node:assert/strict";
      import { Bash } from "@sentropic/bash";
      await (${run.toString().replace(/^async \w+/, "async function")})();`;
    const child = spawnSync(process.execPath, ["--input-type=module", "--eval", source], {
      cwd: new URL("../", import.meta.url), encoding: "utf8", timeout: 3_000, killSignal: "SIGKILL",
    });
    assert.ifError(child.error);
    assert.equal(child.status, 0, child.stderr);
  });
}
