// The driver loop (spec 7.3): runs one fiber to its result, serving host calls through the membrane.
// The loop is a generator so that the asynchronous form (`exec`) and the synchronous form (`execSync`)
// share one body: the generator yields a promise to await, or a tick to give the event loop a turn.
import { concat, decodeUtf8, latin1FromBytes, unsafeBytesFromLatin1, utf8, utf8Length } from "./bytes.js";
import { AsyncCapabilityError, KernelPoisonedError } from "./errors.js";
import { FS_METHODS, Scope, settleWithin } from "./membrane.js";

const CHUNK = 1 << 20;
const FUEL = 100_000n;
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

function limitResult(B, name) {
  return { stdout: "", stderr: `bash: ${name} exceeded\n`, exitCode: 126, env: nullObject(Object.entries(B.initialEnv)) };
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

function anySignal(a, b) {
  if (!a) return b;
  if (!b) return a;
  if (typeof AbortSignal.any === "function") return AbortSignal.any([a, b]);
  const c = new AbortController();
  for (const s of [a, b]) {
    if (s.aborted) c.abort();
    else s.addEventListener("abort", () => c.abort(), { once: true });
  }
  return c.signal;
}

/**
 * The exec generator.
 * @param {object} B internal state of the Bash
 * @param {string} script
 * @param {object} o normalized options: { env: [name, value][], replaceEnv, cwd, rawScript, stdin: Uint8Array, args: string[], signal, bypass, parent }
 */
export function* execLoop(B, script, o) {
  const inst = B.holder.instance;
  inst.check();
  const L = B.limits;
  // Pre-checks (7.3 step 1): nothing is lowered, no kernel call is made.
  const half = Math.floor(B.headroom / 2);
  let total = 0;
  const strings = [...o.env.map((e) => e[1]), ...o.args, ...(o.cwd !== undefined ? [o.cwd] : [])];
  for (const s of strings) {
    const n = utf8Length(s);
    if (n > L.maxStringLength) return limitResult(B, "maxStringLength");
    total += n;
  }
  if (total > half) return limitResult(B, "headroom");

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
    started: false,
  };
  const engine = inst.engine;
  try {
    x.fiber = inst.call(() =>
      engine.Fiber.spawn(B.shell, o.parent?.fiber, {
        script: scriptBytes,
        scriptOverflow: overflow,
        args: o.args.map(utf8),
        env: o.env.map(([n, v]) => ({ name: utf8(n), value: utf8(v), exported: true })),
        replaceEnv: !!o.replaceEnv,
        cwd: o.cwd === undefined ? undefined : utf8(o.cwd),
        rawScript: !!o.rawScript,
        bypassCustom: o.bypass,
      }),
    );
    x.started = true;
    if (o.signal?.aborted) return finish(x, cancel(x, "aborted"));
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
      if (step.tag === "out-of-fuel") {
        drain(x);
        if (o.signal?.aborted) return finish(x, cancel(x, "aborted"));
        if (now() > x.deadline) return finish(x, cancel(x, "deadline"));
        if (now() - lastYield >= YIELD_MS) {
          yield { tick: true };
          lastYield = now();
        }
        step = inst.call(() => x.fiber.run(FUEL));
        continue;
      }
      // host-call
      drain(x);
      const served = yield* serve(x, step.val);
      if (served.cancel) return finish(x, cancel(x, served.cancel));
      step = inst.call(() => x.fiber.resume(step.val.epoch, served.outcome));
    }
  } catch (e) {
    if (e instanceof KernelPoisonedError && x.started) return fatalResult(B);
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
  const r = {
    stdout: decodeUtf8(concat(x.out)),
    stderr: decodeUtf8(concat(x.err)),
    exitCode: er.exitCode,
    env: envToObject(er.env),
  };
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
    promise.then(
      (value) => fin({ kind: "ok", value }),
      (error) => fin({ kind: "err", error }),
    );
    const sig = x.signal;
    if (sig) {
      if (sig.aborted) fin({ kind: "abort" });
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
      return yield* serveFsBatch(x, req.val);
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

function* serveFsBatch(x, batch) {
  const B = x.B;
  const fs = B.userFs;
  if (!fs) return { outcome: { tag: "denied" } };
  return yield* serveExtension(
    x,
    async (scope) => {
      const g = scope.wrap(fs, FS_METHODS);
      const results = [];
      for (const op of batch.ops) {
        if (op.tag !== "stat") {
          results.push({ tag: "err", val: { code: "unsupported", path: new Uint8Array(0) } });
          continue;
        }
        const path = decodeUtf8(op.val);
        try {
          const s = await g.stat(path);
          results.push({ tag: "ok", val: { tag: "info", val: { kind: s.isDirectory ? "directory" : s.isSymbolicLink ? "symlink" : "file", size: BigInt(s.size), mode: s.mode, mtimeMs: BigInt(s.mtime.getTime()), dev: 0n, ino: 0n } } });
        } catch {
          results.push({ tag: "err", val: { code: "not-found", path: op.val } });
        }
      }
      return { tag: "fs", val: results };
    },
    { tag: "failed", val: utf8("fs-batch failed") },
  );
}

const HOST_PATH_HINTS = [];

/** Registers host path prefixes that must never appear in the stderr of a failed command (node loader). */
export function addHostPathHints(...paths) {
  for (const p of paths) if (p && p !== "/" && !HOST_PATH_HINTS.includes(p)) HOST_PATH_HINTS.push(p);
}

const GENERIC_HOST_PATH = /(?:file:\/\/[^\s'"]*|[A-Za-z]:\\[^\s'"]*|\/(?:Users|root|var|tmp|private|opt|mnt|usr|srv)\/[^\s'":]*|\/home\/(?!user(?:\/|\b))[^/\s'"]+\/[^\s'":]*)/g;

/** Replaces host paths in an error message by `<path>`. */
export function sanitizeMessage(text) {
  let out = String(text);
  for (const h of HOST_PATH_HINTS) out = out.split(h).join("<path>");
  return out.replace(GENERIC_HOST_PATH, "<path>");
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
  const envFlags = new Map(call.env.map((e) => [decodeUtf8(e.name), e.exported]));
  const result = yield* serveExtension(
    x,
    async (scope) => {
      const live = (fn) => (...a) => {
        scope.check();
        return fn(...a);
      };
      const env = trusted ? new Map() : new FrozenMap();
      for (const e of call.env) {
        const k = decodeUtf8(e.name);
        if (trusted) env.set(k, decodeUtf8(e.value));
        else Map.prototype.set.call(env, k, decodeUtf8(e.value));
      }
      let stdinText;
      const ctx = {
        fs: scope.wrap(B.userFs ?? B.rootFs, FS_METHODS),
        cwd: decodeUtf8(call.cwd),
        env,
        exportedEnv: nullObject([...envFlags].filter(([, ex]) => ex).map(([k]) => [k, env.get(k)])),
        limits: { ...B.limits },
        signal: anySignal(scope.signal, undefined),
        getRegisteredCommands: () => [...B.commands.keys()],
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
      if (B.coverage) ctx.coverage = B.coverage;
      ctx.assignShellVariable = async (n, v, subscript) => {
        scope.check();
        try {
          inst.call(() => x.fiber.assignVariable(epoch, utf8(n), utf8(v), subscript === undefined ? undefined : utf8(subscript)));
        } catch (e) {
          if (e instanceof KernelPoisonedError) throw e;
          throw new Error(`cannot assign ${n}: ${e?.code ?? "refused"}`);
        }
      };
      ctx.exec = scope.guard((command, options) => B.runChild(command, options, scope, x));
      ctx.origCommand = scope.guard((args) => B.runChild([name, ...args].map(shellQuote).join(" "), { cwd: ctx.cwd, bypass: name }, scope, x));
      const cmd = await B.resolveCommand(entry);
      let r;
      try {
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
  const out = result.outcome;
  if (out.failed !== undefined) {
    pushText(x, epoch, "stderr", `${out.failed}\n`);
    return { outcome: { tag: "command-exit", val: 1 } };
  }
  const r = out.r ?? {};
  const kindBytes = r.stdoutKind === "bytes";
  pushBytes(x, epoch, "stdout", kindBytes ? unsafeBytesFromLatin1(String(r.stdout ?? "")) : utf8(String(r.stdout ?? "")));
  pushText(x, epoch, "stderr", String(r.stderr ?? ""));
  if (trusted && out.env) {
    const list = [];
    for (const [k, v] of out.env) list.push({ name: utf8(k), value: utf8(v), exported: envFlags.get(k) ?? false });
    inst.call(() => x.fiber.setCallEnv(epoch, list));
  }
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

