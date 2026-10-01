// Resolve `cloudflare:workers` to a stub so worker.js loads under node.
import * as nodeModule from "node:module";
import { STUB_URL } from "./hooks.mjs";

if (nodeModule.registerHooks) {
  nodeModule.registerHooks({
    resolve: (specifier, context, next) =>
      specifier === "cloudflare:workers" ? { url: STUB_URL, shortCircuit: true } : next(specifier, context),
  });
} else {
  nodeModule.register("./hooks.mjs", import.meta.url);
}
