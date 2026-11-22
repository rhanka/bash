// Test-only parity adapter for lot L8a (spec section 7.2).
// Translates the three canonical step forms of `test/cases/l8a/*.case.json` to the
// `kernel-test` commands `__regex` and `__glob`, never leaking adapter variables into `env`.
import manifest from "../../package.json" with { type: "json" };
import { Bash } from "../../host/index.js";
import { T } from "../../test/kt.mjs";

const RE_GREP = /^grep((?: -[EFPiwxovcn]+)*) -- ('(?:[^']|'\\'')*')$/;
const RE_CASE = /^\( (?:shopt -s ([a-z ]+); )?case ('(?:[^']|'\\'')*') in (.*)\) echo match ;; \*\) echo no ;; esac \)$/;
const RE_GLOBSTAR = /^\( shopt -s globstar; echo (\S+) \)$/;

function fileData(value) {
  return typeof value === "string" ? value : Buffer.from(value.base64, "base64");
}

function unquote(token) {
  return token.slice(1, -1).replaceAll("'\\''", "'");
}

function quote(value) {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

/**
 * Lowers shell single-quoted spans outside `[...]` brackets in a `case` pattern into
 * backslash-escaped glob literals, while leaving `'` inside `[...]` untouched (matching
 * `just-bash@3.6.0`, where `['a-z']` keeps `'` as a literal bracket member).
 */
function lowerCasePattern(raw) {
  let out = "";
  let i = 0;
  let inBracket = false;
  let bracketFirst = false;
  while (i < raw.length) {
    const c = raw[i];
    if (inBracket) {
      out += c;
      if (c === "\\" && i + 1 < raw.length) {
        out += raw[i + 1];
        bracketFirst = false;
        i += 2;
        continue;
      }
      if (c === "[" && raw[i + 1] === ":") {
        const end = raw.indexOf(":]", i + 2);
        if (end >= 0) {
          out += raw.slice(i + 1, end + 2);
          bracketFirst = false;
          i = end + 2;
          continue;
        }
      }
      if (c === "]" && !bracketFirst) {
        inBracket = false;
        i += 1;
        continue;
      }
      bracketFirst = false;
      i += 1;
      continue;
    }
    if (c === "\\") {
      out += c;
      if (i + 1 < raw.length) {
        out += raw[i + 1];
        i += 2;
      } else {
        i += 1;
      }
      continue;
    }
    if (c === "'") {
      const close = raw.indexOf("'", i + 1);
      if (close >= 0) {
        const inner = raw.slice(i + 1, close);
        for (const ch of inner) {
          if ("*?[]\\()|@+!".includes(ch)) out += "\\";
          out += ch;
        }
        i = close + 1;
        continue;
      }
    }
    if (c === "[") {
      inBracket = true;
      bracketFirst = true;
      out += c;
      i += 1;
      if (raw[i] === "^") {
        out += raw[i];
        i += 1;
      }
      continue;
    }
    out += c;
    i += 1;
  }
  return out;
}

function translateStep(script) {
  let m = RE_GREP.exec(script);
  if (m) {
    const rawFlags = m[1] || "";
    let mode = "bre-grep";
    let flags = "";
    for (const tok of rawFlags.trim().split(/\s+/).filter(Boolean)) {
      for (const ch of tok.slice(1)) {
        if (ch === "E") mode = "ere";
        else if (ch === "F") mode = "fixed";
        else if (ch === "P") mode = "perl";
        else flags += ch;
      }
    }
    const pattern = unquote(m[2]);
    return `__regex ${mode} ${flags || "-"} ${quote(pattern)}`;
  }

  m = RE_CASE.exec(script);
  if (m) {
    const shoptNames = (m[1] || "").trim().split(/\s+/).filter(Boolean);
    let flags = "";
    for (const name of shoptNames) {
      if (name === "extglob") flags += "e";
      else if (name === "nocasematch") flags += "i";
      else {
        const err = new Error(`l8a-adapter: unsupported shopt option: ${name}`);
        err.code = "UNSUPPORTED";
        throw err;
      }
    }
    const subject = unquote(m[2]);
    const pattern = lowerCasePattern(m[3]);
    return `__glob match ${flags || "-"} ${quote(pattern)} ${quote(subject)}`;
  }

  m = RE_GLOBSTAR.exec(script);
  if (m) {
    return `__glob globstar-check - ${quote(m[1])}`;
  }

  const err = new Error(`l8a-adapter: unrecognized step script: ${script}`);
  err.code = "UNSUPPORTED";
  throw err;
}

export default {
  name: `${manifest.name}:l8a-adapter`,
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
    const bash = new Bash(T(options));

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
        const translated = translateStep(text);
        const { stdout, stderr, exitCode, env = {}, stdoutKind } = await bash.exec(translated, opts);
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

      takeTrace: () => [],
      takeCoverage: () => [],
      dispose() {},
    };
  },
};
