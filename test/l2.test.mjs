import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { Bash, defineCommand, getCommandNames } from "../host/index.js";
import { COMMAND_NAMES } from "../dist/js/kernel/commands.js";
import { scanCommandFiles } from "../tools/build.mjs";
import { enc, internal, module } from "./kt.mjs";
import { Instance } from "../host/core/instance.js";

test("the probe lives exclusively in the test build", () => {
  for (const path of ["probe.core.wasm", "probe.component.wasm", "js/probe/probe.js"]) {
    assert.ok(existsSync(new URL("../dist/test/" + path, import.meta.url)), path);
    assert.ok(!existsSync(new URL("../dist/" + path, import.meta.url)), path);
  }
});

test("the generated registry sorts standard commands and filters each shell", async () => {
  const dir = fileURLToPath(new URL("./fixtures/cmd-scan/", import.meta.url));
  assert.deepEqual(scanCommandFiles(dir), ["alpha", "zeta"]);
  assert.deepEqual(getCommandNames(), ["echo"]);
  const bash = new Bash();
  assert.deepEqual(bash.getCommandNames(), ["echo"]);
  assert.deepEqual(await bash.fs.readdir("/bin"), [...COMMAND_NAMES]);
  const restricted = new Bash({ commands: [] });
  assert.deepEqual(restricted.getCommandNames(), []);
  const r = await restricted.exec("echo hi");
  assert.equal(r.exitCode, 127);
  assert.equal(r.stderr, "bash: echo: command not found\n");
  assert.deepEqual(await restricted.fs.readdir("/bin"), []);
});

test("each shell installs its layout and command stubs with the expected contents and modes", async () => {
  const bash = new Bash();
  assert.equal(bash.getCwd(), "/home/user");
  const env = {
    HOME: "/home/user", PATH: "/usr/bin:/bin", IFS: " \t\n", OSTYPE: "linux-gnu", MACHTYPE: "x86_64-pc-linux-gnu",
    HOSTTYPE: "x86_64", HOSTNAME: "localhost", PWD: "/home/user", OLDPWD: "/home/user", OPTIND: "1",
    SHELLOPTS: "braceexpand:hashall:interactive-comments", BASHOPTS: "globskipdots",
  };
  assert.deepEqual(Object.entries(bash.getEnv()), Object.entries(env));
  const files = {
    "/bin/echo": "#!/bin/bash\n# Built-in command: echo\n",
    "/usr/bin/echo": "#!/bin/bash\n# Built-in command: echo\n",
    "/dev/null": "",
    "/proc/version": "Linux version 5.15.0-generic (just-bash) #1 SMP PREEMPT\n",
    "/proc/self/status": "Name:\tbash\nState:\tR (running)\nPid:\t1\nPPid:\t0\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n",
    "/proc/self/cmdline": "bash\0",
  };
  for (const [path, content] of Object.entries(files)) {
    assert.deepEqual(await bash.fs.readFileBuffer(path), enc(content));
    assert.equal((await bash.fs.lstat(path)).mode, 0o644);
  }
  for (const files of [{}, { "/a.txt": "x" }]) {
    const configured = new Bash({ files });
    assert.equal(configured.getCwd(), "/");
    assert.equal(configured.getEnv().HOME, "/");
    assert.equal(await configured.fs.exists("/home/user"), false);
    assert.equal(await configured.fs.exists("/tmp"), false);
    assert.equal(await configured.fs.exists("/bin/echo"), true);
  }
  assert.equal(new Bash({ cwd: "" }).getCwd(), "/home/user");
  assert.equal(await new Bash({ cwd: "/a/b", files: { "/x": "y" } }).fs.exists("/a/b"), true);
  assert.throws(() => new Bash({ files: { "/dev": "x" } }), /EEXIST: file already exists, mkdir '\/dev'/);
  bash.registerCommand(defineCommand("hi", () => ({ stdout: "hi", stderr: "", exitCode: 0 })));
  assert.equal(await bash.fs.readFile("/bin/hi"), "#!/bin/bash\n# Built-in command: hi\n");
  assert.equal((await bash.exec("/usr/bin/hi")).stdout, "hi");
});

test("coverage uses stable ids and records flag occurrences in argument order", () => {
  const { engine } = new Instance(module, internal());
  engine.configure(256n << 20n);
  const names = engine.coverageFeatures();
  assert.equal(names.length, 184);
  assert.equal(new Set(names).size, 184);
  assert.equal(names[0], "bash:cmd:SimpleCommand");
  assert.deepEqual(names.slice(181), ["cmd:flag:echo:-n", "cmd:flag:echo:-e", "cmd:flag:echo:-E"]);
  const fs = engine.Vfs.create(128n << 20n);
  const shell = engine.Shell.create(fs, {
    limits: { profile: "contain", overrides: [] }, grants: {}, customCommands: [], env: [],
    defaultLayout: false, trace: false, coverage: true,
  });
  for (const [script, last] of [["echo -n -e 'a\\nb'", "-e"], ["echo hi -n -E", "-E"]]) {
    const fiber = engine.Fiber.spawn(shell, undefined, { script: enc(script), args: [], env: [], replaceEnv: false, rawScript: false });
    assert.equal(fiber.run(10n).val.exitCode, 0);
    const ids = [...fiber.takeCoverage(2), ...fiber.takeCoverage(64)];
    assert.deepEqual(ids.map((id) => names[id]), [
      "bash:cmd:SimpleCommand",
      "bash:expansion:word_glob", "bash:expansion:word_glob", "bash:expansion:word_glob",
      "bash:builtin:echo", "cmd:flag:echo:-n", "cmd:flag:echo:" + last,
    ]);
    assert.equal(fiber.takeCoverage(64).length, 0);
    fiber[Symbol.dispose]();
  }
});

