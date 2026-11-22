import { COMMAND_NAMES } from "../../dist/js/kernel/commands.js";
// The facade: `Bash`, `defineCommand` and the name helpers (spec 7.1-7.3), bound to a loader.
import { decodeUtf8, unsafeBytesFromLatin1, utf8 } from "./bytes.js";
import { anySignal, envToObject, execLoop, markKernelVolume, nullObject, runAsync, runSync } from "./driver.js";
import { AsyncCapabilityError, NotSupportedError } from "./errors.js";
import { addFiles, createInMemoryFs, fsError, fsState } from "./fs.js";
import { Holder } from "./holder.js";
import { DEFAULT_PROFILE, PROFILE_NAMES, PROFILES, resolveLimits, stringLimit } from "./limits.js";
import { dirname, resolvePath } from "./paths.js";
import { INTERNAL } from "./instance.js";
import { PREDICATE_OPTIONS, isDefaultSnapshotConfig } from "./snapshot.js";

const OPTION_NAMES = new Set([
  "files", "env", "cwd", "fs", "isolation", "executionLimits", "executionLimitProfile", "maxCallDepth", "maxCommandCount", "maxLoopIterations",
  "fetch", "network", "python", "javascript", "commands", "sleep", "customCommands", "logger", "trace", "coverage", "defenseInDepth", "processInfo",
]);

function fail(message) {
  return new TypeError(message);
}

function isObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}

function stringRecord(v, what) {
  if (!isObject(v)) throw fail(`${what} must be an object of strings`);
  const out = [];
  for (const [k, val] of Object.entries(v)) {
    if (typeof val !== "string") throw fail(`${what}.${k} must be a string`);
    out.push([k, val]);
  }
  return out;
}

/** Initial environment in the order of spec 7.2. */
function initialEnvEntries(cwd, home, userEnv) {
  return [...new Map([
    ["HOME", home],
    ["PATH", "/usr/bin:/bin"],
    ["IFS", " \t\n"],
    ["OSTYPE", "linux-gnu"],
    ["MACHTYPE", "x86_64-pc-linux-gnu"],
    ["HOSTTYPE", "x86_64"],
    ["HOSTNAME", "localhost"],
    ["PWD", cwd],
    ["OLDPWD", cwd],
    ["OPTIND", "1"],
    ...userEnv.filter(([key]) => key !== "SHELLOPTS" && key !== "BASHOPTS"),
    ["SHELLOPTS", "braceexpand:hashall:interactive-comments"],
    ["BASHOPTS", "globskipdots"],
  ])];
}

/** Validates a command (Command or LazyCommand). */
function checkCommand(c) {
  if (!isObject(c)) throw fail("a custom command must be an object");
  if (typeof c.name !== "string" || c.name === "" || /[\s/]/.test(c.name)) throw fail("a custom command needs a name without spaces or slashes");
  if (typeof c.execute !== "function" && typeof c.load !== "function") throw fail(`custom command ${c.name} needs an execute or a load function`);
  if (c.trusted !== undefined && typeof c.trusted !== "boolean") throw fail(`custom command ${c.name}: trusted must be a boolean`);
}

/** Creates `defineCommand` result objects. */
export function defineCommand(name, execute, options) {
  const c = { name, execute, trusted: options?.trusted };
  checkCommand(c);
  return c;
}

export const getCommandNames = () => [...COMMAND_NAMES];
export const getNetworkCommandNames = () => ["curl"];
export const getPythonCommandNames = () => ["python3", "python"];
export const getJavaScriptCommandNames = () => ["js-exec", "node"];

