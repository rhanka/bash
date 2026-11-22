#!/usr/bin/env node
// Shipped facade packer (spec/lots/L2c.md section 5): one browser-safe minified shared module
// per package plus binding-specific entries, built from one source graph with the lockfile's
// oxc-minify. The default binding bundles the first-party adapter; the jco package aliases it
// to a generated adapter over the pinned guarded jco glue. No dependency, no network.
import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { stripComments } from "./gates.mjs";
import { repoRoot, toolsDir } from "./lib.mjs";

// Splits ESM source into code and non-code segments so renames never touch strings, comments,
// template raw text or regex literals. Template interpolations stay code.
function splitCode(src) {
  const segs = [];
  let i = 0;
  let buf = "";
  const push = (code, text) => {
    if (!text) return;
    const last = segs[segs.length - 1];
    if (last && last.code === code) last.text += text;
    else segs.push({ code, text });
  };
  const n = src.length;
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === "/" && d === "*") {
      const e = src.indexOf("*/", i + 2);
      push(false, src.slice(i, e < 0 ? n : e + 2));
      i = e < 0 ? n : e + 2;
    } else if (c === "/" && d === "/") {
      const e = src.indexOf("\n", i);
      push(false, src.slice(i, e < 0 ? n : e));
      i = e < 0 ? n : e;
    } else if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && src[j] !== c) j += src[j] === "\\" ? 2 : 1;
      push(false, src.slice(i, j + 1));
      i = j + 1;
    } else if (c === "`") {
      let j = i + 1;
      push(false, "`");
      i++;
      while (i < n && src[i] !== "`") {
        if (src[i] === "\\") {
          push(false, src.slice(i, i + 2));
          i += 2;
        } else if (src[i] === "$" && src[i + 1] === "{") {
          push(false, "${");
          i += 2;
          let depth = 1;
          let expr = "";
          while (i < n && depth > 0) {
            if (src[i] === "{") depth++;
            else if (src[i] === "}") depth--;
            if (depth > 0) expr += src[i];
            i++;
          }
          for (const part of splitCode(expr)) push(part.code, part.text);
          push(false, "}");
        } else {
          push(false, src[i]);
          i++;
        }
      }
      push(false, "`");
      i++;
      void j;
    } else if (c === "/" && isRegexStart(segs, buf)) {
      let j = i + 1;
      let cls = false;
      while (j < n && (cls || src[j] !== "/")) {
        if (src[j] === "\\") j++;
        else if (src[j] === "[") cls = true;
        else if (src[j] === "]") cls = false;
        j++;
      }
      // Include the closing `/` (when present) and its flags so the literal stays
      // one non-code segment; otherwise the closer is re-read as division.
      if (j < n && src[j] === "/") j++;
      while (j < n && /[a-z]/i.test(src[j])) j++;
      push(false, src.slice(i, j));
      i = j;
    } else {
      buf += c;
      push(true, c);
      i++;
    }
  }
  return segs;
}

function isRegexStart(segs, buf) {
  const code = segs.filter((s) => s.code).map((s) => s.text).join("") + buf;
  const m = code.match(/([^\s])/g);
  const prev = m ? m[m.length - 1] : "";
  if (prev === "" || "(,=:[!&|?{};+-*%<>~^".includes(prev)) return true;
  // A `/` after a keyword that cannot end an expression (as in `return /re/`) opens a
  // regex literal, never division: without this the literal body is treated as code and a
  // later rename can rewrite identifiers inside the pattern (host/core/driver.js does this).
  return /\b(?:return|typeof|case|throw|do|else|in|of|new|delete|void|yield|await|instanceof)\s*$/.test(code);
}

const IMPORT = /^[ \t]*import\s+(.+?)\s+from\s*["']([^"']+)["'];?[ \t]*$/;
const SIDE_IMPORT = /^[ \t]*import\s*["']([^"']+)["'];?[ \t]*$/;
const REEXPORT = /^[ \t]*export\s*\{([^}]*)\}\s*from\s*["']([^"']+)["'];?[ \t]*$/;

// Joins statement lines so a multi-line `import` clause (or `{ ... } from` re-export)
// matches as one statement. Every other line passes through unchanged.
function joinStatements(src) {
  const stmts = [];
  let buf = "";
  for (const line of src.split("\n")) {
    buf += (buf ? " " : "") + line;
    const open = /^[ \t]*import\b/.test(buf) || /^[ \t]*export\s*\{/.test(buf);
    if (open && !/;[ \t]*$/.test(buf)) continue;
    stmts.push(buf);
    buf = "";
  }
  if (buf) stmts.push(buf);
  return stmts;
}

// Parses the import statements of a module (the graph uses single-line forms, plus the
// multi-line clauses of the node adapters).
// `export { ... } from` re-exports are recorded with `reExport: true`: they create no local
// binding, but the target still belongs to the entry closure (the node adapters).
function parseImports(src, file) {
  const imports = [];
  for (const line of joinStatements(src)) {
    let m = line.match(IMPORT);
    if (m) {
      const [, clause, spec] = m;
      const names = [];
      const ns = clause.match(/^\*\s*as\s+([A-Za-z_$][\w$]*)$/);
      if (ns) {
        imports.push({ spec, namespace: ns[1], names });
        continue;
      }
      const def = clause.match(/^([A-Za-z_$][\w$]*)$/);
      if (def) throw new Error(file + ": default imports are not supported");
      const list = clause.match(/^\{([^}]*)\}$/);
      if (!list) throw new Error(file + ": unsupported import clause " + clause.trim());
      for (const part of list[1].split(",")) {
        const item = part.trim();
        if (!item) continue;
        const alias = item.match(/^([A-Za-z_$][\w$]*)\s+as\s+([A-Za-z_$][\w$]*)$/);
        if (alias) names.push({ imported: alias[1], local: alias[2] });
        else if (/^[A-Za-z_$][\w$]*$/.test(item)) names.push({ imported: item, local: item });
        else throw new Error(file + ": unsupported import item " + item);
      }
      imports.push({ spec, namespace: null, names });
      continue;
    }
    m = line.match(SIDE_IMPORT);
    if (m) throw new Error(file + ": side-effect imports are not supported");
    m = line.match(REEXPORT);
    if (m) {
      const [, clause, spec] = m;
      const names = [];
      for (const part of clause.split(",")) {
        const item = part.trim();
        if (!item) continue;
        const alias = item.match(/^([A-Za-z_$][\w$]*)\s+as\s+([A-Za-z_$][\w$]*)$/);
        if (alias) names.push({ imported: alias[1], local: alias[2] });
        else if (/^[A-Za-z_$][\w$]*$/.test(item)) names.push({ imported: item, local: item });
        else throw new Error(file + ": unsupported re-export item " + item);
      }
      imports.push({ spec, namespace: null, names, reExport: true });
    }
  }
  return imports;
}

