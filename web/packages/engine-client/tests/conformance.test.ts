import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { Chat, EngineInfo } from "@zeron/proto";
import { EngineClient } from "../src/client";
import { ENGINE_INFO, MUTATE, WATCH_CHATS } from "../src/methods";
import { EngineWatchCache } from "../src/watch-cache";
import { startBrowserRelayFixture, type BrowserRelayFixture } from "./helpers/browser-relay";
import { trackedCookieRelay } from "./helpers/relay-client";
import { statusWhen, waitUntil } from "./helpers/ws";

const FAST_BACKOFF = { initialMs: 25, jitterMs: 1, maxMs: 100 };
let fixture: BrowserRelayFixture | undefined;
let client: EngineClient | undefined;
let tracked: ReturnType<typeof trackedCookieRelay>;

beforeEach(async () => {
  fixture = await startBrowserRelayFixture({ engineLabels: ["conformance"] });
  tracked = trackedCookieRelay(fixture.session);
});
afterEach(async () => {
  client?.close();
  client = undefined;
  try { await fixture?.stop(); }
  finally { fixture = undefined; vi.unstubAllGlobals(); }
});

function connect(transport = tracked): EngineClient {
  const engine = fixture!.engines[0]!;
  const next = new EngineClient({
    endpoint: fixture!.session.relayUrl(engine.deviceId),
    credential: "",
    expectedDeviceId: engine.deviceId,
    webSocket: transport.factory,
    backoff: FAST_BACKOFF,
  });
  next.connect();
  return next;
}

// Isolation/teardown replaces the old parallel direct-listener fixture test.
test("parallel real relay fixtures own distinct origins and release engines and workers", async () => {
  const fixtures: BrowserRelayFixture[] = [];
  try {
    const results = await Promise.allSettled(["parallel-a", "parallel-b"].map(async label => {
      const own = await startBrowserRelayFixture({ engineLabels: [label] });
      fixtures.push(own);
      return own;
    }));
    for (const result of results) if (result.status === "rejected") throw result.reason;
    expect(new Set(fixtures.map(own => own.origin)).size).toBe(2);
    for (const own of fixtures) {
      const origin = new URL(own.origin);
      expect(origin.hostname).toBe("127.0.0.1");
      expect(origin.port).toBe(own.edgeChild.spawnargs[own.edgeChild.spawnargs.indexOf("--port") + 1]);
      expect((await own.session.request("/api/browser/session")).status).toBe(200);
    }
  } finally {
    await Promise.all(fixtures.map(own => own.stop()));
  }
  for (const own of fixtures) {
    for (const engine of own.engines) expect(() => process.kill(engine.child.pid!, 0)).toThrow();
    expect(() => process.kill(own.edgeChild.pid!, 0)).toThrow();
    await expect(fetch(`${own.origin}/api/browser/session`, { signal: AbortSignal.timeout(1000) })).rejects.toThrow();
  }
}, 120_000);

