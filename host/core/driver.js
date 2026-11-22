// The driver loop (spec 7.3): runs one fiber to its result, serving host calls through the membrane.
// The loop is a generator so that the asynchronous form (`exec`) and the synchronous form (`execSync`)
// share one body: the generator yields a promise to await, or a tick to give the event loop a turn.
import { concat, decodeUtf8, latin1FromBytes, unsafeBytesFromLatin1, utf8, utf8Length } from "./bytes.js";
import { AsyncCapabilityError, KernelPoisonedError } from "./errors.js";
import { fsState } from "./fs.js";
import { resolvePath } from "./paths.js";
import { stringLimit } from "./limits.js";
// `ext.js` (`serve-custom`, `fs-batch`, `membrane`) stays off the cold path:
// extension scopes, custom commands, and the L7b host-call batcher load only
// when actually used (never for `new Bash()` plus the first `echo` with no
// options). A static relative `import()` keeps the specifier G2-clean; yielding
// its promise preserves the sync/async contract (`execSync` with host I/O or
// custom commands already rejects with `AsyncCapabilityError`).

const loadExt = () => import("./ext.js");

export const CHUNK = 1 << 20;
const FUEL = 2_048n;
const YIELD_MS = 10;
const SCRIPT_MAX_BYTES = 3 << 20;
const FATAL_STDERR = "bash: internal error: sandbox instance failed\n";

export const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

/** Driver-private tag for a mount entry backed by a kernel `InMemoryFs` volume (L7c D27). */
const KERNEL_VOLUME = Symbol("kernel-volume");

/** Marks mount id 1 as a kernel-backed volume; the facade holds the entry, the driver serves it. */
export function markKernelVolume(fs) {
  return Object.freeze({ [KERNEL_VOLUME]: true, fs });
}

/** Returns the branded volume of a mount entry, or null for host-adapter entries. */
export function kernelVolumeOf(entry) {
  return entry?.[KERNEL_VOLUME] ? entry.fs : null;
}

/** An object keyed by sandbox data: null prototype (EVOL S10). */
export function nullObject(entries) {
  const o = Object.create(null);
  if (entries) for (const [k, v] of entries) o[k] = v;
  return o;
}

export function envToObject(list) {
  const o = Object.create(null);
  for (const e of list) o[decodeUtf8(e.name)] = decodeUtf8(e.value);
  return o;
}

function fatalResult(B) {
  const r = { stdout: "", stderr: FATAL_STDERR, exitCode: 1, env: nullObject(Object.entries(B.initialEnv)) };
  Object.defineProperty(r, "fatal", { value: true, enumerable: false });
  return r;
}

function limitResult(B, name, overlay) {
  const env = nullObject(Object.entries(B.initialEnv));
  for (const [key, value] of overlay) env[key] = value;
  return { stdout: "", stderr: `bash: ${name} exceeded\n`, exitCode: 126, env };
}

/** Awaits the end of an asynchronous generator-driven loop. */
export async function runAsync(gen) {
  let r = gen.next();
  while (!r.done) {
    const y = r.value;
    try {
      const v = y.promise ? await y.promise : await new Promise((res) => setTimeout(res, 0));
      r = gen.next(v);
    } catch (e) {
      r = gen.throw(e);
    }
  }
  return r.value;
}

/** Runs the loop without yielding; an awaitable request is an error. */
export function runSync(gen) {
  let r = gen.next();
  while (!r.done) {
    if (r.value.promise) {
      gen.return(undefined);
      throw new AsyncCapabilityError();
    }
    r = gen.next();
  }
  return r.value;
}

export function anySignal(a, b) {
  if (!a) return b;
  if (!b) return a;
  if (typeof AbortSignal.any === "function") return AbortSignal.any([a, b]);
  const c = new AbortController();
  const abort = (event) => {
    a.removeEventListener("abort", abort);
    b.removeEventListener("abort", abort);
    c.abort(event.target.reason);
  };
  if (a.aborted || b.aborted) c.abort((a.aborted ? a : b).reason);
  else {
    a.addEventListener("abort", abort, { once: true });
    b.addEventListener("abort", abort, { once: true });
  }
  return c.signal;
}

/**
 * The exec generator.
 * @param {object} B internal state of the Bash
 * @param {string} script
 * @param {object} o normalized options: { env: [name, value][], envGiven, replaceEnv, cwd, rawScript, stdin: Uint8Array, args: string[], signal, bypass, parent }
 */
