import { access, chmod, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { advancePrivateSessionGeneration } from "../src/state/private-session-generation";
import { afterAll, beforeAll, expect, test } from "vitest";
import { EngineClient, EngineWatchCache, encodeScopedId } from "@zeron/engine-client";
import { RelaySocket } from "../../engine-client/src/device-frame";
import { startBrowserRelayFixture, type BrowserRelayFixture } from "../../engine-client/tests/helpers/browser-relay";
import { AddSpaceStore } from "../src/state/add-space";
import type { EngineSession } from "../src/state/engine-session";

let fixture: BrowserRelayFixture;
let sessions: EngineSession[];
const originalWebSocket = globalThis.WebSocket;
async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 15_000;
  while (!check()) { if (Date.now() > deadline) throw new Error("project flow timed out"); await new Promise(r => setTimeout(r, 20)); }
}
beforeAll(async () => {
  fixture = await startBrowserRelayFixture();
  // The fixture supplies a real Node ws connection with actual Cookie/Origin
  // headers. RelaySocket and EngineClient remain production implementations.
  globalThis.WebSocket = function (url: string) {
    const deviceId = decodeURIComponent(new URL(url).pathname.split("/").at(-2)!);
    return fixture.session.openSocket(deviceId);
  } as unknown as typeof globalThis.WebSocket;
  sessions = fixture.engines.map(engine => {
    const key = engine.deviceId;
    const client = new EngineClient({ endpoint: fixture.session.relayUrl(key), credential: "", expectedDeviceId: key, engineKey: key, webSocket: url => new RelaySocket(url) });
    const cache = new EngineWatchCache(client);
    client.connect();
    return { engine: { key, endpoint: fixture.session.relayUrl(key), label: engine.label, deviceId: key }, client, cache } as EngineSession;
  });
  await until(() => sessions.every(session => session.client.state === "connected"));
}, 600_000);
afterAll(async () => { sessions?.forEach(session => { session.cache.dispose(); session.client.close(); }); globalThis.WebSocket = originalWebSocket; await fixture?.stop(); }, 30_000);

function flow(): AddSpaceStore {
  const store = new AddSpaceStore();
  store.attach({ session: sessions[0]!, sessions: new Map(sessions.map(s => [s.engine.key, s])), goToCanvas: () => {} });
  store.open();
  store.pickDevice(encodeScopedId(sessions[1]!.engine.key, fixture.engines[1]!.deviceId));
  store.gotoLocation("Project", fixture.engines[1]!.projectRoot);
  return store;
}

test("retains real upstream unsupported PrepareSpacePath reproduction", async () => {
  await expect(sessions[1]!.client.call("PrepareSpacePath", { path: fixture.engines[1]!.projectRoot, createIfMissing: false })).rejects.toThrow(/unknown method/i);
});
test("typed existing path validates through selected actual cookie relay without unsupported RPC", async () => {
  const store = flow();
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    store.setQuery(fixture.engines[1]!.projectRoot);
    await until(() => store.getSnapshot().flow?.manualPath !== null || store.getSnapshot().flow?.error !== null);
    expect(store.getSnapshot().flow?.error).toBeNull();
    expect(store.getSnapshot().flow?.manualPath).toEqual({ path: fixture.engines[1]!.projectRoot });
  } finally { store.forceClose(); }
});
test("missing typed path never creates a directory, including submit", async () => {
  const path = `${fixture.engines[1]!.projectRoot}/must-not-exist`;
  const store = flow();
  try {
    store.setQuery(path);
    await until(() => store.getSnapshot().flow?.error !== null);
    store.submit();
    await until(() => store.getSnapshot().flow?.error !== null);
    await expect(access(path)).rejects.toThrow(/ENOENT/);
    expect(store.getSnapshot().flow?.error).not.toMatch(/unknown method/i);
  } finally { store.forceClose(); }
});

test("explicit managed repository and project are created only on selected engine", async () => {
  const store = flow();
  const name = "ticket10-explicit-project";
  const expected = join(dirname(fixture.engines[1]!.projectRoot), "repos", name);
  const other = join(dirname(fixture.engines[0]!.projectRoot), "repos", name);
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    store.createRepository(name);
    await until(() => store.getSnapshot().status !== "open" || store.getSnapshot().flow?.error !== null);
    expect(store.getSnapshot().flow?.error).toBeNull();
    expect(store.getSnapshot().status).toBe("closing");
    await access(join(expected, ".git"));
    await expect(access(other)).rejects.toThrow(/ENOENT/);
    await until(() => sessions[1]!.cache.getSnapshot().spaces.rows.some(s => s.path === expected));
    expect(sessions[1]!.cache.getSnapshot().spaces.rows.find(s => s.path === expected)?.deviceId).toBe(fixture.engines[1]!.deviceId);
    // Spaces replicate across an owner's engines; the deviceId owns the filesystem.
    // Assert no project was created FOR the other device, not that metadata is private to one watch.
    expect(sessions[0]!.cache.getSnapshot().spaces.rows.some(s => s.deviceId === fixture.engines[0]!.deviceId && (s.path === expected || s.path === other))).toBe(false);
    console.log(`ticket10 actual selected project ${expected}; absent on other engine ${other}`);
  } finally { store.forceClose(); }
});

