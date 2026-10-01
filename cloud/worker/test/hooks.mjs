const STUB = `
export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
`;

export const STUB_URL = `data:text/javascript,${encodeURIComponent(STUB)}`;

/** Async resolve hook, for node versions without `registerHooks`. */
export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: STUB_URL, shortCircuit: true };
  return next(specifier, context);
}