function seedHostLayout(fs, defaultLayout, cwd, processInfo, commands) {
  if (typeof fs?.mkdirSync !== "function" || typeof fs.writeFileSync !== "function") return;
  const mkdir = (path) => ignoreUnimplemented(() => fs.mkdirSync(path, { recursive: true }));
  const write = (path, contents) => ignoreUnimplemented(() => fs.writeFileSync(path, contents));
  mkdir("/bin");
  mkdir("/usr/bin");
  if (defaultLayout) {
    mkdir("/home/user");
    mkdir("/tmp");
  }
  mkdir("/dev");
  mkdir("/dev/fd");
  for (const name of ["null", "zero", "stdin", "stdout", "stderr"]) write(`/dev/${name}`, "");
  mkdir("/proc/self/fd");
  const { pid = 1, ppid = 0, uid = 1000, gid = 1000 } = processInfo ?? {};
  write("/proc/version", "Linux version 5.15.0-generic (just-bash) #1 SMP PREEMPT\n");
  write("/proc/self/exe", "/bin/bash");
  write("/proc/self/cmdline", "bash\0");
  write("/proc/self/comm", "bash\n");
  write("/proc/self/status", `Name:\tbash\nState:\tR (running)\nPid:\t${pid}\nPPid:\t${ppid}\nUid:\t${uid}\t${uid}\t${uid}\t${uid}\nGid:\t${gid}\t${gid}\t${gid}\t${gid}\n`);
  for (const [fd, name] of ["stdin", "stdout", "stderr"].entries()) write(`/proc/self/fd/${fd}`, `/dev/${name}`);
  if (cwd !== "/" && (!defaultLayout || cwd !== "/home/user")) mkdir(cwd);
  for (const name of commands) seedHostCommand(fs, name);
}

function ignoreUnimplemented(operation) {
  try {
    operation();
  } catch (error) {
    if (!String(error?.message).startsWith("ENOSYS")) throw error;
  }
}

/**
 * Seeds `options.files` into a host-backed `fs`, which the kernel mount at `/`
 * would otherwise shadow. Entries are validated now (throwing in the
 * constructor like `addFiles`); adapters with synchronous methods seed
 * immediately, others seed in the background while `exec`/`readFile`/`writeFile`
 * wait for the returned promise.
 */
function seedUserFiles(fs, files, base) {
  if (files === null || typeof files !== "object" || Array.isArray(files)) throw new TypeError("initial files must be an object");
  const entries = [];
  for (const [path, value] of Object.entries(files)) {
    if (typeof value === "function") throw new TypeError("files: lazy content is not supported on a host fs");
    let content = value;
    if (value !== null && typeof value === "object" && !(value instanceof Uint8Array)) content = value.content;
    if (typeof content !== "string" && !(content instanceof Uint8Array)) throw new TypeError(`files: the content of ${path} must be a string or bytes`);
    entries.push([resolvePath(base, path), content]);
  }
  if (typeof fs?.mkdirSync === "function" && typeof fs?.writeFileSync === "function") {
    for (const [p, content] of entries) {
      const dir = dirname(p);
      if (dir !== "/") fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(p, content);
    }
    return null;
  }
  const pending = (async () => {
    for (const [p, content] of entries) {
      const dir = dirname(p);
      if (dir !== "/") await fs.mkdir(dir, { recursive: true });
      await fs.writeFile(p, content);
    }
  })();
  pending.catch(() => {});
  return pending;
}

function seedHostCommand(fs, name) {
  if (typeof fs?.mkdirSync !== "function" || typeof fs.writeFileSync !== "function" || name.startsWith("__")) return;
  // Mirrors the kernel `Shell::stub` guard: `.` and `..` are not commands, and
  // writing `/bin/.` would replace the `/bin` memory entry instead of seeding.
  if (name === "" || name.includes("/") || name === "." || name === "..") return;
  const content = `#!/bin/bash\n# Built-in command: ${name}\n`;
  for (const dir of ["/bin", "/usr/bin"]) ignoreUnimplemented(() => fs.writeFileSync(`${dir}/${name}`, content));
}

/** Names the volume layout turns into stubs (the kernel `Shell::stub` guard). */
function volumeStubNames(names) {
  return names.filter((name) => name !== "" && !name.includes("/") && name !== "." && name !== "..");
}

/**
 * Issues the L2 section 5.2 layout against a branded volume through the owning
 * Vfs's internal `vfs.op`, bypassing the public FS methods (overrides included).
 * Same bytes and order as `Shell::create`: system directories, default home/tmp,
 * device and proc entries, the working directory, then command stubs in both
 * `/bin` and `/usr/bin`. Runs synchronously on a ready store, or staged on the
 * volume's single ordered queue before compilation.
 */
