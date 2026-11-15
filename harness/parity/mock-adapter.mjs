// Deterministic toy engine, only for testing the harness. Not a shell.
import { posix } from "node:path";

const enc = new TextEncoder();

function toBytes(content) {
  return typeof content === "string" ? enc.encode(content) : new Uint8Array(Buffer.from(content.base64, "base64"));
}

function latin1(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return s;
}

function decodeText(bytes) {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return latin1(bytes);
  }
}

function stdinText(step) {
  if (step.stdin === undefined) return "";
  if (typeof step.stdin === "string") return step.stdin;
  const bytes = toBytes(step.stdin);
  return step.stdinKind === "bytes" ? latin1(bytes) : decodeText(bytes);
}

/**
 * @param {{clock?: () => number}} [options] `clock` returns milliseconds and fills trace times.
 */
export function createMockAdapter(options = {}) {
  const clock = options.clock ?? (() => performance.now());
  return {
    name: "mock",
    version: "1",
    async create(init) {
      if (init.engine && init.engine.unsupported === true) {
        throw Object.assign(new Error("mock: unsupported case"), { code: "UNSUPPORTED" });
      }
      /** @type {Map<string, any>} */
      const fs = new Map();
      const mkdirs = (dir) => {
        for (let d = dir; ; d = posix.dirname(d)) {
          if (!fs.has(d)) fs.set(d, { kind: "directory", mode: 0o755 });
          if (d === "/") break;
        }
      };
      mkdirs("/");
      for (const [p, f] of Object.entries(init.files ?? {})) {
        const abs = posix.resolve("/", p);
        const spec = typeof f === "object" && "content" in f ? f : { content: f };
        mkdirs(posix.dirname(abs));
        fs.set(abs, { kind: "file", mode: spec.mode ?? 0o644, content: toBytes(spec.content) });
      }
      const env = { ...(init.env ?? {}) };
      let cwd = init.cwd ?? "/";
      let trace = [];
      let coverage = [];

      const resolveFile = (path, base) => {
        let p = posix.resolve(base, path);
        for (let i = 0; i < 8; i++) {
          const e = fs.get(p);
          if (e && e.kind === "symlink") p = posix.resolve(posix.dirname(p), e.target);
          else break;
        }
        return p;
      };

      return {
        async exec(step) {
          const base = step.cwd ?? cwd;
          const stepEnv = step.replaceEnv ? { ...(step.env ?? {}) } : null;
          if (stepEnv) for (const k of Object.keys(env)) delete env[k];
          Object.assign(env, step.env ?? {});
          let stdout = "";
          let stderr = "";
          let status = 0;
          let kind;
          const script = typeof step.script === "string" ? step.script : decodeText(toBytes(step.script));
          for (const line of script.split("\n")) {
            if (line.trim() === "") continue;
            const sp = line.indexOf(" ");
            const cmd = sp < 0 ? line : line.slice(0, sp);
            const rest = sp < 0 ? "" : line.slice(sp + 1);
            status = 0;
            if (cmd === "echo") stdout += rest + "\n";
            else if (cmd === "err") stderr += rest + "\n";
            else if (cmd === "bytes") {
              stdout += latin1(Buffer.from(rest.trim(), "hex"));
              kind = "bytes";
            } else if (cmd === "stdin") stdout += stdinText(step);
            else if (cmd === "args") for (const a of step.args ?? []) stdout += a + "\n";
            else if (cmd === "write") {
              const i = rest.indexOf(" ");
              const path = posix.resolve(base, i < 0 ? rest : rest.slice(0, i));
              mkdirs(posix.dirname(path));
              fs.set(path, { kind: "file", mode: 0o644, content: enc.encode(i < 0 ? "" : rest.slice(i + 1)) });
            } else if (cmd === "rm") {
              const path = posix.resolve(base, rest);
              if (fs.get(path)?.kind === "file" || fs.get(path)?.kind === "symlink") fs.delete(path);
              else {
                stderr += `rm: ${rest}: No such file\n`;
                status = 1;
              }
            } else if (cmd === "ln") {
              const i = rest.indexOf(" ");
              const path = posix.resolve(base, rest.slice(i + 1));
              mkdirs(posix.dirname(path));
              fs.set(path, { kind: "symlink", target: rest.slice(0, i) });
            } else if (cmd === "cat") {
              const e = fs.get(resolveFile(rest, base));
              if (e && e.kind === "file") stdout += decodeText(e.content);
              else {
                stderr += `cat: ${rest}: No such file or directory\n`;
                status = 1;
              }
            } else if (cmd === "setenv") {
              const i = rest.indexOf(" ");
              env[i < 0 ? rest : rest.slice(0, i)] = i < 0 ? "" : rest.slice(i + 1);
            } else if (cmd === "exit") {
              status = Number(rest) | 0;
              break;
            } else if (cmd === "cover") {
              if (init.capture.coverage) coverage.push(rest);
            } else if (cmd === "trace") {
              const [category, name, ...kv] = rest.split(" ");
              const details = {};
              for (const pair of kv) {
                const j = pair.indexOf("=");
                const key = pair.slice(0, j);
                details[key] = key.endsWith("TimeMs") ? clock() : Number(pair.slice(j + 1));
              }
              if (init.capture.trace) trace.push({ category, name, durationMs: clock(), details });
            } else if (cmd === "random") stdout += `${Math.random()}\n`;
            else if (cmd === "crash") throw new Error(`mock crash while reading ${process.cwd()}/secret/state.bin`);
            else {
              stderr += `mock: ${cmd}: command not found\n`;
              status = 127;
            }
          }
          const result = { stdout, stderr, exitCode: status, env: { ...env } };
          if (kind) result.stdoutKind = kind;
          return result;
        },
        async readFs() {
          const out = new Map();
          for (const [p, e] of fs) out.set(p, e.kind === "file" ? { ...e, content: new Uint8Array(e.content) } : { ...e });
          return out;
        },
        takeTrace() {
          const t = trace;
          trace = [];
          return t;
        },
        takeCoverage() {
          const c = coverage;
          coverage = [];
          return c;
        },
        async dispose() {},
      };
    },
  };
}

export default createMockAdapter();