function stripImportLines(src) {
  return joinStatements(src).filter((line) => !IMPORT.test(line)).join("\n");
}

function stripExportKeyword(src) {
  return splitCode(src).map((seg) => {
    if (!seg.code) return seg.text;
    return seg.text.replace(/^[ \t]*export[ \t]+(?=(?:async[ \t]+)?(?:function\*?|class|const|let|var)[ \t]+)/gm, "");
  }).join("");
}

// Resolves a relative specifier from one module file to an absolute path.
function resolveSpec(from, spec) {
  if (!spec.startsWith("./") && !spec.startsWith("../")) {
    throw new Error(from + ": non-relative specifier " + spec);
  }
  return resolve(dirname(from), spec);
}

// Loads the transitive closure of an entry, applying the adapter alias. Externals (kept as
// separate shipped files) are recorded without being read.
function loadGraph(entry, { alias = {}, externals = new Set(), root, modules = new Map() }) {
  const visit = (file) => {
    const id = file;
    if (modules.has(id)) return modules.get(id);
    if (externals.has(id)) {
      const rec = { file, src: null, imports: [], external: true };
      modules.set(id, rec);
      return rec;
    }
    if (!file.startsWith(root + "/")) throw new Error("module escapes the repository: " + file);
    const src = readFileSync(file, "utf8");
    const rec = { file, src, imports: parseImports(src, relative(root, file)) };
    modules.set(id, rec);
    for (const imp of rec.imports) {
      // Platform imports (`node:...`) stay external: recorded, never visited.
      if (!imp.spec.startsWith("./") && !imp.spec.startsWith("../")) {
        imp.target = imp.spec;
        continue;
      }
      let target = resolveSpec(file, imp.spec);
      if (target === alias.from) target = alias.to;
      imp.target = target;
      visit(target);
    }
    return rec;
  };
  visit(entry);
  return modules;
}

// Deterministic topological order: dependencies first, lexical ties.
function topoOrder(modules, entry) {
  const order = [];
  const seen = new Set();
  const visit = (id) => {
    if (seen.has(id)) return;
    seen.add(id);
    const rec = modules.get(id);
    for (const imp of [...rec.imports].sort((a, b) => (a.target < b.target ? -1 : 1))) {
      if (!modules.has(imp.target)) {
        // Platform imports (`node:...`) are not repository modules; anything else
        // missing here is a graph bug, never a platform import.
        if (imp.target.startsWith("/") || imp.target.startsWith(".")) throw new Error("unknown module: " + imp.target);
        continue;
      }
      visit(imp.target);
    }
    order.push(id);
  };
  visit(entry);
  return order;
}

// Declared export names of a module record (declaration exports only; lists are entry-only).
function declaredExports(rec) {
  const names = [];
  for (const seg of splitCode(rec.src)) {
    if (!seg.code) continue;
    for (const line of seg.text.split("\n")) {
      const m = line.match(/^[ \t]*export[ \t]+(?:async[ \t]+)?(?:function\*?|class|const|let|var)[ \t]+([A-Za-z_$][\w$]*)/);
      if (m) names.push(m[1]);
      else if (/^[ \t]*export[ \t]*\{/.test(line) && !REEXPORT.test(line)) throw new Error(rec.file + ": export lists are entry-only");
    }
  }
  return names;
}

// Flattens the shared modules into one top-level scope (alpha-renaming collisions) before
// minification, so oxc-minify sees cross-module names for mangling and DCE instead of 15
// IIFE closures. Imports from earlier shared modules become direct references; `externals`
// stay module-level imports. Returns { code, mapping, header } where mapping maps
// `moduleId::exportName` to its flattened top-level name.
function prevNonSpace(text, idx) {
  for (let i = idx - 1; i >= 0; i--) {
    const c = text[i];
    if (c !== " " && c !== "\t" && c !== "\n" && c !== "\r") return { char: c, index: i };
  }
  return { char: "", index: -1 };
}

function nextNonSpace(text, idx) {
  for (let i = idx; i < text.length; i++) {
    const c = text[i];
    if (c !== " " && c !== "\t" && c !== "\n" && c !== "\r") return { char: c, index: i };
  }
  return { char: "", index: -1 };
}

function isSpreadPreceding(text, idx) {
  let i = idx - 1;
  while (i >= 0 && (text[i] === " " || text[i] === "\t" || text[i] === "\n" || text[i] === "\r")) i--;
  return i >= 2 && text[i] === "." && text[i - 1] === "." && text[i - 2] === ".";
}

// Top-level declaration names at brace depth 0 (code only, strings/comments excluded).
function topDeclNames(src) {
  const names = [];
  let depth = 0;
  const segs = splitCode(src);
  const decl = /(?:^|[;}\n])\s*(?:export\s+)?(?:async\s+)?(?:function\*?|class|const|let|var)\s+([A-Za-z_$][\w$]*)/g;
  for (const seg of segs) {
    if (!seg.code) continue;
    const text = seg.text;
    let m;
    decl.lastIndex = 0;
    // Collect matches with their depth by scanning braces up to each match.
    let pos = 0;
    let d = depth;
    const events = [];
    while ((m = decl.exec(text)) !== null) {
      events.push({ index: m.index + m[0].length - m[1].length, name: m[1] });
    }
    // Walk text tracking depth to assign each event its depth at match start.
    let e = 0;
    let cur = 0;
    for (let i = 0; i < text.length && e < events.length; i++) {
      while (e < events.length && events[e].index === i) {
        if (d === 0) names.push(events[e].name);
        e++;
      }
      const c = text[i];
      if (c === "{") d++;
      else if (c === "}") d = Math.max(0, d - 1);
      void cur;
    }
    // Update depth for the next segment by counting braces in this segment.
    for (const c of text) {
      if (c === "{") depth++;
      else if (c === "}") depth = Math.max(0, depth - 1);
    }
    void pos;
  }
  return names;
}

