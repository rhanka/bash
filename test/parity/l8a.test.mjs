import { fileURLToPath } from "node:url";
import { defineParityTests } from "../../harness/parity/replay.mjs";
import adapter from "../../harness/parity/l8a-adapter.mjs";

const path = (relative) => fileURLToPath(new URL(relative, import.meta.url));

defineParityTests({
  adapter,
  casesDir: path("../cases/l8a"),
  snapshotsDir: path("../cases/l8a"),
  exclusions: path("./exclusions.json"),
  reportPath: path("../../tmp/parity-report-l8a.json"),
});