function seedVolumeLayout(vfs, inst, { defaultLayout, cwd, processInfo, commandNames }) {
  const mkdir = (path) => {
    const r = inst.call(() => vfs.op({ tag: "mkdir", val: { path: utf8(path), recursive: true, mode: 0o755 } }));
    if (r.tag === "err") throw fsError(r.val.code, "mkdir", path);
  };
  const write = (path, contents) => {
    const r = inst.call(() => vfs.op({
      tag: "write-file",
      val: { path: utf8(path), data: utf8(contents), offset: 0n, truncate: true, mode: 0o644, mtimeMs: 0n },
    }));
    if (r.tag === "err") throw fsError(r.val.code, "open", path);
  };
  // Atomicity: stat every system directory before creating any, mirroring
  // `Shell::layout` — a file collision fails before earlier directories exist.
  for (const dir of ["/bin", "/usr", "/usr/bin", "/dev", "/dev/fd", "/proc", "/proc/self", "/proc/self/fd"]) {
    const s = inst.call(() => vfs.op({ tag: "stat", val: utf8(dir) }));
    if (s.tag === "ok" && s.val.val.kind === "file") throw fsError("exists", "mkdir", dir);
  }
  for (const dir of ["/bin", "/usr", "/usr/bin", "/dev", "/dev/fd", "/proc", "/proc/self", "/proc/self/fd"]) mkdir(dir);
  if (defaultLayout) {
    mkdir("/home/user");
    mkdir("/tmp");
  }
  mkdir("/dev/fd");
  for (const name of ["null", "zero", "stdin", "stdout", "stderr"]) write(`/dev/${name}`, "");
  mkdir("/proc/self/fd");
  const { pid = 1, ppid = 0, uid = 1000, gid = 1000 } = processInfo ?? {};
  write("/proc/version", "Linux version 5.15.0-generic (just-bash) #1 SMP PREEMPT\n");
  write("/proc/self/exe", "/bin/bash");
  write("/proc/self/cmdline", "bash\0");
  write("/proc/self/comm", "bash\n");
  write("/proc/self/status", `Name:\tbash\nState:\tR (running)\nPid:\t${pid}\nPPid:\t${ppid}\nUid:\t${uid}\t${uid}\t${uid}\t${uid}\nGid:\t${gid}\t${gid}\t${gid}\t${gid}\n`);
  for (const [fd, name] of ["stdin", "stdout", "stderr"].entries()) write(`/proc/self/fd/${fd}`, `/dev/${name}`);
  try {
    mkdir(cwd);
  } catch {
    /* the working directory is best-effort, as in `Shell::create` */
  }
  for (const name of volumeStubNames(commandNames)) {
    try {
      const content = `#!/bin/bash\n# Built-in command: ${name}\n`;
      for (const dir of ["/bin", "/usr/bin"]) write(`${dir}/${name}`, content);
    } catch {
      /* stub writes are best-effort, as in `Shell::create` */
    }
  }
}

/** Writes one registering command's stubs to a branded volume (best-effort). */
function seedVolumeStubs(vfs, inst, names) {
  for (const name of volumeStubNames(names)) {
    try {
      const content = `#!/bin/bash\n# Built-in command: ${name}\n`;
      for (const dir of ["/bin", "/usr/bin"]) {
        inst.call(() => vfs.op({
          tag: "write-file",
          val: { path: utf8(`${dir}/${name}`), data: utf8(content), offset: 0n, truncate: true, mode: 0o644, mtimeMs: 0n },
        }));
      }
    } catch {
      /* stub writes are best-effort, as in `Shell::create` */
    }
  }
}

/**
 * Builds the facade classes for one loader.
 * @param {{ syncModule(): any, asyncModule(): Promise<any>, precompile(source?: unknown): Promise<void> }} loader
 */
