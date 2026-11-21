import type { ByteString, DirentEntry, FileContent, FsStat, IFileSystem, ReadOptions, WriteOptions } from "./core.js";
export * from "./core.js";

export interface OverlayFsOptions {
  /** Host directory shown inside the sandbox. */
  root: string;
  /** Virtual mount point. Defaults to "/home/user/project". */
  mountPoint?: string;
  /** Reject asynchronous writes with EROFS. Defaults to false. */
  readOnly?: boolean;
  /** Largest host file read, in bytes. Defaults to 10485760. Zero removes the cap. */
  maxFileReadSize?: number;
  /** Bytes retained by the overlay. Defaults to 1073741824. */
  maxMemoryBytes?: number;
  /** Allow symlinks with targets inside the root. Defaults to false. */
  allowSymlinks?: boolean;
}

export declare class OverlayFs implements IFileSystem {
  constructor(options: OverlayFsOptions);
  getMountPoint(): string;
  /** Creates a directory in the memory layer. */
  mkdirSync(path: string, options?: { recursive?: boolean }): void;
  /** Writes to the memory layer and creates missing parents. */
  writeFileSync(path: string, content: FileContent): void;
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
}

export interface ReadWriteFsOptions {
  root: string;
  /** Largest file read, in bytes. Defaults to 10485760. Zero removes the cap. */
  maxFileReadSize?: number;
  /** Largest file copied to isolate a hard link before append or metadata changes. Defaults to 104857600. Zero removes the cap. */
  maxCopyOnWriteSize?: number;
  /** Largest regular file cp copies, in bytes. Defaults to zero, which removes the cap. */
  maxCopySize?: number;
  /** Allow symlinks with targets inside the root. Defaults to false. */
  allowSymlinks?: boolean;
}

export declare class ReadWriteFs implements IFileSystem {
  constructor(options: ReadWriteFsOptions);
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
}
