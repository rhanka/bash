// Custom-command serving (spec 7.3): runs one extension call in a revocable scope.
// Loads only when a custom command actually runs (never for `new Bash()` plus
// the first `echo` with no options). A static relative `import()` keeps the
// specifier G2-clean; `execSync` with custom commands already rejects with
// `AsyncCapabilityError`, so the yielded import preserves the sync contract.
import { concat, decodeUtf8, latin1FromBytes, unsafeBytesFromLatin1, utf8 } from "./bytes.js";
import { CHUNK, anySignal, cancelReason, now, nullObject, sanitizeMessage, shellQuote } from "./driver.js";
import { KernelPoisonedError } from "./errors.js";
import { stringLimit } from "./limits.js";
import { FS_METHODS, Scope, settleWithin } from "./membrane.js";

function pushText(x, epoch, channel, text) {
  if (text) pushBytes(x, epoch, channel, utf8(text));
}

function pushBytes(x, epoch, channel, bytes) {
  for (let pos = 0; pos < bytes.length; pos += CHUNK) {
    const chunk = bytes.subarray(pos, pos + CHUNK);
    x.inst.call(() => x.fiber.pushCallData(epoch, channel, chunk));
  }
}

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

/** Runs an extension call in a fresh scope; handles abort, deadline, poison and rejection. */
export function* serveExtension(x, start, onError) {
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

function readFd(x, epoch, fd, want) {
  const chunks = [];
  let got = 0;
  while (got < want) {
    const c = x.inst.call(() => x.fiber.readCallInput(epoch, fd, CHUNK));
    if (c.length === 0) break;
    chunks.push(c);
    got += c.length;
  }
  return latin1FromBytes(concat(chunks));
}

export function* serveCustom(x, epoch, call) {
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
        fs: scope.wrap(B.userFs ?? B.volumeFs ?? B.rootFs, FS_METHODS),
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
          return (stdinText ??= readFd(x, epoch, 0, Number(call.inputLen)));
        },
      });
      ctx.fileDescriptors = new Map();
      for (const fd of call.fds) ctx.fileDescriptors.set(fd.fd, readFd(x, epoch, fd.fd, Number(fd.len)));
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
