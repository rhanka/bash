import { fileURLToPath } from "node:url";
import { defineParityTests } from "../../harness/parity/replay.mjs";
import adapter from "../../harness/parity/sentropic-adapter.mjs";

const path = (relative) => fileURLToPath(new URL(relative, import.meta.url));

defineParityTests({
  adapter,
  casesDir: path("../cases/l2"),
  snapshotsDir: path("../cases/l2"),
  exclusions: path("./exclusions.json"),
  reportPath: path("../../tmp/parity-report-l2.json"),
});
