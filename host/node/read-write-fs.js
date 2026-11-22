import { latin1FromBytes } from "../core/bytes.js";
import { resolvePath } from "../core/paths.js";
import { checkedPath, errno, fsFailure, outside, rewriteError, special } from "./fs-errors.js";
import {
  HostPaths, absoluteVirtual, contains, decodeContent, encodeContent, formatStat, fstat, inDirectory, lazyBuiltin, listHost, lstat,
  nodeFs, nodePath, readRegular, resolveVirtual, sameInode, withFile,
} from "./host-paths.js";

// Lazy Node built-ins (Annex H LAZY): kept off the cold path. The adapters stay
// synchronously exported, but `node:fs` / `node:path` / `node:crypto` /
// `node:util` load only when an adapter method runs.
const nodeCrypto = () => lazyBuiltin("node:crypto");
const nodeUtil = () => lazyBuiltin("node:util");

let cachedConsts = null;
function fsConsts() {
  return (cachedConsts ??= (() => {
    const { O_RDONLY, O_WRONLY, O_CREAT, O_EXCL, O_APPEND, O_DIRECTORY = 0, O_NOFOLLOW = 0, O_NONBLOCK = 0 } = nodeFs().constants;
    return { O_RDONLY, O_WRONLY, O_CREAT, O_EXCL, O_APPEND, O_DIRECTORY, O_NOFOLLOW, O_NONBLOCK };
  })());
}

function createFlags() {
  const { O_WRONLY, O_CREAT, O_EXCL, O_NOFOLLOW, O_NONBLOCK } = fsConsts();
  return O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_NONBLOCK;
}

let cachedWriteData = null;
function writeDataAsync(fd, data) {
  cachedWriteData ??= nodeUtil().promisify(nodeFs().writeFile);
  return cachedWriteData(fd, data);
}

let cachedRename = null;
function renameAsync(a, b) {
  cachedRename ??= nodeUtil().promisify(nodeFs().rename);
  return cachedRename(a, b);
}
const jobs = [];

function startMutations() {
  for (let i = 0; i < jobs.length; i++) {
    const job = jobs[i];
    if (job.running || jobs.slice(0, i).some(({ root }) => contains(root, job.root) || contains(job.root, root))) continue;
    job.running = true;
    const finish = () => {
      jobs.splice(jobs.indexOf(job), 1);
      startMutations();
    };
    Promise.resolve().then(job.run).then(
      (value) => { finish(); job.resolve(value); },
      (error) => { finish(); job.reject(error); },
    );
  }
}

function enqueue(root, run) {
  return new Promise((resolve, reject) => {
    jobs.push({ root, run, resolve, reject, running: false });
    startMutations();
  });
}

function requireEntry(entry, op, path) {
  if (!entry.info) throw errno("ENOENT", op, path);
  return entry;
}

function copyBytes(sourceFd, targetFd, size) {
  let left = Number(size);
  const chunk = new Uint8Array(Math.min(left, 65536));
  while (left) {
    const n = nodeFs().readSync(sourceFd, chunk, 0, Math.min(left, chunk.length));
    if (!n) break;
    let offset = 0;
    while (offset < n) offset += nodeFs().writeSync(targetFd, chunk, offset, n - offset);
    left -= n;
  }
}

function temporarySibling(entry) {
  return nodePath().join(nodePath().dirname(entry.name), `.sentropic-write-${nodeCrypto().randomBytes(16).toString("hex")}`);
}

async function replaceFile(entry, path, mode, apply) {
  const name = temporarySibling(entry);
  const fd = nodeFs().openSync(name, createFlags(), mode);
  let renamed = false;
  let opened;
  try {
    opened = fstat(fd);
    if (!opened.isFile() || opened.nlink !== 1n) throw outside(path);
    await apply(fd);
    const sibling = lstat(name);
    if (!sibling?.isFile() || sibling.nlink !== 1n || !sameInode(sibling, opened)) throw outside(path);
    nodeFs().renameSync(name, entry.name);
    renamed = true;
  } finally {
    try {
      if (!renamed && opened && sameInode(lstat(name), opened)) nodeFs().unlinkSync(name);
    } finally {
      nodeFs().closeSync(fd);
    }
  }
}

