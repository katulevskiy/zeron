import { afterEach, describe, expect, it, vi } from "vitest";
import { browserDeviceRoute, handleBrowserRoute, deviceRoomOnline, previewOrigin, rewritePreviewLocation } from "./browser-routes";
import type { Env } from "./env";

describe("browser preview origin and redirects", () => {
  it("only permits HTTPS preview origins for the Secure host-only capability cookie", () => {
    expect(previewOrigin({ BROWSER_PREVIEW_ORIGIN: "https://preview.example/" })?.toString()).toBe("https://preview.example/");
    for (const value of ["http://localhost:8787/", "https://preview.example/path", "https://preview.example/?x=1"]) {
      expect(previewOrigin({ BROWSER_PREVIEW_ORIGIN: value })).toBeUndefined();
    }
  });


  it("keeps controlled localhost aliases inside the capability origin", () => {
    const current = new URL("https://p-ticket.preview.example/app/");
    expect(rewritePreviewLocation("http://device.project.localhost:5173/path?q=1#part", current)).toBe("https://p-ticket.preview.example/path?q=1#part");
    expect(rewritePreviewLocation("https://example.com/path?q=1#part", current)).toBe("https://example.com/path?q=1#part");
  });
});


describe("browser device discovery", () => {
  it("treats an individual Durable Object status failure as offline", async () => {
    const failing = { fetch: async () => { throw new Error("stale room"); } } as unknown as DurableObjectStub;
    const online = { fetch: async () => new Response(JSON.stringify({ hostConnected: true })) } as unknown as DurableObjectStub;
    await expect(deviceRoomOnline(failing, "owner")).resolves.toBe(false);
    await expect(deviceRoomOnline(online, "owner")).resolves.toBe(true);
  });
});

describe("browser configuration fails closed", () => {
  const configured = () => ({ WORKOS_CLIENT_ID: "client_test", WORKOS_API_KEY: "test", WORKOS_BROWSER_ORIGIN: "https://test", WORKOS_ISSUER: "https://issuer.test", WORKOS_JWKS_URL: "https://issuer.test/jwks", BROWSER_SESSION_KEY: "test", BROWSER_SESSIONS: {} }) as Env;
  it("rejects missing client or durable session binding before login or upgrade", async () => {
    for (const key of ["WORKOS_CLIENT_ID", "BROWSER_SESSIONS", "WORKOS_API_KEY", "WORKOS_BROWSER_ORIGIN", "WORKOS_ISSUER", "WORKOS_JWKS_URL", "BROWSER_SESSION_KEY"] as const) {
      const env = configured();
      delete (env as unknown as Record<string, unknown>)[key];
      for (const path of ["login", "device/owned/ws"]) {
        const url = new URL(`https://test/api/browser/${path}`);
        const request = new Request(url, { method: path === "login" ? "POST" : "GET", headers: { origin: "https://test", upgrade: "websocket", cookie: "__Host-comet_session=forged" } });
        const response = path === "login" ? await handleBrowserRoute(request, env, url) : await browserDeviceRoute(request, env, url);
        expect(response?.status).toBe(501);
        expect(await response?.json()).toEqual({ error: "browser_auth_not_configured" });
      }
    }
  });
});

// Exercise the real routes and session helper across the DO fetch boundary.
// Validation succeeds; only the durable revocation operation is unavailable.
describe("browser revocation acknowledgement", () => {
  afterEach(() => vi.restoreAllMocks());
  it.each(["logout", "revoke-all", `sessions/${"a".repeat(64)}/revoke`])(
    "%s does not acknowledge a non-successful durable store response",
    async (path) => {
      const provider = vi.spyOn(globalThis, "fetch").mockResolvedValue(Response.json({}, { status: 503 }));
      const requests: string[] = [];
      const bindings = {
        WORKOS_CLIENT_ID: "client_test", WORKOS_API_KEY: "test",
        WORKOS_BROWSER_ORIGIN: "https://test", WORKOS_ISSUER: "https://issuer.test",
        WORKOS_JWKS_URL: "https://issuer.test/jwks", BROWSER_SESSION_KEY: "test",
        BROWSER_SESSIONS: {
          idFromName: (name: string) => name,
          get: () => ({ fetch: async (request: Request) => {
            const operation = new URL(request.url).pathname;
            requests.push(operation);
            if (operation === "/validate") {
              return Response.json({ authenticated: true, ownerId: "owner", providerSessionId: "provider",
                csrfToken: "csrf", expiresAt: Date.now() + 60_000, generation: 1 });
            }
            return Response.json({ ok: true }, { status: 503 });
          } })
        }
      } as unknown as Env;
      const url = new URL(`https://test/api/browser/${path}`);
      const response = await handleBrowserRoute(new Request(url, { method: "POST",
        headers: { origin: "https://test", cookie: "__Host-comet_session=test-cookie", "x-csrf-token": "csrf" }
      }), bindings, url);
      expect(response?.status).toBe(503);
      expect(response?.headers.has("set-cookie")).toBe(false);
      expect(await response?.json()).toEqual({ error: "revocation_unavailable" });
      expect(requests).toEqual(["/validate", path === "revoke-all" ? "/revoke-all" : "/revoke"]);
      expect(provider).not.toHaveBeenCalled();
    }
  );
});
