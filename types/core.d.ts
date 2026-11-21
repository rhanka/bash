// Declarations of the facade (spec L1b 7.1; EVOL Annex D). Shared by the Node and the browser entries.

/** A string in which every character is one byte (code point 0-255). */
export type ByteString = string;

export type ExecutionLimitProfile = "normal" | "hardened" | "contain" | "micro";

/** The named limits of EVOL Annex B. */
export interface ExecutionLimits {
  maxSourceBytes?: number;
  maxExecDepth?: number;
  maxCallDepth?: number;
  maxSubstitutionDepth?: number;
  maxSourceDepth?: number;
  maxCommandCount?: number;
  maxLoopIterations?: number;
  maxAwkIterations?: number;
  maxSedIterations?: number;
  maxJqIterations?: number;
  maxWorkUnits?: number;
  maxQueryTokens?: number;
  maxQueryDepth?: number;
  maxQueryElements?: number;
  maxAwkParserTokens?: number;
  maxAwkParserDepth?: number;
  maxAwkParserOperations?: number;
  maxCsvRows?: number;
  maxCsvCells?: number;
  maxTraversalEntries?: number;
  maxTraversalDepth?: number;
  maxTraversalWork?: number;
  maxGlobOperations?: number;
  maxArrayElements?: number;
  maxBraceExpansionResults?: number;
  maxFileDescriptors?: number;
  maxLiveBytes?: number;
  maxInputBytes?: number;
  maxFileSystemBytes?: number;
  maxOutputSize?: number;
  maxStringLength?: number;
  maxHeredocSize?: number;
  maxArchiveBytes?: number;
  maxArchiveCompressedBytes?: number;
  maxArchiveEntryBytes?: number;
  maxArchiveEntries?: number;
  maxExecutionTimeMs?: number;
  maxExtensionCleanupTimeMs?: number;
  maxDatabaseBytes?: number;
  maxDatabaseResultBytes?: number;
  maxWorkerMessageBytes?: number;
  maxSqliteTimeoutMs?: number;
  maxPythonTimeoutMs?: number;
  maxJsTimeoutMs?: number;
  maxJsBridgeRequests?: number;
}

export interface BashLogger {
  info(message: string, data?: Record<string, unknown>): void;
  debug(message: string, data?: Record<string, unknown>): void;
}

export interface TraceEvent {
  category: string;
  name: string;
  durationMs: number;
  details?: Record<string, number | string>;
}

/** A stat result (EVOL Annex D.3). */
export interface FsStat {
  isFile: boolean;
  isDirectory: boolean;
  isSymbolicLink: boolean;
  mode: number;
  size: number;
  mtime: Date;
  dev?: number;
  ino?: number;
  identity?: string;
}

export interface DirentEntry {
  name: string;
  isFile: boolean;
  isDirectory: boolean;
  isSymbolicLink: boolean;
}

export type FileContent = string | Uint8Array;
export type ReadOptions = string | { encoding?: string | null };
export type WriteOptions = string | { encoding?: string | null };

/** The filesystem contract (EVOL Annex D.3): 20 required methods and 3 optional ones. */
export interface IFileSystem {
  readFile(path: string, options?: ReadOptions): Promise<string>;
  readFileBytes?(path: string): Promise<ByteString>;
  readFileBuffer(path: string): Promise<Uint8Array>;
  writeFile(path: string, content: FileContent, options?: WriteOptions): Promise<void>;
  appendFile(path: string, content: FileContent, options?: WriteOptions): Promise<void>;
  exists(path: string): Promise<boolean>;
  stat(path: string): Promise<FsStat>;
  lstat(path: string): Promise<FsStat>;
  mkdir(path: string, options?: { recursive?: boolean }): Promise<void>;
  createExclusive?(path: string, options: { mode: number; directory?: boolean }): Promise<void>;
  readdir(path: string): Promise<string[]>;
  readdirWithFileTypes?(path: string): Promise<DirentEntry[]>;
  rm(path: string, options?: { recursive?: boolean; force?: boolean }): Promise<void>;
  cp(source: string, destination: string, options?: { recursive?: boolean }): Promise<void>;
  mv(source: string, destination: string): Promise<void>;
  resolvePath(base: string, path: string): string;
  getAllPaths(): string[];
  chmod(path: string, mode: number): Promise<void>;
  symlink(target: string, linkPath: string): Promise<void>;
  link(existingPath: string, newPath: string): Promise<void>;
  readlink(path: string): Promise<string>;
  realpath(path: string): Promise<string>;
  utimes(path: string, atime: Date, mtime: Date): Promise<void>;
}