test("relative exec working directories anchor to the shell and preserve PWD options", async () => {
  const inspect = defineCommand("inspect", (_args, ctx) => ({ stdout: ctx.cwd, stderr: "", exitCode: 0 }));
  const b = new Bash({ customCommands: [inspect] });
  await b.fs.mkdir("/home/user/sub");
  const relative = await b.exec("inspect", { cwd: "sub" });
  assert.equal(relative.stdout, "/home/user/sub");
  assert.equal(relative.env.PWD, "/home/user/sub");
  const parent = await b.exec("../../bin/echo anchored", { cwd: "." });
  assert.equal(parent.stdout, "anchored\n");
  assert.equal(parent.env.PWD, "/home/user/.");
  const uncollapsed = "/home/user/./sub";
  assert.equal((await b.exec("inspect", { cwd: uncollapsed })).env.PWD, uncollapsed);
  assert.equal((await b.exec("inspect", { cwd: "./sub", env: {} })).env.PWD, "/home/user/sub");
  assert.equal((await b.exec("inspect", { cwd: "./sub", env: { KEY: "value" } })).env.PWD, "/home/user/sub");
  assert.equal((await b.exec("inspect", { cwd: "./missing", env: {} })).env.PWD, "/home/user/./missing");
  const explicit = await b.exec("inspect", { cwd: "./sub", env: { PWD: "logical" } });
  assert.equal(explicit.stdout, uncollapsed);
  assert.equal(explicit.env.PWD, "logical");
  assert.equal(b.getCwd(), "/home/user");
});

test("empty exec env resolves cwd through the configured filesystem", async () => {
  const paths = [];
  const fs = {
    async realpath(path) {
      paths.push(path);
      if (path === "/home/user/./link") return "/elsewhere";
      throw new Error("path missing");
    },
  };
  const inspect = defineCommand("inspect", (_args, ctx) => ({ stdout: ctx.cwd, stderr: "", exitCode: 0 }));
  const b = new Bash({ fs, customCommands: [inspect] });
  const omitted = await b.exec("inspect", { cwd: "./link" });
  assert.equal(omitted.stdout, "/home/user/./link");
  const explicit = await b.exec("inspect", { cwd: "./link", env: {} });
  assert.equal(explicit.stdout, "/elsewhere");
  assert.equal(explicit.env.PWD, "/elsewhere");
  const missing = await b.exec("inspect", { cwd: "./missing", env: { KEY: "value" } });
  assert.equal(missing.stdout, "/home/user/./missing");
  assert.deepEqual(paths, ["/home/user/./link", "/home/user/./missing"]);
});

test("IFS splits on Unicode scalars across expansion chunks", async () => {
  const text = "x".repeat(255);
  const shell = new Bash();
  const r = await shell.exec('echo $S "$S"', { env: { IFS: "é🚀βλЖのあΩø∞", S: text + "🚀élan∞fin" } });
  assert.equal(r.stdout, text + "  lan fin " + text + "🚀élan∞fin\n");
});

test("a host call can resume between pipelines without leaking prefix assignments", async () => {
  const read = defineCommand("readstdin", (_args, ctx) => ({ stdout: ctx.stdin, stderr: "", exitCode: 0 }));
  const bash = new Bash({ customCommands: [read] });
  const r = await bash.exec("X=outer; { X=inner; echo $X; } | readstdin; echo $X");
  assert.equal(r.exitCode, 0);
  assert.equal(r.stdout, "inner\nouter\n");
  assert.equal(r.env.X, "outer");
});

test("exec resolves an existing cwd when env is supplied and preserves explicit PWD", async () => {
  const shell = new Bash({ files: { "/a/b/c": "x" } });
  assert.equal((await shell.exec("echo $PWD", { cwd: "/a/../a/b" })).stdout, "/a/../a/b\n");
  for (const env of [{}, { A: "value" }]) {
    assert.equal((await shell.exec("echo $PWD", { cwd: "/a/../a/b", env })).stdout, "/a/b\n");
    assert.equal((await shell.exec("echo $PWD", { cwd: "/a/../missing", env })).stdout, "/a/../missing\n");
  }
  const explicit = await shell.exec("echo $PWD", { cwd: "/a/../a/b", env: { PWD: "custom" } });
  assert.equal(explicit.stdout, "custom\n");
  const early = await shell.exec("'", { cwd: "/a/b", replaceEnv: true, env: { A: "value" } });
  assert.equal(early.env.PWD, "/");
  assert.equal(early.env.A, "value");
  assert.equal(early.env["?"], undefined);
});

test("a pipeline stage sees its local last argument and restores the outer one", async () => {
  const shell = new Bash();
  const r = await shell.exec("echo outer; echo left | { echo local; echo last=$_; }; echo last=$_");
  assert.equal(r.stdout, "outer\nlocal\nlast=local\nlast=outer\n");
});
