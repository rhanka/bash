import * as fs from "node:fs";
import * as path from "node:path";
import { addHostPathHints } from "../core/driver.js";
import { decodeUtf8, latin1FromBytes, unsafeBytesFromLatin1, utf8 } from "../core/bytes.js";
import { resolvePath } from "../core/paths.js";
import { checkedPath, errno, fsFailure, outside, rootError, special } from "./fs-errors.js";

const { O_RDONLY, O_DIRECTORY = 0, O_NONBLOCK = 0, O_NOFOLLOW = 0 } = fs.constants;
const directoryFlags = O_RDONLY | O_DIRECTORY | O_NONBLOCK | O_NOFOLLOW;

/**
 * Test seam: forces the canonical-path fallback that serves platforms without a
 * usable `/proc/self/fd` (macOS, Windows, procfs-less containers). Tests set it
 * around a single case; production code never touches it.
 */
export const hostTestConfig = { forceFallback: false };

export function contains(root, name) {
  return name === root || name.startsWith(root.endsWith(path.sep) ? root : root + path.sep);
}

export function lstat(name) {
  return fs.lstatSync(name, { throwIfNoEntry: false, bigint: true });
}

export function fstat(fd) {
  return fs.fstatSync(fd, { bigint: true });
}

export function sameInode(a, b) {
  return a?.dev === b.dev && a?.ino === b.ino;
}

function keepOpen(fd, run) {
  let result;
  try {
    result = run();
  } catch (error) {
    fs.closeSync(fd);
    throw error;
  }
  if (result instanceof Promise) return result.finally(() => fs.closeSync(fd));
  fs.closeSync(fd);
  return result;
}

function withDirectory(ctx, name, run) {
  if (!ctx || (ctx.fd && typeof ctx.base !== "string")) {
    throw fsFailure("EIO", "directory context is missing");
  }
  const info = lstat(name);
  if (!info?.isDirectory()) throw Object.assign(new Error(), { code: info ? "ENOTDIR" : "ENOENT" });
  const fd = fs.openSync(name, directoryFlags);
  return keepOpen(fd, () => {
    const opened = fstat(fd);
    if (!opened.isDirectory() || !sameInode(info, opened)) throw Object.assign(new Error(), { code: "EACCES" });
    return run(ctx.fd ? `${ctx.base}/${fd}` : name, fd);
  });
}

export function inDirectory(entry, run) {
  if (entry.directory) return run(entry.name, entry.fd);
  return withDirectory(entry.ctx, entry.name, run);
}

/**
 * Opens the canonical root and verifies the descriptor against procfs when it
 * is usable (Linux). No ancestor above the root is ever opened, so traverse-only
 * ancestors need no read permission. Where `/proc/self/fd` is unavailable
 * (macOS, Windows, procfs-less containers) the call falls back to validated
 * canonical paths: every directory is still opened with `O_NOFOLLOW` and every
 * open is checked against its pre-open `(dev, ino)`, per L7b §5.2.
 */
function openRoot(root, run) {
  if (!hostTestConfig.forceFallback) {
    let fd = null;
    let verified = false;
    try {
      fd = fs.openSync(root, directoryFlags);
      try {
        verified = fs.readlinkSync(`/proc/self/fd/${fd}`) === root;
      } catch {
        // No usable procfs: fall through to the canonical-path fallback below.
      }
      if (verified) {
        return keepOpen(fd, () => run({ fd: true, base: "/proc/self/fd" }, `/proc/self/fd/${fd}`, fd));
      }
    } catch {
      // Opening the root itself failed; the fallback below reports the cause.
    } finally {
      if (!verified && fd !== null) {
        try { fs.closeSync(fd); } catch {}
      }
    }
  }
  // Canonical fallback: operate on canonical path strings. The root descriptor
  // is still held open so directory entries carry a usable fd.
  const fd = fs.openSync(root, directoryFlags);
  return keepOpen(fd, () => run({ fd: false, base: null }, root, fd));
}