export function* execLoop(B, script, o) {
  const inst = B.holder.instance;
  inst.check();
  const L = B.limits;
  // Pre-checks (7.3 step 1): nothing is lowered, no kernel call is made.
  let cwd = o.cwd;
  if (cwd && !cwd.startsWith("/")) cwd = `${B.initialCwd === "/" ? "" : B.initialCwd}/${cwd}`;
  const strings = [...o.env.flat(), ...o.args, cwd ?? "", o.bypass ?? ""];
  const limit = stringLimit(strings, L.maxStringLength, B.headroom, o.env.length * 20 + o.args.length * 8);
  if (limit) return limitResult(B, limit, o.env);

  const utf16 = script.length;
  let scriptBytes;
  let overflow;
  const bytes = utf8Length(script);
  if (bytes > SCRIPT_MAX_BYTES) {
    overflow = { utf16Units: BigInt(utf16), utf8Bytes: BigInt(bytes) };
    scriptBytes = new Uint8Array(0);
  } else scriptBytes = utf8(script);

  const x = {
    B,
    inst,
    fiber: null,
    out: [],
    err: [],
    scopes: new Set(),
    signal: o.signal,
    deadline: now() + L.maxExecutionTimeMs,
  };
  const engine = inst.engine;
  try {
    let reason = cancelReason(x);
    if (!reason && cwd && o.envGiven && !o.env.some(([name]) => name === "PWD")) {
      const candidate = resolvePath("/", cwd);
      if (B.userFs) {
        // Extension scope loads only on a host-backed cwd lookup (never on the
        // cold path). A static relative `import()` keeps the specifier G2-clean.
        const { ext } = yield { promise: loadExt() };
        const resolved = yield* ext.serveExtension(x, async (scope) => {
          const fs = scope.wrap(B.userFs, ext.FS_METHODS);
          if (fs.realpath) return fs.realpath(cwd);
          if (fs.stat && candidate !== cwd && await fs.stat(candidate)) return candidate;
        }, undefined);
        reason = resolved.cancel;
        const path = resolved.outcome;
        if (typeof path === "string" && path.startsWith("/") &&
            !stringLimit([path], L.maxStringLength, B.headroom)) cwd = path;
      } else if (o.env.length === 0 && candidate !== cwd &&
          !stringLimit([candidate], L.maxStringLength, B.headroom)) {
        const st = fsState(B.volumeFs ?? B.rootFs);
        if (st.holder === B.holder) {
          const result = inst.call(() => st.vfs.op({ tag: "exists", val: utf8(candidate) }));
          if (result.tag === "ok" && result.val.val) cwd = candidate;
        } else {
          // A separate volume: a lost volume keeps the requested cwd; guest I/O reports `io`.
          const target = st.holder.instance;
          if (target) {
            try {
              const result = target.call(() => st.vfs.op({ tag: "exists", val: utf8(candidate) }));
              if (result.tag === "ok" && result.val.val) cwd = candidate;
            } catch {
              /* keep the requested cwd */
            }
          }
        }
      }
    }
    x.fiber = inst.call(() =>
      engine.Fiber.spawn(B.shell, o.parent?.fiber, {
        script: scriptBytes,
        scriptOverflow: overflow,
        args: o.args.map(utf8),
        env: o.env.map(([n, v]) => ({ name: utf8(n), value: utf8(v), exported: /^[A-Za-z_][A-Za-z_0-9]*$/.test(n) })),
        replaceEnv: !!o.replaceEnv,
        cwd: cwd === undefined ? undefined : utf8(cwd),
        rawScript: !!o.rawScript,
        bypassCustom: o.bypass,
      }),
    );
    reason ??= o.signal?.aborted ? "aborted" : undefined;
    if (reason) return finish(x, cancel(x, reason));
    // stdin in chunks of at most 1 MiB, the last with `last = true`.
    const stdin = o.stdin;
    let pos = 0;
    do {
      const end = Math.min(stdin.length, pos + CHUNK);
      const last = end >= stdin.length;
      const chunk = stdin.subarray(pos, end);
      inst.call(() => x.fiber.writeStdin(chunk, last));
      pos = end;
    } while (pos < stdin.length);

    let lastYield = now();
    let step = inst.call(() => x.fiber.run(FUEL));
    for (;;) {
      if (step.tag === "done") return finish(x, step.val);
      drain(x);
      let reason = cancelReason(x);
      if (reason) return finish(x, cancel(x, reason));
      if (now() - lastYield >= YIELD_MS) {
        yield { tick: true };
        lastYield = now();
        reason = cancelReason(x);
        if (reason) return finish(x, cancel(x, reason));
      }
      if (step.tag === "out-of-fuel") {
        step = inst.call(() => x.fiber.run(FUEL));
        continue;
      }
      // host-call
      const served = yield* serve(x, step.val);
      if (served.cancel) return finish(x, cancel(x, served.cancel));
      step = inst.call(() => x.fiber.resume(step.val.epoch, served.outcome));
    }
  } catch (e) {
    if (e instanceof KernelPoisonedError) return fatalResult(B);
    if (!x.fiber && e?.code === "ceiling") return limitResult(B, e.detail || "memory-ceiling", o.env);
    throw e;
  } finally {
    for (const s of x.scopes) s.revoke();
    if (x.fiber && !inst.poisoned) {
      try {
        x.fiber[Symbol.dispose]();
      } catch {
        /* disposal of a handle never fails an exec */
      }
    }
  }
}

