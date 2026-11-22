import { fileURLToPath } from "node:url";
import { defineParityTests } from "../../harness/parity/replay.mjs";
import adapter from "../../harness/parity/l8b-adapter.mjs";

const path = (relative) => fileURLToPath(new URL(relative, import.meta.url));

defineParityTests({
  adapter,
  casesDir: path("../cases/l8b"),
  snapshotsDir: path("../cases/l8b"),
  exclusions: path("./exclusions.json"),
  reportPath: path("../../tmp/parity-report-l8b.json"),
});
