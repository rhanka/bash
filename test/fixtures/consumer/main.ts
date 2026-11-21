// A consumer that uses every declaration of the facade (spec L1b 7.1).
import {
  AsyncCapabilityError, Bash, EMPTY_BYTES, ExecutionAbortedError, InMemoryFs, KernelPoisonedError, NotSupportedError, OverlayFs, ReadWriteFs,
  bytesOutput, decodeBytesToUtf8, defineCommand, encodeUtf8ToBytes, getCommandNames, getJavaScriptCommandNames, getNetworkCommandNames,
  getPythonCommandNames, latin1FromBytes, stdoutAsBytes, stdoutKind, textOutput, unsafeBytesFromLatin1,
} from "@sentropic/bash";
import type {
  BashExecResult, BashLogger, BashOptions, Command, CommandContext, CustomCommand, ExecOptions, ExecResult, ExecutionLimitProfile,
  ExecutionLimits, IFileSystem, InitialFiles, LazyCommand, NetworkConfig, OverlayFsOptions, ReadWriteFsOptions, SecureFetch, TraceEvent,
} from "@sentropic/bash";
import { Bash as BrowserBash } from "@sentropic/bash/browser";

const profile: ExecutionLimitProfile = "contain";
const overlayOptions: OverlayFsOptions = { root: ".", mountPoint: "/project", maxMemoryBytes: 1024 };
const readWriteOptions: ReadWriteFsOptions = { root: ".", maxCopyOnWriteSize: 1024 };
const hostFilesystems: IFileSystem[] = [new OverlayFs(overlayOptions), new ReadWriteFs(readWriteOptions)];
void hostFilesystems;
const limits: ExecutionLimits = { maxOutputSize: 10, maxLiveBytes: 1 << 20 };
const logger: BashLogger = { info() {}, debug() {} };
const network: NetworkConfig = { allowedUrlPrefixes: ["https://example.test", { url: "https://x.test/v1", transform: [{ headers: { a: "b" } }] }] };
const fetchFn: SecureFetch = async (url) => ({ status: 200, statusText: "OK", headers: {}, body: url, url });
const files: InitialFiles = { "/a": "x", "/b": new Uint8Array(1), "/c": { content: "y", mode: 0o600, mtime: 1 }, "/d": async () => "lazy" };

const hello: Command = defineCommand("hello", async (args: string[], ctx: CommandContext): Promise<ExecResult> => {
  const stdin: string = ctx.stdin;
  const env: Map<string, string> = ctx.env;
  const l: Required<ExecutionLimits> = ctx.limits;
  await ctx.fs.writeFile("/x", stdin + env.size + l.maxOutputSize + ctx.cwd);
  await ctx.exec?.("echo", { cwd: ctx.cwd, env: { A: "1" }, stdin: "s", stdinKind: "text" });
  await ctx.origCommand?.(args);
  await ctx.assignShellVariable?.("N", "V", "0");
  ctx.trace?.({ category: "c", name: "n", durationMs: 1, details: { k: 1, s: "v" } });
  return textOutput("hello", "", 0);
}, { trusted: false });
const lazy: LazyCommand = { name: "lz", load: async () => hello };
const all: CustomCommand[] = [hello, lazy];

const options: BashOptions = {
  files, env: { A: "1" }, cwd: "/", fs: new InMemoryFs(files), executionLimits: limits, executionLimitProfile: profile, maxCallDepth: 1,
  maxCommandCount: 2, maxLoopIterations: 3, fetch: fetchFn, network, python: true, javascript: { bootstrap: "" }, commands: ["echo"],
  sleep: async () => {}, customCommands: all, logger, trace: (e: TraceEvent) => void e, coverage: { hit: (f: string) => void f },
  defenseInDepth: { enabled: false }, processInfo: { pid: 1, ppid: 0, uid: 2, gid: 3 },
};

async function main(): Promise<void> {
  const bash = new Bash(options);
  const eo: ExecOptions = { env: {}, replaceEnv: true, cwd: "/", rawScript: false, stdin: "x", stdinKind: "bytes", signal: new AbortController().signal, args: ["a"] };
  const r: BashExecResult = await bash.exec("echo", eo);
  const s: BashExecResult = bash.execSync("echo", { env: {} });
  const k: "text" | "bytes" | undefined = r.stdoutKind ?? s.stdoutKind;
  const fs: IFileSystem = bash.fs;
  const text: string = await bash.readFile("/a");
  await bash.writeFile("/a", text);
  const cwd: string = bash.getCwd();
  const env: Record<string, string> = bash.getEnv();
  bash.registerCommand(hello);
  await Bash.precompile();
  await BrowserBash.precompile(undefined);
  void [k, fs, cwd, env];
  const errors: Error[] = [new KernelPoisonedError(), new AsyncCapabilityError(), new NotSupportedError(), new ExecutionAbortedError()];
  const names: string[] = [...bash.getCommandNames(), ...getCommandNames(), ...getNetworkCommandNames(), ...getPythonCommandNames(), ...getJavaScriptCommandNames()];
  const b: string = encodeUtf8ToBytes("x") + decodeBytesToUtf8(EMPTY_BYTES) + latin1FromBytes(unsafeBytesFromLatin1("y"));
  const out: ExecResult = bytesOutput("x");
  void [errors, names, b, stdoutKind(out), stdoutAsBytes(out)];
}
void main;
