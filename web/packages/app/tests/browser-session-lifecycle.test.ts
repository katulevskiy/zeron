// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";

vi.mock("@zeron/engine-client", async (original) => ({
  ...(await original<typeof import("@zeron/engine-client")>()),
  // This seam exercises real HTTP helpers and public session state; actual
  // sockets/DO ownership are covered by the real cookie-relay integration.
  EngineRegistry: class {
    sync() {}
    getSnapshot() { return { engines: registryEntries }; }
    subscribe() { return () => {}; }
    async forget() { registryEntries = []; await pendingForget; }
    async shutdown() {}
  },
}));

let owner: string;
let devicesStatus: number;
let logoutStatus: number;
let pendingDevices: Promise<Response> | undefined;
let pendingLogout: Promise<Response> | undefined;
let signedIn: boolean;
let requests: string[];
let fleet: typeof import("../src/state/fleet") | undefined;
let registryEntries: Array<{ key: string }>;
let pendingForget: Promise<void> | undefined;

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  localStorage.clear();
  owner = "owner-a";
  devicesStatus = 200;
  logoutStatus = 200;
  pendingDevices = undefined;
  pendingLogout = undefined;
  signedIn = true;
  requests = [];
  registryEntries = [];
  pendingForget = undefined;
  vi.stubGlobal("fetch", vi.fn(async (input: string | URL | Request) => {
    const path = String(input);
    requests.push(path);
    if (path.endsWith("/session")) return Response.json(signedIn
      ? { authenticated: true, ownerId: owner, csrfToken: `csrf-${owner}` }
      : { authenticated: false });
    if (path.endsWith("/devices")) return pendingDevices ?? Response.json({ devices: [{ id: `engine-${owner}`, online: true }] }, { status: devicesStatus });
    if (path.endsWith("/logout")) {
      const response = await (pendingLogout ?? Promise.resolve(Response.json({ ok: logoutStatus === 200 }, { status: logoutStatus })));
      if (response.ok) signedIn = false;
      return response;
    }
    throw new Error(`Unexpected request ${path}`);
  }));
});