test("invalid and permission-denied paths are recoverable on the selected engine", async () => {
  const denied = join(fixture.engines[1]!.projectRoot, "denied");
  await mkdir(denied);
  await chmod(denied, 0);
  const store = flow();
  try {
    store.setQuery(denied);
    await until(() => store.getSnapshot().flow?.error !== null);
    expect(store.getSnapshot().flow?.error).toMatch(/access to this folder/);
    store.setQuery(join(fixture.engines[1]!.projectRoot, "fixture.txt"));
    await until(() => store.getSnapshot().flow?.error !== null);
    expect(store.getSnapshot().flow?.error).toMatch(/not a directory/i);
    store.setQuery(fixture.engines[1]!.projectRoot);
    await until(() => store.getSnapshot().flow?.manualPath !== null);
    expect(store.getSnapshot().flow?.error).toBeNull();
  } finally { await chmod(denied, 0o700); store.forceClose(); }
});

/** Delay delivery of an actual engine result, never fabricate RPC data. */
async function holdFolderResponse(store: AddSpaceStore, action: () => void): Promise<void> {
  const client = sessions[1]!.client;
  const original = client.call;
  let release: (() => void) | undefined;
  const gate = new Promise<void>(resolve => { release = resolve; });
  let received = false;
  client.call = async function <T>(method: string, params?: unknown): Promise<T> {
    const result = await original.call(client, method, params) as T;
    if (method === "ListFolders") { received = true; await gate; }
    return result;
  };
  try {
    store.setQuery(fixture.engines[1]!.projectRoot);
    await until(() => received);
    action();
    release!();
    await new Promise(r => setTimeout(r, 50));
  } finally { release!(); client.call = original; }
}

test("cancelled real path response cannot revive a closed flow", async () => {
  const store = flow();
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    await holdFolderResponse(store, () => store.forceClose());
    expect(store.getSnapshot().flow).toBeNull();
    expect(store.getSnapshot().pendingSpaces).toEqual([]);
  } finally { store.forceClose(); }
});
test("real response from old selection cannot overwrite new device", async () => {
  const store = flow();
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    await holdFolderResponse(store, () => {
      store.pickDevice(encodeScopedId(sessions[0]!.engine.key, fixture.engines[0]!.deviceId));
      store.gotoLocation("Project", fixture.engines[0]!.projectRoot);
    });
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    expect(store.getSnapshot().flow?.listing).toMatchObject({ path: fixture.engines[0]!.projectRoot });
    expect(store.getSnapshot().flow?.manualPath).toBeNull();
    expect(store.getSnapshot().flow?.submitBusy).toBe(false);
  } finally { store.forceClose(); }
});
test("private-session generation invalidates old real-engine path response", async () => {
  const store = flow();
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    await holdFolderResponse(store, () => advancePrivateSessionGeneration());
    expect(store.getSnapshot().flow?.manualPath).toBeNull();
    expect(store.getSnapshot().pendingSpaces).toEqual([]);
  } finally { store.resetPrivateState(); }
});

test("selected device disconnection is recoverable without fallback to active engine", async () => {
  const store = flow();
  const selected = fixture.engines[1]!;
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    await selected.disconnect();
    await fixture.waitForDevice(fixture.session, selected, false);
    await until(() => sessions[1]!.client.state !== "connected");
    store.setQuery(selected.projectRoot);
    await until(() => store.getSnapshot().flow?.error !== null);
    expect(store.getSnapshot().flow?.error).toMatch(/offline|reconnecting/i);
    expect(store.getSnapshot().flow?.manualPath).toBeNull();
    await selected.reconnect();
    await fixture.waitForDevice(fixture.session, selected);
    await until(() => sessions[1]!.client.state === "connected");
    store.setQuery(`${selected.projectRoot} `);
    await until(() => store.getSnapshot().flow?.manualPath !== null);
    expect(store.getSnapshot().flow?.error).toBeNull();
  } finally { await selected.reconnect(); store.forceClose(); }
}, 120_000);

test("cancelling after explicit folder creation cannot submit its project or navigate", async () => {
  const store = flow();
  const client = sessions[1]!.client;
  const original = client.call;
  const name = "ticket10-cancel-after-folder";
  const path = join(dirname(fixture.engines[1]!.projectRoot), "repos", name);
  let received = false;
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  try {
    await until(() => typeof store.getSnapshot().flow?.listing === "object");
    client.call = async function <T>(method: string, params?: unknown): Promise<T> {
      const result = await original.call(client, method, params) as T;
      if (method === "CreateRepo") { received = true; await gate; }
      return result;
    };
    store.createRepository(name);
    await until(() => received);
    store.forceClose();
    release();
    await new Promise(r => setTimeout(r, 50));
    await access(join(path, ".git")); // Explicit RPC already committed; cancellation is not filesystem rollback.
    expect(store.getSnapshot().flow).toBeNull();
    expect(store.getSnapshot().pendingSpaces).toEqual([]);
    expect(sessions[1]!.cache.getSnapshot().spaces.rows.some(s => s.path === path)).toBe(false);
    await expect(access(join(dirname(fixture.engines[0]!.projectRoot), "repos", name))).rejects.toThrow(/ENOENT/);
  } finally { release(); client.call = original; store.forceClose(); }
});
