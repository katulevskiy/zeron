import { env as testEnv, abortAllDurableObjects, runInDurableObject } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { handleBrowserRoute } from "../../src/browser-routes";
import { createBrowserSession, revokeBrowserSession, SESSION_COOKIE, SESSION_STORE_NAME, tokenHash, validateBrowserSession } from "../../src/browser-sessions";
import type { Env } from "../../src/env";

const bindings = (): Env => ({
  BROWSER_SESSIONS: testEnv.BROWSER_SESSIONS,
  DEVICE_ROOMS: testEnv.DEVICE_ROOMS,
  AUTH_MODE: "workos",
  WORKOS_CLIENT_ID: "client_test",
  WORKOS_BROWSER_ORIGIN: "https://test",
  WORKOS_API_KEY: "test-only",
  WORKOS_ISSUER: "https://test-issuer",
  WORKOS_JWKS_URL: `https://test-issuer/${crypto.randomUUID()}/jwks`,
  BROWSER_SESSION_KEY: "workerd-test-session-key"
}) as Env;
const route = async (env: Env, path: string, cookie = "", method = "GET") => {
  const url = new URL(`https://test/api/browser/${path}`);
  return (await handleBrowserRoute(new Request(url, { method, headers: { cookie, origin: "https://test" } }), env, url))!;
};
const profile = { email: "fixture@example.test", firstName: "Fixture", lastName: "Person", avatarUrl: "https://images.example.test/avatar.png" };
const wire = (owner: string) => ({ id: owner, email: profile.email, first_name: profile.firstName, last_name: profile.lastName, profile_picture_url: profile.avatarUrl, metadata: { private: "never-public" } });
const legacy = async (env: Env) => {
  const raw = crypto.randomUUID();
  const hash = await tokenHash(raw);
  const owner = `legacy-${crypto.randomUUID()}`;
  await createBrowserSession(env, { hash, ownerId: owner, providerSessionId: `sid-${owner}`, csrfToken: "fixture-csrf", accessToken: "fixture-access", refreshToken: "fixture-refresh", providerExpiresAt: Date.now() + 3600_000 });
  return { owner, hash, cookie: `${SESSION_COOKIE}=${raw}` };
};
const replies: Array<{ owner: string; status: number; body: unknown }> = [];
let scheduled = 0;
const providerFetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
  const reply = replies.shift();
  expect(reply, "unexpected provider request").toBeDefined();
  const url = input instanceof Request ? input.url : input.toString();
  expect(url).toBe(`https://api.workos.com/user_management/users/${reply!.owner}`);
  expect(new Headers(init?.headers).get("authorization")).toBe("Bearer test-only");
  return Response.json(reply!.body, { status: reply!.status });
});
const provider = (owner: string, status: number, body: unknown) => {
  replies.push({ owner, status, body });
  scheduled++;
};
beforeEach(() => { scheduled = 0; replies.length = 0; providerFetch.mockClear(); vi.stubGlobal("fetch", providerFetch); });
afterEach(() => {
  expect(replies).toEqual([]);
  expect(providerFetch).toHaveBeenCalledTimes(scheduled);
  vi.unstubAllGlobals();
});

