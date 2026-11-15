// Pure path helpers (POSIX, absolute paths).

/** Normalizes `path` against `base`: collapses `.` and `..` and never leaves `/`. */
export function resolvePath(base, path) {
  const full = path.startsWith("/") ? path : `${base}/${path}`;
  const out = [];
  for (const part of full.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return `/${out.join("/")}`;
}

/** Parent directory of a normalized path. */
export function dirname(path) {
  const i = path.lastIndexOf("/");
  return i <= 0 ? "/" : path.slice(0, i);
}