export type FileInit = FileContent | { content: FileContent; mode?: number; mtime?: Date | number } | (() => FileContent | Promise<FileContent>);
export type InitialFiles = Record<string, FileInit>;

/** An in-memory filesystem over the kernel's volume. */
export declare class InMemoryFs implements IFileSystem {
  constructor(initialFiles?: InitialFiles);
  readFile(path: string, options?: ReadOptions): Promise<string>;
  readFileBytes(path: string): Promise<ByteString>;
  readFileBuffer(path: string): Promise<Uint8Array>;
  writeFile(path: string, content: FileContent, options?: WriteOptions): Promise<void>;
  appendFile(path: string, content: FileContent, options?: WriteOptions): Promise<void>;
  exists(path: string): Promise<boolean>;
  stat(path: string): Promise<FsStat>;
  lstat(path: string): Promise<FsStat>;
  mkdir(path: string, options?: { recursive?: boolean }): Promise<void>;
  createExclusive(path: string, options: { mode: number; directory?: boolean }): Promise<void>;
  readdir(path: string): Promise<string[]>;
  readdirWithFileTypes(path: string): Promise<DirentEntry[]>;
  rm(path: string, options?: { recursive?: boolean; force?: boolean }): Promise<void>;
  cp(source: string, destination: string, options?: { recursive?: boolean }): Promise<void>;
  mv(source: string, destination: string): Promise<void>;
  resolvePath(base: string, path: string): string;
  getAllPaths(): string[];
  chmod(path: string, mode: number): Promise<void>;
  symlink(target: string, linkPath: string): Promise<void>;
  link(existingPath: string, newPath: string): Promise<void>;
  readlink(path: string): Promise<string>;
  realpath(path: string): Promise<string>;
  utimes(path: string, atime: Date, mtime: Date): Promise<void>;
  mkdirSync(path: string, options?: { recursive?: boolean }): void;
  writeFileSync(path: string, content: FileContent, options?: WriteOptions): void;
  writeFileLazy(path: string, load: () => FileContent | Promise<FileContent>, options?: { mode?: number; mtime?: Date | number }): void;
}

export interface NetworkRule {
  url: string;
  transform?: Array<{ headers: Record<string, string> }>;
}

/** Network policy (EVOL Annex D.4); validated and stored, enforced by lot L16b. */
export interface NetworkConfig {
  allowedUrlPrefixes?: Array<string | NetworkRule>;
  allowedMethods?: string[];
  dangerouslyAllowFullInternetAccess?: boolean;
  maxRedirects?: number;
  timeoutMs?: number;
  maxResponseSize?: number;
  denyPrivateRanges?: boolean;
}

export interface SecureFetchResponse {
  status: number;
  statusText: string;
  headers: Record<string, string>;
  body: string | Uint8Array;
  url: string;
}
export type SecureFetch = (url: string, options?: { method?: string; headers?: Record<string, string>; body?: string | Uint8Array; followRedirects?: boolean; timeoutMs?: number }) => Promise<SecureFetchResponse>;

export interface JavaScriptConfig {
  bootstrap?: string;
  invokeTool?: (name: string, args: unknown) => Promise<unknown>;
}

export interface BashOptions {
  files?: InitialFiles;
  env?: Record<string, string>;
  cwd?: string;
  fs?: IFileSystem;
  executionLimits?: ExecutionLimits;
  executionLimitProfile?: ExecutionLimitProfile;
  maxCallDepth?: number;
  maxCommandCount?: number;
  maxLoopIterations?: number;
  fetch?: SecureFetch;
  network?: NetworkConfig;
  python?: boolean;
  javascript?: boolean | JavaScriptConfig;
  commands?: string[];
  sleep?: (ms: number) => Promise<void>;
  customCommands?: CustomCommand[];
  logger?: BashLogger;
  trace?: (event: TraceEvent) => void;
  coverage?: { hit(feature: string): void };
  defenseInDepth?: unknown;
  processInfo?: { pid?: number; ppid?: number; uid?: number; gid?: number };
}

