import { previewRoute } from "../../src/preview-route";
import { browserDeviceRoute, handleBrowserRoute } from "../../src/browser-routes";
import type { Env } from "../../src/env";
export { ChatRoom } from "../../src/chat-room";
export { PreviewRoom } from "../../src/preview-room";
export { RegistryRoom } from "../../src/registry-room";
export { DeviceRoom } from "../../src/device-room";
export { BrowserSessionStore } from "../../src/browser-sessions";
import { DurableObject } from "cloudflare:workers";

/** Bare SQLite-backed DO; tests reach its real `ctx.storage.sql` via
 * `runInDurableObject` (the cloudflare-os TEST_OVERSEER pattern). */
export class TestLogRoom extends DurableObject {}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const browser = await browserDeviceRoute(request, env, url) ?? await handleBrowserRoute(request, env, url);
    if (browser) return browser;
    // Test credentials exercise the production routing seam without loading
    // the unrelated session-room WASM inside the Workers test runner.
    const bearer = request.headers.get("authorization");
    if (!bearer?.startsWith("Bearer ")) return new Response("Unauthorized", { status: 401 });
    const [userId, orgId] = bearer.slice(7).split("@");
    return previewRoute(request, env, { userId, orgId }) ?? new Response("test fixture", { status: 404 });
  }
};
