// One kernel instance: the instantiated component, its trap state and the hooks used by tests.
import { instantiate as productionInstantiate } from "../../dist/js/kernel/kernel.js";
import { KernelPoisonedError } from "./errors.js";

/**
 * The internal options (not part of the public declarations). Tests use them to load the `kernel-test`
 * component, to observe kernel calls and to choose a ceiling.
 */
export const INTERNAL = Symbol.for("@sentropic/bash.internal");

/** Resolves the main core and any shim modules requested by an injected adapter. */
function coreModules(main, shims) {
  const cache = new Map();
  return (path) => {
    if (path.endsWith(".core.wasm")) return main;
    let m = cache.get(path);
    if (!m) {
      const bytes = shims?.[path];
      if (!bytes) throw new Error(`unknown core module ${path}`);
      m = new WebAssembly.Module(bytes);
      cache.set(path, m);
    }
    return m;
  };
}

export class Instance {
  /** @param {WebAssembly.Module} mainModule @param {object} [hooks] */
  constructor(mainModule, hooks) {
    const impl = hooks?.kernel;
    const instantiate = impl?.instantiate ?? productionInstantiate;
    this.hooks = hooks;
    this.poisoned = false;
    this.waiters = new Set();
    const { engine, _util } = instantiate(coreModules(mainModule, impl?.shims), {}, undefined, {
      onTrap: () => this.poison(),
    });
    this.engine = engine;
    this.util = _util;
    hooks?.onEngine?.(this.engine);
  }

  /** Runs a kernel call; a trap poisons the instance (EVOL D8). */
  call(fn) {
    if (this.poisoned) throw new KernelPoisonedError();
    try {
      return fn();
    } catch (e) {
      if (e instanceof WebAssembly.RuntimeError) {
        this.poison();
        throw new KernelPoisonedError();
      }
      throw e;
    }
  }

  poison() {
    if (this.poisoned) return;
    this.poisoned = true;
    try {
      this.util?.stop();
    } catch {
      // The instance is already poisoned even if an injected adapter fails to stop.
    }
    const waiters = [...this.waiters];
    this.waiters.clear();
    for (const wake of waiters) {
      try { wake(); } catch { /* Continue notifying the pending fibers. */ }
    }
  }

  /** Throws when the instance is poisoned. */
  check() {
    if (this.poisoned) throw new KernelPoisonedError();
  }

  /** Registers a callback run when the instance is poisoned; returns the function that unregisters it. */
  onPoison(fn) {
    this.waiters.add(fn);
    return () => this.waiters.delete(fn);
  }
}
