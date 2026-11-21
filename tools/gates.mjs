// The eight gates of tools/check.mjs. Each gate takes a context { root, out, toolsDir, quick } and returns
// null on success, "SKIPPED (...)" when skipped, or a failure reason string.
import { cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { components, lockProblem, npmProblem, nonRegularEntries, lockResolution, pins, pinsProblem, readJson, run, sha256File, toolBin, toolProblem, tryRun, walk } from "./lib.mjs";
import { bindingsProblems } from "./bindings.mjs";

const wasmToolsOf = (ctx) => toolBin("wasm-tools", ctx.toolsDir, ctx.root);

/** One entry per component declared in tools/components.json: { name, def, path } (path may not exist). */
function declared(ctx) {
  return Object.entries(components(ctx.root)).map(([name, def]) => ({ name, def, path: join(ctx.out, `${name}.component.wasm`) }));
}

// ---------------------------------------------------------------------------------------------- G1

export function g1ZeroImport(ctx) {
  const list = declared(ctx);
  if (list.length === 0) return "no component is declared in tools/components.json";
  for (const { name, path } of list) {
    if (!existsSync(path)) return `${name}: ${relative(ctx.root, path)} was not built`;
    const r = tryRun(wasmToolsOf(ctx), ["component", "wit", path]);
    if (r.status !== 0) return `${relative(ctx.root, path)}: wasm-tools component wit failed: ${r.stderr.trim()}`;
    const line = r.stdout.split("\n").find((l) => /^\s*import\b/.test(l));
    if (line) return `${relative(ctx.root, path)} has an import: ${line.trim()}`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G2
//
// SCOPE OF G2. G2 is a best-effort STATIC GUARD against ACCIDENTAL imports and capability use in OUR OWN
// code (host/, dist/js/, types/). It is NOT a sandbox against adversarial source: a regex-based scanner of
// JavaScript cannot be made sound, and no rule here is a security boundary. Where it cannot classify a
// construct reliably it FAILS CLOSED (reports an error) instead of guessing. The ENFORCEMENT is the runtime
// layer: tests run with --disallow-code-generation-from-strings (tools/run-tests.mjs) and under a module
// resolve hook that refuses bare specifiers and node: imports (tools/test-hooks.mjs).

// Whitespace between tokens. Applied to comment-stripped text only, so a literal cannot bridge statements.
const W = String.raw`\s*`;
const LT = String.raw`\r\n\u2028\u2029`;
// A quoted name (ES2022 module export names): escapes allowed, no line terminator.
const QNAME = String.raw`(?:"(?:[^"\\${LT}]|\\.)*"|'(?:[^'\\${LT}]|\\.)*')`;
// A braced name list; quoted names may hold any character, including } ( ) and comment markers.
const NAMES = String.raw`\{(?:${QNAME}|[^}"'])*\}`;
const CLAUSE = String.raw`(?:${NAMES}|\bas${W}${QNAME}|[\w$*,\s])*?`;
// A specifier holds no backslash and no line terminator.
const STATIC_IMPORT = new RegExp(String.raw`\b(?:import|export)${W}(?:${CLAUSE}${W}\bfrom${W})?(['"])([^'"\\${LT}]+)\1`, "g");
const DYNAMIC_LITERAL = new RegExp(String.raw`(?<![.$\w#])import${W}\(${W}(['"])([^'"` + "`" + String.raw`$\\${LT}]*)\1${W}\)`, "g");
// The keyword rules below run on `code` (literals blanked), so they can neither be hidden by nor fire on a literal.
const DYNAMIC_ANY = /(?<![.$\w#])import\s*\(/g;
const IMPORT_KEYWORD = /(?<![.$\w#])import\b(?!\s*\.\s*meta\b)(?!\s*\()/g;
const EXPORT_FROM = /(?<![.$\w#])export\s*(?:\*(?:\s*as\s*(?:[\w$]+|"\s*"|'\s*'))?|\{[^}]*\})\s*from\b/g;

// A keyword before a slash (or a bracket) means a regex literal (or an array literal) follows, not a division.
const REGEX_KEYWORDS = new Set(["return", "typeof", "case", "do", "else", "in", "of", "void", "delete", "throw", "new", "instanceof", "yield", "await", "default", "extends", "const", "let", "var"]);
const HEADER_WORDS = new Set(["if", "while", "for", "with"]);
const isWordChar = (c) => /[\w$]/.test(c) || (c > "\u007f" && !/\s/.test(c));
const isLT = (c) => c === "\n" || c === "\r" || c === "\u2028" || c === "\u2029";
const blank = (t) => " ".repeat(t.length);

/** One pass over a script. Returns { src, code, unicodeEscape, error, strings, computed }.
 * src: the source with comments replaced by a space. code: src with the body of string, template and regex
 * literals blanked to spaces (same length as src, so positions in both agree), so identifier rules cannot
 * fire on literal text. unicodeEscape: a backslash appears in code position. error: a construct the scanner
 * could not classify reliably (unterminated literal or comment, ambiguous slash); G2 fails closed on it.
 * strings: [{ at, escaped }] string literals; computed: [{ at, kind }] with kind "access" for expr[...] and
 * "key" for a computed object or destructuring key, at the position of the "[" in src.
 * Line terminators are \r \n U+2028 U+2029. */
export function scanSource(input) {
  const n = input.length;
  let i = 0;
  let src = "";
  let code = "";
  let unicodeEscape = false;
  let error = null;
  const strings = [];
  const computed = [];
  const fail = (m) => {
    error ??= m;
  };
  const emit = (text, shown = text) => {
    src += text;
    code += shown;
  };
  function template() {
    emit("`");
    i++;
    while (i < n && input[i] !== "`") {
      if (input[i] === "\\") {
        const t = input.slice(i, i + 2);
        emit(t, blank(t));
        i += 2;
      } else if (input[i] === "$" && input[i + 1] === "{") {
        emit("${");
        i += 2;
        codeMode(true);
        if (input[i] === "}") {
          emit("}");
          i++;
        } else fail("unterminated template expression");
      } else {
        emit(input[i], " ");
        i++;
      }
    }
    if (i < n) emit("`");
    else fail("unterminated template literal");
    i++;
  }
  function codeMode(nested) {
    const stack = []; // "(h" header paren, "(" , "[" , "{"
    let rx = true; // is a "/" here a regex start (an operand is expected)?
    let afterBrace = false;
    let dot = false; // the last token was "." or "#": a following word is a property name, not a keyword
    let word = "";
    let word2 = "";
    let last = ""; // the last punctuation character, when the last token was one
    const token = (nextRx, ch = "") => {
      last = ch;
      rx = nextRx;
      afterBrace = false;
      dot = false;
      word2 = word;
      word = "";
    };
    while (i < n) {
      const c = input[i];
      const d = input[i + 1];
      if (c === "/" && d === "*") {
        const e = input.indexOf("*/", i + 2);
        if (e < 0) fail("unterminated block comment");
        i = e < 0 ? n : e + 2;
        emit(" ");
      } else if (c === "/" && d === "/") {
        let e = i;
        while (e < n && !isLT(input[e])) e++;
        i = e;
        emit(" ");
      } else if (c === '"' || c === "'") {
        let j = i + 1;
        let escaped = false;
        while (j < n && input[j] !== c && !isLT(input[j])) {
          if (input[j] === "\\") {
            escaped = true;
            j += input[j + 1] === "\r" && input[j + 2] === "\n" ? 3 : 2;
          } else j++;
        }
        if (j >= n || input[j] !== c) {
          fail("unterminated string literal");
          j = Math.min(j, n) - 1;
        }
        const text = input.slice(i, j + 1);
        strings.push({ at: src.length, escaped, body: text.slice(1, -1) });
        emit(text, c + blank(text.slice(2)) + c);
        i = j + 1;
        token(false);
      } else if (c === "`") {
        template();
        token(false);
      } else if (c === "/") {
        if (!rx && afterBrace) {
          fail('cannot classify a "/" after "}" (block end or object literal): regex or division');
          i++;
          emit("/");
          token(true);
        } else if (!rx) {
          emit("/");
          i++;
          token(true);
        } else {
          let j = i + 1;
          let cls = false;
          let closed = false;
          while (j < n && !isLT(input[j])) {
            if (input[j] === "\\") j++;
            else if (input[j] === "[") cls = true;
            else if (input[j] === "]") cls = false;
            else if (input[j] === "/" && !cls) {
              closed = true;
              break;
            }
            j++;
          }
          if (!closed) {
            fail("unterminated regular expression literal");
            j = Math.min(j, n) - 1;
          }
          const text = input.slice(i, j + 1);
          emit(text, "/" + blank(text.slice(2)) + "/");
          i = j + 1;
          token(false);
        }
      } else if (isWordChar(c)) {
        let j = i + 1;
        while (j < n && isWordChar(input[j])) j++;
        const w = input.slice(i, j);
        emit(w);
        i = j;
        const kw = !dot && REGEX_KEYWORDS.has(w);
        const keep = word;
        token(kw);
        word = dot ? "" : w;
        word2 = keep;
      } else if (/\s/.test(c)) {
        emit(c);
        i++;
      } else if (c === "(") {
        const header = !dot && (HEADER_WORDS.has(word) || (word === "await" && word2 === "for"));
        stack.push(header ? "(h" : "(");
        emit(c);
        i++;
        token(true);
      } else if (c === ")") {
        const top = stack.pop();
        emit(c);
        i++;
        token(top === "(h");
      } else if (c === "[") {
        const top = stack[stack.length - 1];
        if (!rx) computed.push({ at: src.length, kind: "access" });
        else if (top === "{" && (last === "{" || last === ",")) computed.push({ at: src.length, kind: "key" });
        stack.push("[");
        emit(c);
        i++;
        token(true);
      } else if (c === "]") {
        stack.pop();
        emit(c);
        i++;
        token(false);
      } else if (c === "{") {
        stack.push("{");
        emit(c);
        i++;
        token(true, "{");
      } else if (c === "}") {
        if (nested && stack.length === 0) return;
        stack.pop();
        emit(c);
        i++;
        token(false);
        afterBrace = true;
      } else if ((c === "+" || c === "-") && d === c) {
        emit(c + d);
        i += 2;
        token(rx);
      } else if (c === ".") {
        if (d === "." && input[i + 2] === ".") {
          emit("...");
          i += 3;
          token(true);
        } else {
          emit(c);
          i++;
          token(false);
          dot = true;
        }
      } else if (c === "#") {
        emit(c);
        i++;
        token(false);
        dot = true;
      } else {
        if (c === "\\") unicodeEscape = true;
        emit(c);
        i++;
        token(true, c);
      }
    }
    if (nested) fail("unterminated template expression");
  }
  codeMode(false);
  return { src, code, unicodeEscape, error, strings, computed };
}

/** Replaces JS comments by a space, leaving string, template and regex literals intact. */
export function stripComments(input) {
  return scanSource(input).src;
}

/** Returns { specifiers, nonLiteralDynamic, unparsed, src, code, unicodeEscape, error, strings, computed }.
 * unparsed is a message when an `import` keyword (other than import.meta and import()) or an
 * `export ... from` was found in code that the specifier regexes did not account for: G2 fails closed on it. */
export function scanImports(raw) {
  const scan = scanSource(raw);
  const { src, code } = scan;
  const specifiers = [];
  const covered = new Set();
  for (const m of src.matchAll(STATIC_IMPORT)) {
    specifiers.push(m[2]);
    covered.add(m.index);
  }
  const literal = new Set();
  for (const m of src.matchAll(DYNAMIC_LITERAL)) {
    specifiers.push(m[2]);
    literal.add(m.index);
  }
  const nonLiteralDynamic = [...code.matchAll(DYNAMIC_ANY)].some((m) => !literal.has(m.index));
  let unparsed = null;
  for (const m of code.matchAll(IMPORT_KEYWORD)) if (!covered.has(m.index)) unparsed ??= "an import statement whose specifier could not be parsed";
  for (const m of code.matchAll(EXPORT_FROM)) if (!covered.has(m.index)) unparsed ??= "an export ... from whose specifier could not be parsed";
  return { specifiers, nonLiteralDynamic, unparsed, ...scan };
}

// Constructs that reach a module, a code evaluator or a node API by a route the import scan cannot see.
const FORBIDDEN = [
  [/<!--|-->/, "an HTML-like comment marker"],
  [/(?<![.$\w#])require\s*\(/, "require()"],
  [/(?<![.$\w#])eval\s*\(/, "eval()"],
  [/\bFunction\b/, "the Function constructor"],
  [/\b(?:globalThis|global|self|window|process)\s*(?:\?\.)?\s*\[/, "a bracket lookup on a global object"],
  [/\b(?:eval|require|Function|AsyncFunction|GeneratorFunction|Reflect)\b/, "a banned bare identifier (eval, require, Function, AsyncFunction, GeneratorFunction, Reflect)"],
  [/\.\s*constructor\b/, ".constructor"],
  [/[{,]\s*constructor\s*[,}:=]/, "a destructured constructor"],
];
// Rules that read the comment-stripped source, where a literal key is still visible.
const FORBIDDEN_SRC = [[/\[\s*(["'`])constructor\1\s*\]/, "a [\"constructor\"] lookup"]];
// hand-written host/ only (never dist/js): no dynamic route to a global or to an arbitrary property.
const HOST_IDENTIFIERS = [
  [/(?<![.$\w#])(?:globalThis|global|self|window)\b/, "a global object identifier (globalThis, global, self, window)"],
];
const HOST_LITERAL_ACCESS = /^\[\s*(?:\d+|"[^"\\\r\n\u2028\u2029]*"|'[^'\\\r\n\u2028\u2029]*')\s*\]/;
// The one legitimate use in generated jco glue (dist/js only): a literal-key read from the component state
// object. A literal key cannot reach an arbitrary property, so it is removed before the identifier scan.
const GENERATED_ALLOWED = [/(?<![.$\w#])Reflect\.get\(cstate, "callingSyncExport"\)/g];
const NODE_ONLY = [
  [/\bgetBuiltinModule\b/, "getBuiltinModule"],
  [/\bcreateRequire\b/, "createRequire"],
];

const isRelative = (s) => s.startsWith("./") || s.startsWith("../");
const SCRIPT = (p) => /\.(?:js|mjs|cjs)$/.test(p);
const isRegularFile = (p) => existsSync(p) && lstatSync(p).isFile();
const inside = (dir, p) => p.startsWith(dir + sep);

/** Symbolic links and special files under any of the directories, as a failure reason or null. */
function oddEntries(ctx, dirs) {
  for (const d of dirs) {
    if (!existsSync(d)) continue;
    const odd = nonRegularEntries(d);
    if (odd.length) return `${relative(ctx.root, d)}: symbolic link or special file: ${odd.join(", ")}`;
  }
  return null;
}

/** Key-position strings: a quoted property key that holds an escape, or is "constructor". */
function keyProblem({ src, strings }) {
  for (const s of strings) {
    if (!s.escaped && s.body !== "constructor") continue;
    const prev = src.slice(0, s.at).trimEnd().slice(-1);
    const after = src.slice(s.at + s.body.length + 2).trimStart()[0];
    const key = (prev === "{" || prev === ",") && after === ":";
    if (prev === "[" && s.escaped) return "a backslash escape inside a bracket-key string";
    if (key && s.escaped) return "a backslash escape inside a quoted property key";
    if (key && s.body === "constructor") return 'a quoted "constructor" key (destructuring)';
  }
  return null;
}

export function g2NoBareImport(ctx) {
  const hostDir = join(ctx.root, "host");
  const jsDir = join(ctx.out, "js");
  const typesDir = join(ctx.root, "types");
  const nodeDir = join(hostDir, "node") + sep;
  const link = oddEntries(ctx, [hostDir, jsDir, typesDir]);
  if (link) return link;
  // Scripts: every file of host/ and dist/js/. A relative specifier must reach a regular script file in them.
  function checkScript(f, inNodeDir) {
    const rel = relative(ctx.root, f);
    let text = readFileSync(f, "utf8");
    if (inside(jsDir, f)) for (const a of GENERATED_ALLOWED) text = text.replace(a, "0");
    const scan = scanImports(text);
    const { specifiers, nonLiteralDynamic, unparsed, src, code, unicodeEscape, error } = scan;
    if (/<!--|-->/.test(code)) return `${rel}: uses an HTML-like comment marker`;
    if (error) return `${rel}: ${error} (fail closed)`;
    if (unparsed) return `${rel}: ${unparsed} (fail closed)`;
    if (nonLiteralDynamic) return `${rel}: import() with a non-literal argument`;
    if (unicodeEscape) return `${rel}: backslash escape outside a string literal`;
    for (const [re, what] of FORBIDDEN) if (re.test(code)) return `${rel}: uses ${what}`;
    for (const [re, what] of FORBIDDEN_SRC) if (re.test(src)) return `${rel}: uses ${what}`;
    const key = keyProblem(scan);
    if (key) return `${rel}: uses ${key}`;
    if (!inNodeDir) for (const [re, what] of NODE_ONLY) if (re.test(src)) return `${rel}: uses ${what} outside host/node/`;
    if (inside(hostDir, f)) {
      for (const [re, what] of HOST_IDENTIFIERS) if (re.test(code)) return `${rel}: host/ uses ${what}`;
      if (!f.startsWith(nodeDir) && /(?<![.$\w#])process\b/.test(code)) return `${rel}: host/ uses process outside host/node/`;
      for (const c of scan.computed) {
        if (c.kind === "key") return `${rel}: host/ uses a computed object or destructuring key`;
        if (!HOST_LITERAL_ACCESS.test(src.slice(c.at))) return `${rel}: host/ uses a non-literal computed member access`;
      }
    }
    for (const s of specifiers) {
      if (isRelative(s)) {
        const target = resolve(dirname(f), s);
        if (!inside(hostDir, target) && !inside(jsDir, target)) return `${rel}: relative import "${s}" leaves host/ and dist/js/`;
        if (!isRegularFile(target) || !SCRIPT(target)) return `${rel}: relative import "${s}" is not a regular script file`;
        continue;
      }
      if (s.startsWith("node:") && inNodeDir) continue;
      return `${rel}: bare specifier "${s}"`;
    }
    return null;
  }
  for (const f of [...walk(jsDir, SCRIPT), ...walk(hostDir, SCRIPT)]) {
    const r = checkScript(f, f.startsWith(nodeDir));
    if (r) return r;
  }
  // Declarations: the same specifier rules; a relative one must reach a declaration file inside types/.
  for (const f of walk(typesDir, (p) => p.endsWith(".ts"))) {
    const rel = relative(ctx.root, f);
    const { specifiers, nonLiteralDynamic, unparsed, error } = scanImports(readFileSync(f, "utf8"));
    if (error) return `${rel}: ${error} (fail closed)`;
    if (unparsed) return `${rel}: ${unparsed} (fail closed)`;
    if (nonLiteralDynamic) return `${rel}: import() with a non-literal argument`;
    for (const s of specifiers) {
      if (!isRelative(s)) return `${rel}: bare specifier "${s}"`;
      const t = resolve(dirname(f), s);
      const ok = [t, t.replace(/\.js$/, ".d.ts")].some((c) => inside(typesDir, c) && isRegularFile(c));
      if (!ok) return `${rel}: relative import "${s}" does not resolve to a regular file in types/`;
    }
  }
  // The browser entry must not reach host/node/, and every file it reaches obeys the full script rules.
  const entry = join(hostDir, "browser.js");
  if (!existsSync(entry)) return null;
  const seen = new Set();
  const queue = [entry];
  while (queue.length) {
    const f = queue.pop();
    if (seen.has(f)) continue;
    seen.add(f);
    const rel = relative(ctx.root, f);
    if (f.startsWith(nodeDir)) return `${rel}: reachable from host/browser.js but lives in host/node/`;
    const r = checkScript(f, false);
    if (r) return `reachable from host/browser.js: ${r}`;
    for (const s of scanImports(readFileSync(f, "utf8")).specifiers) queue.push(resolve(dirname(f), s));
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G3

function trackedAndNew(root) {
  const r = run("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], { cwd: root });
  return r.split("\0").filter((p) => p && existsSync(join(root, p)));
}

/** Map of relative path -> sha256 for the reproducible artefacts of a dist directory. */
function distHashes(dist) {
  const map = new Map();
  for (const p of walk(dist, (f) => !relative(dist, f).startsWith("target/"))) {
    const rel = relative(dist, p);
    if (rel.endsWith(".core.wasm") || rel.endsWith(".component.wasm") || rel.startsWith("js/")) map.set(rel, sha256File(p));
  }
  return map;
}

export function g3TwoBuildIdentity(ctx) {
  if (ctx.quick) return "SKIPPED (quick)";
  const base = join(ctx.root, "tmp");
  const dirs = [join(base, "g3-a", "copy-one"), join(base, "g3-b", "nested", "copy-two")];
  const hashes = [];
  try {
    for (const d of dirs) {
      rmSync(d, { recursive: true, force: true });
      for (const p of trackedAndNew(ctx.root)) {
        mkdirSync(dirname(join(d, p)), { recursive: true });
        cpSync(join(ctx.root, p), join(d, p));
      }
      const r = tryRun("node", [join(d, "tools", "build.mjs")], {
        cwd: d,
        env: { ...process.env, SENTROPIC_TOOLS_DIR: ctx.toolsDir },
      });
      if (r.status !== 0) return `build failed in ${relative(ctx.root, d)}: ${(r.stderr || r.stdout).trim().slice(0, 400)}`;
      hashes.push(distHashes(join(d, "dist")));
    }
  } finally {
    for (const d of ["g3-a", "g3-b"]) rmSync(join(base, d), { recursive: true, force: true });
  }
  const [a, b] = hashes;
  if (a.size === 0) return "no artefact produced";
  for (const k of new Set([...a.keys(), ...b.keys()])) {
    if (a.get(k) !== b.get(k)) return `${k} differs between the two builds (${a.get(k)} vs ${b.get(k)})`;
  }
  // The shipped dist/ must be the artefact the two reproducible builds produce.
  const shipped = distHashes(ctx.out);
  for (const k of new Set([...a.keys(), ...shipped.keys()])) {
    if (a.get(k) !== shipped.get(k)) return `${k} in ${relative(ctx.root, ctx.out)} differs from the reproducible build (${shipped.get(k)} vs ${a.get(k)})`;
  }
  ctx.detail = [...a].map(([k, v]) => `  ${v}  ${k}`);
  return null;
}

// ---------------------------------------------------------------------------------------------- G4

const DROP = new Set(["docs", "package", "stability", "owner"]);

/** Expands type and interface ids of a wasm-tools WIT JSON document into a self-contained structure, so
 * two documents compare by what they declare (functions, types, interfaces) and not by their ids. */
export function normalizeWorld(json, worldName) {
  const w = json.worlds.find((x) => x.name === worldName) ?? (json.worlds.length === 1 ? json.worlds[0] : null);
  if (!w) return null;
  const type = (id) => walkValue(json.types[id]);
  const iface = (id) => walkValue(json.interfaces[id]);
  function walkValue(v, key = "") {
    if (Array.isArray(v)) return v.map((x) => walkValue(x, key));
    if (v && typeof v === "object") {
      const o = {};
      for (const k of Object.keys(v).sort()) {
        if (DROP.has(k)) continue;
        o[k] = k === "interface" && typeof v[k]?.id === "number" ? iface(v[k].id) : walkValue(v[k], k);
      }
      return o;
    }
    if (typeof v === "number" && key !== "length") return type(v);
    return v;
  }
  const side = (m) => Object.fromEntries(Object.keys(m ?? {}).sort().map((k) => [k, walkValue(m[k], k)]));
  return { imports: side(w.imports), exports: side(w.exports) };
}

export function g4ComponentValidity(ctx) {
  const list = declared(ctx);
  if (list.length === 0) return "no component is declared in tools/components.json";
  const wt = wasmToolsOf(ctx);
  // Annex A: the committed WIT package must parse and resolve.
  const annexA = join(ctx.root, "wit");
  if (existsSync(annexA)) {
    const r = tryRun(wt, ["component", "wit", annexA]);
    if (r.status !== 0) return `wit/: wasm-tools component wit failed: ${r.stderr.trim()}`;
  }
  for (const { name, def, path } of list) {
    if (!existsSync(path)) return `${name}: ${relative(ctx.root, path)} was not built`;
    const v = tryRun(wt, ["validate", path]);
    if (v.status !== 0) return `${name}: wasm-tools validate failed: ${v.stderr.trim()}`;
    const t = tryRun(wt, ["component", "targets", "-w", def.world, join(ctx.root, def.witDir), path]);
    if (t.status !== 0) return `${name}: component does not conform to world ${def.world}: ${t.stderr.trim()}`;
    const got = tryRun(wt, ["component", "wit", "--json", path]);
    const want = tryRun(wt, ["component", "wit", "--json", join(ctx.root, def.witDir)]);
    if (got.status !== 0 || want.status !== 0) return `${name}: cannot print WIT as JSON`;
    const a = normalizeWorld(JSON.parse(got.stdout), "root");
    const b = normalizeWorld(JSON.parse(want.stdout), def.world);
    if (!a || !b) return `${name}: world ${def.world} not found`;
    if (JSON.stringify(a) !== JSON.stringify(b)) return `${name}: the component's interface differs from the committed WIT world ${def.world}`;
  }
  const extra = readdirOrEmpty(ctx.out).filter((f) => f.endsWith(".component.wasm") && !(f.replace(/\.component\.wasm$/, "") in components(ctx.root)));
  if (extra.length) return `${extra.join(", ")}: not listed in tools/components.json`;
  return null;
}

function readdirOrEmpty(dir) {
  return existsSync(dir) ? readdirSync(dir) : [];
}

// ---------------------------------------------------------------------------------------------- G5

export function g5OneCoreModule(ctx) {
  const list = declared(ctx);
  if (list.length === 0) return "no component is declared in tools/components.json";
  for (const { name } of list) {
    const dir = join(ctx.out, "js", name);
    if (!existsSync(dir)) return `${name}: no generated JavaScript in ${relative(ctx.root, dir)}`;
    const wasm = walk(dir, (p) => p.endsWith(".wasm"));
    if (wasm.length !== 1) return `${name}: expected exactly one .wasm, found ${wasm.length}`;
    for (const js of walk(dir, (p) => p.endsWith(".js"))) {
      const m = readFileSync(js, "utf8").match(/[A-Za-z0-9+/]{1000,}={0,2}/);
      if (m) return `${relative(ctx.root, js)} embeds a base64 literal of ${m[0].length} characters`;
    }
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G6

// Any path into the alloc crate: `alloc::...`, `::alloc::...`, `use alloc`, `extern crate alloc`.
const ALLOC_PATH = /(?<![\w:])alloc\s*::|(?<![\w:])::\s*alloc\b|\buse\s+alloc\b|\bextern\s+crate\s+alloc\b/m;
const ALLOC_CRATE = ALLOC_PATH;

const withoutLineComments = (src) =>
  src
    .split("\n")
    .filter((l) => !l.trimStart().startsWith("//"))
    .join("\n");

function workspaceNames(root) {
  const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
  const members = (manifest.match(/members\s*=\s*\[([^\]]*)\]/)?.[1] ?? "").match(/"([^"]+)"/g)?.map((s) => s.slice(1, -1)) ?? [];
  const names = new Set();
  for (const m of members) {
    const f = join(root, m, "Cargo.toml");
    if (!existsSync(f)) continue;
    const n = readFileSync(f, "utf8").match(/^\s*name\s*=\s*"([^"]+)"/m);
    if (n) names.add(n[1]);
  }
  return names;
}

function sourcesUnder(dir) {
  return walk(dir, (p) => p.endsWith(".rs"));
}

export function g6ZeroCrate(ctx) {
  const lockPath = join(ctx.root, "Cargo.lock");
  if (!existsSync(lockPath)) return "Cargo.lock is missing";
  const members = workspaceNames(ctx.root);
  const lock = readFileSync(lockPath, "utf8");
  for (const m of lock.matchAll(/\[\[package\]\]\s*\nname\s*=\s*"([^"]+)"/g)) {
    if (!members.has(m[1])) return `Cargo.lock contains the non-workspace package ${m[1]}`;
  }
  if (/^\s*source\s*=/m.test(lock)) return "Cargo.lock has a package with a source (registry or git)";
  const kernelSrc = join(ctx.root, "kernel", "src");
  const probeSrc = join(ctx.root, "probe", "src");
  const link = oddEntries(ctx, [kernelSrc, probeSrc]);
  if (link) return link;
  // Only the declared, verified bindings directories are exempt from the heap rule, besides mem/.
  const bindingDirs = Object.values(components(ctx.root))
    .filter((d) => d.bindings)
    .map((d) => join(ctx.root, d.bindings.dir) + sep);
  for (const f of sourcesUnder(kernelSrc)) {
    const rel = relative(kernelSrc, f);
    if (rel.startsWith("mem/") || bindingDirs.some((d) => f.startsWith(d))) continue;
    if (ALLOC_PATH.test(withoutLineComments(readFileSync(f, "utf8")))) return `kernel/src/${rel} uses the alloc crate outside kernel/src/mem/ and the declared bindings directories`;
  }
  for (const f of sourcesUnder(probeSrc)) {
    if (ALLOC_CRATE.test(withoutLineComments(readFileSync(f, "utf8")))) return `probe/src/${relative(probeSrc, f)} uses the alloc crate`;
  }
  // kernel/src/bindings/ is exempt from the heap rule only because it is exactly the generated output.
  for (const [name, def] of Object.entries(components(ctx.root))) {
    if (!def.bindings) continue;
    const problems = bindingsProblems(ctx.root, name, def.bindings, toolBin("wit-bindgen", ctx.toolsDir, ctx.root));
    if (problems.length) return problems[0];
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G7

export function checkRootManifest(root, file = "package.json") {
  const f = join(root, file);
  if (!existsSync(f)) return "package.json is missing";
  const p = readJson(f);
  for (const k of ["dependencies", "peerDependencies", "optionalDependencies"]) {
    if (Object.keys(p[k] ?? {}).length) return `package.json has ${k} entries`;
  }
  for (const s of ["preinstall", "install", "postinstall", "prepare"]) {
    if (p.scripts && s in p.scripts) return `package.json has a ${s} script`;
  }
  return null;
}

export function g7Manifests(ctx) {
  const own = checkRootManifest(ctx.root, ctx.manifestFile);
  if (own) return own;
  if (ctx.skipMake) return null;
  for (const target of ["check-attribution", "check-no-python", "check-no-deps"]) {
    const r = tryRun("make", ["--no-print-directory", "-C", ctx.root, target]);
    if (r.status !== 0) return `make ${target} failed: ${(r.stdout + r.stderr).trim().slice(0, 400)}`;
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- G8

function findNodeModules(dir, root, found) {
  for (const name of readdirSync(dir)) {
    if (name === ".git" || name === "tmp" || name === "target") continue;
    const p = join(dir, name);
    let st;
    try {
      st = lstatSync(p);
    } catch {
      continue;
    }
    if (!st.isDirectory()) continue;
    if (name === "node_modules") found.push(relative(root, p));
    else findNodeModules(p, root, found);
  }
  return found;
}

export function g8ToolPins(ctx) {
  const list = pins(ctx.root);
  const bad = pinsProblem(list);
  if (bad) return bad;
  for (const pin of list) {
    const problem = toolProblem(pin, ctx.toolsDir);
    if (problem) return problem;
  }
  const committed = join(ctx.root, "tools", "package-lock.json");
  const early = lockProblem(readJson(committed), readJson(join(ctx.root, "tools", "package.json")));
  if (early) return early;
  const staged = join(ctx.toolsDir, "npm", "package-lock.json");
  if (!existsSync(staged)) return "staged npm lockfile is missing";
  if (lockResolution(readJson(staged)) !== lockResolution(readJson(committed))) return "staged npm lockfile resolves differently from tools/package-lock.json";
  const npm = npmProblem(join(ctx.toolsDir, "npm"), readJson(join(ctx.root, "tools", "package.json")).devDependencies);
  if (npm) return npm;
  const nm = findNodeModules(ctx.root, ctx.root, []);
  if (nm.length) return `node_modules inside the repository: ${nm.join(", ")}`;
  return null;
}

export const GATES = [
  ["G1", "zero-import", g1ZeroImport],
  ["G2", "no bare import", g2NoBareImport],
  ["G3", "two-build identity", g3TwoBuildIdentity],
  ["G4", "component validity", g4ComponentValidity],
  ["G5", "one core module", g5OneCoreModule],
  ["G6", "zero crate, heap discipline", g6ZeroCrate],
  ["G7", "manifests", g7Manifests],
  ["G8", "tool pins", g8ToolPins],
];
