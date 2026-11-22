// The holder of one kernel instance. In Node the instance exists at construction; in the browser it is
// created when the module is compiled, and operations issued before that are staged in order (EVOL D15).
import { Instance } from "./instance.js";

export class Holder {
  /**
   * @param {{ syncModule(): WebAssembly.Module | null, asyncModule(): Promise<WebAssembly.Module> }} loader
   * @param {object | undefined} hooks internal options
   * @param {number} ceiling memory ceiling in bytes
   * @param {{ expectRestore?: boolean }} [options] when true, the first attach skips the eager
   *   `configure`: a restore-first staged function owns store creation (spec L2c 4.4-4.6)
   */
  constructor(loader, hooks, ceiling, options) {
    this.loader = loader;
    this.hooks = hooks;
    this.ceiling = ceiling;
    this.expectRestore = options?.expectRestore === true;
    this.instance = null;
    this.queue = [];
    this.pending = null;
    // `hooks.kernel.module` is a module chosen by a test (the `kernel-test` component).
    const mod = hooks?.moduleProvider ? null : (hooks?.kernel?.module ?? loader.syncModule());
    if (mod) this.attach(mod);
  }

  attach(mod) {
    const inst = new Instance(mod, this.hooks);
    // A restore-first queue installs the validated default (or its ordinary setup on refusal)
    // instead of the eager `configure`: the first staged function owns store creation.
    const restoreFirst = this.expectRestore || this.queue[0]?.restoreFirst === true;
    this.expectRestore = false;
    if (!restoreFirst) {
      inst.call(() => inst.engine.configure(BigInt(this.ceiling)));
    }
    this.instance = inst;
    const queue = this.queue;
    this.queue = [];
    if (!restoreFirst) {
      for (const fn of queue) fn(inst);
      return;
    }
    try {
      for (const fn of queue) fn(inst);
    } catch (error) {
      // A failed construction discards its fresh store as one unit.
      try {
        inst.util?.stop?.();
      } catch {
        // The store is already stopped or poisoned; the original error decides.
      }
      throw error;
    }
  }

  /** Raises the ceiling when a shell attached later needs more. */
  raiseCeiling(bytes) {
    if (bytes <= this.ceiling) return;
    this.ceiling = bytes;
    this.stage((inst) => inst.call(() => inst.engine.raiseCeiling(BigInt(bytes))));
  }

  /** Runs `fn(instance)` now when the instance exists, else at instantiation, in order. */
  stage(fn) {
    if (this.instance) fn(this.instance);
    else this.queue.push(fn);
  }

  /** Resolves with the instance, creating it (and replaying the staged operations) if needed. */
  ensure() {
    if (this.instance) return Promise.resolve(this.instance);
    this.pending ??= Promise.resolve().then(
      () => this.hooks?.moduleProvider ? this.hooks.moduleProvider() : this.loader.asyncModule(),
    ).then((mod) => {
      if (!this.instance) this.attach(mod);
      return this.instance;
    }, (error) => {
      this.pending = null;
      throw error;
    });
    return this.pending;
  }
}
