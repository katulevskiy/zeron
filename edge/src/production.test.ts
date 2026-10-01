import { describe, expect, it, vi } from "vitest";
import production from "./production";
import type { Env } from "./env";
// Wrangler imports shell scripts as text; mirror that loader in Node tests.
vi.mock("./install.sh", () => ({ default: "installer fixture" }));
const origin = "https://candidate.test";
const config = () => ({ WORKOS_BROWSER_ORIGIN: origin, ASSETS: { fetch: vi.fn(async (request: Request) => new Response(new URL(request.url).pathname)) } }) as unknown as Env & { ASSETS: Fetcher };
describe("React candidate routing", () => {
  it("serves deep links and Vite assets, but never masks browser API/configuration errors", async () => {
    const env = config();
    expect(await (await production.fetch(new Request(`${origin}/settings/accounts`, { headers: { accept: "text/html" } }), env)).text()).toBe("/index.html");
    expect(await (await production.fetch(new Request(`${origin}/assets/app-abc.js`), env)).text()).toBe("/assets/app-abc.js");
    const api = await production.fetch(new Request(`${origin}/api/browser/session`), env);
    expect(api.status).toBe(501);
    expect(await api.json()).toEqual({ error: "browser_auth_not_configured" });
    expect(env.ASSETS.fetch).toHaveBeenCalledTimes(2);
  });
  it("serves declared Vite public artwork, icons and sounds only at the configured app origin", async () => {
    const env = config();
    for (const path of ["/backgrounds/default-new-thread-background.png", "/file-icons/dark/files/code-pink.svg", "/sounds/message.wav", "/favicon.svg", "/favicon.png", "/apple-touch-icon.png"]) {
      expect(await (await production.fetch(new Request(`${origin}${path}`), env)).text()).toBe(path);
    }
    expect(env.ASSETS.fetch).toHaveBeenCalledTimes(6);
    vi.mocked(env.ASSETS.fetch).mockClear();
    for (const request of [
      new Request("https://wrong-origin.test/backgrounds/default-new-thread-background.png"),
      new Request(`${origin}/backgrounds/default-new-thread-background.png`, { method: "POST" }),
      new Request(`${origin}/file-icons/dark/files/code-pink.svg`, { headers: { upgrade: "websocket" } }),
    ]) {
      expect((await production.fetch(request, env)).status).toBeGreaterThanOrEqual(400);
    }
    expect(env.ASSETS.fetch).not.toHaveBeenCalled();
  });
  it("keeps every upstream native HTTP route out of document fallback", async () => {
    const env = config();
    for (const path of ["tail/id", "stats/id", "diff/id", "snapshot/id", "append/id", "chat2/id", "blob/id/part", "device/id/status", "api/unknown"]) {
      const response = await production.fetch(new Request(`${origin}/${path}`, { headers: { accept: "text/html" } }), env);
      expect(response.status).toBe(401);
      expect(response.headers.get("content-type")).toContain("application/json");
    }
    expect(env.ASSETS.fetch).not.toHaveBeenCalled();
  });
});
