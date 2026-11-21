// Filesystem operations are performed in a single revocable extension turn.
import { decodeUtf8, utf8 } from "./bytes.js";
import { FS_METHODS } from "./membrane.js";
import { dirname, resolvePath } from "./paths.js";

const CHUNK = 1 << 20;
const YIELD_EVERY = 128;
const ERRNO = new Map([
  ["ENOENT", "not-found"], ["EEXIST", "exists"], ["ENOTDIR", "not-directory"],
  ["EISDIR", "is-directory"], ["ERR_FS_EISDIR", "is-directory"], ["ENOTEMPTY", "not-empty"],
  ["EACCES", "permission"], ["EPERM", "permission"], ["EINVAL", "invalid"], ["ENAMETOOLONG", "invalid"],
  ["ELOOP", "link-loop"], ["EFBIG", "too-large"], ["ENOSPC", "no-space"], ["EBUSY", "busy"],
  ["EXDEV", "cross-device"], ["EROFS", "read-only"], ["ENOSYS", "unsupported"], ["ENOTSUP", "unsupported"],
]);

const ROOT = utf8("/");
const failure = (code, path) => ({ tag: "err", val: { code, path } });
const ok = (tag, val) => ({ tag: "ok", val: tag === "none" ? { tag } : { tag, val } });
const kind = (info) => info.isDirectory ? "directory" : info.isSymbolicLink ? "symlink" : "file";
const missing = (error) => (error?.code ?? /^([A-Z_0-9]+):/.exec(String(error?.message))?.[1]) === "ENOENT";

function stat(info) {
  return {
    kind: kind(info), size: BigInt(info.size), mode: info.mode, mtimeMs: BigInt(info.mtime.getTime()),
    dev: BigInt(info.dev ?? 0), ino: BigInt(info.ino ?? 0),
  };
}

function pathOf({ tag, val }) {
  if (tag === "symlink") return val.target;
  return val instanceof Uint8Array ? val : val?.path ?? val?.root ?? val?.source ?? ROOT;
}

function consume(budget, bytes) {
  if (bytes > budget.remaining) return false;
  budget.remaining -= bytes;
  return true;
}

function transfer(budget, bytes) {
  if (bytes > CHUNK || !consume(budget, bytes)) throw new Error("EFBIG:");
}

function letTimersRun() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

// A listing is fetched once. Metadata is fetched only when the response needs it.
async function list(g, path) {
  if (g.readdirWithFileTypes) return g.readdirWithFileTypes(path);
  return (await g.readdir(path)).map((name) => ({ name }));
}

async function walk(g, root, depth, count, work, budget) {
  const entries = [];
  if (!depth || !count || !work) return entries;
  const dirs = [{ path: root, depth: 0 }];
  for (let i = 0; i < dirs.length; i++) {
    const parent = dirs[i];
    for (const entry of await list(g, parent.path)) {
      if (work-- <= 0 || entries.length >= count) return entries;
      if (work % YIELD_EVERY === 0) await letTimersRun();
      const path = resolvePath(parent.path, entry.name);
      const bytes = utf8(path);
      if (bytes.length > CHUNK || (entries.length + 1) * 48 > CHUNK || !consume(budget, 48 + bytes.length)) return entries;
      let info;
      try {
        info = await g.lstat(path);
      } catch (error) {
        if (missing(error)) continue;
        throw error;
      }
      entries.push({ path: bytes, info: stat(info) });
      if (info.isDirectory && parent.depth + 1 < depth) dirs.push({ path, depth: parent.depth + 1 });
    }
    if (work === 0 || entries.length >= count) break;
  }
  return entries;
}

async function directory(g, path, plus, limits, budget) {
  const result = [];
  let work = limits.maxTraversalWork;
  for (const entry of await list(g, path)) {
    if (work-- <= 0 || result.length >= limits.maxTraversalEntries) break;
    if (work % YIELD_EVERY === 0) await letTimersRun();
    const bytes = utf8(plus ? resolvePath(path, entry.name) : entry.name);
    const fixed = plus ? 48 : 12;
    if (bytes.length > CHUNK || (result.length + 1) * fixed > CHUNK || !consume(budget, bytes.length + fixed)) break;
    let info = entry;
    if (plus || entry.isDirectory === undefined) {
      try {
        info = await g.lstat(resolvePath(path, entry.name));
      } catch (error) {
        if (missing(error)) continue;
        throw error;
      }
    }
    result.push(plus ? { path: bytes, info: stat(info) } : { name: bytes, kind: kind(info) });
  }
  return result;
}