export function createFacade(loader) {
  const InMemoryFs = createInMemoryFs(loader);
  const STATE = new WeakMap();

  class Bash {
    constructor(options = {}) {
      if (!isObject(options)) throw fail("options must be an object");
      for (const k of Object.keys(options)) if (!OPTION_NAMES.has(k)) throw fail(`unknown option ${k}`);
      const hooks = options[INTERNAL];
      const profile = options.executionLimitProfile ?? DEFAULT_PROFILE;
      if (!PROFILE_NAMES.includes(profile)) throw fail(`executionLimitProfile: unknown profile ${String(profile)}`);
      if (options.cwd !== undefined && typeof options.cwd !== "string") throw fail("cwd must be a string");
      const userEnv = options.env === undefined ? [] : stringRecord(options.env, "env");
      if (options.files !== undefined && !isObject(options.files)) throw fail("files must be an object");
      if (options.executionLimits !== undefined && !isObject(options.executionLimits)) throw fail("executionLimits must be an object");
      const overrides = Object.assign(Object.create(null), options.executionLimits ?? {});
      for (const alias of ["maxCallDepth", "maxCommandCount", "maxLoopIterations"]) {
        if (options[alias] !== undefined) {
          if (typeof options[alias] !== "number") throw fail(`${alias} must be a number`);
          overrides[alias] = options[alias];
        }
      }
      for (const [name, check] of [["fetch", "function"], ["sleep", "function"], ["trace", "function"]]) {
        if (options[name] !== undefined && typeof options[name] !== check) throw fail(`${name} must be a function`);
      }
      if (options.network !== undefined && !isObject(options.network)) throw fail("network must be an object");
      if (options.commands !== undefined && (!Array.isArray(options.commands) || options.commands.some((c) => typeof c !== "string"))) throw fail("commands must be an array of strings");
      if (options.logger !== undefined && (!isObject(options.logger) || typeof options.logger.info !== "function" || typeof options.logger.debug !== "function")) throw fail("logger needs info and debug functions");
      if (options.coverage !== undefined && (!isObject(options.coverage) || typeof options.coverage.hit !== "function")) throw fail("coverage needs a hit function");
      if (options.processInfo !== undefined && !isObject(options.processInfo)) throw fail("processInfo must be an object");
      if (options.customCommands !== undefined && !Array.isArray(options.customCommands)) throw fail("customCommands must be an array");
      const customs = options.customCommands ?? [];
      customs.forEach(checkCommand);

      // The filesystem and its instance. A branded volume is an `InMemoryFs` with
      // facade-private state; a prototype or a lookalike gives no synchronous
      // kernel capability and stays on the host-adapter path.
      const given = options.fs;
      const branded = given instanceof InMemoryFs && fsState(given) ? given : null;
      const userFs = branded ? null : (given ?? null);
      if (given !== undefined && !isObject(given)) throw fail("fs must be an object");
      // Omitted/undefined means "separate"; any other value (including null) throws.
      const isolation = options.isolation === undefined ? "separate" : options.isolation;
      if (isolation !== "separate" && isolation !== "shared") throw fail('isolation must be "separate" or "shared"');
      if (isolation === "shared" && given !== undefined && !branded) throw fail('isolation "shared" needs a kernel-backed InMemoryFs');
      const separate = branded && isolation === "separate";
      const volumeHolder = branded ? fsState(branded).holder : null;
      const effective = hooks?.ceilingBytes ?? Math.max(isolation === "shared" && volumeHolder ? volumeHolder.ceiling : 0, PROFILES[profile].ceiling);
      const limits = resolveLimits(profile, overrides, effective);
      const hasFiles = options.files !== undefined;
      const defaultLayout = !options.cwd && !options.files;
      const home = defaultLayout ? "/home/user" : "/";
      const cwd = options.cwd || home;
      const initialCwd = resolvePath("/", cwd);
      const envEntries = initialEnvEntries(initialCwd, home, userEnv);
      const commands = [...(options.commands ?? []), ...customs.map((c) => c.name)];
      const exported = new Set(["HOME", "PATH", "PWD", "OLDPWD", ...userEnv.map(([key]) => key)]);
      if (hasFiles && !branded) {
        const paths = new Set(Object.keys(options.files).map((path) => resolvePath(initialCwd, path)));
        const systemDirs = ["/bin", "/usr", "/usr/bin", "/dev", "/dev/fd", "/proc", "/proc/self", "/proc/self/fd"];
        const conflict = systemDirs.find((path) => paths.has(path));
        if (conflict) throw new Error("EEXIST: file already exists, mkdir '" + conflict + "'");
      }
      const headroom = PROFILES[profile].headroom;
      const limit = stringLimit([...userEnv.flat(), ...commands, options.cwd === undefined ? "" : initialCwd], limits.maxStringLength, headroom) ??
        stringLimit([...envEntries.flat(), ...commands, initialCwd, ...Object.keys(overrides)], Infinity, headroom,
          envEntries.length * 20 + commands.length * 8 + Object.keys(overrides).length * 16);
      if (limit) throw new RangeError(`shell configuration: ${limit} exceeded`);

      // Provenance and predicate for the Wasm-owned default snapshot (spec L2c 4.5-4.6): only
      // the default binding with the package's own production loader core may restore, and only
      // for a validated ordinary `contain` default. Any `fs`, explicit `isolation`, or host
      // mount takes the ordinary path.
      const bindingRestore = loader.defaultBinding === true && loader.customized !== true;
      const restorable = bindingRestore && given === undefined && userFs === null && !separate && !branded && isDefaultSnapshotConfig({
        profile,
        overridesEmpty: Object.keys(overrides).length === 0,
        ceiling: effective,
        // Only behavior-changing internal overrides disqualify; passive observers do not.
        // The snapshot capture tool forces ordinary setup explicitly.
        hasHooks: hooks !== undefined && (hooks.kernel !== undefined ||
          hooks.moduleProvider !== undefined || hooks.ceilingBytes !== undefined ||
          hooks.snapshotCapture === true),
        present: Object.fromEntries(Array.from(new Set([...PREDICATE_OPTIONS, "isolation"])).map((name) => [name, options[name] !== undefined])),
        loggerPresent: options.logger !== undefined,
      });
      // A supplied volume keeps its contents: `files` only affects the default
      // cwd/layout choice. A separate shell gets a fresh execution holder and a
      // local root mounted over the volume; a shared shell joins the volume's holder.
      let holder;
      let rootFs;
      let volumeFs = null;
      if (separate) {
        volumeFs = branded;
        holder = new Holder(loader, hooks, hooks?.ceilingBytes ?? PROFILES[profile].ceiling);
        rootFs = new InMemoryFs(undefined, {
          hooks, profile, maxFileSystemBytes: limits.maxFileSystemBytes, maxStringLength: limits.maxStringLength, holder, deferVfs: false,
        });
      } else if (branded) {
        volumeFs = branded;
        holder = volumeHolder;
        rootFs = branded;
      } else {
        rootFs = new InMemoryFs(undefined, { hooks, profile, maxFileSystemBytes: limits.maxFileSystemBytes, maxStringLength: limits.maxStringLength, deferVfs: restorable });
        holder = fsState(rootFs).holder;
      }
      if (hooks?.ceilingBytes === undefined && isolation === "shared") volumeHolder?.raiseCeiling(effective);
      const B = {
        holder,
        rootFs,
        volumeFs,
        volumeHolder: separate ? volumeHolder : null,
        userFs,
        mounts: new Map(userFs === null ? (separate ? [[1, markKernelVolume(branded)]] : []) : [[1, userFs]]),
        limits,
        headroom,
        profile,
        commands: new Map(),
        commandNames: options.commands === undefined ? COMMAND_NAMES : COMMAND_NAMES.filter((name) => options.commands.includes(name)),
        sleep: options.sleep,
        fetch: options.fetch,
        network: options.network,
        trace: options.trace,
        coverage: options.coverage,
        logger: options.logger,
        hooks,
        shell: null,
        initialEnv: nullObject(envEntries),
        initialCwd,
        resolveCommand: async (entry) => {
          if (entry.cmd) return entry.cmd;
          if (typeof entry.src.execute === "function") entry.cmd = entry.src;
          else {
            const loaded = await entry.src.load();
            checkCommand(loaded);
            entry.cmd = loaded;
          }
          return entry.cmd;
        },
        runChild: null,
      };
      B.runChild = (command, o, scope, parent) => this.#runChild(B, command, o, scope, parent);
      STATE.set(this, B);
      if (hasFiles && !branded && userFs === null) addFiles(rootFs, options.files, B.initialCwd);
      seedHostLayout(userFs, defaultLayout, B.initialCwd, options.processInfo, [...B.commandNames, ...customs.map((c) => c.name)]);
      // A host mount at `/` shadows the kernel volume: `options.files` belong
      // in the host `fs`, never in the shadowed `rootFs`.
      B.seeded = hasFiles && userFs !== null ? seedUserFiles(userFs, options.files, B.initialCwd) : null;
      for (const c of customs) this.#register(B, c, false);
      const shellConfig = {
        limits: { profile, overrides: Object.entries(overrides).map(([name, value]) => ({ name, value: BigInt(value) })) },
        grants: { customCommands: true, realSleep: typeof options.sleep === "function", hostFs: userFs !== null || Boolean(separate) },
        commands: options.commands?.slice(),
        customCommands: [...B.commands.keys()],
        env: envEntries.map(([n, v]) => ({ name: utf8(n), value: utf8(v), exported: exported.has(n) })),
        cwd: utf8(B.initialCwd),
        processInfo: options.processInfo ? { pid: options.processInfo.pid ?? 1, ppid: options.processInfo.ppid ?? 0, uid: options.processInfo.uid ?? 1000, gid: options.processInfo.gid ?? 1000 } : undefined,
        defaultLayout,
        trace: typeof options.trace === "function",
        coverage: !!options.coverage,
      };
      if (branded) {
        const st = fsState(branded);
        const layout = {
          defaultLayout, cwd: B.initialCwd, processInfo: options.processInfo,
          commandNames: [...B.commandNames, ...customs.map((c) => c.name)],
        };
        st.holder.stage((volInst) => seedVolumeLayout(st.vfs, volInst, layout));
      }
      if (restorable) {
        // One restore call replaces configure, Vfs.create and Shell.create. A `0n` refusal is
        // a normal return and falls back to the ordinary setup in the same staged step.
        // Restorable implies no branded volume, no separate store, and no host mount,
        // so the fallback needs no volume layout and no mountHost.
        const restoreFirst = (inst) => {
          const restored = inst.util.restoreDefault?.();
          if (restored) {
            fsState(rootFs).vfs = restored.vfs;
            B.shell = restored.shell;
            return;
          }
          inst.call(() => inst.engine.configure(BigInt(effective)));
          fsState(rootFs).vfs = inst.call(() => inst.engine.Vfs.create(BigInt(limits.maxFileSystemBytes)));
          B.shell = inst.call(() => inst.engine.Shell.create(fsState(rootFs).vfs, shellConfig));
        };
        restoreFirst.restoreFirst = true;
        B.holder.stage(restoreFirst);
      } else {
        B.holder.stage((inst) => {
          const { vfs } = fsState(rootFs);
          if (userFs !== null || separate) inst.call(() => vfs.mountHost(utf8("/"), 1));
          B.shell = inst.call(() => inst.engine.Shell.create(vfs, shellConfig));
        });
      }
    }

    #register(B, c, stage) {
      checkCommand(c);
      if (stage) {
        const limit = stringLimit([c.name], B.limits.maxStringLength, B.headroom);
        if (limit) throw new RangeError(`command name: ${limit} exceeded`);
        seedHostCommand(B.userFs, c.name);
        if (B.volumeFs) {
          const st = fsState(B.volumeFs);
          st.holder.stage((volInst) => seedVolumeStubs(st.vfs, volInst, [c.name]));
        }
        B.holder.stage((inst) => {
          inst.call(() => B.shell.registerCommand(c.name));
        });
      }
      B.commands.set(c.name, { src: c, cmd: null, trusted: c.trusted });
    }

    get fs() {
      const B = STATE.get(this);
      return B.userFs ?? B.volumeFs ?? B.rootFs;
    }

    registerCommand(command) {
      const B = STATE.get(this);
      B.holder.instance?.check();
      this.#register(B, command, true);
    }

    getCommandNames() {
      return [...STATE.get(this).commandNames];
    }

    getCwd() {
      const B = STATE.get(this);
      const inst = B.holder.instance;
      if (!inst) return B.initialCwd;
      inst.check();
      return decodeUtf8(inst.call(() => B.shell.cwd()));
    }

    getEnv() {
      const B = STATE.get(this);
      const inst = B.holder.instance;
      if (!inst) return nullObject(Object.entries(B.initialEnv));
      inst.check();
      return envToObject(inst.call(() => B.shell.env()));
    }

    async readFile(path) {
      const B = STATE.get(this);
      B.holder.instance?.check();
      if (B.seeded) await B.seeded;
      return this.fs.readFile(resolvePath(B.initialCwd, path));
    }

    async writeFile(path, content) {
      const B = STATE.get(this);
      B.holder.instance?.check();
      if (B.seeded) await B.seeded;
      return this.fs.writeFile(resolvePath(B.initialCwd, path), content);
    }

    #normalize(B, options, forSync) {
      if (options === undefined) options = {};
      if (!isObject(options)) throw fail("exec options must be an object");
      const env = options.env === undefined ? [] : stringRecord(options.env, "env");
      if (options.cwd !== undefined && typeof options.cwd !== "string") throw fail("cwd must be a string");
      if (options.stdin !== undefined && typeof options.stdin !== "string") throw fail("stdin must be a string");
      if (options.stdinKind !== undefined && options.stdinKind !== "text" && options.stdinKind !== "bytes") throw fail('stdinKind must be "text" or "bytes"');
      if (options.args !== undefined && (!Array.isArray(options.args) || options.args.some((a) => typeof a !== "string"))) throw fail("args must be an array of strings");
      if (options.signal !== undefined && !(isObject(options.signal) && typeof options.signal.aborted === "boolean")) throw fail("signal must be an AbortSignal");
      if (options.replaceEnv !== undefined && typeof options.replaceEnv !== "boolean") throw fail("replaceEnv must be a boolean");
      if (options.rawScript !== undefined && typeof options.rawScript !== "boolean") throw fail("rawScript must be a boolean");
      void forSync;
      let stdin = new Uint8Array(0);
      if (options.stdin) stdin = options.stdinKind === "bytes" ? unsafeBytesFromLatin1(options.stdin) : utf8(options.stdin);
      return {
        env,
        replaceEnv: options.replaceEnv,
        cwd: options.cwd || undefined,
        envGiven: options.env !== undefined,
        rawScript: options.rawScript,
        stdin,
        args: options.args ?? [],
        signal: options.signal,
        bypass: options.bypass,
        parent: options.parent,
      };
    }

    #log(B, command, result) {
      const lg = B.logger;
      if (!lg || command.trim() === "") return;
      lg.info("exec", { command });
      if (result.stdout) lg.debug("stdout", { output: result.stdout });
      if (result.stderr) lg.info("stderr", { output: result.stderr });
      lg.info("exit", { exitCode: result.exitCode });
    }

    async exec(commandLine, options) {
      const B = STATE.get(this);
      if (B.seeded) await B.seeded;
      if (typeof commandLine !== "string") throw fail("the command line must be a string");
      const o = this.#normalize(B, options, false);
      // A separate volume is laid out on its own holder: both stores are ready
      // before the shell enters its driver.
      if (B.volumeHolder) await B.volumeHolder.ensure();
      const inst = await B.holder.ensure();
      inst.check();
      const result = await runAsync(execLoop(B, commandLine, o));
      this.#log(B, commandLine, result);
      return result;
    }

    execSync(commandLine, options) {
      const B = STATE.get(this);
      if (typeof commandLine !== "string") throw fail("the command line must be a string");
      B.holder.instance?.check();
      if (B.userFs || B.fetch || B.sleep || B.commands.size > 0 || B.network !== undefined) throw new AsyncCapabilityError();
      if (!B.holder.instance || (B.volumeHolder && !B.volumeHolder.instance)) {
        throw new AsyncCapabilityError("execSync needs the kernel module: call `await Bash.precompile()` first");
      }
      const o = this.#normalize(B, options, true);
      B.holder.instance.check();
      const result = runSync(execLoop(B, commandLine, o));
      this.#log(B, commandLine, result);
      return result;
    }

    async #runChild(B, command, options, scope, parent) {
      const o = this.#normalize(B, options ?? {}, false);
      o.signal = anySignal(scope.signal, o.signal);
      o.parent = parent;
      const r = await runAsync(execLoop(B, command, o));
      const { stdout, stderr, exitCode, stdoutKind } = r;
      return stdoutKind === "bytes" ? { stdout, stderr, exitCode, stdoutKind } : { stdout, stderr, exitCode };
    }

    transform() {
      throw new NotSupportedError("transform() is not supported");
    }

    registerTransformPlugin() {
      throw new NotSupportedError("registerTransformPlugin() is not supported");
    }

    static precompile(source) {
      return loader.precompile(source);
    }
  }

  return { Bash, InMemoryFs };
}

