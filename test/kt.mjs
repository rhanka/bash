// Test support: loads the `kernel-test` component (with the test-only commands) through the internal
// options of the facade, and exposes recording helpers.
import { readFileSync } from "node:fs";
import { instantiate } from "../dist/test/js/kernel-test/kernel-test.js";
import { Bash, InMemoryFs } from "@sentropic/bash";

export const INTERNAL = Symbol.for("@sentropic/bash.internal");
export const module = new WebAssembly.Module(readFileSync(new URL("../dist/test/js/kernel-test/kernel-test.core.wasm", import.meta.url)));

export { Bash, InMemoryFs };

/** Internal options selecting the test kernel. `extra` adds hooks (`onEngine`, `onHostCall`, `ceilingBytes`...). */
export function internal(extra = {}) {
  return { kernel: { instantiate, module }, ...extra };
}

/** Bash options on the test kernel. */
export function T(options = {}, extra) {
  return { ...options, [INTERNAL]: internal(extra) };
}

/** A new Bash on the test kernel. */
export function bash(options, extra) {
  return new Bash(T(options, extra));
}

/** Counts every kernel call (methods of the generated classes and static functions of the engine). */
export function countKernelCalls() {
  const state = { calls: 0, names: [], maxList: 0, lists: [] };
  const hooks = {
    onEngine(engine) {
      const wrap = (obj, name, label) => {
        const f = obj[name];
        if (typeof f !== "function") return;
        obj[name] = function (...args) {
          state.calls++;
          state.names.push(label);
          for (const a of args) if (a instanceof Uint8Array) state.maxList = Math.max(state.maxList, a.length);
          return f.apply(this, args);
        };
      };
      for (const cls of ["Fiber", "Shell", "Vfs"]) {
        for (const n of Object.getOwnPropertyNames(engine[cls].prototype)) if (n !== "constructor") wrap(engine[cls].prototype, n, `${cls}.${n}`);
        for (const n of ["spawn", "create"]) wrap(engine[cls], n, `${cls}.${n}`);
      }
    },
  };
  return { state, hooks };
}

export const enc = (s) => new TextEncoder().encode(s);
export const dec = (b) => new TextDecoder().decode(b);
