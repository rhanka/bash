// Runtime enforcement layer B, part 2: a synchronous module resolve hook (node:module registerHooks).
// Under the hook:
//  - any import of a bare specifier (including a node builtin written without the node: prefix) or of a
//    non-file URL from a module under host/ or dist/js/ throws;
//  - a node: import from a module under host/ or dist/js/ throws unless the importer lives in host/node/
//    and is NOT reachable from host/browser.js;
//  - any node: or bare import from a module reachable from host/browser.js throws, whatever the syntax
//    that produced the import (static, import(), computed specifier).
// It does not see process.getBuiltinModule(); G2 bans that outside host/node/.
import { registerHooks } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const URL_SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;

function refuse(specifier, parent, why) {
  const e = new Error(`sentropic test hook: forbidden import of "${specifier}" from ${parent}: ${why}`);
  e.code = "ERR_SENTROPIC_FORBIDDEN_IMPORT";
  return e;
}

/** Installs the hook for the repository tree at `root`. Call once per process, before importing host/. */
export function installHooks({ root }) {
  const dir = (...p) => pathToFileURL(join(root, ...p)).href + "/";
  const hostUrl = dir("host");
  const jsUrl = dir("dist", "js");
  const nodeUrl = dir("host", "node");
  const graph = new Set([pathToFileURL(join(root, "host", "browser.js")).href]);
  registerHooks({
    resolve(specifier, context, nextResolve) {
      const parent = context.parentURL;
      const inGraph = parent !== undefined && graph.has(parent);
      const governed = parent !== undefined && (parent.startsWith(hostUrl) || parent.startsWith(jsUrl));
      if (governed || inGraph) {
        const relative = specifier.startsWith("./") || specifier.startsWith("../") || specifier.startsWith("/");
        const builtin = specifier.startsWith("node:");
        if (!relative && !builtin && !specifier.startsWith("file:")) {
          throw refuse(specifier, parent, URL_SCHEME.test(specifier) ? "non-file URL" : "bare specifier");
        }
        if (builtin && (inGraph || !parent.startsWith(nodeUrl))) {
          throw refuse(specifier, parent, inGraph ? "node: import reachable from host/browser.js" : "node: import outside host/node/");
        }
      }
      const next = nextResolve(specifier, context);
      if (inGraph) graph.add(next.url);
      return next;
    },
  });
}