// Replaces identifier `oldName` with `newName` in code only, preserving member properties
// (`obj.old`), property definitions (`{ old: ... }`) and shorthand (`{ old }` -> `{ old: new }`).
// `$` counts as an identifier character throughout: `\b` would otherwise split `name$1`.
function replaceIdent(body, oldName, newName) {
  if (oldName === newName) return body;
  const word = new RegExp("\\b" + oldName.replace(/[$]/g, "\\$&") + "\\b", "g");
  const shorthand = new RegExp("([{,]\\s*)\\b" + oldName.replace(/[$]/g, "\\$&") + "\\b(\\s*[,}])", "g");
  // Innermost unclosed bracket before a match, advancing a stack carried across segments
  // (non-code segments hold no real brackets). Only `{` admits the shorthand reading:
  // inside `(...)` or `[...]` a `, old,` is a call argument or element, never shorthand.
  const stack = [];
  const innerOpener = (prefix) => {
    const st = [...stack];
    for (const c of prefix) {
      if (c === "{" || c === "(" || c === "[") st.push(c);
      else if (c === "}" || c === ")" || c === "]") {
        const want = c === "}" ? "{" : c === ")" ? "(" : "[";
        if (st[st.length - 1] === want) st.pop();
      }
    }
    return st[st.length - 1];
  };
  const scanBraces = (text) => {
    for (const c of text) {
      if (c === "{" || c === "(" || c === "[") stack.push(c);
      else if (c === "}" || c === ")" || c === "]") {
        const want = c === "}" ? "{" : c === ")" ? "(" : "[";
        if (stack[stack.length - 1] === want) stack.pop();
      }
    }
  };
  return splitCode(body).map((seg) => {
    if (!seg.code) return seg.text;
    let text = seg.text;
    // Expand shorthands first, but only inside `{...}`: `{ old }`, `{ old,`, `, old,`, `, old }`
    // -> `{ old: new ... }`. Any other bracket context keeps the bare reference for the
    // general replacement below.
    text = text.replace(shorthand, (m, a, b, offset, whole) =>
      innerOpener(whole.slice(0, offset + a.length)) === "{" ? a + oldName + ": " + newName + b : m);
    // General replacement with property/member guards.
    let out = "";
    let last = 0;
    word.lastIndex = 0;
    let m;
    while ((m = word.exec(text)) !== null) {
      const start = m.index;
      const end = start + oldName.length;
      if (start > 0 && text[start - 1] === "$") continue;
      if (text[end] === "$") continue;
      const prev = prevNonSpace(text, start);
      if (prev.char === "." && !isSpreadPreceding(text, start)) continue;
      const prev2 = prevNonSpace(text, start);
      const nxt = nextNonSpace(text, end);
      if ((prev2.char === "{" || prev2.char === ",") && nxt.char === ":") continue;
      out += text.slice(last, start) + newName;
      last = end;
    }
    out += text.slice(last);
    scanBraces(out);
    return out;
  }).join("");
}

// Tokenizes code segments of `src` (replacing strings, regexes and template text with a LIT
// marker while keeping template interpolations as code) so binding-declaration scanning sees
// exact syntax without regex guessing.
function tokenizeDecls(src) {
  const tokens = [];
  for (const seg of splitCode(src)) {
    if (!seg.code) {
      // Comments are skipped (no token); strings, regexes and template literals become LIT so
      // adjacent identifiers cannot merge across them.
      if (!seg.text.startsWith("//") && !seg.text.startsWith("/*")) tokens.push({ type: "lit", text: "" });
      continue;
    }
    const text = seg.text;
    let i = 0;
    while (i < text.length) {
      const c = text[i];
      if (c === " " || c === "\t" || c === "\n" || c === "\r") { i++; continue; }
      if ((c >= "A" && c <= "Z") || (c >= "a" && c <= "z") || c === "_" || c === "$") {
        let j = i + 1;
        while (j < text.length && /[\w$]/.test(text[j])) j++;
        tokens.push({ type: "id", text: text.slice(i, j) });
        i = j;
      } else if (c >= "0" && c <= "9") {
        let j = i + 1;
        while (j < text.length && /[\w.]/.test(text[j])) j++;
        tokens.push({ type: "lit", text: text.slice(i, j) });
        i = j;
      } else if (text.startsWith("...", i)) {
        tokens.push({ type: "punc", text: "..." });
        i += 3;
      } else if (text.startsWith("=>", i)) {
        tokens.push({ type: "punc", text: "=>" });
        i += 2;
      } else if (text.startsWith("?.", i)) {
        tokens.push({ type: "punc", text: "?." });
        i += 2;
      } else {
        tokens.push({ type: "punc", text: c });
        i++;
      }
    }
  }
  return tokens;
}