describe("browser profile at the provider/session seam", () => {
  it.each([null, undefined])("keeps callback profile when picture is %s without credentials or repeated lookups", async (picture) => {
    const env = bindings();
    const owner = `callback-${crypto.randomUUID()}`;
    const pair = await generateKeyPair("RS256");
    const jwk = await exportJWK(pair.publicKey);
    const jwt = await new SignJWT({ sid: `sid-${owner}`, auth_time: Math.floor(Date.now() / 1000) }).setProtectedHeader({ alg: "RS256", kid: "profile" })
      .setSubject(owner).setIssuer(env.WORKOS_ISSUER!).setExpirationTime("1h").sign(pair.privateKey);
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url === env.WORKOS_JWKS_URL) return Response.json({ keys: [{ ...jwk, kid: "profile" }] });
      expect(url).toBe("https://api.workos.com/user_management/authenticate");
      expect(JSON.parse(init!.body as string).code_verifier).toMatch(/^[A-Za-z0-9._~-]{43,128}$/);
      return Response.json({ user: { ...wire(owner), first_name: null, profile_picture_url: picture }, access_token: jwt, refresh_token: "fixture-refresh" });
    }));
    const started = await route(env, "login", "", "POST");
    const authorization = new URL((await started.json() as { authorizationUrl: string }).authorizationUrl);
    const callback = await route(env, `callback?code=fixture&state=${authorization.searchParams.get("state")}`, started.headers.get("set-cookie")!.split(";")[0]);
    expect(callback.status).toBe(303);
    const cookie = `${SESSION_COOKIE}=${callback.headers.get("set-cookie")!.match(/__Host-comet_session=([^;]+)/)![1]}`;
    const expected = { email: profile.email, lastName: profile.lastName };
    for (let poll = 0; poll < 3; poll++) {
      const response = await route(env, "session", cookie);
      expect(response.headers.get("cache-control")).toBe("no-store");
      const session = await response.json() as { profile: unknown };
      expect(session.profile).toEqual(expected);
      expect(Object.keys(session).sort()).toEqual(["authenticated", "csrfToken", "expiresAt", "ownerId", "profile"]);
    }
    expect(await (await route(env, "session")).json()).toEqual({ authenticated: false });
    expect(vi.mocked(fetch)).toHaveBeenCalledTimes(2);
  });

  it("additively upgrades a legacy SQL session and backfills only its authenticated owner once", async () => {
    const env = bindings();
    const user = await legacy(env);
    const stub = env.BROWSER_SESSIONS.get(env.BROWSER_SESSIONS.idFromName(SESSION_STORE_NAME));
    // Reproduce the deployed pre-profile schema without replacing the store.
    await runInDurableObject(stub, async (_instance, state) => {
      state.storage.sql.exec("ALTER TABLE browser_sessions DROP COLUMN profile_json");
      state.storage.sql.exec("ALTER TABLE browser_sessions DROP COLUMN profile_retry_after");
    });
    await abortAllDurableObjects();
    // Device/frame validation must not contact the profile provider.
    expect(await validateBrowserSession(env, user.hash)).toMatchObject({ ownerId: user.owner });
    provider(user.owner, 200, wire(user.owner));
    const first = await (await route(env, "session?ownerId=another-user", user.cookie)).json();
    expect(first).toMatchObject({ authenticated: true, ownerId: user.owner, profile });
    await abortAllDurableObjects();
    expect(await (await route(env, "session", user.cookie)).json()).toEqual(first);
    expect(await validateBrowserSession(env, user.hash)).toMatchObject({ profile });
    expect(Object.keys(first as object).sort()).toEqual(["authenticated", "csrfToken", "expiresAt", "ownerId", "profile"]);
  });

  it.each([503, 404, 429])("keeps legacy authentication on provider %s and throttles retry persistently", async (status) => {
    const env = bindings();
    const user = await legacy(env);
    provider(user.owner, status, { error: "fixture-provider-outage" });
    for (let poll = 0; poll < 3; poll++) {
      expect(await (await route(env, "session", user.cookie)).json()).toMatchObject({ authenticated: true, ownerId: user.owner });
    }
    await abortAllDurableObjects();
    expect(await (await route(env, "session", user.cookie)).json()).toMatchObject({ authenticated: true, ownerId: user.owner });
    // Simulate the persisted retry deadline elapsing; no logout or new cookie.
    const stub = env.BROWSER_SESSIONS.get(env.BROWSER_SESSIONS.idFromName(SESSION_STORE_NAME));
    await runInDurableObject(stub, async (_instance, state) => {
      state.storage.sql.exec("UPDATE browser_sessions SET profile_retry_after = 0 WHERE hash = ?", user.hash);
    });
    provider(user.owner, 200, wire(user.owner));
    expect(await (await route(env, "session", user.cookie)).json()).toMatchObject({ authenticated: true, ownerId: user.owner, profile });
  });

  it("rejects mismatched provider ownership without changing authentication or exposing the other profile", async () => {
    const env = bindings();
    const user = await legacy(env);
    provider(user.owner, 200, wire("another-owner"));
    const session = await (await route(env, "session", user.cookie)).json() as { profile?: unknown };
    expect(session).toMatchObject({ authenticated: true, ownerId: user.owner });
    expect(session.profile).toBeUndefined();
    expect(await (await route(env, "session", user.cookie)).json()).toEqual(session);
  });
  it("does not resurrect a revoked session when a late profile response completes, or duplicate concurrent probes", async () => {
    const env = bindings();
    const user = await legacy(env);
    let started!: () => void;
    let finish!: () => void;
    const entered = new Promise<void>((resolve) => { started = resolve; });
    const response = new Promise<void>((resolve) => { finish = resolve; });
    scheduled++;
    providerFetch.mockImplementationOnce(async (input) => {
      expect(input.toString()).toBe(`https://api.workos.com/user_management/users/${user.owner}`);
      started();
      await response;
      return Response.json(wire(user.owner));
    });
    const pending = validateBrowserSession(env, user.hash, false, true, true);
    await entered;
    expect(await (await route(env, "session", user.cookie)).json()).toMatchObject({ authenticated: true, ownerId: user.owner });
    expect(await revokeBrowserSession(env, user.owner, user.hash)).toBe(true);
    finish();
    expect(await pending).toBeUndefined();
    expect(await (await route(env, "session", user.cookie)).json()).toEqual({ authenticated: false });
  });

  it("keeps authentication on a profile network timeout without retrying on the next probe", async () => {
    const env = bindings();
    const user = await legacy(env);
    scheduled++;
    providerFetch.mockRejectedValueOnce(new DOMException("fixture-timeout", "TimeoutError"));
    const first = await (await route(env, "session", user.cookie)).json();
    expect(first).toMatchObject({ authenticated: true, ownerId: user.owner });
    expect(await (await route(env, "session", user.cookie)).json()).toEqual(first);
  });
});
