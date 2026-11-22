import { fileURLToPath } from "node:url";
import { defineCommand } from "@sentropic/bash";
import { defineParityTests } from "../../harness/parity/replay.mjs";
import sentropic from "../../harness/parity/sentropic-adapter.mjs";

const path = (relative) => fileURLToPath(new URL(relative, import.meta.url));

// Minimal stdin-only `cat` shim for L3 heredoc/pipeline oracle cases until L7a adds `kernel/src/cmd/cat.rs`.
const cat = defineCommand("cat", (_args, ctx) => ({
  stdout: ctx.stdin,
  stderr: "",
  exitCode: 0,
}));

const adapter = {
  name: sentropic.name,
  version: sentropic.version,
  create(init) {
    const engine = { ...(init.engine ?? {}) };
    const existing = engine.customCommands ?? [];
    if (!existing.some((c) => c.name === "cat")) {
      engine.customCommands = [...existing, cat];
    }
    return sentropic.create({ ...init, engine });
  },
};

defineParityTests({
  adapter,
  casesDir: path("../cases/l3"),
  snapshotsDir: path("../cases/l3"),
  exclusions: path("./exclusions.json"),
  reportPath: path("../../tmp/parity-report-l3.json"),
});
