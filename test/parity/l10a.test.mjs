// Replays the committed L10a sample on the mock adapter. Lot L2 plugs its own adapter the same way.
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineParityTests } from "../../harness/parity/replay.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

defineParityTests({
  adapter: join(root, "harness/parity/mock-adapter.mjs"),
  casesDir: join(root, "test/cases/l10a"),
  snapshotsDir: join(root, "test/snapshots/l10a"),
  exclusions: join(root, "test/parity/exclusions.json"),
  reportPath: join(root, "tmp/parity-report-l10a.json"),
});