async function writeFile(g, path, { data, offset, truncate, mode, mtimeMs }, limits) {
  if (offset === undefined) await g.appendFile(path, data);
  else if (offset === 0n && truncate) await g.writeFile(path, data);
  else {
    if (offset > BigInt(limits.maxFileSystemBytes - data.length)) throw new Error("EFBIG:");
    let prior;
    try {
      prior = await g.readFileBuffer(path);
    } catch (error) {
      if (!missing(error)) throw error;
      prior = new Uint8Array(0);
    }
    const pos = Number(offset);
    const size = truncate ? pos + data.length : Math.max(pos + data.length, prior.length);
    if (size > limits.maxFileSystemBytes) throw new Error("EFBIG:");
    const patched = new Uint8Array(size);
    patched.set(prior.subarray(0, truncate ? pos : size));
    patched.set(data, pos);
    await g.writeFile(path, patched);
  }
  if (mode !== undefined) await g.chmod(path, mode);
  if (mtimeMs !== undefined) {
    const when = new Date(Number(mtimeMs));
    await g.utimes(path, when, when);
  }
}

async function createExclusive(g, path, mode, directory) {
  if (g.createExclusive) return g.createExclusive(path, { mode, directory });
  if (await g.exists(path)) throw new Error("EEXIST:");
  if (!(await g.stat(dirname(resolvePath("/", path)))).isDirectory) throw new Error("ENOTDIR:");
  if (directory) await g.mkdir(path, { recursive: false });
  else await g.writeFile(path, new Uint8Array(0));
  await g.chmod(path, mode);
}

async function runOp(g, { tag, val }, path, limits, budget) {
  switch (tag) {
    case "stat": return ok("info", stat(await g.stat(path)));
    case "lstat": return ok("info", stat(await g.lstat(path)));
    case "exists": return ok("flag", await g.exists(path));
    case "read-link":
    case "real-path": {
      const value = await (tag === "read-link" ? g.readlink(path) : g.realpath(path));
      const bytes = utf8(value);
      transfer(budget, bytes.length);
      return ok("path", bytes);
    }
    case "read-dir": return ok("entries", await directory(g, path, false, limits, budget));
    case "read-dir-plus": return ok("entries-plus", await directory(g, path, true, limits, budget));
    case "walk": return ok("entries-plus", await walk(g, path, Math.min(val.maxDepth, limits.maxTraversalDepth),
      Math.min(val.maxEntries, limits.maxTraversalEntries), limits.maxTraversalWork, budget));
    case "all-paths": {
      const paths = [];
      for (const path of g.getAllPaths()) {
        const bytes = utf8(path);
        if (bytes.length > CHUNK || (paths.length + 1) * 8 > CHUNK || !consume(budget, bytes.length + 8)) break;
        paths.push(bytes);
      }
      return ok("paths", paths);
    }
    case "read-file": {
      const contents = await g.readFileBuffer(path);
      const start = Number(val.offset < BigInt(contents.length) ? val.offset : BigInt(contents.length));
      const len = Math.min(val.len, contents.length - start, CHUNK, budget.remaining);
      consume(budget, len);
      return ok("data", Uint8Array.from(contents.subarray(start, start + len)));
    }
    case "write-file": await writeFile(g, path, val, limits); break;
    case "write-lazy": throw new Error("ENOSYS:");
    case "create-exclusive": await createExclusive(g, path, val.mode, val.directory); break;
    case "mkdir":
      await g.mkdir(path, { recursive: val.recursive });
      if (val.mode !== undefined) await g.chmod(path, val.mode);
      break;
    case "remove": await g.rm(path, { recursive: val.recursive, force: val.force }); break;
    case "copy": await g.cp(path, decodeUtf8(val.target), { recursive: val.recursive }); break;
    case "rename": await g.mv(path, decodeUtf8(val.target)); break;
    case "symlink": await g.symlink(decodeUtf8(val.source), path); break;
    case "hard-link": await g.link(path, decodeUtf8(val.target)); break;
    case "chmod": await g.chmod(path, val.mode); break;
    case "utimes": await g.utimes(path, new Date(Number(val.atimeMs)), new Date(Number(val.mtimeMs))); break;
    default: throw new Error("ENOSYS:");
  }
  return ok("none");
}

export function* serveFsBatch(x, batch, serveExtension) {
  const { B } = x;
  const fs = B.mounts.get(batch.mountId);
  if (!fs) return { outcome: { tag: "denied" } };
  const paths = batch.ops.map(pathOf);
  const fixed = paths.length * 64;
  const budget = { remaining: Math.floor(B.headroom / 2) - fixed };
  if (fixed > CHUNK || paths.some((path) => path.length > CHUNK || !consume(budget, path.length))) {
    return { outcome: { tag: "limit-exceeded", val: "headroom" } };
  }
  return yield* serveExtension(x, async (scope) => {
    const g = scope.wrap(fs, FS_METHODS);
    const results = [];
    for (let i = 0; i < batch.ops.length; i++) {
      if (i && i % YIELD_EVERY === 0) await letTimersRun();
      scope.check();
      const path = paths[i];
      try {
        results.push(await runOp(g, batch.ops[i], decodeUtf8(path), B.limits, budget));
      } catch (error) {
        scope.check();
        const code = /^([A-Z_0-9]+):/.exec(String(error?.message))?.[1] ?? error?.code;
        results.push(failure(ERRNO.get(code) ?? "io", path));
      }
    }
    return { tag: "fs", val: results };
  }, { tag: "failed", val: utf8("fs-batch failed") });
}