afterEach(async () => {
  if (fleet && "stopEdgeFleet" in fleet) await fleet.stopEdgeFleet();
  fleet = undefined;
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function boot() {
  fleet = await import("../src/state/fleet");
  await vi.advanceTimersByTimeAsync(50);
  expect(fleet.edgeFleet.getSnapshot().devices).toHaveLength(1);
  return fleet;
}

test("device-list expiry immediately removes private devices and supervision", async () => {
  const app = await boot();
  devicesStatus = 401;
  await vi.advanceTimersByTimeAsync(10_000);
  expect(app.edgeFleet.getSnapshot().session.authenticated).toBe(false);
  expect(app.edgeFleet.getSnapshot().devices).toEqual([]);
});

test("failed revocation clears private state but does not claim sign-out success", async () => {
  const app = await boot();
  logoutStatus = 403;
  await expect(app.signOut()).rejects.toMatchObject({ status: 403 });
  expect(app.edgeFleet.getSnapshot().session.authenticated).toBe(false);
  expect(app.edgeFleet.getSnapshot().devices).toEqual([]);
  expect(app.edgeFleet.getSnapshot().error).toMatch(/sign-out.*incomplete/i);
});

test("an old device-list response cannot revive data after sign-out", async () => {
  const app = await boot();
  let resolve!: (value: Response) => void;
  pendingDevices = new Promise((done) => { resolve = done; });
  await vi.advanceTimersByTimeAsync(10_000);
  await app.signOut();
  resolve(Response.json({ devices: [{ id: "old-owner-private-device", online: true }] }));
  await vi.advanceTimersByTimeAsync(10);
  expect(app.edgeFleet.getSnapshot().session.authenticated).toBe(false);
  expect(app.edgeFleet.getSnapshot().devices).toEqual([]);
});

test("owner changes replace, rather than merge, the authenticated device set", async () => {
  const app = await boot();
  owner = "owner-b";
  await vi.advanceTimersByTimeAsync(10_000);
  expect(app.edgeFleet.getSnapshot().session.ownerId).toBe("owner-b");
  expect(app.edgeFleet.getSnapshot().devices.map((device) => device.id)).toEqual(["engine-owner-b"]);
});

test("passive polling never extends the backend's idle lifetime", async () => {
  await boot();
  await vi.advanceTimersByTimeAsync(20_000);
  expect(requests.filter((path) => path.endsWith("/activity"))).toEqual([]);
});

test("a failed sign-out survives reload without reviving the still-valid cookie's private data", async () => {
  const app = await boot();
  logoutStatus = 500;
  await expect(app.signOut()).rejects.toMatchObject({ status: 500 });
  await app.stopEdgeFleet();
  vi.resetModules();
  fleet = await import("../src/state/fleet");
  await vi.advanceTimersByTimeAsync(50);
  expect(fleet.edgeFleet.getSnapshot().status).toBe("logout-failed");
  expect(fleet.edgeFleet.getSnapshot().session.authenticated).toBe(false);
  expect(fleet.edgeFleet.getSnapshot().devices).toEqual([]);
  logoutStatus = 200;
  await fleet.signOut();
  await fleet.retryBrowserSession();
  expect(fleet.edgeFleet.getSnapshot().status).toBe("signed-out");
  expect(localStorage.getItem("zeron.browser.logout-pending")).toBeNull();
});

test("repeated sign-out actions share a single revocation request", async () => {
  const app = await boot();
  let resolve!: (response: Response) => void;
  pendingLogout = new Promise((done) => { resolve = done; });
  const first = app.signOut();
  const second = app.signOut();
  const count = requests.filter((path) => path.endsWith("/logout")).length;
  resolve(Response.json({ ok: true }));
  await Promise.all([first, second]);
  expect(count).toBe(1);
});

test("concurrent session retry cannot mount the new owner before old registry writes drain", async () => {
  const app = await boot();
  let drain!: () => void;
  pendingForget = new Promise<void>((resolve) => { drain = resolve; });
  registryEntries = [{ key: "engine-owner-a" }];
  owner = "owner-b";
  await vi.advanceTimersByTimeAsync(10_000);
  expect(app.edgeFleet.getSnapshot().status).toBe("checking");
  const retry = app.retryBrowserSession();
  await vi.advanceTimersByTimeAsync(10);
  const whileDraining = app.edgeFleet.getSnapshot();
  drain();
  await retry;
  await vi.advanceTimersByTimeAsync(10);
  expect(whileDraining.status).toBe("checking");
  expect(whileDraining.session.authenticated).toBe(false);
  expect(whileDraining.devices).toEqual([]);
  expect(app.edgeFleet.getSnapshot().session.ownerId).toBe("owner-b");
});

test("blocked legacy cache retirement stays behind a recoverable gate without an unhandled retry rejection", async () => {
  const app = await boot();
  let blocked = true;
  vi.stubGlobal("indexedDB", { deleteDatabase: () => {
    const request = { onsuccess: null as (() => void) | null, onblocked: null as (() => void) | null };
    queueMicrotask(() => { if (blocked) request.onblocked?.(); else request.onsuccess?.(); });
    return request;
  } });
  owner = "owner-b";
  let rejected: unknown;
  try { await app.retryBrowserSession(); } catch (error) { rejected = error; }
  const snapshot = app.edgeFleet.getSnapshot();
  blocked = false;
  expect(rejected).toBeUndefined();
  expect(snapshot.status).toBe("error");
  expect(snapshot.error).toMatch(/private.*cache|cache.*(open|retir)/i);
  expect(snapshot.session.authenticated).toBe(false);
  expect(snapshot.devices).toEqual([]);
  await app.retryBrowserSession();
  expect(app.edgeFleet.getSnapshot().session.ownerId).toBe("owner-b");
});
