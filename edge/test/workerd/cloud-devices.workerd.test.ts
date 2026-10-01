import { SELF, env, runInDurableObject } from "cloudflare:test";
import { SignJWT, createLocalJWKSet, generateKeyPair, jwtVerify } from "jose";
import { describe, expect, it } from "vitest";
import { FAIL_LIMIT } from "../../src/device-credentials";
import { signDeviceToken } from "../../src/device-tokens";

const base = "https://edge.test";

const enroll = async (bearer: string, name?: string) => {
  const response = await SELF.fetch(`${base}/cloud/devices`, {
    method: "POST",
    headers: { authorization: `Bearer ${bearer}` },
    body: JSON.stringify(name === undefined ? {} : { name })
  });
  return { status: response.status, body: (await response.json()) as Record<string, any> };
};

const exchange = async (deviceId: string, credential: string) => {
  const response = await SELF.fetch(`${base}/auth/device-token`, {
    method: "POST",
    body: JSON.stringify({ deviceId, credential })
  });
  return { response, body: (await response.json()) as Record<string, any> };
};

const enrolled = async (user = crypto.randomUUID(), org = "org1") => {
  const minted = await enroll(`${user}@${org}`, "Box");
  expect(minted.status).toBe(200);
  const { deviceId, credential } = minted.body as { deviceId: string; credential: string };
  const { body } = await exchange(deviceId, credential);
  return { user, org, deviceId, credential, token: body.token as string };
};

const get = (path: string, token: string, init: RequestInit = {}) =>
  SELF.fetch(`${base}${path}`, {
    ...init,
    headers: { authorization: `Bearer ${token}`, ...(init.headers as Record<string, string>) }
  });

const hostJoin = (deviceId: string, token: string) =>
  SELF.fetch(`${base}/device/${deviceId}/ws?role=host&token=${token}`, {
    headers: { upgrade: "websocket" }
  });

describe("cloud-box enrollment", () => {
  it("publishes the public key only", async () => {
    const response = await SELF.fetch(`${base}/.well-known/zeron-device-jwks.json`);
    expect(response.status).toBe(200);
    const { keys } = (await response.json()) as { keys: Array<Record<string, string>> };
    expect(keys).toHaveLength(1);
    expect(keys[0]).toMatchObject({ kty: "EC", crv: "P-256", alg: "ES256", use: "sig" });
    expect(keys[0]!.kid).toBeTruthy();
    expect(keys[0]!.d).toBeUndefined();
  });

  it("mints once, stores only a hash, and exchanges for a signed 30-minute token", async () => {
    expect((await enroll("solo")).status).toBe(403); // no organization
    expect(
      (await SELF.fetch(`${base}/cloud/devices`, { method: "POST", body: "{}" })).status
    ).toBe(401);

    const user = crypto.randomUUID();
    const minted = await enroll(`${user}@org1`, "  My box ");
    expect(minted.status).toBe(200);
    const { deviceId, credential } = minted.body;
    expect(Object.keys(minted.body).sort()).toEqual(["credential", "deviceId"]);
    expect(deviceId).toMatch(/^[0-9a-f-]{36}$/);
    expect(credential).toMatch(/^[A-Za-z0-9_-]{43}$/);

    const room = env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(`d2/${deviceId}`));
    await runInDurableObject(room, (_instance, state) => {
      const rows = [...state.storage.sql.exec("SELECT * FROM device_credential")];
      expect(rows).toHaveLength(1);
      expect(rows[0]).toMatchObject({ user_id: user, org_id: "org1", name: "My box", revoked_at: null });
      expect(JSON.stringify(rows)).not.toContain(credential);
      const owner = [...state.storage.sql.exec("SELECT value FROM meta WHERE key = 'owner'")];
      expect(owner[0]?.value).toBe(user);
    });

    const before = Date.now();
    const { response, body } = await exchange(deviceId, credential);
    expect(response.status).toBe(200);
    const jwks = (await (await SELF.fetch(`${base}/.well-known/zeron-device-jwks.json`)).json()) as {
      keys: [];
    };
    const { payload, protectedHeader } = await jwtVerify(body.token, createLocalJWKSet(jwks), {
      issuer: "zeron-edge"
    });
    expect(protectedHeader.alg).toBe("ES256");
    expect(payload).toMatchObject({ sub: user, org_id: "org1", did: deviceId, kind: "cloud" });
    expect(payload.exp! - payload.iat!).toBe(30 * 60);
    expect(body.expiresAt).toBe(payload.exp! * 1000);
    expect(body.expiresAt).toBeGreaterThan(before + 29 * 60_000);
  });

  it("rejects wrong and unknown credentials, then rate-limits the device", async () => {
    const { deviceId, credential } = await enrolled();
    expect((await exchange(deviceId, "nope")).response.status).toBe(401);
    expect((await exchange(deviceId, "nope")).body.error).toBe("invalid_credential");
    expect((await exchange(crypto.randomUUID(), credential)).response.status).toBe(401);
    const bad = await SELF.fetch(`${base}/auth/device-token`, {
      method: "POST",
      body: JSON.stringify({ deviceId: "../x", credential })
    });
    expect(bad.status).toBe(400);

    // A success resets the count; FAIL_LIMIT straight failures lock it.
    expect((await exchange(deviceId, credential)).response.status).toBe(200);
    for (let i = 0; i < FAIL_LIMIT; i++) {
      expect((await exchange(deviceId, `wrong-${i}`)).response.status).toBe(401);
    }
    const locked = await exchange(deviceId, credential);
    expect(locked.response.status).toBe(429);
    expect(Number(locked.response.headers.get("retry-after"))).toBeGreaterThan(0);
    expect(locked.body.error).toBe("rate_limited");
  });

  it("revokes for the owner only, refusing new tokens and host joins", async () => {
    const box = await enrolled();
    // Live host socket, joined with the device token.
    const joined = await hostJoin(box.deviceId, box.token);
    expect(joined.status).toBe(101);
    const ws = joined.webSocket!;
    ws.accept();
    const closed = new Promise<number>((resolve) =>
      ws.addEventListener("close", (event) => resolve(event.code), { once: true })
    );

    const del = (bearer: string) =>
      SELF.fetch(`${base}/cloud/devices/${box.deviceId}`, {
        method: "DELETE",
        headers: { authorization: `Bearer ${bearer}` }
      });
    expect((await del("someone-else@org1")).status).toBe(403);
    expect((await del(box.token)).status).toBe(403); // a box can't revoke itself
    expect(
      (await SELF.fetch(`${base}/cloud/devices/${crypto.randomUUID()}`, {
        method: "DELETE",
        headers: { authorization: `Bearer ${box.user}@org1` }
      })).status
    ).toBe(404);
    const revoked = await del(`${box.user}@org1`);
    expect(revoked.status).toBe(200);
    expect(((await revoked.json()) as { revokedAt: number }).revokedAt).toBeGreaterThan(0);
    expect((await del(`${box.user}@org1`)).status).toBe(200); // idempotent

    expect(await closed).toBe(4403);
    const after = await exchange(box.deviceId, box.credential);
    expect(after.response.status).toBe(401);
    expect(after.body.error).toBe("revoked");
    // The still-unexpired token can no longer host the room.
    expect((await hostJoin(box.deviceId, box.token)).status).toBe(403);
  });
});

