/**
 * Cloud-box enrollment (docs/cloud.md §6):
 *
 *  - POST   /cloud/devices            — WorkOS user mints `{deviceId, credential}`
 *  - DELETE /cloud/devices/:deviceId  — the owner revokes it
 *  - POST   /auth/device-token        — `{deviceId, credential}` → `{token, expiresAt}`
 *  - GET    /.well-known/zeron-device-jwks.json — the device-token public key
 *
 * Credential records live in the device's own DeviceRoom (`d2/{deviceId}`,
 * see device-credentials.ts); the Worker reaches them on internal
 * `/credential/*` paths that no public route forwards to.
 *
 * `deviceTokenScope` is the route gate for device tokens: a box may host its
 * own device room and use the owner's sync surface (registry, chat rooms,
 * tool-output blobs, preview signaling, and client dials to the owner's other
 * devices, which moving a chat back off the box needs), never manage
 * credentials, orgs, or operator resets, nor host any other device.
 */
import { authenticate, type Verified } from "./auth";
import type { CredentialRecord, VerifyOutcome } from "./device-credentials";
import { newCredential } from "./device-credentials";
import { deviceJwks, deviceTokensConfigured, signDeviceToken } from "./device-tokens";
import { AUTH_USER_HEADER, type Env } from "./env";

const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;
const MAX_NAME_LEN = 80;
const DEFAULT_NAME = "Cloud";

const json = (value: unknown, status = 200, headers?: Record<string, string>): Response =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", ...headers }
  });

const notConfigured = (): Response => json({ error: "device credentials not configured" }, 501);

const bodyJson = async <T>(request: Request): Promise<T | undefined> => {
  try {
    return (await request.json()) as T;
  } catch {
    return undefined;
  }
};

type CloudEnv = Pick<Env, "DEVICE_ROOMS" | "AUTH_MODE" | "WORKOS_CLIENT_ID" | "WORKOS_ISSUER" | "WORKOS_JWKS_URL" | "ZERON_DEVICE_JWT_KEY">;

/** The credential record's DO — the device's own room (`d2/` as in index.ts). */
const credentialRoom = (env: CloudEnv, deviceId: string, path: string, init: RequestInit) => {
  const ns = env.DEVICE_ROOMS;
  return ns.get(ns.idFromName(`d2/${deviceId}`)).fetch(`https://device-room/credential/${path}`, {
    method: "POST",
    ...init
  });
};

/** Handle an enrollment route; undefined means "not one of ours". */
export const handleCloudRoute = async (
  request: Request,
  env: Env,
  url: URL
): Promise<Response | undefined> => {
  const parts = url.pathname.split("/").filter(Boolean);

  if (url.pathname === "/.well-known/zeron-device-jwks.json" && request.method === "GET") {
    return json(await deviceJwks(env), 200, { "cache-control": "public, max-age=300" });
  }

  if (url.pathname === "/auth/device-token" && request.method === "POST") {
    if (!deviceTokensConfigured(env)) return notConfigured();
    const body = await bodyJson<{ deviceId?: unknown; credential?: unknown }>(request);
    if (
      typeof body?.deviceId !== "string" ||
      !ID_RE.test(body.deviceId) ||
      typeof body.credential !== "string"
    ) {
      return json({ error: "missing deviceId or credential" }, 400);
    }
    const deviceId = body.deviceId;
    const verdict = await credentialRoom(env, deviceId, "verify", {
      body: JSON.stringify({ credential: body.credential })
    });
    const outcome = (await verdict.json().catch(() => undefined)) as VerifyOutcome | undefined;
    if (!outcome) return json({ error: "temporarily unavailable" }, 503);
    if (!outcome.ok) {
      console.warn(
        "auth/device-token rejected",
        deviceId,
        outcome.error,
        request.headers.get("cf-connecting-ip") ?? "unknown-ip"
      );
      if (outcome.error === "rate_limited") {
        const retryAfter = Math.max(1, Math.ceil(outcome.retryAfterMs / 1000));
        return json({ error: "rate_limited", retryAfter }, 429, { "retry-after": String(retryAfter) });
      }
      return json({ error: outcome.error }, 401);
    }
    const record: CredentialRecord = outcome.record;
    return json(
      await signDeviceToken(env, { userId: record.userId, orgId: record.orgId, deviceId })
    );
  }

  if (parts[0] === "cloud" && parts[1] === "devices") {
    if (!deviceTokensConfigured(env)) return notConfigured();
    const auth = await authenticate(env, request);
    if (!auth) return json({ error: "unauthenticated" }, 401);
    // Boxes never enroll or revoke boxes.
    if (auth.deviceId) return json({ error: "forbidden" }, 403);

    if (parts.length === 2 && request.method === "POST") {
      if (!auth.orgId) return json({ error: "organization required" }, 403);
      const body = (await bodyJson<{ name?: unknown }>(request)) ?? {};
      if (body.name !== undefined && typeof body.name !== "string") {
        return json({ error: "name must be a string" }, 400);
      }
      const name = (body.name ?? "").trim() || DEFAULT_NAME;
      if (name.length > MAX_NAME_LEN) {
        return json({ error: `name must be 1-${MAX_NAME_LEN} characters` }, 400);
      }
      const deviceId = crypto.randomUUID();
      const credential = newCredential();
      const created = await credentialRoom(env, deviceId, "create", {
        headers: { [AUTH_USER_HEADER]: auth.userId },
        body: JSON.stringify({ credential, orgId: auth.orgId, name })
      });
      if (!created.ok) return json({ error: "enrollment failed" }, 500);
      return json({ deviceId, credential });
    }

    if (parts.length === 3 && request.method === "DELETE") {
      const deviceId = parts[2]!;
      if (!ID_RE.test(deviceId)) return json({ error: "not_found" }, 404);
      const revoked = await credentialRoom(env, deviceId, "revoke", {
        headers: { [AUTH_USER_HEADER]: auth.userId }
      });
      return new Response(revoked.body, {
        status: revoked.status,
        headers: { "content-type": "application/json" }
      });
    }
    return json({ error: "not_found" }, 404);
  }

  return undefined;
};

const REGISTRY_ROUTES = new Set(["ws", "rows", "push", "stats"]);
const CHAT_ROUTES = new Set(["ws", "checkpoint", "rows", "tail", "diff", "stats"]);

/** `undefined` = allowed (or not a device token); otherwise the 403 to send. */
export const deviceTokenScope = (
  auth: Verified,
  request: Request,
  url: URL
): Response | undefined => {
  const did = auth.deviceId;
  if (did === undefined) return undefined;
  const parts = url.pathname.split("/").filter(Boolean);
  const [head, id, sub] = parts;
  const allowed = ((): boolean => {
    switch (head) {
      case "device": {
        if (!id) return false;
        if (sub === "ws" && parts.length === 3) {
          const host = url.searchParams.get("role") === "host";
          // Host only its own room; dial the owner's other devices as a client.
          return host ? id === did : id !== did;
        }
        if (sub === "sidecar") return id === did;
        return (sub === "status" || sub === "nudge") && parts.length === 3;
      }
      case "registry":
        return parts.length === 3 && REGISTRY_ROUTES.has(sub ?? "");
      case "chat2":
        return parts.length === 3 && CHAT_ROUTES.has(sub ?? "");
      case "blob":
        return true;
      case "preview":
        return parts.length === 3 && sub === "ws";
      // The checkout diff publisher still posts the legacy per-chat slot.
      case "diff":
        return parts.length === 2 && request.method === "POST";
      default:
        return false;
    }
  })();
  return allowed ? undefined : json({ error: "forbidden", reason: "device token" }, 403);
};
