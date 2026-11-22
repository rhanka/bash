// Lazy extension and host-filesystem serving (custom commands, sleep, host fs-batch).
import { FS_METHODS } from "./membrane.js";
import { serveExtension, serveCustom } from "./serve-custom.js";
import { serveFsBatch } from "./fs-batch.js";

export const ext = { FS_METHODS, serveExtension, serveCustom, serveFsBatch };