function removeTree(entry, list) {
  if (!entry.info.isDirectory()) {
    nodeFs().unlinkSync(entry.name);
    return;
  }
  inDirectory(entry, (dir) => {
    for (const { name: child } of list(dir)) {
      const name = nodePath().join(dir, child);
      const info = lstat(name);
      if (info) removeTree({ name, info, ctx: entry.ctx }, list);
    }
  });
  nodeFs().rmdirSync(entry.name);
}

/** Joins a virtual child without collapsing `..` in the parent (finding 3). */
function joinVirtual(base, child) {
  return base === "/" ? `/${child}` : `${base}/${child}`;
}

/** A writable virtual filesystem rooted at an explicitly granted host directory. */
export class ReadWriteFs {
  #paths;
  #maxRead;
  #maxCopyOnWrite;
  #maxCopy;

  /** @param {import("../../types/index.js").ReadWriteFsOptions} options */
  constructor(options) {
    this.#paths = new HostPaths(options.root, "ReadWriteFs", !!options.allowSymlinks);
    this.#maxRead = options.maxFileReadSize ?? 10485760;
    this.#maxCopyOnWrite = options.maxCopyOnWriteSize ?? 104857600;
    this.#maxCopy = options.maxCopySize ?? 0;
  }

  #use(path, op, options, run) {
    checkedPath(op, path);
    try {
      return this.#paths.use(absoluteVirtual(path), op, { ...options, display: path }, run);
    } catch (error) {
      throw rewriteError(error, op, path, true);
    }
  }

  #mutate(op, path, run) {
    return enqueue(this.#paths.root, async () => {
      try {
        try {
          await this._onMutation?.("start", op, path);
          return await run();
        } finally {
          await this._onMutation?.("end", op, path);
        }
      } catch (error) {
        throw rewriteError(error, op, path, true);
      }
    });
  }

  _listHost(dir) {
    return listHost(dir);
  }

  resolvePath(base, path) {
    return resolveVirtual(base, path);
  }

  async exists(path) {
    if (path.includes("\0")) return false;
    try {
      return this.#use(path, "access", {}, (entry) => !!entry.info);
    } catch (error) {
      if (["ENOENT", "ENOTDIR", "EACCES", "ELOOP"].includes(error.code)) return false;
      throw error;
    }
  }

  async #stat(path, leaf) {
    const op = leaf ? "lstat" : "stat";
    return this.#use(path, op, { leaf }, (entry) => formatStat(requireEntry(entry, op, path).info, true));
  }

  stat(path) { return this.#stat(path, false); }
  lstat(path) { return this.#stat(path, true); }

  async readFileBuffer(path) {
    return this.#use(path, "open", {}, (entry) => {
      requireEntry(entry, "open", path);
      if (entry.info.isDirectory()) throw errno("EISDIR", "read", path);
      return readRegular(entry, this.#maxRead, path);
    });
  }

  async readFile(path, options) { return decodeContent(await this.readFileBuffer(path), options); }
  async readFileBytes(path) { return latin1FromBytes(await this.readFileBuffer(path)); }

  async #editFile(path, op, entry, apply) {
    if (!entry.info) {
      return replaceFile(entry, path, 0o666, apply);
    }
    if (entry.info.isDirectory()) throw fsFailure("EISDIR", `${op} '${path}'`);
    const metadata = op === "chmod" || op === "utimes";
    const flags = metadata ? fsConsts().O_RDONLY : op === "append" ? fsConsts().O_WRONLY | fsConsts().O_APPEND : fsConsts().O_WRONLY;
    // Check the descriptor before truncating or editing the inode.
    const initial = entry.info;
    if (initial.nlink === 1n) {
      const done = await withFile(entry, flags, op, path, async (fd, opened) => {
        if (opened.nlink !== 1n) return false;
        if (op === "write") nodeFs().ftruncateSync(fd, 0);
        await apply(fd);
        return true;
      });
      if (done) return;
    }
    if (op === "write") {
      if (!initial.isFile()) throw special(op, path);
      return replaceFile(entry, path, Number(initial.mode), (fd) => {
        nodeFs().fchmodSync(fd, Number(initial.mode));
        return apply(fd);
      });
    }
    return withFile(entry, fsConsts().O_RDONLY, op, path, (sourceFd, source) => {
      if (this.#maxCopyOnWrite && source.size > this.#maxCopyOnWrite) {
        throw fsFailure("EFBIG", `file too large for copy-on-write ${op} '${path}' (${source.size} bytes, max ${this.#maxCopyOnWrite})`);
      }
      return replaceFile(entry, path, Number(source.mode), (fd) => {
        copyBytes(sourceFd, fd, source.size);
        nodeFs().fchmodSync(fd, Number(source.mode));
        if (op !== "append") nodeFs().futimesSync(fd, source.atime, source.mtime);
        return apply(fd);
      });
    });
  }

  #writeOp(op, path, content, options) {
    checkedPath(op, path);
    const data = Uint8Array.from(encodeContent(content, options));
    return this.#mutate(op, path, () => this.#use(path, op, { parents: true }, (entry) =>
      this.#editFile(path, op, entry, (fd) => writeDataAsync(fd, data))));
  }

  async writeFile(path, content, options) { return this.#writeOp("write", path, content, options); }
  async appendFile(path, content, options) { return this.#writeOp("append", path, content, options); }

  async mkdir(path, options) {
    checkedPath("mkdir", path);
    return this.#mutate("mkdir", path, () => this.#use(path, "mkdir", { leaf: true, parents: !!options?.recursive }, (entry) => {
      if (entry.missingParent) throw errno("ENOENT", "mkdir", path);
      if (!entry.info) nodeFs().mkdirSync(entry.name, { mode: 0o755 });
      else if (!options?.recursive || !entry.info.isDirectory()) throw errno("EEXIST", "mkdir", path);
    }));
  }

  async createExclusive(path, { mode, directory = false }) {
    const op = directory ? "mkdir" : "open";
    checkedPath(op, path);
    return this.#mutate(op, path, async () => {
      const opened = this.#use(path, op, { leaf: true }, (entry) => {
        if (entry.missingParent) throw errno("ENOENT", op, path);
        if (entry.info) throw errno("EEXIST", op, path);
        if (!directory) return { canonical: entry.canonical, fd: nodeFs().openSync(entry.name, createFlags(), mode) };
        nodeFs().mkdirSync(entry.name, { mode: mode | 0o700 });
        let fd;
        try {
          const made = lstat(entry.name);
          if (!made?.isDirectory()) throw outside(path, ` during ${op}`);
          fd = nodeFs().openSync(entry.name, fsConsts().O_RDONLY | fsConsts().O_DIRECTORY | fsConsts().O_NOFOLLOW | fsConsts().O_NONBLOCK);
          if (!sameInode(made, fstat(fd))) throw outside(path, ` during ${op}`);
          return { canonical: entry.canonical, fd };
        } catch (error) {
          if (fd !== undefined) nodeFs().closeSync(fd);
          if (["ENOENT", "ENOTDIR", "ELOOP"].includes(error.code)) throw outside(path, ` during ${op}`);
          throw error;
        }
      });
      try {
        await this._afterExclusiveCreate?.(path);
        const info = fstat(opened.fd);
        let verified;
        try {
          verified = this.#use(path, op, { leaf: true }, (entry) =>
            entry.canonical === opened.canonical && sameInode(entry.info, info));
        } catch {
          verified = false;
        }
        if (!verified || !(directory ? info.isDirectory() : info.isFile() && info.nlink === 1n)) {
          throw outside(path, ` during ${op}`);
        }
        nodeFs().fchmodSync(opened.fd, mode);
      } finally {
        nodeFs().closeSync(opened.fd);
      }
    });
  }

  #listing(path, map) {
    return this.#use(path, "scandir", {}, (entry) => {
      requireEntry(entry, "scandir", path);
      if (!entry.info.isDirectory()) throw errno("ENOTDIR", "scandir", path);
      return inDirectory(entry, (dir) => map(dir, this._listHost(dir)));
    });
  }

  async readdir(path) { return this.#listing(path, (_dir, entries) => entries.map(({ name }) => name)); }

  async readdirWithFileTypes(path) {
    return this.#listing(path, (_dir, entries) => entries.map((info) => ({
      name: info.name, isFile: info.isFile(), isDirectory: info.isDirectory(), isSymbolicLink: info.isSymbolicLink(),
    })));
  }

  getAllPaths() {
    const paths = [];
    const visit = (virtual) => this.#listing(virtual, (_dir, entries) => {
      for (const info of entries) {
        const { name } = info;
        const next = resolvePath(virtual, name);
        paths.push(next);
        if (info.isDirectory()) visit(next);
      }
    });
    visit("/");
    return paths;
  }

  async rm(path, options) {
    checkedPath("rm", path);
    return this.#mutate("rm", path, () => this.#use(path, "rm", { leaf: true }, (entry) => {
      if (entry.canonical === this.#paths.root) throw outside(path);
      if (!entry.info && options?.force) return;
      requireEntry(entry, "rm", path);
      if (options?.recursive) removeTree(entry, (dir) => this._listHost(dir));
      else if (entry.info.isDirectory()) {
        try {
          nodeFs().rmdirSync(entry.name);
        } catch (error) {
          if (error.code === "ENOTEMPTY" || error.code === "EEXIST") throw fsFailure("ERR_FS_EISDIR", `rm '${path}'`);
          throw error;
        }
      } else nodeFs().unlinkSync(entry.name);
    }));
  }

  async #copy(src, dest, source, target, recursive) {
    requireEntry(source, "cp", src);
    if (source.info.isSymbolicLink()) {
      // A contained link copies through to its target when links are allowed;
      // otherwise copying a link is rejected as before.
      if (!this.#paths.allowSymlinks) throw fsFailure("EIO", `cp '${src}'`);
      const raw = nodeFs().readlinkSync(source.name);
      const at = nodePath().relative(this.#paths.root, nodePath().dirname(source.canonical)).split(nodePath().sep).filter(Boolean);
      const parts = this.#paths.targetParts(raw, at, src);
      return this.#paths.use(`/${parts.join("/")}`, "cp", { followLinks: true, strictDotDot: true, display: src }, (resolved) =>
        this.#copy(src, dest, resolved, target, recursive));
    }
    if (target.missingParent) throw errno("ENOENT", "cp", dest);
    if (source.info.isDirectory()) {
      if (!recursive) throw fsFailure("EIO", `cp '${src}'`);
      if (contains(source.canonical, target.canonical)) throw fsFailure("EINVAL", `cannot copy '${src}' into itself, '${dest}'`);
      if (!target.info) {
        nodeFs().mkdirSync(target.name, { mode: Number(source.info.mode) });
        target.info = lstat(target.name);
      }
      if (!target.info.isDirectory()) throw errno("ENOTDIR", "cp", dest);
      return inDirectory(source, (from) => inDirectory(target, async (to) => {
        for (const { name: child } of this._listHost(from)) {
          const name = nodePath().join(from, child);
          const next = nodePath().join(to, child);
          await this.#copy(joinVirtual(src, child), joinVirtual(dest, child),
            { name, info: lstat(name), canonical: nodePath().join(source.canonical, child), ctx: source.ctx },
            { name: next, info: lstat(next), canonical: nodePath().join(target.canonical, child), ctx: target.ctx }, recursive);
        }
      }));
    }
    if (!source.info.isFile() || target.info && !target.info.isFile()) throw fsFailure("EIO", `cp '${src}'`);
    if (sameInode(target.info, source.info) && source.canonical === target.canonical) return;
    return withFile(source, fsConsts().O_RDONLY, "read", src, (fd, info) => {
      if (this.#maxCopy && info.size > this.#maxCopy) {
        throw fsFailure("EFBIG", `file too large to copy '${src}' (${info.size} bytes, max ${this.#maxCopy})`);
      }
      if (!target.info) return replaceFile(target, dest, Number(info.mode), (out) => copyBytes(fd, out, info.size));
      return this.#editFile(dest, "write", target, (out) => copyBytes(fd, out, info.size));
    });
  }

  async cp(src, dest, options) {
    checkedPath("cp", src, dest);
    return this.#mutate("cp", src, () => this.#use(src, "cp", {}, (source) =>
      this.#use(dest, "cp", { leaf: true }, (target) => this.#copy(src, dest, source, target, !!options?.recursive))));
  }

  #checkMovedLinks(entry, path) {
    if (entry.info.isSymbolicLink()) {
      const raw = nodeFs().readlinkSync(entry.name);
      // A stored absolute target is a host path, unlike the virtual argument to symlink().
      const parent = nodePath().relative(this.#paths.root, nodePath().dirname(entry.canonical)).split(nodePath().sep).filter(Boolean);
      const parts = this.#paths.targetParts(raw, parent, path);
      this.#paths.use(`/${parts.join("/")}`, "mv", { followLinks: true, strictDotDot: true }, () => {});
    } else if (entry.info.isDirectory()) {
      inDirectory(entry, (dir) => {
        for (const { name: child } of this._listHost(dir)) {
          const name = nodePath().join(dir, child);
          const info = lstat(name);
          if (info) this.#checkMovedLinks({ name, info, canonical: nodePath().join(entry.canonical, child), ctx: entry.ctx }, path);
        }
      });
    }
  }

  async mv(src, dest) {
    checkedPath("mv", src, dest);
    return this.#mutate("mv", src, () => this.#use(src, "mv", { leaf: true }, (source) =>
      this.#use(dest, "mv", { leaf: true }, (target) => {
        requireEntry(source, "mv", src);
        if (target.missingParent) throw errno("ENOENT", "mv", dest);
        if (source.info.isDirectory() && contains(source.canonical, target.canonical)) {
          throw fsFailure("EINVAL", `cannot move '${src}' into itself, '${dest}'`);
        }
        if (target.canonical === this.#paths.root || source.canonical === this.#paths.root) throw outside(dest);
        const escape = (plural) => fsFailure("EACCES", `permission denied, mv '${src}' -> '${dest}' would create symlink${plural ? "s" : ""} escaping sandbox`);
        if (source.info.isSymbolicLink()) {
          try {
            this.#checkMovedLinks({ ...source, canonical: target.canonical }, dest);
          } catch {
            throw escape(false);
          }
        }
        if (!source.info.isDirectory()) {
          return renameAsync(source.name, target.name);
        }
        let backup;
        if (target.info?.isDirectory()) {
          if (inDirectory(target, (dir) => this._listHost(dir).length)) throw errno("ENOTEMPTY", "mv", dest);
          backup = temporarySibling(target);
          nodeFs().renameSync(target.name, backup);
        }
        try {
          nodeFs().renameSync(source.name, target.name);
          try {
            this.#checkMovedLinks({ ...target, info: source.info }, dest);
          } catch {
            nodeFs().renameSync(target.name, source.name);
            throw escape(true);
          }
        } catch (error) {
          if (backup) nodeFs().renameSync(backup, target.name);
          throw error;
        }
        if (backup) nodeFs().rmdirSync(backup);
      })));
  }

  #setMeta(op, path, apply) {
    checkedPath(op, path);
    return this.#mutate(op, path, () => this.#use(path, op, {}, (entry) => {
      requireEntry(entry, op, path);
      if (entry.info.isDirectory()) inDirectory(entry, (_dir, fd) => apply(fd));
      else return this.#editFile(path, op, entry, apply);
    }));
  }

  async chmod(path, mode) { return this.#setMeta("chmod", path, (fd) => nodeFs().fchmodSync(fd, mode)); }
  async utimes(path, atime, mtime) { return this.#setMeta("utimes", path, (fd) => nodeFs().futimesSync(fd, atime, mtime)); }

  async symlink(target, path) {
    checkedPath("symlink", target, path);
    if (!this.#paths.allowSymlinks) throw errno("EPERM", "symlink", path);
    return this.#mutate("symlink", path, () => this.#use(path, "symlink", { leaf: true }, (entry) => {
      if (entry.missingParent) throw errno("ENOENT", "symlink", path);
      if (entry.info) throw errno("EEXIST", "symlink", path);
      nodeFs().symlinkSync(this.#paths.linkTarget(target, entry.canonical, path), entry.name);
    }));
  }

  async link(src, dest) {
    checkedPath("link", src, dest);
    return this.#mutate("link", src, () => this.#use(src, "link", {}, (source) =>
      this.#use(dest, "link", { leaf: true }, (target) => {
        requireEntry(source, "link", src);
        if (!source.info.isFile()) throw errno("EPERM", "link", src);
        if (target.info) throw errno("EEXIST", "link", dest);
        if (target.missingParent) throw errno("ENOENT", "link", dest);
        nodeFs().linkSync(source.name, target.name);
      })));
  }

  async readlink(path) {
    return this.#use(path, "readlink", { leaf: true }, (entry) => {
      requireEntry(entry, "readlink", path);
      if (!entry.info.isSymbolicLink()) throw errno("EINVAL", "readlink", path);
      return this.#paths.readlink(entry, path);
    });
  }

  async realpath(path) {
    return this.#use(path, "realpath", {}, (entry) => this.#paths.virtual(requireEntry(entry, "realpath", path).canonical));
  }
}
