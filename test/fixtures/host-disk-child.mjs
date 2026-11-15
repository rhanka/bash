// Child process of the host-disk test: wraps node:fs before importing the package, then runs a scenario.
import fs, { writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";

const sentinel = process.argv[2];
writeFileSync(sentinel, "secret");
const calls = [];
let recording = false;
let depth = 0;
for (const name of Object.keys(fs)) {
  const f = fs[name];
  if (typeof f !== "function" || /^[A-Z]/.test(name)) continue;
  fs[name] = function (...args) {
    // Calls that node makes on its own behalf inside one public call (readFileSync opens, reads, closes) do not count.
    if (recording && depth === 0) calls.push([name, String(args[0])]);
    depth++;
    try {
      return f.apply(this, args);
    } finally {
      depth--;
    }
  };
}
syncBuiltinESMExports();
const { Bash } = await import("@sentropic/bash");
recording = true;
const b = new Bash();
const exists = await b.fs.exists(sentinel);
const read = await b.readFile(sentinel).then(() => "read", () => "rejected");
const r = await b.exec(`__cat ${sentinel}`).catch(() => ({ exitCode: -1 }));
recording = false;
process.stdout.write(JSON.stringify({ exists, read, exit: r.exitCode, calls }));