function collectDecls(body) {
  const tokens = tokenizeDecls(body);
  const decls = new Set();
  const n = tokens.length;
  const isPunc = (i, s) => i < n && tokens[i].type === "punc" && tokens[i].text === s;
  const isPropAccess = (i) => i > 0 && (isPunc(i - 1, ".") || isPunc(i - 1, "?."));

  // Finds the matching closer `)` / `]` / `}` starting from the opener at `openIdx`.
  function matchClose(openIdx) {
    const open = tokens[openIdx].text;
    const close = open === "(" ? ")" : open === "[" ? "]" : "}";
    let depth = 0;
    for (let j = openIdx; j < n; j++) {
      if (tokens[j].type !== "punc") continue;
      if (tokens[j].text === open) depth++;
      else if (tokens[j].text === close && --depth === 0) return j;
    }
    return n - 1;
  }

  // Skips an initializer expression (`= ...`) inside a pattern or declarator up to one of
  // `stops` at depth 0, returning the index of the stopping token.
  function skipInit(i, stops) {
    let dParen = 0, dBrack = 0, dBrace = 0;
    while (i < n) {
      const t = tokens[i];
      if (t.type === "punc") {
        if (dParen === 0 && dBrack === 0 && dBrace === 0 && stops.includes(t.text)) return i;
        if (t.text === "(") dParen++;
        else if (t.text === ")") { if (dParen === 0) return i; dParen--; }
        else if (t.text === "[") dBrack++;
        else if (t.text === "]") { if (dBrack === 0) return i; dBrack--; }
        else if (t.text === "{") dBrace++;
        else if (t.text === "}") { if (dBrace === 0) return i; dBrace--; }
      }
      i++;
    }
    return i;
  }

  // Parses a binding pattern starting at `i` and returns the index after the pattern.
  function parsePattern(i) {
    if (i >= n) return i;
    if (tokens[i].type === "id") {
      decls.add(tokens[i].text);
      return i + 1;
    }
    if (isPunc(i, "[")) {
      i++;
      while (i < n && !isPunc(i, "]")) {
        if (isPunc(i, ",")) { i++; continue; }
        if (isPunc(i, "...")) i++;
        i = parsePattern(i);
        if (isPunc(i, "=")) i = skipInit(i + 1, [",", "]"]);
        if (isPunc(i, ",")) i++;
      }
      return isPunc(i, "]") ? i + 1 : i;
    }
    if (isPunc(i, "{")) {
      i++;
      while (i < n && !isPunc(i, "}")) {
        if (isPunc(i, ",")) { i++; continue; }
        if (isPunc(i, "...")) {
          i++;
          if (i < n && tokens[i].type === "id") decls.add(tokens[i++].text);
          if (isPunc(i, ",")) i++;
          continue;
        }
        if (isPunc(i, "[")) {
          i = matchClose(i) + 1;
          if (isPunc(i, ":")) i = parsePattern(i + 1);
        } else if (i + 1 < n && isPunc(i + 1, ":")) {
          i = parsePattern(i + 2);
        } else if (tokens[i].type === "id") {
          decls.add(tokens[i++].text);
        } else {
          i++;
        }
        if (isPunc(i, "=")) i = skipInit(i + 1, [",", "}"]);
        if (isPunc(i, ",")) i++;
      }
      return isPunc(i, "}") ? i + 1 : i;
    }
    return i;
  }

  function parseParams(openIdx) {
    let i = openIdx + 1;
    const end = matchClose(openIdx);
    while (i < end) {
      if (isPunc(i, ",")) { i++; continue; }
      if (isPunc(i, "...")) i++;
      const next = parsePattern(i);
      i = next === i ? i + 1 : next;
      if (i < end && isPunc(i, "=")) i = skipInit(i + 1, [",", ")"]);
      if (i < end && isPunc(i, ",")) i++;
    }
    return end + 1;
  }

  const NON_METHOD = new Set(["if", "while", "for", "switch", "with", "catch"]);
  for (let i = 0; i < n; i++) {
    const t = tokens[i];
    if (t.type !== "id" && !isPunc(i, "(")) continue;
    if (t.type === "id" && !isPropAccess(i)) {
      if (t.text === "const" || t.text === "var" ||
          (t.text === "let" && i + 1 < n && (tokens[i + 1].type === "id" || isPunc(i + 1, "{") || isPunc(i + 1, "[")))) {
        let j = i + 1;
        while (j < n) {
          const next = parsePattern(j);
          if (next === j) break;
          j = next;
          if (isPunc(j, "=")) j = skipInit(j + 1, [",", ";", ")"]);
          if (isPunc(j, ",")) { j++; continue; }
          break;
        }
      } else if (t.text === "function") {
        let j = i + 1;
        if (isPunc(j, "*")) j++;
        if (j < n && tokens[j].type === "id") decls.add(tokens[j++].text);
        if (isPunc(j, "(")) parseParams(j);
      } else if (t.text === "class") {
        if (i + 1 < n && tokens[i + 1].type === "id" &&
            tokens[i + 1].text !== "extends") {
          decls.add(tokens[i + 1].text);
        }
      } else if (t.text === "catch") {
        if (isPunc(i + 1, "(")) parseParams(i + 1);
      } else if (isPunc(i + 1, "=>")) {
        decls.add(t.text);
      } else if (isPunc(i + 1, "(") && !NON_METHOD.has(t.text)) {
        const close = matchClose(i + 1);
        if (isPunc(close + 1, "{")) parseParams(i + 1);
      }
    } else if (isPunc(i, "(")) {
      const close = matchClose(i);
      if (isPunc(close + 1, "=>")) parseParams(i);
    }
  }
  return decls;
}

function hasWord(decls, word) {
  return decls.has(word);
}

