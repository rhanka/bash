import { test } from "node:test";
import assert from "node:assert/strict";
import { Instance } from "../../host/core/instance.js";
import { adapterPair, coreName, importsName, watModule } from "./support.mjs";

for (const name of ["kernel", "kernel-test"]) {
  test(name + " implements the declared instantiate arguments and Root shape", async () => {
    const { module } = await adapterPair(name);
    const dir = name === "kernel" ? "../../dist/js/kernel/" : "../../dist/test/js/kernel-test/";
    const { instantiate } = await import(dir + name + ".js");
    const { exports: { fail } } = new WebAssembly.Instance(watModule('(module (func (export "fail") unreachable))'));
    const notifications = [];
    const paths = [];
    const store = instantiate((path) => {
      paths.push(path);
      return module;
    }, {}, (compiled, imports) => {
      assert.equal(compiled, module);
      assert.deepEqual(Object.keys(imports), [importsName]);
      assert.equal(Object.keys(imports[importsName]).length, 6);
      const { exports } = new WebAssembly.Instance(compiled, imports);
      return { exports: { ...exports, [coreName("usage")]: fail } };
    }, { onTrap: (error) => notifications.push(error) });
    assert.deepEqual(paths, [name + ".core.wasm"]);
    assert.equal(store["sentropic:bash/engine@0.1.0-draft"], store.engine);
    assert.equal(store["sentropic:bash/types@0.1.0-draft"], store.types);
    assert.deepEqual(store.types, {});
    assert.ok(store._util.whenIdle() instanceof Promise);
    await store._util.whenIdle();
    assert.throws(() => store.engine.usage(), WebAssembly.RuntimeError);
    assert.throws(() => store.engine.usage(), { name: "KernelPoisonedError" });
    assert.equal(notifications.length, 1);
    store._util.stop();
    await store._util.whenIdle();

    let instantiations = 0;
    class WasmInstance extends WebAssembly.Instance {
      constructor(...args) { super(...args); instantiations++; }
    }
    const shimmed = instantiate(() => module, {}, undefined, {
      shim: { globals: { WebAssembly: { Instance: WasmInstance } } },
    });
    assert.ok(shimmed.engine.usage().ceiling > 0n);
    assert.equal(instantiations, 1);
    shimmed._util.stop();
  });

  test(name + " jco reports a finalizer trap through Instance without an active call", async () => {
    const { jcoHooks } = await adapterPair(name);
    const { exports: { fail } } = new WebAssembly.Instance(watModule('(module (func (export "fail") unreachable))'));
    const { FinalizationRegistry: Registry } = globalThis;
    const finalizers = new WeakMap();
    globalThis.FinalizationRegistry = class extends Registry {
      constructor(cleanup) { super(cleanup); this.cleanup = cleanup; }
      register(owner, token) { finalizers.set(owner, () => this.cleanup(token)); }
    };
    let store;
    let vfs;
    let destructions = 0;
    try {
      store = new Instance(jcoHooks.module, { kernel: {
        ...jcoHooks,
        instantiate(getCoreModule, imports, _instantiateCore, options) {
          return jcoHooks.instantiate(getCoreModule, imports, (compiled, coreImports) => {
            const instance = new WebAssembly.Instance(compiled, coreImports);
            if (compiled !== jcoHooks.module) return instance;
            const dtor = coreName("[dtor]vfs");
            return { exports: { ...instance.exports, [dtor]() { destructions++; fail(); } } };
          }, options);
        },
      } });
      store.call(() => store.engine.configure(64n << 20n));
      vfs = store.call(() => store.engine.Vfs.create(1n << 20n));
    } finally {
      globalThis.FinalizationRegistry = Registry;
    }
    let wakeups = 0;
    store.onPoison(() => wakeups++);
    assert.doesNotThrow(finalizers.get(vfs));
    assert.equal(destructions, 1);
    assert.equal(wakeups, 1);
    assert.throws(() => store.call(() => assert.fail("entered poisoned store")), { name: "KernelPoisonedError" });
  });
}
