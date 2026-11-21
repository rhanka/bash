// The capability membrane (EVOL D9, S6): revocable scopes tied to one host call of one fiber.
import { ExecutionAbortedError } from "./errors.js";

/** A scope hands extensions wrapper objects; after `revoke` every wrapper method throws or rejects. */
export class Scope {
  constructor() {
    this.live = true;
    this.controller = new AbortController();
  }

  /** Throws `ExecutionAbortedError` when the scope is revoked. */
  check() {
    if (!this.live) throw new ExecutionAbortedError();
  }

  revoke() {
    if (!this.live) return;
    this.live = false;
    this.controller.abort(new ExecutionAbortedError());
  }

  get signal() {
    return this.controller.signal;
  }

  /**
   * Wraps one function of an extension capability. A revoked scope rejects (or throws, for a synchronous
   * method); results are checked again when they settle.
   */
  guard(fn, self, sync = false) {
    return (...args) => {
      if (!this.live) {
        const e = new ExecutionAbortedError();
        if (sync) throw e;
        return Promise.reject(e);
      }
      const r = fn.apply(self, args);
      if (r !== null && typeof r === "object" && typeof r.then === "function") {
        return r.then(
          (v) => {
            this.check();
            return v;
          },
          (e) => {
            this.check();
            throw e;
          },
        );
      }
      return r;
    };
  }

  /** A wrapper object exposing the listed methods of `target` through `guard`. */
  wrap(target, names) {
    const out = {};
    for (const n of names) if (typeof target[n] === "function") out[n] = this.guard(target[n], target, SYNC_FS.has(n));
    return out;
  }
}

const SYNC_FS = new Set(["resolvePath", "getAllPaths"]);

/** The methods of IFileSystem (Annex D.3) that a wrapper exposes. */
export const FS_METHODS = [
  "readFile",
  "readFileBytes",
  "readFileBuffer",
  "writeFile",
  "appendFile",
  "exists",
  "stat",
  "lstat",
  "mkdir",
  "createExclusive",
  "readdir",
  "readdirWithFileTypes",
  "rm",
  "cp",
  "mv",
  "resolvePath",
  "getAllPaths",
  "chmod",
  "symlink",
  "link",
  "readlink",
  "realpath",
  "utimes",
];

/** Waits for `promise`, or until `ms` have passed, whichever is first; never rejects. */
export function settleWithin(promise, ms) {
  return new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    promise.then(
      () => {
        clearTimeout(t);
        resolve();
      },
      () => {
        clearTimeout(t);
        resolve();
      },
    );
  });
}