export function cancelReason(x) {
  if (x.signal?.aborted) return "aborted";
  if (now() >= x.deadline) return "deadline";
}

function cancel(x, reason) {
  for (const s of x.scopes) s.revoke();
  return x.inst.call(() => x.fiber.cancel(reason));
}

function drain(x) {
  const { inst, fiber } = x;
  for (;;) {
    const b = inst.call(() => fiber.takeStdout(CHUNK));
    if (b.length === 0) break;
    x.out.push(b);
  }
  for (;;) {
    const b = inst.call(() => fiber.takeStderr(CHUNK));
    if (b.length === 0) break;
    x.err.push(b);
  }
}

function finish(x, er) {
  drain(x);
  const env = envToObject(er.env);
  let stderr = decodeUtf8(concat(x.err));
  if (x.err.length === 0) {
    if (er.status === "limit-exceeded" && er.limitName) {
      stderr = `bash: ${er.limitName} exceeded\n`;
    }
    if (er.status === "aborted" || er.status === "deadline") stderr = "bash: execution aborted\n";
  }
  const r = {
    stdout: (er.stdoutKind === "bytes" ? latin1FromBytes : decodeUtf8)(concat(x.out)),
    stderr,
    exitCode: er.exitCode,
    env,
  };
  if (er.stdoutKind === "bytes") r.stdoutKind = "bytes";
  return r;
}

// ---------------------------------------------------------------------------------------- host calls

/**
 * Serves one `fs-batch` against a kernel-backed volume (L7c D27), synchronously.
 * Every op runs in order through the volume owner's `Instance.call(() => vfs.op(op))`,
 * so the result records keep the existing WIT forms; ops the volume does not
 * implement (L7a: links, chmod, lazy, ...) come back `unsupported` from the volume itself.
 * The serialized request and the aggregate response are each bounded by
 * `min(1 MiB, floor(requesting shell headroom / 2))`, counting the encoded records
 * and their variable-length payloads. An oversized request reaches zero volume ops;
 * on response overflow the batch stops with its completed mutations visible and no
 * partial listing is delivered. Either overflow resumes the execution fiber once at
 * the same epoch with `limit-exceeded("headroom")` (the fiber ends that exec with 126).
 * A lost volume yields an ordinary `io` error per op and zero guest calls to its owner.
 */
/** Served outcome of one synchronous volume batch; exported for the L7c bound tests. */
export function serveKernelVolume(x, pc, volFs) {
  const ops = pc.req.val.ops;
  const bound = Math.min(1 << 20, Math.floor(x.B.headroom / 2));
  const st = fsState(volFs);
  const target = st.holder.instance;
  const io = (op) => ({ tag: "err", val: { code: "io", path: batchOpPath(op) } });
  if (!target || target.poisoned) return { outcome: { tag: "fs", val: ops.map(io) } };
  let request = 0;
  for (const op of ops) request += REQ_FIXED + payloadBytes(op.val);
  if (request > bound) return { outcome: { tag: "limit-exceeded", val: "headroom" } };
  const results = [];
  let response = 0;
  for (const op of ops) {
    let r;
    try {
      r = target.call(() => st.vfs.op(op));
    } catch {
      // The volume was lost mid-batch: the remaining ops report `io` without
      // touching the failed owner again.
      return { outcome: { tag: "fs", val: [...results, ...ops.slice(results.length).map(io)] } };
    }
    response += resultSize(r);
    if (response > bound) return { outcome: { tag: "limit-exceeded", val: "headroom" } };
    results.push(r);
  }
  return { outcome: { tag: "fs", val: results } };
}

