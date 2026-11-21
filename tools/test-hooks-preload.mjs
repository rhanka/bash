// Preloaded (node --import) by tools/run-tests.mjs into every test process: installs the resolve hook for this repository.
import { installHooks } from "./test-hooks.mjs";
import { repoRoot } from "./lib.mjs";

installHooks({ root: process.env.SENTROPIC_HOOK_ROOT || repoRoot });