describe("product EngineClient and RelaySocket against real cookie relay", () => {
  beforeEach(async () => {
    client = connect();
    await statusWhen(client, value => value.state === "connected");
  });
  test("actual cookie session and owned discovery replace direct sign-in config", async () => {
    const session = await (await fixture!.session.request("/api/browser/session")).json();
    expect(session).toMatchObject({ authenticated: true, ownerId: fixture!.session.ownerId });
    const discovery = await (await fixture!.session.request("/api/browser/devices")).json();
    expect(discovery.devices).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: fixture!.engines[0]!.deviceId, online: true }),
    ]));
  });
  test("cookie-authenticated relay verifies engine identity and makes typed calls", async () => {
    const engine = fixture!.engines[0]!;
    expect(client!.status).toMatchObject({ state: "connected", info: { deviceId: engine.deviceId } });
    expect(client!.generation).toBe(1);
    expect(tracked.sockets).toHaveLength(1);
    const info = await client!.call<EngineInfo>(ENGINE_INFO, {});
    expect(info.deviceId).toBe(engine.deviceId);
    expect(info.workspaceScope).toBe("development");
    expect(info.capabilities).not.toContain("web-client");
    expect(info.capabilities).toContain("message-queue-v1");
  });
  test("watches the chat list stream through the real relay", async () => {
    const items: Array<{ item: Chat[]; generation: number }> = [];
    const watch = client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (item, context) => items.push({ item, generation: context.generation }),
    });
    try {
      await waitUntil(() => items.length >= 1, 10_000, "first WatchChats item");
      expect(items[0]!.generation).toBe(1);
      expect(items[0]!.item.some(chat => chat.id === fixture!.engines[0]!.chatId)).toBe(true);
      for (const chat of items[0]!.item) {
        expect(typeof chat.id).toBe("string");
        expect(typeof chat.deviceId).toBe("string");
        expect(typeof chat.archived).toBe("boolean");
        expect(typeof chat.createdAt).toBe("string");
      }
    } finally { watch.cancel(); }
  });
  test("a hard drop re-verifies identity and resubscribes an existing watch", async () => {
    const generations: number[] = [];
    const watch = client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (_item, context) => generations.push(context.generation),
    });
    try {
      await waitUntil(() => generations.includes(1));
      tracked.sockets[0]!.terminate();
      await statusWhen(client!, value => value.state === "connected" && value.generation === 2);
      expect(tracked.sockets).toHaveLength(2);
      expect((await client!.call<EngineInfo>(ENGINE_INFO, {})).deviceId).toBe(fixture!.engines[0]!.deviceId);
      await waitUntil(() => generations.includes(2), 10_000, "existing watch refilled on generation 2");
    } finally { watch.cancel(); }
  });
});

test("watch cache applies mutations and refills after reconnect without ghost rows", async () => {
  client = connect();
  const cache = new EngineWatchCache(client);
  await statusWhen(client, value => value.state === "connected");
  await waitUntil(() => {
    const snapshot = cache.getSnapshot();
    return snapshot.chats.loaded && snapshot.spaces.loaded && snapshot.devices.loaded && snapshot.statuses.loaded;
  }, 10_000, "every collection loaded");
  const initial = cache.getSnapshot();
  const initialIds = initial.chats.rows.map(chat => chat.id);
  expect(initial.generation).toBe(1);
  expect(initial.capabilities).not.toContain("web-client");
  expect(cache.supports("message-queue-v1")).toBe(true);
  const deviceId = fixture!.engines[0]!.deviceId;
  for (const chatId of ["watch-cache-a", "watch-cache-b"]) {
    await client.call(MUTATE, { op: "createChat", chatId, deviceId });
  }
  await waitUntil(() => cache.getSnapshot().chats.rows.length === initialIds.length + 2, 10_000, "created chats arrive");
  const before = cache.getSnapshot().chats.rows;
  await client.call(MUTATE, { op: "renameChat", chatId: "watch-cache-b", title: "Renamed" });
  await waitUntil(() => cache.getSnapshot().chats.rows.some(row => row.id === "watch-cache-b" && row.title === "Renamed"));
  expect(cache.getSnapshot().chats.rows.find(row => row.id === "watch-cache-a")).toBe(before.find(row => row.id === "watch-cache-a"));
  tracked.sockets[0]!.terminate();
  const second = connect(trackedCookieRelay(fixture!.session));
  try {
    await statusWhen(second, value => value.state === "connected");
    await second.call(MUTATE, { op: "deleteChat", chatId: "watch-cache-b" });
    await second.call(MUTATE, { op: "createChat", chatId: "watch-cache-c", deviceId });
  } finally { second.close(); }
  await statusWhen(client, value => value.state === "connected" && value.generation === 2);
  await waitUntil(() => {
    const snapshot = cache.getSnapshot();
    return snapshot.generation === 2 && snapshot.chats.loaded && snapshot.statuses.loaded &&
      snapshot.chats.rows.some(chat => chat.id === "watch-cache-c") &&
      !snapshot.chats.rows.some(chat => chat.id === "watch-cache-b");
  }, 10_000, "cache refilled after mutation/reconnect");
  const swapped = cache.getSnapshot();
  expect(swapped.chats.rows.map(row => row.id).sort()).toEqual([...initialIds, "watch-cache-a", "watch-cache-c"].sort());
  expect(swapped.statuses.loaded).toBe(true);
  expect(swapped.capabilities).not.toContain("web-client");
});
