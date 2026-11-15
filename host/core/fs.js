// `InMemoryFs`: the IFileSystem of EVOL Annex D.3 over the kernel's `vfs.op` (EVOL D5).
import { decodeUtf8, concat, latin1FromBytes, unsafeBytesFromLatin1, utf8 } from "./bytes.js";
import { NotSupportedError } from "./errors.js";
import { Holder } from "./holder.js";
import { DEFAULT_PROFILE, PROFILES, profileLimits } from "./limits.js";
import { resolvePath as resolve, dirname } from "./paths.js";

const CHUNK = 1 << 20;

const ERRNO = {
  "not-found": ["ENOENT", "no such file or directory"],
  exists: ["EEXIST", "file already exists"],
  "not-directory": ["ENOTDIR", "not a directory"],
  "is-directory": ["EISDIR", "illegal operation on a directory"],
  "not-empty": ["ENOTEMPTY", "directory not empty"],
  permission: ["EACCES", "permission denied"],
  invalid: ["EINVAL", "invalid argument"],
  "link-loop": ["ELOOP", "too many symbolic links encountered"],
  "too-large": ["EFBIG", "file too large"],
  "no-space": ["ENOSPC", "no space left on device"],
  busy: ["EBUSY", "resource busy or locked"],
  "cross-device": ["EXDEV", "cross-device link not permitted"],
  "read-only": ["EROFS", "read-only file system"],
  unsupported: ["ENOSYS", "function not implemented"],
  io: ["EIO", "i/o error"],
};

/** An Error whose message starts with the POSIX code (Annex D.3). */
export function fsError(code, syscall, path) {
  const [errno, text] = ERRNO[code] ?? ERRNO.io;
  const e = new Error(`${errno}: ${text}, ${syscall} '${path}'`);
  e.code = errno;
  e.syscall = syscall;
  e.path = path;
  return e;
}

export function enosys(syscall, path) {
  return fsError("unsupported", syscall, path);
}

function encodeContent(content, encoding) {
  if (typeof content === "string") return encoding === "latin1" || encoding === "binary" ? unsafeBytesFromLatin1(content) : utf8(content);
  if (content instanceof Uint8Array) return content;
  throw new TypeError("file content must be a string or a Uint8Array");
}

function encodingOf(o) {
  const e = typeof o === "string" ? o : o?.encoding;
  return e === undefined || e === null ? "utf8" : String(e).toLowerCase().replace("-", "");
}

function statOut(s) {
  return {
    isFile: s.kind === "file",
    isDirectory: s.kind === "directory",
    isSymbolicLink: s.kind === "symlink",
    mode: s.mode,
    size: Number(s.size),
    mtime: new Date(Number(s.mtimeMs)),
    dev: Number(s.dev),
    ino: Number(s.ino),
  };
}

function toMs(v) {
  if (v === undefined) return undefined;
  if (v instanceof Date) return BigInt(v.getTime());
  if (typeof v === "number") return BigInt(Math.trunc(v));
  return undefined;
}

/**
 * Writes the `files` option into a filesystem: a path maps to a string, bytes, a `{content, mode, mtime}`
 * object or a function (a lazy file). Missing parent directories are created.
 */
export function addFiles(fs, files, base = "/") {
  if (files === null || typeof files !== "object") throw new TypeError("initial files must be an object");
  for (const [path, value] of Object.entries(files)) {
    const p = resolve(base, path);
    const dir = dirname(p);
    if (dir !== "/") fs.mkdirSync(dir, { recursive: true });
    if (typeof value === "function") {
      fs.writeFileLazy(p, value);
      continue;
    }
    let content = value;
    let options;
    if (value !== null && typeof value === "object" && !(value instanceof Uint8Array)) {
      content = value.content;
      options = { mode: value.mode, mtime: value.mtime };
    }
    if (typeof content !== "string" && !(content instanceof Uint8Array)) throw new TypeError(`files: the content of ${path} must be a string or bytes`);
    fs.writeFileSync(p, content, undefined, options);
  }
}

/** Private state of an InMemoryFs. */
const STATE = new WeakMap();