export interface ExecOptions {
  env?: Record<string, string>;
  replaceEnv?: boolean;
  cwd?: string;
  rawScript?: boolean;
  stdin?: string;
  stdinKind?: "text" | "bytes";
  signal?: AbortSignal;
  args?: string[];
}

export interface BashExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  env: Record<string, string>;
  stdoutKind?: "text" | "bytes";
}

export declare class Bash {
  constructor(options?: BashOptions);
  readonly fs: IFileSystem;
  exec(commandLine: string, options?: ExecOptions): Promise<BashExecResult>;
  execSync(commandLine: string, options?: Omit<ExecOptions, "signal">): BashExecResult;
  readFile(path: string): Promise<string>;
  writeFile(path: string, content: string): Promise<void>;
  getCwd(): string;
  getEnv(): Record<string, string>;
  registerCommand(command: Command): void;
  transform(...args: unknown[]): never;
  registerTransformPlugin(...args: unknown[]): never;
  static precompile(source?: unknown): Promise<void>;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
  stdoutKind?: "text" | "bytes";
}

export interface Command {
  name: string;
  trusted?: boolean;
  execute(args: string[], ctx: CommandContext): Promise<ExecResult>;
}
export interface LazyCommand {
  name: string;
  trusted?: boolean;
  load: () => Promise<Command>;
}
export type CustomCommand = Command | LazyCommand;

export declare function defineCommand(
  name: string,
  execute: (args: string[], ctx: CommandContext) => Promise<ExecResult>,
  options?: { trusted?: boolean },
): Command;

export interface CommandContext {
  fs: IFileSystem;
  cwd: string;
  env: Map<string, string>;
  exportedEnv?: Record<string, string>;
  stdin: ByteString;
  limits: Required<ExecutionLimits>;
  signal?: AbortSignal;
  exec?(
    command: string,
    options: { cwd: string; env?: Record<string, string>; replaceEnv?: boolean; stdin?: string; stdinKind?: "text" | "bytes"; args?: string[]; signal?: AbortSignal },
  ): Promise<ExecResult>;
  origCommand?(args: string[]): Promise<ExecResult>;
  fetch?: SecureFetch;
  sleep?(ms: number): Promise<void>;
  assignShellVariable?(name: string, value: string, subscript?: string): Promise<void>;
  fileDescriptors?: Map<number, string>;
  getRegisteredCommands?(): string[];
  trace?(event: TraceEvent): void;
  coverage?: { hit(feature: string): void };
  xpgEcho?: boolean;
}

/** Any call after a kernel trap. */
export declare class KernelPoisonedError extends Error {}
/** `execSync` with an asynchronous capability configured. */
export declare class AsyncCapabilityError extends Error {}
/** `transform()`, `registerTransformPlugin()` and the lazy files of this version. */
export declare class NotSupportedError extends Error {}
/** Raised by the capability wrappers of a revoked scope. */
export declare class ExecutionAbortedError extends Error {}

export declare function getCommandNames(): string[];
export declare function getNetworkCommandNames(): string[];
export declare function getPythonCommandNames(): string[]; // ["python3", "python"]
export declare function getJavaScriptCommandNames(): string[]; // ["js-exec", "node"]

// Byte helpers (EVOL Annex D.5).
export declare const EMPTY_BYTES: ByteString;
export declare function encodeUtf8ToBytes(text: string): ByteString;
export declare function decodeBytesToUtf8(bytes: ByteString): string;
export declare function latin1FromBytes(bytes: Uint8Array): ByteString;
export declare function unsafeBytesFromLatin1(text: ByteString): Uint8Array;
export declare function bytesOutput(bytes: ByteString | Uint8Array, stderr?: string, exitCode?: number): ExecResult;
export declare function textOutput(text: string, stderr?: string, exitCode?: number): ExecResult;
export declare function stdoutKind(result: ExecResult): "text" | "bytes";
export declare function stdoutAsBytes(result: ExecResult): ByteString;
