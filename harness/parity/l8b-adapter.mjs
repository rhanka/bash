import manifest from "../../package.json" with { type: "json" };
import { Bash } from "../../host/index.js";
import { T } from "../../test/kt.mjs";

function quote(s) {
  return "'" + s.replaceAll("'", "'\\''") + "'";
}

/**
 * Decodes a shell word composed of adjacent single-quoted, double-quoted,
 * backslash-escaped, or bare safe `[A-Za-z0-9_./+-]+` segments.
 */
function parseShellWords(rest) {
  const tokens = [];
  let i = 0;
  const n = rest.length;
  while (i < n) {
    while (i < n && rest[i] === " ") i++;
    if (i >= n) break;
    let word = "";
    let matched = false;
    while (i < n && rest[i] !== " ") {
      const c = rest[i];
      if (c === "'") {
        const end = rest.indexOf("'", i + 1);
        if (end < 0) return null;
        word += rest.slice(i + 1, end);
        i = end + 1;
        matched = true;
      } else if (c === '"') {
        i++;
        let closed = false;
        while (i < n) {
          const ch = rest[i];
          if (ch === '"') {
            i++;
            closed = true;
            break;
          }
          if (ch === "\\") {
            if (i + 1 >= n) return null;
            const nx = rest[i + 1];
            if (nx === '"' || nx === "$" || nx === "`" || nx === "\\") {
              word += nx;
              i += 2;
              continue;
            }
          }
          if (ch === "$" || ch === "`") return null;
          word += ch;
          i++;
        }
        if (!closed) return null;
        matched = true;
      } else if (c === "\\") {
        if (i + 1 >= n) return null;
        word += rest[i + 1];
        i += 2;
        matched = true;
      } else if (/^[A-Za-z0-9_./+-]$/.test(c)) {
        word += c;
        i++;
        matched = true;
      } else {
        return null;
      }
    }
    if (!matched) return null;
    tokens.push(word);
  }
  return tokens;
}

function translateStep(script) {
  if (script === "printf") return "__printf";
  if (script === "printf --") return "__printf --";
  if (script.startsWith("printf ")) {
    let rest = script.slice("printf ".length);
    let dashDash = false;
    if (rest.startsWith("-- ")) {
      dashDash = true;
      rest = rest.slice(3);
    }
    const words = parseShellWords(rest);
    if (!words || words.length === 0) return null;
    const prefix = dashDash ? "__printf -- " : "__printf ";
    return prefix + words.map(quote).join(" ");
  }

  const dateMatch = /^date -u -d @(-?[0-9]+) '(\+(?:[^']|'\\'')*)'$/.exec(script);
  if (dateMatch) {
    const epoch = dateMatch[1];
    const fmt = dateMatch[2].slice(1).replaceAll("'\\''", "'");
    return `__strftime ${epoch} ${quote(fmt)} 'UTC'`;
  }

  const b64Match = /^base64(?: -w ([0-9]+)| (-d|--decode))?$/.exec(script);
  if (b64Match) {
    if (b64Match[2]) return "__base64 decode";
    return `__base64 encode ${b64Match[1] ?? "76"}`;
  }

  const hashMatch = /^(md5sum|sha1sum|sha256sum)$/.exec(script);
  if (hashMatch) {
    const alg = hashMatch[1].slice(0, -3);
    return `__hash ${alg} -`;
  }

  return null;
}

export default {
  name: manifest.name,
  version: manifest.version,

  async create(init) {
    const options = { executionLimitProfile: init.profile ?? "normal", ...init.engine };
    if (init.env !== undefined) options.env = init.env;
    if (init.cwd !== undefined) options.cwd = init.cwd;
    const bash = new Bash(T(options));

    return {
      async exec(step) {
        const { script, stdin, stdinKind, ...opts } = step;
        const text = typeof script === "string" ? script : Buffer.from(script.base64, "base64").toString("utf8");
        const translated = translateStep(text);
        if (translated === null) {
          const err = new Error(`UNSUPPORTED l8b step: ${text}`);
          err.code = "UNSUPPORTED";
          throw err;
        }
        if (stdin && typeof stdin === "object") {
          opts.stdin = Buffer.from(stdin.base64, "base64").toString("latin1");
          opts.stdinKind = "bytes";
        } else if (stdin !== undefined) {
          opts.stdin = stdin;
          opts.stdinKind = stdinKind;
        }
        const { stdout, stderr, exitCode, env = {} } = await bash.exec(translated, opts);
        return { stdout, stderr, exitCode, env };
      },

      async readFs() {
        return new Map();
      },

      takeTrace: () => [],
      takeCoverage: () => [],
      dispose() {},
    };
  },
};
