import { sanitizeMessage } from "../core/driver.js";

const messages = {
  ENOENT: "no such file or directory",
  ENOTDIR: "not a directory",
  EEXIST: "file already exists",
  EISDIR: "illegal operation on a directory",
  ENOTEMPTY: "directory not empty",
  EINVAL: "invalid argument",
  ELOOP: "too many symbolic links encountered",
  EPERM: "operation not permitted",
  EROFS: "read-only file system",
};

class FsError extends Error {
  constructor(code, message) {
    super(sanitizeMessage(`${code}: ${message}`));
    this.code = code;
    this.stack = this.toString();
  }
}

export function fsFailure(code, message) {
  return new FsError(code, message);
}

export function errno(code, op, path) {
  return fsFailure(code, messages[code] ? `${messages[code]}, ${op} '${path}'` : `${op} '${path}'`);
}

export function outside(path, suffix = "") {
  return fsFailure("EACCES", `permission denied, '${path}' resolves outside sandbox${suffix}`);
}

export function special(op, path) {
  return fsFailure("EACCES", `cannot ${op} special file '${path}'`);
}

export function checkedPath(op, ...paths) {
  for (const path of paths) {
    if (path.includes("\0")) throw fsFailure("ENOENT", `path contains null byte, ${op} '${path}'`);
  }
}

export function rewriteError(error, op, path, writable = false) {
  if (error instanceof FsError) return error;
  const code = typeof error?.code === "string" && /^[A-Z][A-Z_0-9]*$/.test(error.code) ? error.code : "EIO";
  if (writable && code === "ELOOP" && error.syscall === "open") {
    return fsFailure("EACCES", `permission denied, '${path}' is a symlink`);
  }
  return fsFailure(code, `${op} '${path}'`);
}

export function rootError(name, reason) {
  const error = new Error(`${name} root is ${reason}`);
  error.stack = error.toString();
  return error;
}