/** Returns the internal state of an InMemoryFs, or undefined for any other object. */
export function fsState(fs) {
  return STATE.get(fs);
}

/**
 * Builds the InMemoryFs class bound to a loader (Node or browser).
 * @param {{ syncModule(): any, asyncModule(): Promise<any> }} loader
 */
export function createInMemoryFs(loader) {
  class InMemoryFs {
    /**
     * @param {Record<string, unknown>} [initialFiles]
     */
    constructor(initialFiles, internal) {
      const hooks = internal?.hooks;
      const profile = internal?.profile ?? DEFAULT_PROFILE;
      const maxBytes = internal?.maxFileSystemBytes ?? profileLimits(profile).maxFileSystemBytes;
      const ceiling = hooks?.ceilingBytes ?? PROFILES[profile].ceiling;
      const holder = internal?.holder ?? new Holder(loader, hooks, ceiling);
      const st = { holder, vfs: null, lazy: new Set(), owner: internal?.owner ?? null };
      STATE.set(this, st);
      holder.stage((inst) => {
        st.vfs = inst.call(() => inst.engine.Vfs.create(BigInt(maxBytes)));
      });
      if (initialFiles !== undefined) addFiles(this, initialFiles);
    }

    #state() {
      const st = STATE.get(this);
      return st;
    }

    #instance() {
      const inst = this.#state().holder.instance;
      if (!inst) throw new TypeError("the kernel module is not compiled yet: call `await Bash.precompile()` before synchronous reads");
      inst.check();
      return inst;
    }

    /** Runs one vfs operation; the result is `{tag: "ok" | "err", val}`. */
    #op(inst, req) {
      const st = this.#state();
      return inst.call(() => st.vfs.op(req));
    }

    #unwrap(inst, req, syscall, path) {
      const r = this.#op(inst, req);
      if (r.tag === "err") throw fsError(r.val.code, syscall, path);
      return r.val.val;
    }

    async #ready() {
      const inst = await this.#state().holder.ensure();
      inst.check();
      return inst;
    }

    #writeSync(path, bytes, opts = {}, append = false) {
      const holder = this.#state().holder;
      const run = (inst) => {
        let pos = 0;
        do {
          const chunk = bytes.subarray(pos, pos + CHUNK);
          const first = pos === 0;
          this.#unwrap(
            inst,
            {
              tag: "write-file",
              val: {
                path: utf8(path),
                data: chunk,
                offset: append ? undefined : BigInt(pos),
                truncate: first && !append,
                mode: opts.mode,
                mtimeMs: toMs(opts.mtime) ?? BigInt(Date.now()),
              },
            },
            "open",
            path,
          );
          pos += chunk.length;
        } while (pos < bytes.length);
        this.#state().lazy.delete(path);
      };
      if (holder.instance) {
        holder.instance.check();
        run(holder.instance);
      } else holder.stage(run);
    }

    // ---- synchronous additions ----

    mkdirSync(path, options) {
      const p = resolve("/", path);
      const recursive = !!options?.recursive;
      const run = (inst) => this.#unwrap(inst, { tag: "mkdir", val: { path: utf8(p), recursive, mode: undefined } }, "mkdir", p);
      const holder = this.#state().holder;
      if (holder.instance) {
        holder.instance.check();
        run(holder.instance);
      } else holder.stage(run);
    }

    writeFileSync(path, content, encoding, options) {
      this.#writeSync(resolve("/", path), encodeContent(content, encodingOf(encoding)), options);
    }

    /** Stores a lazy file; reading it rejects with NotSupportedError in this version. */
    writeFileLazy(path, load, options) {
      if (typeof load !== "function") throw new TypeError("a lazy file needs a function");
      const p = resolve("/", path);
      this.#writeSync(p, new Uint8Array(0), { mode: options?.mode, mtime: options?.mtime });
      this.#state().lazy.add(p);
    }

    // ---- IFileSystem ----

    async readFile(path, options) {
      const bytes = await this.#readBytes(resolve("/", path));
      return encodingOf(options) === "latin1" || encodingOf(options) === "binary" ? latin1FromBytes(bytes) : decodeUtf8(bytes);
    }

    async readFileBytes(path) {
      return latin1FromBytes(await this.#readBytes(resolve("/", path)));
    }

    async readFileBuffer(path) {
      return this.#readBytes(resolve("/", path));
    }

    async #readBytes(p) {
      const inst = await this.#ready();
      if (this.#state().lazy.has(p)) throw new NotSupportedError(`lazy file ${p}: lazy files are not supported yet`);
      const chunks = [];
      let offset = 0n;
      for (;;) {
        const d = this.#unwrap(inst, { tag: "read-file", val: { path: utf8(p), offset, len: CHUNK } }, "open", p);
        chunks.push(d);
        offset += BigInt(d.length);
        if (d.length < CHUNK) break;
      }
      return chunks.length === 1 ? chunks[0] : concat(chunks);
    }

    async writeFile(path, content, encoding) {
      const p = resolve("/", path);
      const inst = await this.#ready();
      void inst;
      this.#writeSync(p, encodeContent(content, encodingOf(encoding)));
    }

    async appendFile(path, content, encoding) {
      const p = resolve("/", path);
      await this.#ready();
      this.#writeSync(p, encodeContent(content, encodingOf(encoding)), {}, true);
    }

    async exists(path) {
      const inst = await this.#ready();
      return this.#unwrap(inst, { tag: "exists", val: utf8(resolve("/", path)) }, "access", path);
    }

    async stat(path) {
      const inst = await this.#ready();
      const p = resolve("/", path);
      return statOut(this.#unwrap(inst, { tag: "stat", val: utf8(p) }, "stat", p));
    }

    async lstat(path) {
      const inst = await this.#ready();
      const p = resolve("/", path);
      return statOut(this.#unwrap(inst, { tag: "lstat", val: utf8(p) }, "lstat", p));
    }

    async mkdir(path, options) {
      const inst = await this.#ready();
      const p = resolve("/", path);
      this.#unwrap(inst, { tag: "mkdir", val: { path: utf8(p), recursive: !!options?.recursive, mode: undefined } }, "mkdir", p);
    }

    async readdir(path) {
      const inst = await this.#ready();
      const p = resolve("/", path);
      return this.#unwrap(inst, { tag: "read-dir", val: utf8(p) }, "scandir", p).map((e) => decodeUtf8(e.name));
    }

    async readdirWithFileTypes(path) {
      const inst = await this.#ready();
      const p = resolve("/", path);
      return this.#unwrap(inst, { tag: "read-dir", val: utf8(p) }, "scandir", p).map((e) => ({
        name: decodeUtf8(e.name),
        isFile: e.kind === "file",
        isDirectory: e.kind === "directory",
        isSymbolicLink: e.kind === "symlink",
      }));
    }

    async rm(path, options) {
      const inst = await this.#ready();
      const p = resolve("/", path);
      this.#unwrap(inst, { tag: "remove", val: { path: utf8(p), recursive: !!options?.recursive, force: !!options?.force } }, "rm", p);
    }

    resolvePath(base, path) {
      return resolve(base, path);
    }

    getAllPaths() {
      const inst = this.#instance();
      return this.#unwrap(inst, { tag: "all-paths" }, "readdir", "/").map((p) => decodeUtf8(p));
    }

    // ---- not backed yet: later lots (EVOL L7a) ----

    async createExclusive(path) {
      throw enosys("open", path);
    }
    async cp(src) {
      throw enosys("cp", src);
    }
    async mv(src) {
      throw enosys("rename", src);
    }
    async chmod(path) {
      throw enosys("chmod", path);
    }
    async symlink(_target, path) {
      throw enosys("symlink", path);
    }
    async link(_existing, path) {
      throw enosys("link", path);
    }
    async readlink(path) {
      throw enosys("readlink", path);
    }
    async realpath(path) {
      throw enosys("realpath", path);
    }
    async utimes(path) {
      throw enosys("utimes", path);
    }
  }
  return InMemoryFs;
}
