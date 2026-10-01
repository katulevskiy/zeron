import { env } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { handleBrowserRoute } from "../../src/browser-routes";
import { SESSION_COOKIE, tokenHash } from "../../src/browser-sessions";
import type { Env } from "../../src/env";

const store = () => env.BROWSER_SESSIONS.get(env.BROWSER_SESSIONS.idFromName("browser-sessions-v1"));
const post = (path: string, body: unknown) => store().fetch(new Request(`https://store${path}`, {
  method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body)
}));
// The workerd fixture binds the browser/relay subset, not unrelated R2/session rooms.
const config = (): Env => ({ ...env, AUTH_MODE: "workos" }) as unknown as Env;
const authenticated = async (hash: string): Promise<boolean> => {
  const response = await post("/validate", { hash, touch: false, refresh: false });
  return (await response.json() as { authenticated: boolean }).authenticated;
};
const create = async (ownerId: string) => {
  const raw = crypto.randomUUID();
  const hash = await tokenHash(raw);
  const csrf = crypto.randomUUID();
  const response = await post("/create", { hash, ownerId, providerSessionId: `provider-${ownerId}`, csrfToken: csrf,
    accessToken: "expired-access", refreshToken: "unusable-refresh", providerExpiresAt: Date.now() - 1 });
  expect(response.status).toBe(200);
  return { hash, csrf, cookie: `${SESSION_COOKIE}=${raw}` };
};
const request = (path: string, session: { csrf: string; cookie: string }) => {
  const url = new URL(`https://test/api/browser/${path}`);
  return new Request(url, { method: "POST", headers: {
    origin: "https://test", cookie: session.cookie, "x-csrf-token": session.csrf
  } });
};
const failures = [
  { name: "non-2xx with an apparent ACK", response: () => Response.json({ ok: true }, { status: 503 }) },
  { name: "malformed JSON", response: () => new Response("{broken", { status: 200 }) },
  { name: "missing ACK", response: () => Response.json({}) },
  { name: "false ACK", response: () => Response.json({ ok: false }) },
  { name: "truthy but non-boolean ACK", response: () => Response.json({ ok: "true" }) },
  { name: "null payload", response: () => Response.json(null) },
  { name: "transport rejection", response: (): Response => { throw new Error("session store unavailable"); } }
];

afterEach(() => vi.unstubAllGlobals());

describe("durable browser revocation route acknowledgements", () => {
  for (const route of ["logout", "revoke-all", "current-session", "other-session"] as const) {
    it.each(failures)(`${route}: $name fails recoverably and a durable retry succeeds`, async ({ response }) => {
      // Real SQLite-backed sessions, with expired provider credentials: local
      // revocation must neither need a refresh nor treat provider failure as fatal.
      const owner = `owner-${crypto.randomUUID()}`;
      const caller = await create(owner);
      const other = await create(owner);
      const foreign = await create(`foreign-${crypto.randomUUID()}`);
      const target = route === "other-session" ? other : caller;
      const path = route === "current-session" || route === "other-session" ? `sessions/${target.hash}/revoke` : route;
      const operation = route === "revoke-all" ? "/revoke-all" : "/revoke";
      const provider = vi.fn(async () => { throw new Error("provider unavailable"); });
      vi.stubGlobal("fetch", provider);
      const calls: string[] = [];
      const bindings = config();
      // Forward validation to the actual DO. Fail only its revocation response;
      // do not replace route helpers or synthesize route-level success.
      bindings.BROWSER_SESSIONS = {
        idFromName: (name: string) => env.BROWSER_SESSIONS.idFromName(name),
        get: (id: DurableObjectId) => ({ fetch: async (input: Request) => {
          const called = new URL(input.url).pathname;
          calls.push(called);
          return called === operation ? response() : env.BROWSER_SESSIONS.get(id).fetch(input);
        } })
      } as unknown as DurableObjectNamespace;
      const failedRequest = request(path, caller);
      const failed = (await handleBrowserRoute(failedRequest, bindings, new URL(failedRequest.url)))!;
      expect(failed.status).toBe(503);
      expect(await failed.json()).toEqual({ error: "revocation_unavailable" });
      expect(failed.headers.has("set-cookie")).toBe(false);
      expect(failed.headers.get("cache-control")).toBe("no-store");
      expect(calls).toEqual(["/validate", operation]);
      expect(provider).not.toHaveBeenCalled();
      expect(await authenticated(caller.hash)).toBe(true);
      expect(await authenticated(other.hash)).toBe(true);

      // Retry against the unmodified actual session-store binding, observing its
      // public validate interface after the acknowledged durable mutation.
      const retryRequest = request(path, caller);
      const retry = (await handleBrowserRoute(retryRequest, config(), new URL(retryRequest.url)))!;
      expect(retry.status).toBe(200);
      expect(await retry.json()).toEqual({ ok: true });
      expect(retry.headers.has("set-cookie")).toBe(route !== "other-session");
      if (route !== "other-session") expect(retry.headers.get("set-cookie")).toContain("Max-Age=0");
      expect(await authenticated(caller.hash)).toBe(route === "other-session");
      expect(await authenticated(other.hash)).toBe(route !== "other-session" && route !== "revoke-all");
      expect(await authenticated(foreign.hash)).toBe(true);
      expect(provider).toHaveBeenCalledTimes(route === "logout" ? 1 : 0);
    });
  }
});
