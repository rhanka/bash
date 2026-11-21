// The facade: `Bash`, `defineCommand` and the name helpers (spec 7.1-7.3), bound to a loader.
import { decodeUtf8, unsafeBytesFromLatin1, utf8 } from "./bytes.js";
import { anySignal, envToObject, execLoop, nullObject, runAsync, runSync } from "./driver.js";
import { AsyncCapabilityError, NotSupportedError } from "./errors.js";
import { addFiles, createInMemoryFs, fsState } from "./fs.js";
import { DEFAULT_PROFILE, PROFILE_NAMES, PROFILES, resolveLimits, stringLimit } from "./limits.js";
import { resolvePath } from "./paths.js";
import { INTERNAL } from "./instance.js";

const OPTION_NAMES = new Set([
  "files", "env", "cwd", "fs", "executionLimits", "executionLimitProfile", "maxCallDepth", "maxCommandCount", "maxLoopIterations",
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
  return [
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
    ...userEnv,
    ["SHELLOPTS", "braceexpand:hashall:interactive-comments"],
    ["BASHOPTS", "globskipdots"],
  ];
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

export const getCommandNames = () => [];
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

function seedHostCommand(fs, name) {
  if (typeof fs?.mkdirSync !== "function" || typeof fs.writeFileSync !== "function" || name.startsWith("__")) return;
  const content = `#!/bin/bash\n# Built-in command: ${name}\n`;
  for (const dir of ["/bin", "/usr/bin"]) ignoreUnimplemented(() => fs.writeFileSync(`${dir}/${name}`, content));
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

      // The filesystem and its instance.
      const given = options.fs;
      const sharedFs = given instanceof InMemoryFs ? given : null;
      const userFs = sharedFs ? null : (given ?? null);
      if (given !== undefined && !isObject(given)) throw fail("fs must be an object");
      const sharedHolder = sharedFs && fsState(sharedFs).holder;
      const effective = hooks?.ceilingBytes ?? Math.max(sharedHolder?.ceiling ?? 0, PROFILES[profile].ceiling);
      const limits = resolveLimits(profile, overrides, effective);
      const hasFiles = options.files !== undefined;
      const defaultLayout = !options.cwd && !hasFiles;
      const cwd = options.cwd || (defaultLayout ? "/home/user" : "/");
      const initialCwd = resolvePath("/", cwd);
      const home = defaultLayout ? "/home/user" : "/";
      const envEntries = initialEnvEntries(initialCwd, home, userEnv);
      const commands = customs.map((c) => c.name);
      const headroom = PROFILES[profile].headroom;
      const limit = stringLimit([...userEnv.flat(), ...commands, options.cwd === undefined ? "" : initialCwd], limits.maxStringLength, headroom) ??
        stringLimit([...envEntries.flat(), ...commands, initialCwd, ...Object.keys(overrides)], Infinity, headroom,
          envEntries.length * 20 + commands.length * 8 + Object.keys(overrides).length * 16);
      if (limit) throw new RangeError(`shell configuration: ${limit} exceeded`);

      if (hooks?.ceilingBytes === undefined) sharedHolder?.raiseCeiling(effective);
      const rootFs = sharedFs ?? new InMemoryFs(undefined, { hooks, profile, maxFileSystemBytes: limits.maxFileSystemBytes, maxStringLength: limits.maxStringLength });
      const B = {
        holder: fsState(rootFs).holder,
        rootFs,
        userFs,
        mounts: new Map(userFs === null ? [] : [[1, userFs]]),
        limits,
        headroom,
        profile,
        commands: new Map(),
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
      if (hasFiles) addFiles(rootFs, options.files, B.initialCwd);
      seedHostLayout(userFs, defaultLayout, B.initialCwd, options.processInfo, commands);
      for (const c of customs) this.#register(B, c, false);
      const shellConfig = {
        limits: { profile, overrides: Object.entries(overrides).map(([name, value]) => ({ name, value: BigInt(value) })) },
        grants: { customCommands: true, realSleep: typeof options.sleep === "function", hostFs: userFs !== null },
        commands: undefined,
        customCommands: [...B.commands.keys()],
        env: envEntries.map(([n, v]) => ({ name: utf8(n), value: utf8(v), exported: true })),
        cwd: utf8(B.initialCwd),
        processInfo: options.processInfo ? { pid: options.processInfo.pid ?? 1, ppid: options.processInfo.ppid ?? 0, uid: options.processInfo.uid ?? 1000, gid: options.processInfo.gid ?? 1000 } : undefined,
        defaultLayout: false,
        trace: typeof options.trace === "function",
        coverage: !!options.coverage,
      };
      B.holder.stage((inst) => {
        const { vfs } = fsState(rootFs);
        if (userFs !== null) inst.call(() => vfs.mountHost(utf8("/"), 1));
        B.shell = inst.call(() => inst.engine.Shell.create(vfs, shellConfig));
      });
    }

    #register(B, c, stage) {
      checkCommand(c);
      if (stage) {
        const limit = stringLimit([c.name], B.limits.maxStringLength, B.headroom);
        if (limit) throw new RangeError(`command name: ${limit} exceeded`);
        seedHostCommand(B.userFs, c.name);
        B.holder.stage((inst) => {
          inst.call(() => B.shell.registerCommand(c.name));
        });
      }
      B.commands.set(c.name, { src: c, cmd: null, trusted: c.trusted });
    }

    get fs() {
      const B = STATE.get(this);
      return B.userFs ?? B.rootFs;
    }

    registerCommand(command) {
      const B = STATE.get(this);
      B.holder.instance?.check();
      this.#register(B, command, true);
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
      return this.fs.readFile(resolvePath(B.initialCwd, path));
    }

    async writeFile(path, content) {
      const B = STATE.get(this);
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
        cwd: options.cwd === undefined ? undefined : resolvePath(B.initialCwd, options.cwd),
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
      if (typeof commandLine !== "string") throw fail("the command line must be a string");
      const o = this.#normalize(B, options, false);
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
      if (!B.holder.instance) throw new AsyncCapabilityError("execSync needs the kernel module: call `await Bash.precompile()` first");
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

