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
  return prev === "" || "(,=:[!&|?{};+-*%<>~^".includes(prev);
}

const IMPORT = /^[ \t]*import\s+(.+?)\s+from\s*["']([^"']+)["'];?[ \t]*$/;
const SIDE_IMPORT = /^[ \t]*import\s*["']([^"']+)["'];?[ \t]*$/;

// Parses the import statements of a module (single-line forms used by this source graph).
function parseImports(src, file) {
  const imports = [];
  for (const line of src.split("\n")) {
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
  }
  return imports;
}

function stripImportLines(src) {
  return src.split("\n").filter((line) => !IMPORT.test(line)).join("\n");
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
    for (const imp of [...rec.imports].sort((a, b) => (a.target < b.target ? -1 : 1))) visit(imp.target);
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
      else if (/^[ \t]*export[ \t]*\{/.test(line)) throw new Error(rec.file + ": export lists are entry-only");
    }
  }
  return names;
}

// Bundles the shared modules into separately scoped wrappers: every module keeps its own
// top-level bindings, so colliding declarations across modules cannot clash and no identifier
// is ever rewritten (shorthands, strings and regexes stay untouched). Imports become wrapper
// parameters fed from earlier wrappers; `externals` stay module-level imports closed over by
// the wrappers. Returns { parts, index } where index maps a module file to its wrapper const.
function bundleShared(modules, sharedIds, root, externals = new Map()) {
  const order = [...sharedIds];
  const index = new Map(order.map((id, i) => [id, i]));
  const parts = [];
  const keptHeader = [];
  for (const id of order) {
    const rec = modules.get(id);
    const params = [];
    const args = [];
    for (const imp of rec.imports) {
      if (imp.namespace) throw new Error(rec.file + ": namespace imports are entry-only");
      if (externals.has(imp.target)) {
        keptHeader.push("import { " + imp.names.map(({ imported, local }) =>
          imported === local ? local : imported + " as " + local).join(", ") +
          " } from " + JSON.stringify(externals.get(imp.target)) + ";");
        continue;
      }
      if (!index.has(imp.target)) throw new Error(rec.file + ": import escapes the shared set " + imp.target);
      for (const { imported, local } of imp.names) {
        const provider = modules.get(imp.target);
        if (!declaredExports(provider).includes(imported)) {
          throw new Error(rec.file + ": no export " + imported);
        }
        params.push(local);
        args.push("__mod" + index.get(imp.target) + "." + imported);
      }
    }
    for (const name of declaredExports(rec)) {
      if (params.includes(name)) throw new Error(rec.file + ": import shadows an export: " + name);
    }
    const body = stripExportKeyword(stripImportLines(rec.src)).trim();
    if (/\bimport\.meta\b/.test(stripStrings(body))) {
      throw new Error(rec.file + ": import.meta cannot be bundled");
    }
    const returned = declaredExports(rec);
    parts.push("// " + relative(root, id) + "\nconst __mod" + index.get(id) + " = ((" +
      params.join(", ") + ") => {\n" + body + "\nreturn { " + returned.join(", ") + " };\n})( " +
      args.join(", ") + " );");
  }
  return { parts, index, header: [...new Set(keptHeader)] };
}

function stripStrings(src) {
  return splitCode(src).map((seg) => (seg.code ? seg.text : " ")).join("");
}

// Rewrites one entry file: shared imports point at the shared chunk, namespace imports expand,
// core URLs point at the shipped layout. Returns { code, needed } where needed maps each shared
// public name to its wrapper reference (for the final export statement).
function rewriteEntry(entryFile, outEntry, sharedFile, modules, sharedIds, index, root, urls) {
  const ref = (target, name) => "__mod" + index.get(target) + "." + name;
  const rec = modules.get(entryFile);
  const needed = new Map(); // public name -> local name in the entry
  const lines = [];
  for (const line of rec.src.split("\n")) {
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
  for (const id of indexClosure) {
    if (id === indexEntry || id === loadEntry || externals.has(id)) continue;
    if (!sharedIds.has(id)) throw new Error("entry-private module outside the node loader: " + relative(root, id));
  }
  const ordered = topoOrder(modules, browserEntry).filter((id) => sharedIds.has(id));
  const { parts, index, header } = bundleShared(modules, ordered, root, externals);
  const rewritten = new Map();
  const needed = new Map();
  for (const [entry, out, entryUrls] of [
    [indexEntry, outIndex, urls.index ?? []], [browserEntry, outBrowser, urls.browser ?? []],
    [loadEntry, outLoad, urls.load ?? []],
  ]) {
    const r = rewriteEntry(entry, out, outShared, modules, sharedIds, index, root, entryUrls);
    rewritten.set(entry, r);
    for (const [name, ref] of r.needed) {
      if (needed.has(name) && needed.get(name) !== ref) throw new Error("shared export conflict: " + name);
      needed.set(name, ref);
    }
  }
  const exported = [...needed].map(([name, ref]) => "export const " + name + " = " + ref + ";");
  const sharedCode = header.join("\n") + (header.length ? "\n" : "") +
    parts.join("\n\n") + "\n\n" + exported.join("\n") + "\n";
  mkdirSync(dirname(outShared), { recursive: true });
  writeFileSync(outShared, await minifyFile(relative(root, outShared), sharedCode));
  for (const [entry, out] of [[indexEntry, outIndex], [browserEntry, outBrowser], [loadEntry, outLoad]]) {
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, await minifyFile(relative(root, out), rewritten.get(entry).code));
  }
  return { shared: outShared, modules: ordered.length };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  throw new Error("usage: import { bundlePackage } from tools/bundle.mjs");
}