function flattenShared(modules, sharedIds, root, externals = new Map(), lazyRewritesByModule = null) {
  const order = [...sharedIds];
  const set = new Set(sharedIds);
  const used = new Set();
  for (const id of order) {
    for (const imp of modules.get(id).imports) {
      if (!imp.reExport && externals.has(imp.target)) {
        for (const { local } of imp.names) used.add(local);
      }
    }
  }
  const mapping = new Map();
  const bodies = [];
  const externalBySpec = new Map();
  for (const id of order) {
    const rec = modules.get(id);
    const bindings = rec.imports.filter((imp) => !imp.reExport);
    const allLocals = bindings.flatMap((imp) => imp.names.map((x) => x.local));
    for (const imp of bindings) {
      if (imp.namespace) throw new Error(rec.file + ": namespace imports are entry-only");
      if (externals.has(imp.target)) {
        const spec = externals.get(imp.target);
        let specMap = externalBySpec.get(spec);
        if (!specMap) externalBySpec.set(spec, specMap = new Map());
        for (const { imported, local } of imp.names) {
          if (specMap.has(local) && specMap.get(local) !== imported) {
            throw new Error(rec.file + ": external import conflict on " + local);
          }
          specMap.set(local, imported);
        }
      }
      if (!imp.namespace && !externals.has(imp.target) && !set.has(imp.target)) {
        throw new Error(rec.file + ": import escapes the shared set " + imp.target);
      }
    }
    for (const imp of bindings) {
      if (imp.namespace || externals.has(imp.target)) continue;
      const provider = modules.get(imp.target);
      for (const { imported } of imp.names) {
        if (!declaredExports(provider).includes(imported)) {
          throw new Error(rec.file + ": no export " + imported);
        }
      }
    }
    // Top-level renames first so provider names stay stable.
    const topNames = topDeclNames(rec.src);
    const rename = new Map();
    for (const name of topNames) {
      if (allLocals.includes(name)) {
        throw new Error(rec.file + ": import shadows a declaration: " + name);
      }
      if (used.has(name)) {
        let n = 1;
        let candidate = name + "$" + n;
        while (used.has(candidate)) {
          n++;
          candidate = name + "$" + n;
        }
        rename.set(name, candidate);
        used.add(candidate);
      } else {
        used.add(name);
      }
    }
    let body = stripExportKeyword(stripImportLines(rec.src)).trim();
    const lazyMap = lazyRewritesByModule?.get(id);
    if (lazyMap) body = rewriteLazyImports(body, lazyMap);
    for (const [oldName, newName] of rename) body = replaceIdent(body, oldName, newName);
    // Imports become direct references. When the provider name already occurs as a shadowing
    // nested binding, rename that nested binding first so the replacement cannot be captured.
    const decls = collectDecls(body);
    for (const imp of bindings) {
      if (imp.namespace || externals.has(imp.target)) continue;
      for (const { imported, local } of imp.names) {
        const flat = mapping.get(imp.target + "::" + imported);
        if (!flat) throw new Error(rec.file + ": provider not yet flattened for " + imported);
        if (local === flat) continue;
        if (hasWord(decls, flat)) {
          let n = 1;
          let candidate = flat + "$" + n;
          while (used.has(candidate)) {
            n++;
            candidate = flat + "$" + n;
          }
          body = replaceIdent(body, flat, candidate);
          decls.delete(flat);
          decls.add(candidate);
          used.add(candidate);
        }
        body = replaceIdent(body, local, flat);
      }
    }
    for (const { local } of rec.imports.flatMap((imp) => imp.names)) {
      if (externals.has(rec.imports.find((imp) => imp.names.some((x) => x.local === local))?.target)) {
        used.add(local);
      }
    }
    if (/\bimport\.meta\b/.test(stripStrings(body))) {
      throw new Error(rec.file + ": import.meta cannot be bundled");
    }
    for (const name of declaredExports(rec)) {
      if (allLocals.includes(name)) throw new Error(rec.file + ": import shadows an export: " + name);
      mapping.set(id + "::" + name, rename.get(name) ?? name);
    }
    bodies.push("// " + relative(root, id) + "\n" + body);
  }
  const keptHeader = [...externalBySpec].map(([spec, specMap]) =>
    "import { " + [...specMap].map(([local, imported]) =>
      imported === local ? local : imported + " as " + local).join(", ") +
    " } from " + JSON.stringify(spec) + ";");
  return { code: bodies.join("\n\n"), mapping, header: keptHeader };
}

function stripStrings(src) {
  return splitCode(src).map((seg) => (seg.code ? seg.text : " ")).join("");
}

