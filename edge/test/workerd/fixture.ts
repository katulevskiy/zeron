import { previewRoute } from "../../src/preview-route";
import { authenticate } from "../../src/auth";
import { deviceTokenScope, handleCloudRoute } from "../../src/cloud-devices";
import { forward } from "../../src/forward";
import type { Env } from "../../src/env";
export { DeviceRoom } from "../../src/device-room";
export { ChatRoom } from "../../src/chat-room";
export { PreviewRoom } from "../../src/preview-room";
export { RegistryRoom } from "../../src/registry-room";
import { DurableObject } from "cloudflare:workers";

/** Bare SQLite-backed DO; tests reach its real `ctx.storage.sql` via
 * `runInDurableObject` (the cloudflare-os TEST_OVERSEER pattern). */
export class TestLogRoom extends DurableObject {}

export default {
  // The production routing seams (enrollment, dev-mode/device-token auth, the
  // device-token scope gate, preview + device-room forwarding) without
  // loading index.ts, whose session-room WASM cannot run in this pool.
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const cloud = await handleCloudRoute(request, env, url);
    if (cloud) return cloud;
    const auth = await authenticate(env, request);
    if (!auth) return new Response("Unauthorized", { status: 401 });
    const outOfScope = deviceTokenScope(auth, request, url);
    if (outOfScope) return outOfScope;
    const preview = previewRoute(request, env, auth);
    if (preview) return preview;
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] === "device" && parts[1] && parts[2] === "ws") {
      const role = url.searchParams.get("role") === "host" ? "host" : "client";
      return forward(env.DEVICE_ROOMS, `d2/${parts[1]}`, request, auth, "/ws", `?role=${role}`);
    }
    // Any other in-scope route: echo the identity the Worker would stamp.
    return Response.json({ userId: auth.userId, orgId: auth.orgId, deviceId: auth.deviceId });
  }
};
