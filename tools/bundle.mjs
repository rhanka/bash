#!/usr/bin/env node
// Shipped facade packer (spec/lots/L2c.md section 5): one browser-safe minified shared module
// per package plus binding-specific entries, built from one source graph with the lockfile's
// oxc-minify. The default binding bundles the first-party adapter; the jco package aliases it
// to a generated adapter over the pinned guarded jco glue. No dependency, no network.
import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";
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
    while ((m = decl.exec(text)) !== null) events.push({ index: m.index, name: m[1], length: m[0].length });
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

function hasDecl(body, word) {
  // True when `word` is declared (not merely a property or method name) in the body:
  // a shadowing binding that would capture a direct import replacement. Over-approximating
  // with `(`/`,` triggers is safe (an extra alias), under-approximating is not.
  const esc = word.replace(/[$]/g, "\\$&");
  const re = new RegExp("(?:const|let|var|function|class|catch|\\(|,)\\s*" + esc + "\\b", "g");
  for (const seg of splitCode(body)) {
    if (!seg.code) continue;
    if (re.test(seg.text)) return true;
  }
  return false;
}

function hasWord(body, word) {
  return hasDecl(body, word);
}

function flattenShared(modules, sharedIds, root, externals = new Map()) {
  const order = [...sharedIds];
  const set = new Set(sharedIds);
  const used = new Set();
  const mapping = new Map();
  const bodies = [];
  const keptHeader = [];
  const seenHeaders = new Set();
  for (const id of order) {
    const rec = modules.get(id);
    const bindings = rec.imports.filter((imp) => !imp.reExport);
    const allLocals = bindings.flatMap((imp) => imp.names.map((x) => x.local));
    for (const imp of bindings) {
      if (imp.namespace) throw new Error(rec.file + ": namespace imports are entry-only");
      if (externals.has(imp.target)) {
        const stmt = "import { " + imp.names.map(({ imported, local }) =>
          imported === local ? local : imported + " as " + local).join(", ") +
          " } from " + JSON.stringify(externals.get(imp.target)) + ";";
        if (!seenHeaders.has(stmt)) {
          seenHeaders.add(stmt);
          keptHeader.push(stmt);
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
    for (const [oldName, newName] of rename) body = replaceIdent(body, oldName, newName);
    // Imports become direct references. When the provider name already occurs as a shadowing
    // nested binding, rename that nested binding first so the replacement cannot be captured.
    for (const imp of bindings) {
      if (imp.namespace || externals.has(imp.target)) continue;
      for (const { imported, local } of imp.names) {
        const flat = mapping.get(imp.target + "::" + imported);
        if (!flat) throw new Error(rec.file + ": provider not yet flattened for " + imported);
        if (local === flat) continue;
        if (hasWord(body, flat)) {
          let n = 1;
          let candidate = flat + "$" + n;
          while (used.has(candidate)) {
            n++;
            candidate = flat + "$" + n;
          }
          body = replaceIdent(body, flat, candidate);
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
export { splitCode, flattenShared };

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
  const { code, mapping, header } = flattenShared(modules, ordered, root, externals);
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
  // Mirror the node-private files into `dist/js/node/`: their relative specifiers stay valid
  // because the mirror preserves the `host/` layout, and their shared needs join the chunk.
  const jsDir = dirname(outShared);
  const extraOuts = [];
  for (const id of topoOrder(modules, indexEntry).filter((x) => nodePrivate.includes(x))) {
    const out = join(jsDir, relative(join(root, "host"), id));
    const r = rewriteEntry(id, out, outShared, modules, sharedIds, mapping, root, []);
    rewritten.set(id, r);
    mergeNeeded(r);
    extraOuts.push([id, out]);
  }
  const exported = "export { " + [...needed].map(([name, ref]) =>
    ref === name ? name : ref + " as " + name).join(", ") + " };";
  const sharedCode = header.join("\n") + (header.length ? "\n" : "") +
    code + "\n\n" + exported + "\n";
  mkdirSync(dirname(outShared), { recursive: true });
  writeFileSync(outShared, await minifyFile(relative(root, outShared), sharedCode));
  for (const [entry, out] of [[indexEntry, outIndex], [browserEntry, outBrowser], [loadEntry, outLoad], ...extraOuts]) {
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, await minifyFile(relative(root, out), rewritten.get(entry).code));
  }
  return { shared: outShared, modules: ordered.length + extraOuts.length };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  throw new Error("usage: import { bundlePackage } from tools/bundle.mjs");
}