// Rewrites one entry file: shared imports point at the shared chunk, namespace imports expand,
// core URLs point at the shipped layout. Returns { code, needed } where needed maps each shared
// public name to its flattened reference (for the final export statement).
function rewriteEntry(entryFile, outEntry, sharedFile, modules, sharedIds, mapping, root, urls) {
  const ref = (target, name) => {
    const flat = mapping.get(target + "::" + name);
    if (!flat) throw new Error(entryFile + ": no flattened export " + name);
    return flat;
  };
  const rec = modules.get(entryFile);
  const needed = new Map(); // public name -> local name in the entry
  const lines = [];
  for (const line of joinStatements(rec.src)) {
    const m = line.match(IMPORT);
    if (!m) {
      lines.push(line);
      continue;
    }
    const [, clause, spec] = m;
    if (!spec.startsWith(".")) {
      // Bare and node: specifiers (entry-private platform imports) pass through.
      lines.push(line);
      continue;
    }
    const target = resolveSpec(entryFile, spec);
    if (!sharedIds.has(target)) {
      // Entry-private modules (the node loader) keep their relative specifier.
      lines.push(line);
      continue;
    }
    const targetExports = declaredExports(modules.get(target));
    const specOut = JSON.stringify(relativeTo(sharedFile, outEntry));
    const ns = clause.match(/^\*\s*as\s+([A-Za-z_$][\w$]*)$/);
    if (ns) {
      const destructure = rec.src.match(new RegExp("export const \\{([^}]*)\\} = " + ns[1] + ";"));
      if (!destructure) throw new Error(entryFile + ": namespace use beyond the entry re-export is not supported");
      const names = destructure[1].split(",").map((s) => s.trim()).filter(Boolean);
      for (const name of names) {
        if (!targetExports.includes(name)) throw new Error(entryFile + ": no export " + name);
        needed.set(name, ref(target, name));
      }
      lines.push("import { " + names.join(", ") + " } from " + specOut + ";");
      continue;
    }
    const rewritten = [];
    const list = clause.match(/^\{([^}]*)\}$/)[1];
    for (const part of list.split(",")) {
      const item = part.trim();
      if (!item) continue;
      const alias = item.match(/^([A-Za-z_$][\w$]*)\s+as\s+([A-Za-z_$][\w$]*)$/);
      const [imported, local] = alias ? [alias[1], alias[2]] : [item, item];
      if (!targetExports.includes(imported)) throw new Error(entryFile + ": no export " + imported);
      needed.set(imported, ref(target, imported));
      rewritten.push(imported === local ? local : imported + " as " + local);
    }
    lines.push("import { " + rewritten.join(", ") + " } from " + specOut + ";");
  }
  let code = lines.join("\n");
  // The namespace re-export becomes a plain re-export of the shared names.
  code = code.replace(/export const \{([^}]*)\} = [A-Za-z_$][\w$]*;/, (_, names) => "export { " + names.split(",").map((s) => s.trim()).filter(Boolean).join(", ") + " };");
  for (const [from, to] of urls) {
    if (!code.includes(from)) throw new Error(entryFile + ": core URL literal not found: " + from);
    code = code.split(from).join(to);
  }
  return { code, needed };
}

function relativeTo(target, from) {
  let rel = relative(dirname(from), target);
  if (!rel.startsWith(".")) rel = "./" + rel;
  return rel.split("\\").join("/");
}

let minifyImpl = null;
async function minifyFile(name, code) {
  if (!minifyImpl) {
    const pkg = join(toolsDir(), "npm", "node_modules", "oxc-minify", "index.js");
    minifyImpl = (await import(pathToFileURL(pkg).href)).minifySync;
  }
  const out = minifyImpl(name, code, { module: true });
  if (out.errors.length) throw new Error("oxc-minify " + name + ": " + JSON.stringify(out.errors).slice(0, 500));
  if (!out.code) throw new Error("oxc-minify " + name + ": empty output");
  return out.code;
}

// Pure units exported for the collision-fixture suite (test/bundle-flatten.test.mjs):
// splitCode proves renames never touch strings, comments, template raw text or regex
// literals; flattenShared proves cross-module collisions alpha-rename instead of colliding.
export { splitCode, flattenShared, collectDecls, rewriteLazyImports };

