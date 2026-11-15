// The holder of one kernel instance. In Node the instance exists at construction; in the browser it is
// created when the module is compiled, and operations issued before that are staged in order (EVOL D15).
import { Instance } from "./instance.js";

export class Holder {
  /**
   * @param {{ syncModule(): WebAssembly.Module | null, asyncModule(): Promise<WebAssembly.Module> }} loader
   * @param {object | undefined} hooks internal options
   * @param {number} ceiling memory ceiling in bytes
   */
  constructor(loader, hooks, ceiling) {
    this.loader = loader;
    this.hooks = hooks;
    this.ceiling = ceiling;
    this.instance = null;
    this.queue = [];
    this.pending = null;
    // `hooks.kernel.module` is a module chosen by a test (the `kernel-test` component).
    const mod = hooks?.moduleProvider ? null : (hooks?.kernel?.module ?? loader.syncModule());
    if (mod) this.attach(mod);
  }

  attach(mod) {
    const inst = new Instance(mod, this.hooks);
    inst.call(() => inst.engine.configure(BigInt(this.ceiling)));
    this.instance = inst;
    const queue = this.queue;
    this.queue = [];
    for (const fn of queue) fn(inst);
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
    this.pending ??= (async () => {
      const mod = await (this.hooks?.moduleProvider ? this.hooks.moduleProvider() : this.loader.asyncModule());
      if (!this.instance) this.attach(mod);
      return this.instance;
    })();
    return this.pending;
  }
}
