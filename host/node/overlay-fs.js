import { concat, latin1FromBytes, utf8 } from "../core/bytes.js";
import { dirname, resolvePath } from "../core/paths.js";
import { checkedPath, errno, fsFailure, outside, plainError, rewriteError, special } from "./fs-errors.js";
import {
  HostPaths, absoluteVirtual, decodeContent, encodeContent, formatStat, inDirectory, listHost, readRegular, resolveVirtual,
} from "./host-paths.js";

function beneath(dir, name) {
  return dir === "/" || name === dir || name.startsWith(dir + "/");
}

function typeInfo(kind) {
  return { isFile: kind === "file", isDirectory: kind === "directory", isSymbolicLink: kind === "symlink" };
}

function requireEntry(entry, op, path) {
  if (!entry?.info) throw errno("ENOENT", op, path);
  return entry;
}

/** An in-memory write layer over a directory that the adapter only reads. */
export class OverlayFs {
  #host;
  #mount;
  #readOnly;
  #maxRead;
  #maxMemory;
  #entries = new Map();
  #removed = new Set();
  #used = 0;
  #nextIdentity = 1;

  /** @param {import("../../types/index.js").OverlayFsOptions} options */
  constructor(options) {
    const given = options.mountPoint ?? "/home/user/project";
    const mount = given.startsWith("/") ? given.replace(/\/+$/, "") || "/" : given.replace(/\/+$/, "");
    if (!mount.startsWith("/")) throw plainError(`Mount point must be an absolute path: ${given}`);
    this.#mount = resolveVirtual("/", mount);
    this.#maxMemory = options.maxMemoryBytes ?? 1073741824;
    if (!Number.isSafeInteger(this.#maxMemory) || this.#maxMemory < 0) throw plainError("OverlayFs: invalid maxMemoryBytes");
    this.#host = new HostPaths(options.root, "OverlayFs", !!options.allowSymlinks);
    this.#readOnly = !!options.readOnly;
    this.#maxRead = options.maxFileReadSize ?? 10485760;
    this.mkdirSync(this.#mount, { recursive: true });
  }

  getMountPoint() { return this.#mount; }

  resolvePath(base, path) { return resolveVirtual(base, path); }

  _listHost(dir) { return listHost(dir); }

  #dir() { return { kind: "directory", mode: 0o755, mtime: new Date() }; }

  #file(data, mode = 0o644, mtime = new Date()) { return { kind: "file", mode, mtime, data }; }

  #checkWrite(op, path, ...other) {
    checkedPath(op, path, ...other);
    if (this.#readOnly) throw errno("EROFS", op, path);
    return absoluteVirtual(path);
  }

  #hidden(virtual) {
    for (let p = virtual;; p = dirname(p)) {
      if (this.#removed.has(p)) return true;
      if (p === "/") return false;
    }
  }

  #mounted(suffix) {
    return resolvePath(this.#mount, suffix.replace(/^\//, ""));
  }

  #lookup(virtual, op, path, leaf, run, hops = 0, strict = false) {
    // `..` is resolved against the scanned prefix (never lexically pre-collapsed)
    // so an intermediate symlink is still checked; the raw `..`-preserving path
    // is what the host validates, and its canonical answer re-enters here.
    // `strict` rejects a `..` climbing above the root and is used for symlink
    // targets, mirroring the host adapter's `mv` escape gate.
    const raw = virtual.split("/").filter((part) => part !== "" && part !== ".");
    const resolved = [];
    let edge = 0;
    let blockedScan = false;
    for (let i = 0; i < raw.length; i++) {
      const part = raw[i];
      if (part === "..") {
        if (resolved.length) resolved.pop();
        continue;
      }
      const prefix = `/${[...resolved, part].join("/")}`;
      const node = this.#entries.get(prefix);
      if (node) {
        const final = i === raw.length - 1;
        if (node.kind === "symlink" && (!final || !leaf)) {
          // Denied symlinks block the host subtree but still allow memory-layer
          // writes below them; keep scanning so the reported path stays resolved.
          if (!this.#host.allowSymlinks) blockedScan = true;
          else {
            if (hops === 40) throw errno("ELOOP", op, path);
            // The target applies to the resolved prefix, so a `..` in the
            // remainder collapses against resolved directories only.
            const target = resolvePath(dirname(prefix), node.target);
            if (beneath(this.#mount, prefix) && !beneath(this.#mount, target)) throw outside(path);
            return this.#lookup(resolvePath(target, raw.slice(i + 1).join("/")), op, path, leaf, run, hops + 1, strict);
          }
        } else if (!blockedScan && !final && node.kind !== "directory") throw errno("ENOTDIR", op, path);
      }
      resolved.push(part);
      if (prefix === this.#mount) edge = i + 1;
    }
    const v = `/${resolved.join("/")}`;
    if (blockedScan) return run({ blocked: true, virtual: v });
    // The host suffix restarts where the walk last enters the mount, with any
    // `..` kept, so `HostPaths.use` still resolves each crossing symlink-aware
    // instead of skipping it the way a lexically collapsed path would.
    const suffix = beneath(this.#mount, v) ? `/${raw.slice(edge).join("/")}` : null;
    const memory = this.#entries.get(v);
    const own = memory && { memory, info: this.#memoryStat(memory), virtual: v };
    if ((own && own.memory.kind !== "directory") || suffix === null || this.#hidden(v)) {
      return run(own ?? { virtual: v });
    }
    try {
      return this.#host.use(suffix, op, { leaf, ignoreLinks: true, display: path, strictDotDot: strict }, (entry) => {
        if (entry.blocked) return run(own ?? { ...entry, virtual: v });
        const actual = this.#mounted(this.#host.virtual(entry.canonical));
        if (actual !== v) {
          if (hops === 40) throw errno("ELOOP", op, path);
          return this.#lookup(actual, op, path, leaf, run, hops + 1, strict);
        }
        if (own) return run({ ...own, host: entry.info?.isDirectory() ? entry : undefined });
        return run(entry.info ? { info: formatStat(entry.info), host: entry, virtual: v } : { virtual: v });
      });
    } catch (error) {
      throw rewriteError(error, op, path);
    }
  }

  #memoryStat(node) {
    return {
      ...typeInfo(node.kind), mode: node.mode, size: node.data?.length ?? (node.target ? utf8(node.target).length : 0),
      mtime: new Date(node.mtime), identity: `overlay:${node.identity}`,
    };
  }

  #capacity(size) {
    if (size > this.#maxMemory) throw fsFailure("ENOSPC", `overlay memory byte limit exceeded (${this.#maxMemory} bytes)`);
  }

  #commit(changes, tombstone) {
    let used = this.#used;
    for (const [p, value] of changes) used += (value?.data?.length ?? 0) - (this.#entries.get(p)?.data?.length ?? 0);
    this.#capacity(used);
    for (const [p, value] of changes) {
      if (value) {
        value.identity ??= this.#nextIdentity++;
        this.#entries.set(p, value);
      } else this.#entries.delete(p);
    }
    if (tombstone) this.#removed.add(tombstone);
    this.#used = used;
  }

  #parents(changes, virtual, op, path, create, memoryOnly = false) {
    const parent = dirname(virtual);
    const pending = [];
    let at = parent;
    for (;;) {
      const staged = changes.get(at);
      const node = this.#entries.get(at);
      const state = staged ? { info: typeInfo(staged.kind) } : memoryOnly
        ? node && { info: typeInfo(node.kind) }
        : this.#lookup(at, op, path, false, (entry) => entry);
      const entry = state?.info;
      if (entry) {
        if (!entry.isDirectory) throw errno("ENOTDIR", op, path);
        break;
      }
      if (!create && !state?.blocked) throw errno("ENOENT", op, path);
      pending.push(at);
      if (at === "/") break;
      at = dirname(at);
    }
    for (const p of pending.reverse()) changes.set(p, this.#dir());
  }

  #setFile(changes, p, file) {
    for (const child of this.#entries.keys()) {
      if (child !== p && beneath(p, child)) changes.set(child, undefined);
    }
    changes.set(p, file);
  }

  mkdirSync(path, options) {
    checkedPath("mkdir", path);
    // Resolve through the lookup so `..` and symlinks settle before the memory
    // map is keyed; the lookup itself is side-effect free.
    this.#lookup(absoluteVirtual(path), "mkdir", path, true, (entry) => {
      const p = entry.virtual;
      const node = this.#entries.get(p);
      if (node) {
        if (!options?.recursive) throw errno("EEXIST", "mkdir", path);
        return;
      }
      if (entry.info && !options?.recursive) throw errno("EEXIST", "mkdir", path);
      const changes = new Map();
      if (p !== "/") this.#parents(changes, p, "mkdir", path, !!options?.recursive, true);
      changes.set(p, this.#dir());
      this.#commit(changes);
    });
  }

  writeFileSync(path, content) {
    checkedPath("write", path);
    this.#lookup(absoluteVirtual(path), "write", path, false, (entry) => {
      this.#write(entry.virtual, path, content, undefined, true);
    });
  }

  #write(p, path, content, options, memoryOnly = false) {
    if (!memoryOnly) p = this.#lookup(p, "write", path, false, (entry) => entry.virtual);
    const data = Uint8Array.from(encodeContent(content, options));
    const changes = new Map();
    if (p === "/") throw errno("EISDIR", "write", path);
    this.#parents(changes, p, "write", path, true, memoryOnly);
    const prev = this.#entries.get(p);
    this.#setFile(changes, p, this.#file(data, prev?.kind === "file" ? prev.mode : undefined));
    this.#commit(changes);
  }

  async writeFile(path, content, options) {
    this.#write(this.#checkWrite("write", path), path, content, options);
  }

  async appendFile(path, content, options) {
    const p = this.#checkWrite("append", path);
    const data = encodeContent(content, options);
    const changes = new Map();
    this.#lookup(p, "append", path, false, (entry) => {
      const dest = entry.virtual;
      this.#parents(changes, dest, "append", path, true);
      if (entry?.info?.isDirectory) throw errno("EISDIR", "append", path);
      if (entry?.info && !entry.info.isFile) throw special("append", path);
      const available = this.#maxMemory - this.#used + (entry?.memory?.data?.length ?? 0);
      this.#capacity(this.#maxMemory - available + (entry?.info?.size ?? 0) + data.length);
      let prior = entry?.memory?.data;
      if (!prior && entry?.host) prior = this.#readForMemory(entry.host, available - data.length, path, "append");
      this.#setFile(changes, dest, this.#file(prior ? concat([prior, data]) : Uint8Array.from(data), entry?.info?.mode));
    });
    this.#commit(changes);
  }

  async exists(path) {
    if (path.includes("\0")) return false;
    try {
      return this.#lookup(absoluteVirtual(path), "access", path, false, (entry) => !!entry?.info);
    } catch (error) {
      if (["ENOENT", "ENOTDIR", "EACCES", "ELOOP"].includes(error.code)) return false;
      throw error;
    }
  }

  async stat(path) {
    checkedPath("stat", path);
    return this.#lookup(absoluteVirtual(path), "stat", path, false, (entry) => requireEntry(entry, "stat", path).info);
  }

  async lstat(path) {
    checkedPath("lstat", path);
    return this.#lookup(absoluteVirtual(path), "lstat", path, true, (entry) => requireEntry(entry, "lstat", path).info);
  }

  async readFileBuffer(path) {
    checkedPath("open", path);
    return this.#lookup(absoluteVirtual(path), "open", path, false, (entry) => {
      requireEntry(entry, "open", path);
      if (entry.info.isDirectory) throw fsFailure("EIO", `open '${path}'`);
      // `maxFileReadSize` applies to memory entries as well: metadata and copy
      // operations may pull a large host file into memory, but reading it back
      // must not bypass the cap (L7b §6).
      if (entry.memory) {
        if (this.#maxRead && entry.info.size > this.#maxRead) {
          throw fsFailure("EFBIG", `file too large, read '${path}' (${entry.info.size} bytes, max ${this.#maxRead})`);
        }
        return Uint8Array.from(entry.memory.data);
      }
      return readRegular(entry.host, this.#maxRead, path);
    });
  }

  async readFile(path, options) { return decodeContent(await this.readFileBuffer(path), options); }
  async readFileBytes(path) { return latin1FromBytes(await this.readFileBuffer(path)); }

  async mkdir(path, options) {
    const p = this.#checkWrite("mkdir", path);
    this.#lookup(p, "mkdir", path, true, (entry) => {
      if (entry?.info) {
        if (!options?.recursive) throw errno("EEXIST", "mkdir", path);
        return;
      }
      const changes = new Map();
      this.#parents(changes, entry.virtual, "mkdir", path, !!options?.recursive);
      changes.set(entry.virtual, this.#dir());
      this.#commit(changes);
    });
  }

  async createExclusive(path, { mode, directory = false }) {
    const op = directory ? "mkdir" : "open";
    const p = this.#checkWrite(op, path);
    this.#lookup(p, op, path, true, (entry) => {
      if (entry?.info) throw errno("EEXIST", op, path);
      const changes = new Map();
      this.#parents(changes, entry.virtual, op, path, false);
      changes.set(entry.virtual, directory ? { ...this.#dir(), mode } : this.#file(new Uint8Array(0), mode));
      this.#commit(changes);
    });
  }

  #listing(entry, path) {
    if (entry?.blocked) return [];
    requireEntry(entry, "scandir", path);
    if (!entry.info.isDirectory) throw errno("ENOTDIR", "scandir", path);
    const entries = new Map();
    const p = entry.virtual;
    if (entry.host) {
      inDirectory(entry.host, (dir) => {
        for (const info of this._listHost(dir)) {
          const { name } = info;
          if (this.#hidden(resolvePath(p, name))) continue;
          entries.set(name, { name, isDirectory: info.isDirectory(), isFile: info.isFile(), isSymbolicLink: info.isSymbolicLink() });
        }
      });
    }
    for (const [child, node] of this.#entries) {
      if (p === child || dirname(child) !== p) continue;
      const name = child.slice(child.lastIndexOf("/") + 1);
      entries.set(name, { name, ...typeInfo(node.kind) });
    }
    return [...entries.keys()].sort().map((name) => entries.get(name));
  }

  async readdirWithFileTypes(path) {
    checkedPath("scandir", path);
    return this.#lookup(absoluteVirtual(path), "scandir", path, false, (entry) => this.#listing(entry, path));
  }

  async readdir(path) { return (await this.readdirWithFileTypes(path)).map(({ name }) => name); }

  getAllPaths() {
    if (this.#mount !== "/") return [...this.#entries.keys()];
    const result = ["/"];
    const visit = (p) => this.#lookup(p, "scandir", p, false, (entry) => {
      for (const { name, isDirectory } of this.#listing(entry, p)) {
        const child = resolvePath(p, name);
        result.push(child);
        if (isDirectory) visit(child);
      }
    });
    visit("/");
    return result;
  }

  async rm(path, options) {
    const p = this.#checkWrite("rm", path);
    if (p === "/") throw errno("ENOTEMPTY", "rm", path);
    this.#lookup(p, "rm", path, true, (entry) => {
      if (!entry?.info && options?.force) return;
      requireEntry(entry, "rm", path);
      if (entry.info.isDirectory && !options?.recursive && this.#listing(entry, path).length) throw errno("ENOTEMPTY", "rm", path);
      const changes = new Map();
      for (const child of this.#entries.keys()) if (beneath(entry.virtual, child)) changes.set(child, undefined);
      this.#commit(changes, entry.virtual);
    });
  }

  #readForMemory(entry, allowance, path, op) {
    try {
      return readRegular(entry, Math.max(1, allowance), path, op);
    } catch (error) {
      if (error.code === "EFBIG") this.#capacity(this.#maxMemory + 1);
      throw error;
    }
  }

  #copyEntry(stage, src, dest, path, flags) {
    this.#lookup(src, flags.op, path, true, (entry) => {
      requireEntry(entry, flags.op, path);
      const { info, memory } = entry;
      if (info.isDirectory) {
        if (!flags.recursive) throw fsFailure("EISDIR", `is a directory, cp '${path}'`);
        stage.set(dest, { ...this.#dir(), mode: info.mode, mtime: info.mtime });
        for (const { name } of this.#listing(entry, path)) {
          const child = resolvePath(src, name);
          this.#copyEntry(stage, child, resolvePath(dest, name), child, flags);
        }
      } else if (info.isSymbolicLink || info.isFile) {
        for (const child of this.#entries.keys()) {
          if (child !== dest && beneath(dest, child)) stage.set(child, undefined);
        }
        if (info.isSymbolicLink) {
          const target = memory?.target ?? this.#host.readlink(entry.host, src, this.#mount);
          stage.set(dest, { kind: "symlink", mode: info.mode, mtime: info.mtime, target });
          return;
        }
        const allowance = stage.available(dest);
        this.#capacity(this.#maxMemory - allowance + info.size);
        const data = memory?.data ?? this.#readForMemory(entry.host, allowance, src, "read");
        stage.set(dest, this.#file(data, info.mode, info.mtime));
      }
    });
  }

  #copy(src, dest, op, recursive) {
    const destination = this.#checkWrite(op, dest, src);
    const to = this.#lookup(destination, op, dest, true, (entry) => entry.virtual);
    const source = absoluteVirtual(src);
    const changes = new Map();
    const from = this.#lookup(source, op, src, true, (entry) => {
      requireEntry(entry, op, src);
      if (entry.info.isDirectory && beneath(entry.virtual, to)) throw fsFailure("EINVAL", `cannot copy '${src}' into itself, '${dest}'`);
      return entry.virtual;
    });
    if (from === to) return;
    let used = this.#used;
    const prior = (p) => (changes.has(p) ? changes.get(p) : this.#entries.get(p))?.data?.length ?? 0;
    const stage = {
      available: (p) => this.#maxMemory - used + prior(p),
      set: (p, value) => {
        used += (value?.data?.length ?? 0) - prior(p);
        this.#capacity(used);
        changes.set(p, value);
      },
    };
    if (op === "mv") {
      for (const p of this.#entries.keys()) if (beneath(from, p)) stage.set(p, undefined);
    }
    this.#copyEntry(stage, from, to, src, { op, recursive });
    if (!changes.size && op === "cp") return;
    this.#parents(changes, to, op, dest, true);
    this.#commit(changes, op === "mv" ? from : undefined);
  }

  async cp(src, dest, options) { this.#copy(src, dest, "cp", !!options?.recursive); }

  async mv(src, dest) { this.#copy(src, dest, "mv", true); }

  #setMetadata(path, op, update) {
    const p = this.#checkWrite(op, path);
    this.#lookup(p, op, path, false, (entry) => {
      requireEntry(entry, op, path);
      const { memory, info } = entry;
      let node = memory;
      if (!node) {
        if (info.isDirectory) node = this.#dir();
        else if (info.isFile) {
          this.#capacity(this.#used + info.size);
          node = this.#file(this.#readForMemory(entry.host, this.#maxMemory - this.#used, path, "read"), info.mode, info.mtime);
        } else throw special(op, path);
      }
      this.#commit(new Map([[entry.virtual, { ...node, ...update }]]));
    });
  }

  async chmod(path, mode) { this.#setMetadata(path, "chmod", { mode }); }
  async utimes(path, _atime, mtime) { this.#setMetadata(path, "utimes", { mtime: new Date(mtime) }); }

  async symlink(target, path) {
    checkedPath("symlink", target, path);
    if (!this.#host.allowSymlinks || this.#readOnly) throw errno("EPERM", "symlink", path);
    // Resolve the link path itself first: the target is read against the
    // resolved parent, never against a lexically collapsed prefix.
    this.#lookup(absoluteVirtual(path), "symlink", path, true, (entry) => {
      const base = dirname(entry.virtual);
      const resolved = target.startsWith("/") ? absoluteVirtual(target) : resolvePath(base, target);
      let targetVirtual = null;
      this.#lookup(resolved, "symlink", path, false, (targetEntry) => {
        targetVirtual = targetEntry.virtual;
      }, 0, true);
      if (beneath(this.#mount, entry.virtual) && !beneath(this.#mount, targetVirtual)) throw outside(path);
      if (entry?.info) throw errno("EEXIST", "symlink", path);
      const changes = new Map();
      this.#parents(changes, entry.virtual, "symlink", path, false);
      changes.set(entry.virtual, { kind: "symlink", mode: 0o777, mtime: new Date(), target });
      this.#commit(changes);
    });
  }

  async link(src, dest) {
    const p = this.#checkWrite("link", dest, src);
    this.#lookup(absoluteVirtual(src), "link", src, false, (entry) => {
      requireEntry(entry, "link", src);
      if (!entry.info.isFile) throw errno("EPERM", "link", src);
      this.#lookup(p, "link", dest, true, (target) => {
        if (target?.info) throw errno("EEXIST", "link", dest);
        const changes = new Map();
        this.#parents(changes, target.virtual, "link", dest, false);
        this.#capacity(this.#used + entry.info.size);
        const data = entry.memory?.data ?? this.#readForMemory(entry.host, this.#maxMemory - this.#used, src, "read");
        changes.set(target.virtual, this.#file(data, entry.info.mode, entry.info.mtime));
        this.#commit(changes);
      });
    });
  }

  async readlink(path) {
    checkedPath("readlink", path);
    return this.#lookup(absoluteVirtual(path), "readlink", path, true, (entry) => {
      requireEntry(entry, "readlink", path);
      if (!entry.info.isSymbolicLink) throw errno("EINVAL", "readlink", path);
      return entry.memory?.target ?? this.#host.readlink(entry.host, path, this.#mount);
    });
  }

  async realpath(path) {
    checkedPath("realpath", path);
    return this.#lookup(absoluteVirtual(path), "realpath", path, false, (entry) => requireEntry(entry, "realpath", path).virtual);
  }
}
