#!/usr/bin/env node
// Command line recorder. Usage:
//   node tools/parity-record.mjs --adapter <module> --cases <dir> --out <dir> [--filter <s>] [--check]
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { recordAll } from "../harness/parity/record.mjs";

const USAGE = "usage: parity-record --adapter <module path> --cases <dir> --out <dir> [--filter <substring>] [--check]";
const argv = process.argv.slice(2);
const opts = { check: false };
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === "--check") opts.check = true;
  else if (["--adapter", "--cases", "--out", "--filter"].includes(a) && i + 1 < argv.length) opts[a.slice(2)] = argv[++i];
  else {
    console.error(`parity-record: unknown or incomplete argument ${a}\n${USAGE}`);
    process.exit(2);
  }
}
if (!opts.adapter || !opts.cases || !opts.out) {
  console.error(USAGE);
  process.exit(2);
}

const mod = await import(pathToFileURL(resolve(opts.adapter)).href);
const res = await recordAll({
  adapter: mod.default,
  casesDir: resolve(opts.cases),
  snapshotsDir: resolve(opts.out),
  check: opts.check,
  filter: opts.filter,
});
for (const line of res.lines) console.log(line);
process.exit(res.failed ? 1 : 0);