/** Resolves a virtual path and keeps its validated parent available for the synchronous operation. */
export class HostPaths {
  constructor(root, name, allowSymlinks) {
    this.absolute = path.resolve(root);
    this.allowSymlinks = allowSymlinks;
    try {
      this.root = fs.realpathSync.native(this.absolute);
      if (!fs.statSync(this.root).isDirectory()) throw rootError(name, "not a directory");
    } catch (error) {
      if (error.code === "ENOENT") {
        const missing = new Error(`${name} root does not exist`);
        missing.stack = missing.toString();
        throw missing;
      }
      if (error.code === "ENOTDIR") throw rootError(name, "not a directory");
      if (error.code) throw fsFailure("EACCES", `${name} root is inaccessible`);
      throw error;
    }
    addHostPathHints(this.absolute, this.root);
  }

  targetParts(target, parent, virtual) {
    if (path.isAbsolute(target)) {
      for (const root of [this.root, this.absolute]) {
        if (contains(root, target)) {
          const suffix = target.slice(root.length).split(path.sep).filter(Boolean);
          if (root !== this.root) {
            try {
              if (fs.realpathSync.native(root) !== this.root) throw outside(virtual);
            } catch {
              throw outside(virtual);
            }
          }
          return suffix;
        }
      }
      throw outside(virtual);
    }
    return [...parent, ...target.split(path.sep === "/" ? "/" : /[\\/]/).filter(Boolean)];
  }

  use(virtual, op, options, run) {
    const parts = virtual.split("/").filter(Boolean);
    // Prevent a virtual filename from becoming a host path component separator or a drive/stream name.
    if (path.sep !== "/" && parts.some((part) => /[\\:]/.test(part))) throw outside(virtual);
    let hops = 0;
    return openRoot(this.root, (ctx, root, rootFd) => {
      const visit = (dir, fd, parent, pending) => {
        if (pending.length === 0) {
          return run({ name: dir, canonical: path.join(this.root, ...parent), info: fstat(fd), directory: true, fd, ctx });
        }
        const [part, ...tail] = pending;
        if (part === ".") return visit(dir, fd, parent, tail);
        if (part === "..") {
          // A `..` above the virtual root clamps for direct paths (L7b §5.1:
          // `/a/../../b` is `/b`), but symlink-target validation stays strict:
          // climbing above the root there rejects, preserving the `mv` escape
          // gate. A deeper `..` restarts from the root so every re-walked
          // component is symlink-checked again instead of being skipped.
          if (!parent.length) {
            if (options.strictDotDot) throw outside(options.display ?? virtual);
            return visit(dir, fd, parent, tail);
          }
          return visit(root, rootFd, [], [...parent.slice(0, -1), ...tail]);
        }
        const name = path.join(dir, part);
        const canonical = path.join(this.root, ...parent, part);
        let info = lstat(name);
        if (info?.isSymbolicLink() && (tail.length || !options.leaf)) {
          if (!this.allowSymlinks && !options.followLinks) {
            if (options.ignoreLinks) return run({ blocked: true, ctx });
            throw outside(options.display ?? virtual);
          }
          if (++hops > 40) throw errno("ELOOP", op, options.display ?? virtual);
          const target = this.targetParts(fs.readlinkSync(name), parent, options.display ?? virtual);
          return visit(root, rootFd, [], [...target, ...tail]);
        }
        if (tail.length === 0) return run({ name, canonical, info, ctx });
        if (!info && options.parents) {
          fs.mkdirSync(name, { mode: 0o755 });
          info = lstat(name);
        }
        if (!info) {
          let depth = parent.length + 1;
          for (const suffix of tail) {
            if (suffix === "..") depth--;
            else if (suffix !== ".") depth++;
            if (depth < 0) throw outside(options.display ?? virtual);
          }
          return run({ missingParent: true, canonical: path.join(canonical, ...tail), ctx });
        }
        if (!info.isDirectory()) throw errno("ENOTDIR", op, options.display ?? virtual);
        return withDirectory(ctx, name, (child, childFd) => visit(child, childFd, [...parent, part], tail));
      };
      return visit(root, rootFd, [], parts);
    });
  }

  virtual(canonical) {
    const relative = path.relative(this.root, canonical);
    return relative ? `/${relative.split(path.sep).join("/")}` : "/";
  }

  readlink(entry, virtual, mountPoint = "/") {
    const target = fs.readlinkSync(entry.name);
    if (!path.isAbsolute(target)) return target;
    try {
      const parts = this.targetParts(target, [], virtual);
      // Check chained targets as well, including a contained dangling target.
      this.use(`/${parts.join("/")}`, "readlink", { followLinks: true }, () => {});
      return resolvePath(mountPoint, parts.join("/"));
    } catch {
      return path.basename(target);
    }
  }