describe("device-token scope", () => {
  it("maps to the owner's identity on allowed routes", async () => {
    const box = await enrolled();
    for (const path of [
      "/registry/org1/rows",
      "/registry/org1/stats",
      "/chat2/chat-1/checkpoint",
      "/chat2/chat-1/tail",
      `/device/${crypto.randomUUID()}/status`,
      `/device/${box.deviceId}/sidecar/repos`,
      "/blob/chat-1/part-1"
    ]) {
      const response = await get(path, box.token);
      expect(response.status, path).toBe(200);
      expect(await response.json()).toEqual({ userId: box.user, orgId: "org1", deviceId: box.deviceId });
    }
    const pushed = await get("/registry/org1/push", box.token, { method: "POST", body: "{}" });
    expect(pushed.status).toBe(200);
    const host = await hostJoin(box.deviceId, box.token);
    expect(host.status).toBe(101);
    host.webSocket!.accept();
    host.webSocket!.close();
  });

  it("forbids everything else", async () => {
    const box = await enrolled();
    const other = crypto.randomUUID();
    const forbidden: Array<[string, RequestInit?]> = [
      [`/device/${other}/ws?role=host`, { headers: { upgrade: "websocket" } }],
      [`/device/${box.deviceId}/ws?role=client`, { headers: { upgrade: "websocket" } }],
      [`/device/${other}/sidecar/repos`],
      ["/registry/org1/reset", { method: "POST" }],
      ["/registry/org1/push-target", { method: "POST" }],
      ["/chat2/chat-1/reset", { method: "POST" }],
      ["/workspace/org1/ws"],
      ["/session/chat-1/ws"],
      ["/snapshot/chat-1"],
      ["/cloud/devices", { method: "POST", body: "{}" }]
    ];
    for (const [path, init] of forbidden) {
      expect((await get(path, box.token, init)).status, path).toBe(403);
    }
  });

  it("fails closed on forged, tampered, or expired tokens", async () => {
    const box = await enrolled();
    const { privateKey } = await generateKeyPair("ES256");
    const forged = await new SignJWT({ org_id: "org1", did: box.deviceId, kind: "cloud" })
      .setProtectedHeader({ alg: "ES256" })
      .setIssuer("zeron-edge")
      .setSubject(box.user)
      .setIssuedAt()
      .setExpirationTime("10m")
      .sign(privateKey);
    // In dev auth mode a bare bearer is a user id; a token naming our issuer
    // must never be reinterpreted that way.
    expect((await get("/registry/org1/rows", forged)).status).toBe(401);
    const [h, p, s] = box.token.split(".");
    const claims = JSON.parse(atob(p!.replaceAll("-", "+").replaceAll("_", "/")));
    const tampered = `${h}.${btoa(JSON.stringify({ ...claims, sub: "victim" }))
      .replaceAll("+", "-")
      .replaceAll("/", "_")
      .replace(/=+$/, "")}.${s}`;
    expect((await get("/registry/org1/rows", tampered)).status).toBe(401);
    const expired = await signDeviceToken(
      env,
      { userId: box.user, orgId: "org1", deviceId: box.deviceId },
      Date.now() - 31 * 60_000
    );
    expect((await get("/registry/org1/rows", expired.token)).status).toBe(401);
  });
});
