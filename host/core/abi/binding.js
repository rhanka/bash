// One synchronous engine store with canonical resource intrinsics and post-return handling.
import { KernelPoisonedError } from "../errors.js";
import { Codec, trap, unsigned32 } from "./codec.js";
import { camelCase, functions, namespace, resourceKinds } from "./layout.js";

export function instantiateEngine(getCoreModule, { onTrap } = {}, coreName = "kernel.core.wasm") {
  return linkEngine((imports) => new WebAssembly.Instance(getCoreModule(coreName), imports).exports, { onTrap });
}

/** Links core exports. The linker and registry are injectable for deterministic ABI fixtures. */
export function linkEngine(link, { onTrap, Registry = FinalizationRegistry } = {}, bindings = functions) {
  const table = new Map();
  const wrappers = new WeakMap();
  const resources = new Map();
  const engine = {};
  const intrinsics = Object.create(null);
  const registry = new Registry(finalize);
  let nextHandle = 1;
  let busy = false;
  let inGuest = false;
  let stopped = false;
  let core;

  function stop() {
    if (stopped) return;
    stopped = true;
    core = undefined;
    for (const entry of table.values()) registry.unregister(entry);
    table.clear();
    for (const { reps } of resources.values()) reps.clear();
  }

  function fail(cause) {
    const error = cause instanceof WebAssembly.RuntimeError ? cause : trap(cause?.message ?? "guest call failed");
    if (!stopped) {
      stop();
      try { onTrap?.(error); } catch { /* Preserve the original trap. */ }
    }
    return error;
  }

  function checkEntry() {
    if (stopped) throw new KernelPoisonedError();
    if (busy) throw new TypeError("the canonical ABI store is already in a call");
  }

  function guestNew(kind, rep) {
    if (!inGuest || stopped) throw trap("resource creation outside the live guest");
    const bits = unsigned32(rep);
    const { reps } = resources.get(kind);
    const id = nextHandle;
    // The generated Rust Resource uses u32::MAX as its taken-handle sentinel.
    if (id >= 0xffffffff || table.has(id) || reps.has(bits)) throw trap("resource handle table collision or exhaustion");
    nextHandle++;
    reps.add(bits);
    table.set(id, { id, kind, rep: bits, hostOwned: false, borrows: 0 });
    return id;
  }

  function destroy(entry) {
    const { dtor, reps } = resources.get(entry.kind);
    table.delete(entry.id);
    reps.delete(entry.rep);
    registry.unregister(entry);
    core[dtor](entry.rep);
  }

  function guestDrop(kind, handle) {
    if (!inGuest || stopped) throw trap("resource drop outside the live guest");
    const entry = table.get(unsigned32(handle));
    if (!entry || entry.kind !== kind || entry.hostOwned || entry.borrows) {
      throw trap("invalid guest-owned " + kind + " drop");
    }
    destroy(entry);
  }

  function liftOwner(kind, handle) {
    const entry = table.get(handle);
    if (!entry || entry.kind !== kind || entry.hostOwned || entry.borrows) {
      throw trap("invalid owned " + kind + " handle");
    }
    const wrapper = Object.create(resources.get(kind).prototype);
    entry.hostOwned = true;
    wrappers.set(wrapper, entry);
    registry.register(wrapper, entry, entry);
    return wrapper;
  }

  function destroyOwner(entry) {
    busy = true;
    inGuest = true;
    try {
      destroy(entry);
    } catch (error) {
      throw fail(error);
    } finally {
      inGuest = false;
      busy = false;
    }
  }

  function finalize(entry) {
    if (stopped || table.get(entry.id) !== entry || !entry.hostOwned) return;
    if (busy) {
      queueMicrotask(() => finalize(entry));
      return;
    }
    try { destroyOwner(entry); } catch { /* fail already notified the host. */ }
  }

  for (const kind of resourceKinds) {
    const name = kind[0].toUpperCase() + kind.slice(1);
    const reps = new Set();
    const dtor = namespace + "#[dtor]" + kind;
    const Resource = class {
      constructor() {
        throw new TypeError(name + " has no public constructor");
      }

      [Symbol.dispose]() {
        const entry = wrappers.get(this);
        if (!entry || entry.kind !== kind) throw new TypeError("invalid " + kind + " disposal receiver");
        if (stopped || table.get(entry.id) !== entry) return;
        checkEntry();
        destroyOwner(entry);
      }
    };
    Object.defineProperty(Resource, "name", { value: name });
    engine[name] = Resource;
    resources.set(kind, { Resource, prototype: Resource.prototype, reps, dtor });
    intrinsics["[resource-new]" + kind] = (rep) => guestNew(kind, rep);
    intrinsics["[resource-drop]" + kind] = (handle) => guestDrop(kind, handle);
  }

  for (const fn of bindings) {
    const receiver = /^\[(method|static)\]([^.]+)\.(.+)$/.exec(fn.name);
    const resource = receiver && resources.get(receiver[2]);
    const method = receiver?.[1] === "method";
    const target = resource ? (method ? resource.prototype : resource.Resource) : engine;
    target[camelCase(receiver ? receiver[3] : fn.name)] = function (...args) {
      checkEntry();
      busy = true;

      // Entries track the dynamic borrows. The wrappers stay strongly reachable through post-return.
      const borrowed = [];
      const borrow = (kind, wrapper, path) => {
        const entry = wrappers.get(wrapper);
        if (!entry || entry.kind !== kind || !entry.hostOwned || table.get(entry.id) !== entry) {
          throw new TypeError(path + " must be a live " + kind + " of this store");
        }
        borrowed.push(wrapper);
        entry.borrows++;
        return entry.rep;
      };

      try {
        const oneshotBytes = fn.name === "exec-oneshot" ? fn.params.size : undefined;
        const codec = new Codec({ core, borrow, own: liftOwner, oneshotBytes });
        const receiverArg = method ? { value: this } : undefined;
        let staged;
        try {
          staged = codec.stageParams(fn.params, args, fn.name, receiverArg);
        } catch (error) {
          // An exception from a caller getter is a host failure, even if it resembles a Wasm trap.
          if (error instanceof WebAssembly.RuntimeError) throw new TypeError("ABI input getter failed", { cause: error });
          throw error;
        }
        if (oneshotBytes !== undefined) codec.liftBudget = codec.lowerBudget;
        const streamLimit = fn.streamStride && Math.min(1 << 20, staged.at(-1) * fn.streamStride);
        let result;
        inGuest = true;
        try {
          const slots = [];
          if (fn.indirectParams) {
            const ptr = codec.allocate(fn.params.size, fn.params.align);
            codec.write(fn.params, staged, ptr);
            // Ownership of the whole parameter tuple passes to the guest on entry.
            slots.push(ptr);
          } else {
            codec.flatten(fn.params, staged, slots);
          }
          const raw = core[fn.exportName](...slots);
          result = fn.indirectResult ? codec.read(fn.result, unsigned32(raw), streamLimit) : codec.liftFlat(fn.result, [raw]);
          if (fn.postName) core[fn.postName](raw);
        } catch (error) {
          throw fail(error);
        }
        if (!fn.unwrap) return result;
        if (result.tag === "err") throw result.val;
        return result.val;
      } finally {
        for (const wrapper of borrowed) wrappers.get(wrapper).borrows--;
        inGuest = false;
        busy = false;
      }
    };
  }

  core = link({ ["[export]" + namespace]: intrinsics });
  return {
    engine,
    _util: {
      stop() {
        if (stopped) return;
        checkEntry();
        stop();
      },
    },
  };
}