// Collects static relative `import()` specifiers (lazy chunks) from one source.
// Comments are stripped first (JSDoc type imports such as
// `import("../../types/...")` are not runtime chunks); strings stay so the
// literal remains visible. Only single- or double-quoted literals qualify;
// anything else stays a G2 error at gate time and is ignored here.
function lazySpecs(src) {
  const stripped = stripComments(src);
  const out = [];
  const re = /\bimport\s*\(\s*(['"])(\.[^'"]*?\.js)\1\s*\)/g;
  let m;
  while ((m = re.exec(stripped)) !== null) out.push(m[2]);
  return out;
}

// Rewrites literal `import("./...")` specifiers inside one module body using `splitCode` so
// non-import strings, comments, template raw text and regex literals are never touched, and
// both single- and double-quoted specifiers are rewritten per originating module.
function rewriteLazyImports(src, rewrites) {
  if (!rewrites || rewrites.size === 0) return src;
  const segs = splitCode(src);
  const isWsOrComment = (seg) =>
    seg.code ? /^\s*$/.test(seg.text) : (seg.text.startsWith("//") || seg.text.startsWith("/*"));
  const prevCodeText = (idx) => {
    let acc = "";
    for (let j = idx - 1; j >= 0 && acc.length < 32; j--) {
      if (segs[j].code) acc = segs[j].text + acc;
      else if (!segs[j].text.startsWith("//") && !segs[j].text.startsWith("/*")) break;
    }
    return acc;
  };
  const nextCodeText = (idx) => {
    let acc = "";
    for (let j = idx + 1; j < segs.length && acc.length < 16; j++) {
      if (segs[j].code) acc += segs[j].text;
      else if (!segs[j].text.startsWith("//") && !segs[j].text.startsWith("/*")) break;
    }
    return acc;
  };
  for (let i = 0; i < segs.length; i++) {
    const seg = segs[i];
    if (seg.code || isWsOrComment(seg)) continue;
    const q = seg.text[0];
    if (q !== '"' && q !== "'") continue;
    const spec = seg.text.slice(1, -1);
    const replacement = rewrites.get(spec);
    if (!replacement) continue;
    if (/\bimport\s*\(\s*$/.test(prevCodeText(i)) && /^\s*\)/.test(nextCodeText(i))) {
      seg.text = JSON.stringify(replacement);
    }
  }
  return segs.map((s) => s.text).join("");
}

function restoreLazyQuotes(minified, relPaths) {
  let out = minified;
  for (const runtimeRel of relPaths) {
    out = out.split("import(`" + runtimeRel + "`)").join("import(" + JSON.stringify(runtimeRel) + ")");
  }
  return out;
}

// Packs one package: the shared browser-safe chunk plus the entry files, minified. `files`
// names the entry sources and outputs; `alias` swaps the engine adapter; `externals` maps kept
// separate files to their specifier from the shared chunk; `urls` rewrites core-URL literals.
export async function bundlePackage({
  root = repoRoot,
  indexEntry, browserEntry, loadEntry,
  outShared, outIndex, outBrowser, outLoad,
  alias = {}, externals = new Map(), urls = {},
}) {
  const modules = loadGraph(browserEntry, { alias, externals: new Set(externals.keys()), root });
  loadGraph(indexEntry, { alias, externals: new Set(externals.keys()), root, modules });
  loadGraph(loadEntry, { alias, externals: new Set(externals.keys()), root, modules });
  const browserClosure = new Set(topoOrder(modules, browserEntry));
  const indexClosure = new Set(topoOrder(modules, indexEntry));
  const sharedIds = new Set([...browserClosure].filter((id) =>
    id !== browserEntry && !externals.has(id) && modules.get(id).src !== null));
  // Index-only modules outside the shared chunk are node-private adapter files (the L7b
  // host filesystems): they ship as mirrored files next to the node loader, with shared
  // imports pointed at the shared chunk and platform imports untouched. Anything else is
  // still an error, as is a node-private file outside `host/node/`.
  const nodeDir = join(root, "host", "node") + "/";
  const nodePrivate = [];
  for (const id of indexClosure) {
    if (id === indexEntry || id === loadEntry || externals.has(id)) continue;
    if (sharedIds.has(id)) continue;
    if (!id.startsWith(nodeDir)) throw new Error("entry-private module outside the node loader: " + relative(root, id));
    nodePrivate.push(id);
  }
  // Re-exported names must exist on their provider, or the shipped entry dangles.
  for (const id of [indexEntry, ...nodePrivate]) {
    const rec = modules.get(id);
    for (const imp of rec.imports) {
      if (!imp.reExport || imp.namespace || externals.has(imp.target)) continue;
      const provider = modules.get(imp.target);
      for (const { imported } of imp.names) {
        if (!provider || !declaredExports(provider).includes(imported)) {
          throw new Error(rec.file + ": no export " + imported);
        }
      }
    }
  }
  const ordered = topoOrder(modules, browserEntry).filter((id) => sharedIds.has(id));
  // Lazy chunks: static relative `import()` targets (L7b fs-batch, L7c volume
  // helpers) stay out of the shared chunk and ship as separate minified files.
  // They load only when used (`fs` given, `isolation` set, or a host mount),
  // keeping them off the cold path while staying G2-clean.
  const jsDir = dirname(outShared);
  const hostDir = join(root, "host");
  const outAdapters = join(jsDir, "node", "adapters.js");
  // Lazy chunks: static relative `import()` targets (L7b fs-batch, L7c volume
  // helpers, and any nested lazy chunks) stay out of the shared chunk and ship as
  // separate minified files. They load only when used, keeping them off the cold
  // path while staying G2-clean.
  const lazySeen = new Set();
  const lazyQueue = [];
  const lazyEdges = [];
  const enqueueLazy = (fromId, runtimeOut, rec) => {
    if (!rec || rec.src === null) return;
    for (const spec of lazySpecs(rec.src)) {
      if (fromId === browserEntry || fromId === indexEntry || fromId === loadEntry) {
        throw new Error(relative(root, fromId) + ": lazy chunk entry is not supported for " + spec);
      }
      let target = resolveSpec(fromId, spec);
      if (target === alias.from) target = alias.to;
      if (sharedIds.has(target) || externals.has(target)) continue;
      lazyEdges.push({ fromId, runtimeOut, spec, target });
      if (!lazySeen.has(target)) {
        lazySeen.add(target);
        lazyQueue.push(target);
      }
    }
  };
  for (const id of [...browserClosure, ...indexClosure]) {
    const runtimeOut = sharedIds.has(id) ? outShared : outAdapters;
    enqueueLazy(id, runtimeOut, modules.get(id));
  }
  const lazyChunks = [];
  const lazyChunkSet = new Set();
  for (let qi = 0; qi < lazyQueue.length; qi++) {
    const chunkRoot = lazyQueue[qi];
    if (!chunkRoot.startsWith(root + "/")) throw new Error("lazy chunk escapes the repository: " + chunkRoot);
    if (!chunkRoot.startsWith(hostDir + "/")) {
      throw new Error("lazy chunk outside host/: " + relative(root, chunkRoot));
    }
    loadGraph(chunkRoot, { alias, externals: new Set(externals.keys()), root, modules });
    if (externals.has(chunkRoot) || sharedIds.has(chunkRoot)) continue;
    const chunkOrder = topoOrder(modules, chunkRoot).filter((x) => !sharedIds.has(x) && !externals.has(x));
    for (const x of chunkOrder) {
      if (nodePrivate.includes(x) || x.startsWith(nodeDir)) {
        throw new Error("lazy chunk reaches host/node/: " + relative(root, x));
      }
    }
    const chunkOut = join(jsDir, relative(hostDir, chunkRoot));
    lazyChunkSet.add(chunkRoot);
    lazyChunks.push({ chunkRoot, chunkOut, chunkOrder });
    for (const memberId of chunkOrder) {
      enqueueLazy(memberId, chunkOut, modules.get(memberId));
    }
  }
  const rewritesByOut = new Map();
  const relPathsByOut = new Map();
  for (const { fromId, runtimeOut, spec, target } of lazyEdges) {
    if (!lazyChunkSet.has(target)) continue;
    const targetOut = join(jsDir, relative(hostDir, target));
    const runtimeRel = relativeTo(targetOut, runtimeOut);
    let byModule = rewritesByOut.get(runtimeOut);
    if (!byModule) rewritesByOut.set(runtimeOut, byModule = new Map());
    let modMap = byModule.get(fromId);
    if (!modMap) byModule.set(fromId, modMap = new Map());
    modMap.set(spec, runtimeRel);
    let rels = relPathsByOut.get(runtimeOut);
    if (!rels) relPathsByOut.set(runtimeOut, rels = new Set());
    rels.add(runtimeRel);
  }
  const { code, mapping, header } = flattenShared(modules, ordered, root, externals, rewritesByOut.get(outShared));
  const rewritten = new Map();
  const needed = new Map();
  const mergeNeeded = (r) => {
    for (const [name, ref] of r.needed) {
      if (needed.has(name) && needed.get(name) !== ref) throw new Error("shared export conflict: " + name);
      needed.set(name, ref);
    }
  };
  for (const [entry, out, entryUrls] of [
    [indexEntry, outIndex, urls.index ?? []], [browserEntry, outBrowser, urls.browser ?? []],
    [loadEntry, outLoad, urls.load ?? []],
  ]) {
    const r = rewriteEntry(entry, out, outShared, modules, sharedIds, mapping, root, entryUrls);
    rewritten.set(entry, r);
    mergeNeeded(r);
  }
  const flatOuts = [];
  const addSharedNeed = (target, name) => {
    const flat = mapping.get(target + "::" + name);
    if (!flat) throw new Error(relative(root, target) + ": no flattened export " + name);
    if (needed.has(name) && needed.get(name) !== flat) throw new Error("shared export conflict: " + name);
    needed.set(name, flat);
  };
  const flattenClosure = (order, outFile, exportSpecs) => {
    const chunkExternals = new Map(externals);
    const sharedSpec = relativeTo(outShared, outFile);
    for (const sid of sharedIds) chunkExternals.set(sid, sharedSpec);
    for (const id of order) {
      for (const imp of modules.get(id).imports) {
        if (imp.reExport || !sharedIds.has(imp.target)) continue;
        for (const { imported } of imp.names) addSharedNeed(imp.target, imported);
      }
    }
    const { code: chunkCode, mapping: chunkMap, header: chunkHeader } =
      flattenShared(modules, order, root, chunkExternals, rewritesByOut.get(outFile));
    const exportList = exportSpecs.map(([id, name]) => {
      const ref = chunkMap.get(id + "::" + name);
      if (!ref) throw new Error(relative(root, id) + ": no flattened export " + name);
      return ref === name ? name : ref + " as " + name;
    });
    const out = chunkHeader.join("\n") + (chunkHeader.length ? "\n" : "") +
      chunkCode + "\n\nexport { " + exportList.join(", ") + " };\n";
    return { out, map: chunkMap };
  };
  // Flatten the node-private adapter files into one module (`dist/js/node/adapters.js`)
  // so Node cold import parses one file instead of four and shared imports point at
  // `../shared.js`. Rewrite `index.js` imports/re-exports of those modules to it.
  if (nodePrivate.length) {
    const nodeOrder = topoOrder(modules, indexEntry).filter((x) => nodePrivate.includes(x));
    const nodeExports = [];
    const nodeSet = new Set(nodePrivate);
    for (const imp of modules.get(indexEntry).imports) {
      if (!nodeSet.has(imp.target)) continue;
      for (const { imported } of imp.names) nodeExports.push([imp.target, imported]);
    }
    const { out } = flattenClosure(nodeOrder, outAdapters, nodeExports);
    flatOuts.push([outAdapters, out]);
    const indexRec = rewritten.get(indexEntry);
    const adaptersSpec = relativeTo(outAdapters, outIndex);
    indexRec.code = joinStatements(indexRec.code).map((line) => {
      const m = line.match(REEXPORT) ?? line.match(IMPORT);
      if (!m) return line;
      const spec = m[2];
      if (!spec.startsWith(".")) return line;
      const target = resolveSpec(indexEntry, spec);
      return nodeSet.has(target) ? line.split(JSON.stringify(spec)).join(JSON.stringify(adaptersSpec)) : line;
    }).join("\n");
  }
  // Emit lazy chunks (static relative `import()` targets outside the shared set)
  // as flattened minified modules under `dist/js/`, with shared imports pointed at
  // `shared.js`. Their specifiers in the importing code are rewritten to the shipped path.
  for (const { chunkRoot, chunkOut, chunkOrder } of lazyChunks) {
    const exportSpecs = declaredExports(modules.get(chunkRoot)).map((name) => [chunkRoot, name]);
    const { out: chunkCode } = flattenClosure(chunkOrder, chunkOut, exportSpecs);
    flatOuts.push([chunkOut, chunkCode]);
  }
  const exported = "export { " + [...needed].map(([name, ref]) =>
    ref === name ? name : ref + " as " + name).join(", ") + " };";
  const sharedCode = header.join("\n") + (header.length ? "\n" : "") +
    code + "\n\n" + exported + "\n";
  const minifiedShared = restoreLazyQuotes(
    await minifyFile(relative(root, outShared), sharedCode),
    relPathsByOut.get(outShared) ?? [],
  );
  mkdirSync(dirname(outShared), { recursive: true });
  writeFileSync(outShared, minifiedShared);
  for (const [entry, out] of [[indexEntry, outIndex], [browserEntry, outBrowser], [loadEntry, outLoad]]) {
    const entryCode = rewritten.get(entry).code;
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, await minifyFile(relative(root, out), entryCode));
  }
  for (const [out, flatCode] of flatOuts) {
    mkdirSync(dirname(out), { recursive: true });
    const minified = restoreLazyQuotes(
      await minifyFile(relative(root, out), flatCode),
      relPathsByOut.get(out) ?? [],
    );
    writeFileSync(out, minified);
  }
  return { shared: outShared, modules: ordered.length + nodePrivate.length + lazyChunks.length };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  throw new Error("usage: import { bundlePackage } from tools/bundle.mjs");
}
