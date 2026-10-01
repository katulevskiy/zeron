import { env as testEnv } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { browserDeviceRoute, handleBrowserRoute } from "../../src/browser-routes";
import { registerBrowserDevice, SESSION_COOKIE, tokenHash } from "../../src/browser-sessions";
import { AUTH_USER_HEADER, type Env } from "../../src/env";

const env = testEnv;

// Explicit production inputs: no single-user subject and no development login.
const config = (): Env => ({
  BROWSER_SESSIONS: env.BROWSER_SESSIONS,
  DEVICE_ROOMS: env.DEVICE_ROOMS,
  AUTH_MODE: "workos",
  WORKOS_BROWSER_ORIGIN: "https://test",
  WORKOS_CLIENT_ID: "client_test",
  WORKOS_API_KEY: "test-only",
  WORKOS_ISSUER: "https://multi-user.test",
  WORKOS_JWKS_URL: `https://multi-user.test/${crypto.randomUUID()}/jwks`,
  BROWSER_SESSION_KEY: "workerd-test-session-key"
}) as Env;

const call = async (bindings: Env, path: string, cookie = "", method = "GET", csrf = "") => {
  const url = new URL(`https://test/api/browser/${path}`);
  return (await handleBrowserRoute(new Request(url, {
    method, headers: { cookie, origin: "https://test", "x-csrf-token": csrf }
  }), bindings, url))!;
};

afterEach(() => vi.unstubAllGlobals());

