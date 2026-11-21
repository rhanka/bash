// Builds the kernel core module with the `mem-test-exports` feature (lot L1a-2) and instantiates it.
// The module has no import: the allocator tests call its exported workloads directly.
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { repoRoot } from "./helpers.mjs";

const MAX_MEMORY = 1 << 30;

/** Builds (or reuses cargo's cache of) the test module and returns a compiled WebAssembly.Module. */
export function buildMemModule({ addressSpaceProbe = false } = {}) {
  const target = join(repoRoot, "tmp", addressSpaceProbe ? "memtest-address-space-target" : "memtest-target");
  mkdirSync(target, { recursive: true });
  const rustflags = [
    "-C link-self-contained=no",
    "-C link-arg=--no-entry",
    "-C link-arg=--skip-wit-component",
    addressSpaceProbe ? "-C debug-assertions=yes" : `-C link-arg=--max-memory=${MAX_MEMORY}`,
    `--remap-path-prefix=${repoRoot}=.`,
  ].join(" ");
  const env = { ...process.env, RUSTFLAGS: rustflags, CARGO_TARGET_DIR: target };
  delete env.CARGO_ENCODED_RUSTFLAGS;
  const r = spawnSync(
    "cargo",
    ["build", "--release", "--locked", "--offline", "--target", "wasm32-wasip2", "-p", "sentropic-bash-kernel", "--features", "mem-test-exports"],
    { cwd: repoRoot, env, encoding: "utf8" },
  );
  if (r.status !== 0) throw new Error(`cargo build failed:\n${r.stdout}\n${r.stderr}`);
  const bytes = readFileSync(join(target, "wasm32-wasip2", "release", "sentropic_bash_kernel.wasm"));
  return new WebAssembly.Module(bytes);
}

/** A fresh instance (fresh allocator state). */
export function instantiate(module) {
  return new WebAssembly.Instance(module, {});
}
