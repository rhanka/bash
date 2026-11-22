// The driver loop (spec 7.3): runs one fiber to its result, serving host calls through the membrane.
// The loop is a generator so that the asynchronous form (`exec`) and the synchronous form (`execSync`)
// share one body: the generator yields a promise to await, or a tick to give the event loop a turn.
import { concat, decodeUtf8, latin1FromBytes, unsafeBytesFromLatin1, utf8, utf8Length } from "./bytes.js";
import { AsyncCapabilityError, KernelPoisonedError } from "./errors.js";
import { fsState } from "./fs.js";
import { resolvePath } from "./paths.js";
import { stringLimit } from "./limits.js";
import { FS_METHODS, Scope, settleWithin } from "./membrane.js";
import { serveFsBatch } from "./fs-batch.js";

const CHUNK = 1 << 20;
const FUEL = 2_048n;
const YIELD_MS = 10;
const SCRIPT_MAX_BYTES = 3 << 20;
const FATAL_STDERR = "bash: internal error: sandbox instance failed\n";

const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

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
        const resolved = yield* serveExtension(x, async (scope) => {
          const fs = scope.wrap(B.userFs, FS_METHODS);
          if (fs.realpath) return fs.realpath(cwd);
          if (fs.stat && candidate !== cwd && await fs.stat(candidate)) return candidate;
        }, undefined);
        reason = resolved.cancel;
        const path = resolved.outcome;
        if (typeof path === "string" && path.startsWith("/") &&
            !stringLimit([path], L.maxStringLength, B.headroom)) cwd = path;
      } else if (o.env.length === 0 && candidate !== cwd &&
          !stringLimit([candidate], L.maxStringLength, B.headroom)) {
        const { vfs } = fsState(B.rootFs);
        const result = inst.call(() => vfs.op({ tag: "exists", val: utf8(candidate) }));
        if (result.tag === "ok" && result.val.val) cwd = candidate;
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

function cancelReason(x) {
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

/** Waits for `promise` or for the first of abort, wall-time deadline and poisoning. */
function raceExec(x, promise) {
  return new Promise((resolve) => {
    let over = false;
    const cleanups = [];
    const fin = (r) => {
      if (over) return;
      over = true;
      for (const c of cleanups) c();
      resolve(r);
    };
    const interrupted = () => {
      if (x.inst.poisoned) return { kind: "poison" };
      const reason = cancelReason(x);
      if (reason) return { kind: reason === "aborted" ? "abort" : "deadline" };
    };
    promise.then(
      (value) => fin(interrupted() ?? { kind: "ok", value }),
      (error) => fin(interrupted() ?? { kind: "err", error }),
    );
    const already = interrupted();
    if (already) return fin(already);
    const sig = x.signal;
    if (sig) {
      const on = () => fin({ kind: "abort" });
      sig.addEventListener("abort", on, { once: true });
      cleanups.push(() => sig.removeEventListener("abort", on));
    }
    const t = setTimeout(() => fin({ kind: "deadline" }), Math.max(0, x.deadline - now()));
    cleanups.push(() => clearTimeout(t));
    cleanups.push(x.inst.onPoison(() => fin({ kind: "poison" })));
  });
}

function* serve(x, pc) {
  const B = x.B;
  const req = pc.req;
  B.hooks?.onHostCall?.(req);
  switch (req.tag) {
    case "custom-command":
      return yield* serveCustom(x, pc.epoch, req.val);
    case "sleep": {
      if (!B.sleep) return { outcome: { tag: "denied" } };
      return yield* serveExtension(x, (scope) => scope.guard(B.sleep)(Number(req.val)).then(() => ({ tag: "completed" })), { tag: "failed", val: utf8("sleep failed") });
    }
    case "fs-batch":
      return yield* serveFsBatch(x, req.val, serveExtension);
    default:
      return { outcome: { tag: "denied" } };
  }
}

/** Runs an extension call in a fresh scope; handles abort, deadline, poison and rejection. */
function* serveExtension(x, start, onError) {
  const scope = new Scope();
  x.scopes.add(scope);
  let p;
  try {
    p = Promise.resolve(start(scope));
  } catch (e) {
    p = Promise.reject(e);
  }
  const r = yield { promise: raceExec(x, p) };
  if (r.kind === "poison") throw new KernelPoisonedError();
  if (r.kind === "abort" || r.kind === "deadline") {
    scope.revoke();
    yield { promise: settleWithin(p, x.B.limits.maxExtensionCleanupTimeMs) };
    return { cancel: r.kind === "abort" ? "aborted" : "deadline" };
  }
  scope.revoke();
  x.scopes.delete(scope);
  if (r.kind === "err") return { outcome: onError };
  return { outcome: r.value };
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

class FrozenMap extends Map {
  constructor(entries) {
    super(entries);
    this.frozen = true;
  }
  set(k, v) {
    if (this.frozen) throw new TypeError("ctx.env of an untrusted command is read-only");
    return super.set(k, v);
  }
  delete() {
    throw new TypeError("ctx.env of an untrusted command is read-only");
  }
  clear() {
    throw new TypeError("ctx.env of an untrusted command is read-only");
  }
}

function* serveCustom(x, epoch, call) {
  const B = x.B;
  const inst = x.inst;
  const name = call.name;
  const entry = B.commands.get(name);
  if (!entry) return { outcome: { tag: "denied" } };
  const trusted = entry.trusted !== false;
  const originalEnv = new Map();
  const envFlags = new Map();
  for (const e of call.env) {
    const key = decodeUtf8(e.name);
    originalEnv.set(key, decodeUtf8(e.value));
    envFlags.set(key, e.exported);
  }
  let assignmentLimit;
  const result = yield* serveExtension(
    x,
    async (scope) => {
      const live = (fn) => (...a) => {
        scope.check();
        return fn(...a);
      };
      const env = trusted ? new Map() : new FrozenMap();
      for (const [k, v] of originalEnv) Map.prototype.set.call(env, k, v);
      let stdinText;
      const ctx = {
        fs: scope.wrap(B.userFs ?? B.rootFs, FS_METHODS),
        cwd: decodeUtf8(call.cwd),
        env,
        exportedEnv: nullObject([...envFlags].filter(([, ex]) => ex).map(([k]) => [k, env.get(k)])),
        limits: { ...B.limits },
        signal: anySignal(scope.signal, x.signal),
        getRegisteredCommands: live(() => [...B.commands.keys()]),
        xpgEcho: !!call.xpgEcho,
      };
      Object.defineProperty(ctx, "stdin", {
        enumerable: true,
        get() {
          scope.check();
          if (stdinText === undefined) {
            const chunks = [];
            let got = 0;
            const want = Number(call.inputLen);
            while (got < want) {
              const c = inst.call(() => x.fiber.readCallInput(epoch, 0, CHUNK));
              if (c.length === 0) break;
              chunks.push(c);
              got += c.length;
            }
            stdinText = latin1FromBytes(concat(chunks));
          }
          return stdinText;
        },
      });
      ctx.fileDescriptors = new Map();
      for (const fd of call.fds) {
        const chunks = [];
        let got = 0;
        while (got < Number(fd.len)) {
          const c = inst.call(() => x.fiber.readCallInput(epoch, fd.fd, CHUNK));
          if (c.length === 0) break;
          chunks.push(c);
          got += c.length;
        }
        ctx.fileDescriptors.set(fd.fd, latin1FromBytes(concat(chunks)));
      }
      if (B.fetch) ctx.fetch = scope.guard(B.fetch);
      if (B.sleep) ctx.sleep = scope.guard(B.sleep);
      if (B.trace) ctx.trace = live(B.trace);
      if (B.coverage) ctx.coverage = { hit: live((feature) => B.coverage.hit(feature)) };
      if (trusted) ctx.assignShellVariable = async (n, v, subscript) => {
        scope.check();
        if (typeof n !== "string" || typeof v !== "string" || (subscript !== undefined && typeof subscript !== "string")) {
          throw new TypeError("assignment name, value and subscript must be strings");
        }
        const limit = stringLimit([n, v, subscript ?? ""], B.limits.maxStringLength, B.headroom);
        if (limit) {
          assignmentLimit ??= limit;
          throw new RangeError(`${limit} exceeded`);
        }
        try {
          inst.call(() => x.fiber.assignVariable(epoch, utf8(n), utf8(v), subscript === undefined ? undefined : utf8(subscript)));
        } catch (e) {
          if (e instanceof KernelPoisonedError) throw e;
          throw new Error(`cannot assign ${n}: ${e?.code ?? "refused"}`);
        }
        env.set(n, v);
        if (!envFlags.has(n)) envFlags.set(n, false);
        if (envFlags.get(n)) ctx.exportedEnv[n] = v;
      };
      ctx.exec = scope.guard((command, options) => B.runChild(command, options, scope, x));
      ctx.origCommand = scope.guard((args) => B.runChild([name, ...args].map(shellQuote).join(" "), { cwd: ctx.cwd, bypass: name }, scope, x));
      let r;
      try {
        const cmd = await B.resolveCommand(entry);
        scope.check();
        r = await cmd.execute(call.args.map((a) => decodeUtf8(a)), ctx);
      } catch (e) {
        scope.check();
        return { failed: sanitizeMessage(e?.message ?? e) };
      }
      scope.check();
      return { r, env: trusted ? env : null };
    },
    { failed: "command failed" },
  );
  if (result.cancel) return result;
  if (assignmentLimit) return { outcome: { tag: "limit-exceeded", val: assignmentLimit } };
  const out = result.outcome;
  if (out.failed !== undefined) {
    pushText(x, epoch, "stderr", `${out.failed}\n`);
    return { outcome: { tag: "command-exit", val: 1 } };
  }
  const r = out.r ?? {};
  if (trusted && out.env) {
    const entries = [...out.env].map(([k, v]) => [String(k), String(v)]);
    const changed = entries.filter(([k, v]) => originalEnv.get(k) !== v);
    const originals = originalEnv.keys();
    const reordered = entries.length !== originalEnv.size || entries.some(([k]) => originals.next().value !== k);
    if (changed.length || reordered) {
      const limit = stringLimit(changed.flat(), B.limits.maxStringLength, B.headroom) ??
        stringLimit(entries.flat(), Infinity, B.headroom, entries.length * 20);
      if (limit) return { outcome: { tag: "limit-exceeded", val: limit } };
      const list = entries.map(([k, v]) => ({ name: utf8(k), value: utf8(v), exported: envFlags.get(k) ?? false }));
      try {
        inst.call(() => x.fiber.setCallEnv(epoch, list));
      } catch (e) {
        if (e?.code !== "ceiling") throw e;
        return { outcome: { tag: "limit-exceeded", val: e.detail || "memory-ceiling" } };
      }
    }
  }
  const kindBytes = r.stdoutKind === "bytes";
  const stdout = String(r.stdout ?? "");
  if (kindBytes && stdout.length === 0) inst.call(() => x.fiber.pushCallData(epoch, "stdout-bytes", new Uint8Array(0)));
  else pushBytes(x, epoch, kindBytes ? "stdout-bytes" : "stdout", kindBytes ? unsafeBytesFromLatin1(stdout) : utf8(stdout));
  pushText(x, epoch, "stderr", String(r.stderr ?? ""));
  const code = typeof r.exitCode === "number" ? r.exitCode | 0 : 0;
  return { outcome: { tag: "command-exit", val: code } };
}

function pushText(x, epoch, channel, text) {
  if (text) pushBytes(x, epoch, channel, utf8(text));
}

function pushBytes(x, epoch, channel, bytes) {
  for (let pos = 0; pos < bytes.length; pos += CHUNK) {
    const chunk = bytes.subarray(pos, pos + CHUNK);
    x.inst.call(() => x.fiber.pushCallData(epoch, channel, chunk));
  }
}

/** Quotes an argument for the shell language (single quotes), leaving plain words untouched. */
export function shellQuote(s) {
  return /^[A-Za-z0-9_@%+=:,./-]+$/.test(s) ? s : `'${s.replaceAll("'", "'\\''")}'`;
}

