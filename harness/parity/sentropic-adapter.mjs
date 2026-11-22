import manifest from "../../package.json" with { type: "json" };
import { Bash } from "../../host/index.js";

function fileData(value) {
  return typeof value === "string" ? value : Buffer.from(value.base64, "base64");
}

export default {
  name: manifest.name,
  version: manifest.version,

  async create(init) {
    const options = { executionLimitProfile: init.profile ?? "normal", ...init.engine };
    if (init.files && Object.keys(init.files).length > 0) {
      options.files = Object.fromEntries(Object.entries(init.files).map(([path, value]) => {
        const data = typeof value === "object" && "content" in value
          ? { ...value, content: fileData(value.content) }
          : fileData(value);
        return [path, data];
      }));
    }
    if (init.env !== undefined) options.env = init.env;
    if (init.cwd !== undefined) options.cwd = init.cwd;
    let trace = [];
    let coverage = [];
    if (init.capture?.trace) options.trace = (ev) => trace.push(ev);
    if (init.capture?.coverage) options.coverage = { hit: (feature) => coverage.push(feature) };
    const bash = new Bash(options);

    return {
      async exec(step) {
        const { script, stdin, stdinKind, ...opts } = step;
        if (stdin && typeof stdin === "object") {
          opts.stdin = Buffer.from(stdin.base64, "base64").toString("latin1");
          opts.stdinKind = "bytes";
        } else if (stdin !== undefined) {
          opts.stdin = stdin;
          opts.stdinKind = stdinKind;
        }
        const text = typeof script === "string" ? script : Buffer.from(script.base64, "base64").toString("utf8");
        const { stdout, stderr, exitCode, env = {}, stdoutKind } = await bash.exec(text, opts);
        return { stdout, stderr, exitCode, env, ...(stdoutKind ? { stdoutKind } : {}) };
      },

      async readFs() {
        const entries = new Map();
        for (const path of bash.fs.getAllPaths()) {
          const stat = await bash.fs.lstat(path);
          if (stat.isFile) entries.set(path, { kind: "file", mode: stat.mode, content: await bash.fs.readFileBuffer(path) });
          else if (stat.isDirectory) entries.set(path, { kind: "directory", mode: stat.mode });
          else if (stat.isSymbolicLink) entries.set(path, { kind: "symlink", target: await bash.fs.readlink(path) });
        }
        return entries;
      },

      takeTrace() {
        const out = trace;
        trace = [];
        return out;
      },
      takeCoverage() {
        const out = coverage;
        coverage = [];
        return out;
      },
      dispose() {},
    };
  },
};