  linkTarget(target, canonicalLink, virtual) {
    const parent = path.relative(this.root, path.dirname(canonicalLink)).split(path.sep).filter(Boolean);
    // Keep `..` segments so the containment check resolves them symlink-aware
    // instead of collapsing across a link; a `..` climbing above the root
    // rejects here (strict target validation, like the `mv` escape gate). The
    // stored host path keeps the same segments (`path.join` would collapse them
    // to a different file).
    const parts = target.startsWith("/")
      ? target.split("/").filter((part) => part !== "" && part !== ".")
      : this.targetParts(target, parent, virtual);
    this.use(`/${parts.join("/")}`, "symlink", { strictDotDot: true }, () => {});
    if (!target.startsWith("/")) return target;
    return `${this.root.replace(/\/+$/, "")}/${parts.join("/")}`;
  }
}

export function formatStat(info, identity = false) {
  const { mode, size, mtime, dev, ino } = info;
  const result = {
    isFile: info.isFile(), isDirectory: info.isDirectory(), isSymbolicLink: info.isSymbolicLink(),
    mode: Number(mode), size: Number(size), mtime, dev: Number(dev), ino: Number(ino),
  };
  if (identity) result.identity = `real:${dev}:${ino}`;
  return result;
}

export function listHost(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
}

/** Opens without truncating and verifies the descriptor before any data operation. */
export function withFile(entry, flags, op, virtual, run) {
  if (!entry.info?.isFile()) throw special(op, virtual);
  const fd = fs.openSync(entry.name, flags | O_NONBLOCK | O_NOFOLLOW);
  return keepOpen(fd, () => {
    const opened = fstat(fd);
    if (!opened.isFile()) throw special(op, virtual);
    if (!sameInode(entry.info, opened)) throw outside(virtual);
    return run(fd, opened);
  });
}

export function readRegular(entry, max, virtual, op = "read") {
  return withFile(entry, O_RDONLY, op, virtual, (fd, info) => {
    const tooLarge = (size) => fsFailure("EFBIG", `file too large, read '${virtual}' (${size} bytes, max ${max})`);
    if (!max) return fs.readFileSync(fd);
    if (info.size > max) throw tooLarge(info.size);
    const data = new Uint8Array(Number(info.size));
    let count = 0;
    while (count < data.length) {
      const n = fs.readSync(fd, data, count, data.length - count);
      if (!n) return data.subarray(0, count);
      count += n;
    }
    // The file may have grown after fstat. Keep the cap even when its writer is outside the sandbox.
    const chunks = [data];
    for (;;) {
      const chunk = new Uint8Array(Math.min(65536, max - count + 1));
      const n = fs.readSync(fd, chunk, 0, chunk.length);
      if (!n) return chunks.length === 1 ? data : Buffer.concat(chunks, count);
      count += n;
      if (count > max) throw tooLarge(count);
      chunks.push(chunk.subarray(0, n));
    }
  });
}

function encodingOf(options) {
  return String((typeof options === "string" ? options : options?.encoding) ?? "utf8").toLowerCase().replace("-", "");
}

export function decodeContent(bytes, options) {
  const encoding = encodingOf(options);
  return encoding === "latin1" || encoding === "binary" ? latin1FromBytes(bytes) : decodeUtf8(bytes);
}

export function encodeContent(content, options) {
  if (content instanceof Uint8Array) return content;
  if (typeof content !== "string") throw new TypeError("file content must be a string or a Uint8Array");
  const encoding = encodingOf(options);
  return encoding === "latin1" || encoding === "binary" ? unsafeBytesFromLatin1(content) : utf8(content);
}

export function resolveVirtual(base, name) {
  checkedPath("resolvePath", base, name);
  return resolvePath(base, name);
}

/**
 * Makes a virtual path absolute without collapsing `..` or `.`: symlink-aware
 * lookup must see every component, so lexical pre-collapse is forbidden. The
 * lookup itself (`HostPaths.use`, overlay `#lookup`) resolves `..` against the
 * symlink-checked prefix and clamps it at the root (L7b §5.1).
 */
export function absoluteVirtual(p) {
  return p.startsWith("/") ? p : `/${p}`;
}