/** Fixed per-record sizes for the synchronous mount bounds (the kernel batch gate uses 72 per op). */
const REQ_FIXED = 72;
const RES_FIXED = 16;

/** Sums the variable-length payloads (`Uint8Array`) of a lifted ABI value. */
function payloadBytes(value) {
  if (value instanceof Uint8Array) return value.length;
  if (Array.isArray(value)) return value.reduce((n, e) => n + payloadBytes(e), 0);
  if (value !== null && typeof value === "object") {
    let n = 0;
    for (const e of Object.values(value)) {
      if (typeof e !== "bigint" && typeof e !== "number" && typeof e !== "string" && typeof e !== "boolean") n += payloadBytes(e);
    }
    return n;
  }
  return 0;
}

/**
 * Encoded size of one batch result: the record plus its variable-length
 * payloads. Listings count one fixed record per entry (12 for `read-dir`,
 * 56 for `read-dir-plus`/`walk`, 8 for `all-paths`), mirroring the ABI
 * lowering, so an oversized listing overflows instead of truncating.
 */
function resultSize(r) {
  if (r.tag === "err") return RES_FIXED + r.val.path.length;
  const v = r.val;
  switch (v.tag) {
    case "path":
    case "data":
      return RES_FIXED + v.val.length;
    case "paths":
      return RES_FIXED + v.val.reduce((n, p) => n + 8 + p.length, 0);
    case "entries":
      return RES_FIXED + v.val.reduce((n, e) => n + 12 + e.name.length, 0);
    case "entries-plus":
      return RES_FIXED + v.val.reduce((n, e) => n + 56 + e.path.length, 0);
    default:
      return RES_FIXED + 32;
  }
}

/** The virtual path carried by a batch op (mirrors the async `fs-batch` mapping). */
function batchOpPath(op) {
  const val = op.val;
  if (op.tag === "symlink") return val.target;
  if (val instanceof Uint8Array) return val;
  return val?.path ?? val?.root ?? val?.source ?? utf8("/");
}

function* serve(x, pc) {
  const B = x.B;
  const req = pc.req;
  B.hooks?.onHostCall?.(req);
  switch (req.tag) {
    case "custom-command": {
      const { ext } = yield { promise: loadExt() };
      return yield* ext.serveCustom(x, pc.epoch, req.val);
    }
    case "sleep": {
      if (!B.sleep) return { outcome: { tag: "denied" } };
      const { ext } = yield { promise: loadExt() };
      return yield* ext.serveExtension(x, (scope) => scope.guard(B.sleep)(Number(req.val)).then(() => ({ tag: "completed" })), { tag: "failed", val: utf8("sleep failed") });
    }
    case "fs-batch": {
      // A kernel-backed volume is served synchronously against the owning Vfs:
      // no promises, no public FS callbacks, no yields for FS I/O.
      const volFs = kernelVolumeOf(B.mounts.get(req.val.mountId));
      if (volFs) return serveKernelVolume(x, pc, volFs);
      const { ext } = yield { promise: loadExt() };
      return yield* ext.serveFsBatch(x, req.val, ext.serveExtension);
    }
    default:
      return { outcome: { tag: "denied" } };
  }
}

const HOST_PATH_HINTS = new Set();
const PATH_TAIL = String.raw`[^\s'":<>|]*`;
const GENERIC_HOST_PREFIX = String.raw`file:\/\/(?:(?:[^\s/'":<>|]*\/)?[A-Za-z]:)?|[A-Za-z]:[\\/]|\/(?:Users|root|var|tmp|private|opt|mnt|usr|srv|etc|workspace|app|nix|proc|run)\/|\/home\/(?!user(?:\/|\b))`;
let hostPath = new RegExp(`(?:${GENERIC_HOST_PREFIX})${PATH_TAIL}`, "g");

/** Registers host path prefixes that must never appear in the stderr of a failed command (node loader). */
export function addHostPathHints(...paths) {
  for (const p of paths) {
    if (p && p !== "/") HOST_PATH_HINTS.add(p);
  }
  const hints = [...HOST_PATH_HINTS].sort((a, b) => b.length - a.length).map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  hostPath = new RegExp(`(?:${[...hints, GENERIC_HOST_PREFIX].join("|")})${PATH_TAIL}`, "g");
}

/** Replaces host paths in an error message by `<path>`. */
export function sanitizeMessage(text) {
  return String(text).replace(hostPath, "<path>");
}

/** Quotes an argument for the shell language (single quotes), leaving plain words untouched. */
export function shellQuote(s) {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(s) ? s : `'${s.replaceAll("'", "'\\''")}'`;
}

