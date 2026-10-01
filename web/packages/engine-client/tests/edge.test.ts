import { afterEach, describe, expect, test, vi } from "vitest";
import { browserLogout, fetchBrowserSession, relayDeviceUrl } from "../src/edge";

afterEach(() => vi.unstubAllGlobals());

describe("canonical browser API boundary", () => {
  test("reports a missing backend instead of parsing SPA HTML as a session", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("<!doctype html><html></html>", {
      headers: { "content-type": "text/html" },
    })));
    await expect(fetchBrowserSession()).rejects.toThrow(/browser.*JSON.*backend/i);
  });

  test("accepts an authenticated canonical session without optional profile data", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({
      authenticated: true, ownerId: "user_one", csrfToken: "csrf-one",
    })));
    expect(await fetchBrowserSession()).toMatchObject({ authenticated: true, ownerId: "user_one" });
  });

  test("rejects an authenticated response without its required owner and CSRF fields", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ authenticated: true })));
    await expect(fetchBrowserSession()).rejects.toThrow(/incomplete browser session/i);
  });

  test.each([401, 403, 500])("logout rejects HTTP %s rather than reporting success", async (status) => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ error: "revocation_failed" }, { status })));
    await expect(browserLogout("csrf-one")).rejects.toMatchObject({ status });
  });

  test("missing logout CSRF is an explicit failure without a revocation request", async () => {
    const request = vi.fn();
    vi.stubGlobal("fetch", request);
    await expect(browserLogout("")).rejects.toThrow(/CSRF/i);
    expect(request).not.toHaveBeenCalled();
  });

  test("acknowledged logout and network failure remain distinct", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ ok: true })));
    await expect(browserLogout("csrf-one")).resolves.toBeUndefined();
    vi.stubGlobal("fetch", vi.fn(async () => { throw new Error("network unavailable"); }));
    await expect(browserLogout("csrf-one")).rejects.toThrow("network unavailable");
  });
  test("canonical device IDs include underscores and use the current browser origin", () => {
    vi.stubGlobal("window", { location: { protocol: "https:", host: "candidate.example.test" } });
    expect(relayDeviceUrl("engine_one-2")).toBe("wss://candidate.example.test/api/browser/device/engine_one-2/ws");
    expect(() => relayDeviceUrl("../engine")).toThrow("Invalid remote device id");
  });
});