describe("multi-user browser authentication", () => {
  it("isolates owners in the same and other organizations, sessions, revocation, and device connections", async () => {
    const bindings = config();
    const pair = await generateKeyPair("RS256");
    const jwk = await exportJWK(pair.publicKey);
    const replies: unknown[] = [];
    let challenge = "";
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url === bindings.WORKOS_JWKS_URL) return Response.json({ keys: [{ ...jwk, kid: "multi-user" }] });
      expect(url).toBe("https://api.workos.com/user_management/authenticate");
      const value = JSON.parse(init!.body as string) as { code_verifier: string; grant_type: string };
      expect(value.grant_type).toBe("authorization_code");
      expect(value.code_verifier).toMatch(/^[A-Za-z0-9._~-]{43,128}$/);
      const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value.code_verifier)));
      expect(btoa(String.fromCharCode(...digest)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")).toBe(challenge);
      expect(replies.length).toBeGreaterThan(0);
      return Response.json(replies.shift());
    }));

    const login = async (owner: string, responseOwner = owner, organization = "org-shared") => {
      const started = await call(bindings, "login", "", "POST");
      expect(started.status).toBe(200);
      const authorization = new URL((await started.json() as { authorizationUrl: string }).authorizationUrl);
      expect(authorization.searchParams.get("code_challenge_method")).toBe("S256");
      challenge = authorization.searchParams.get("code_challenge")!;
      const transactionCookie = started.headers.get("set-cookie")!.split(";")[0]!;
      const jwt = await new SignJWT({ sid: `sid-${owner}`, org_id: organization, auth_time: Math.floor(Date.now() / 1000) })
        .setProtectedHeader({ alg: "RS256", kid: "multi-user" }).setSubject(owner)
        .setIssuer(bindings.WORKOS_ISSUER!).setExpirationTime("1h").sign(pair.privateKey);
      replies.push({ user: { id: responseOwner, email: `${owner}@example.test`, first_name: "Fixture", last_name: owner, profile_picture_url: `https://images.example.test/${owner}.png`, metadata: { private: "not-public" } }, access_token: jwt, refresh_token: `refresh-${owner}` });
      const path = `callback?code=code&state=${authorization.searchParams.get("state")}`;
      const response = await call(bindings, path, transactionCookie);
      expect((await call(bindings, path, transactionCookie)).status).toBe(401);
      return response;
    };

    const users = [];
    for (const [index, owner] of [`alice-${crypto.randomUUID()}`, `bob-${crypto.randomUUID()}`, `charlie-${crypto.randomUUID()}`].entries()) {
      const response = await login(owner, owner, index < 2 ? "org-shared" : "org-other");
      expect(response.status).toBe(303);
      const setCookie = response.headers.get("set-cookie")!;
      expect(setCookie).toContain("Secure; HttpOnly; Path=/; SameSite=Strict");
      const cookie = `${SESSION_COOKIE}=${setCookie.match(/__Host-comet_session=([^;]+)/)![1]}`;
      const session = await (await call(bindings, "session", cookie)).json() as { ownerId: string; csrfToken: string };
      expect(session).toMatchObject({ authenticated: true, ownerId: owner, profile: { email: `${owner}@example.test`, firstName: "Fixture", lastName: owner, avatarUrl: `https://images.example.test/${owner}.png` } });
      expect(Object.keys(session).sort()).toEqual(["authenticated", "csrfToken", "expiresAt", "organizationId", "ownerId", "profile"]);
      expect(await (await call(bindings, `session?ownerId=someone-else`, cookie)).json()).toEqual(session);
      users.push({ owner, cookie, csrf: session.csrfToken, hash: await tokenHash(cookie.slice(cookie.indexOf("=") + 1)), device: `device-${crypto.randomUUID()}` });
    }
    expect((await login("signed-user", "different-response-user")).status).toBe(403);

    for (const user of users) {
      const room = env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(`d2/${user.device}`));
      const host = await room.fetch(new Request("https://device/ws?role=host&connId=host", { headers: { upgrade: "websocket", [AUTH_USER_HEADER]: user.owner } }));
      expect(host.status).toBe(101);
      host.webSocket!.accept();
      await registerBrowserDevice(bindings, user.owner, user.device);
    }
    for (const [index, user] of users.entries()) {
      const other = users[(index + 1) % users.length]!;
      const sessions = await (await call(bindings, `sessions?ownerId=${other.owner}`, user.cookie)).json() as { sessions: { hash: string }[] };
      expect(sessions.sessions.map(({ hash }) => hash)).toEqual([user.hash]);
      expect(await (await call(bindings, `devices?ownerId=${other.owner}`, user.cookie)).json()).toEqual({ devices: [{ id: user.device, online: true }] });
      expect((await call(bindings, `sessions/${other.hash}/revoke`, user.cookie, "POST", user.csrf)).status).toBe(200);
      expect(await (await call(bindings, "session", other.cookie)).json()).toMatchObject({ authenticated: true, ownerId: other.owner, profile: { email: `${other.owner}@example.test`, avatarUrl: `https://images.example.test/${other.owner}.png` } });
      expect((await call(bindings, "revoke-all", user.cookie, "POST", other.csrf)).status).toBe(401);
      expect((await call(bindings, "activity", user.cookie, "POST")).status).toBe(401);
      const activity = new URL("https://test/api/browser/activity");
      expect((await handleBrowserRoute(new Request(activity, { method: "POST", headers: { cookie: user.cookie, origin: "https://evil.test", "x-csrf-token": user.csrf } }), bindings, activity))!.status).toBe(401);
      const wrongOriginWs = new URL(`https://test/api/browser/device/${user.device}/ws`);
      expect((await browserDeviceRoute(new Request(wrongOriginWs, { headers: { cookie: user.cookie, origin: "https://evil.test", upgrade: "websocket" } }), bindings, wrongOriginWs))!.status).toBe(403);
      for (const target of users) {
        const url = new URL(`https://test/api/browser/device/${target.device}/ws`);
        const response = (await browserDeviceRoute(new Request(url, { headers: {
          upgrade: "websocket", origin: "https://test", cookie: user.cookie,
          [AUTH_USER_HEADER]: target.owner
        } }), bindings, url))!;
        expect(response.status).toBe(target === user ? 101 : 403);
        if (response.webSocket) { response.webSocket.accept(); response.webSocket.close(); }
      }
    }
    const [alice, bob] = users;
    expect((await call(bindings, "revoke-all", alice!.cookie, "POST", alice!.csrf)).status).toBe(200);
    expect(await (await call(bindings, "session", alice!.cookie)).json()).toEqual({ authenticated: false });
    expect(await (await call(bindings, "session", bob!.cookie)).json()).toMatchObject({ authenticated: true, ownerId: bob!.owner });
    expect(replies).toEqual([]);
  });
  it("rejects wrong Origin, CSRF, invalid callbacks, and dev login in WorkOS mode", async () => {
    const bindings = config();
    expect(await (await call(bindings, "session")).json()).toEqual({ authenticated: false });
    expect((await call(bindings, "devices")).status).toBe(401);
    expect((await call(bindings, "dev-login", "", "POST")).status).toBe(403);
    for (const origin of ["https://evil.test", ""]) {
      const login = new URL("https://test/api/browser/login");
      expect((await handleBrowserRoute(new Request(login, { method: "POST", headers: { origin } }), bindings, login))!.status).toBe(403);
      const ws = new URL("https://test/api/browser/device/owned/ws");
      expect((await browserDeviceRoute(new Request(ws, { headers: { origin, upgrade: "websocket", authorization: "Bearer owner", [AUTH_USER_HEADER]: "owner" } }), bindings, ws))!.status).toBe(403);
    }
    expect((await call(bindings, "activity", "", "POST")).status).toBe(401);
    for (const path of ["callback?code=code&state=state", "callback?code=a&code=b&state=state"]) {
      expect((await call(bindings, path)).status).toBe(400);
    }
    const started = await call(bindings, "login", "", "POST");
    const authorization = new URL((await started.json() as { authorizationUrl: string }).authorizationUrl);
    const state = authorization.searchParams.get("state")!;
    expect((await call(bindings, `callback?code=code&state=${state}`, "__Host-comet_auth_txn=wrong")).status).toBe(401);
    // Wrong nonce consumes the transaction; a later correct cookie cannot replay it.
    expect((await call(bindings, `callback?code=code&state=${state}`, started.headers.get("set-cookie")!.split(";")[0]!)).status).toBe(401);
  });
});
