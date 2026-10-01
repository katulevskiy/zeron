import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { Chat, EngineInfo } from "@zeron/proto";
import { EngineClient } from "../src/client";
import { ENGINE_INFO, MUTATE, WATCH_CHATS, WATCH_SPACES } from "../src/methods";
import { EngineWatchCache } from "../src/watch-cache";
import { startBrowserRelayFixture, type BrowserRelayFixture } from "./helpers/browser-relay";
import { trackedCookieRelay } from "./helpers/relay-client";
import { statusWhen, waitUntil } from "./helpers/ws";

/** Real cookie-session/relay smoke using the product transport and seeded EngineCore.
 * Mutations/watches here do not claim a rendered composer or QueueCommand response.
 */
const FAST_BACKOFF = { initialMs: 25, jitterMs: 1, maxMs: 100 };
let fixture: BrowserRelayFixture | undefined;
let client: EngineClient | undefined;
let cache: EngineWatchCache | undefined;

beforeEach(async () => {
  fixture = await startBrowserRelayFixture({ engineLabels: ["smoke"] });
  const engine = fixture.engines[0]!;
  client = new EngineClient({
    endpoint: fixture.session.relayUrl(engine.deviceId),
    credential: "",
    expectedDeviceId: engine.deviceId,
    webSocket: trackedCookieRelay(fixture.session).factory,
    backoff: FAST_BACKOFF,
  });
  cache = new EngineWatchCache(client);
  client.connect();
  await statusWhen(client, value => value.state === "connected");
});
afterEach(async () => {
  client?.close();
  client = undefined;
  cache = undefined;
  try { await fixture?.stop(); }
  finally { fixture = undefined; vi.unstubAllGlobals(); }
});

describe("cookie relay smoke against a real seeded engine", () => {
  test("actual cookie session replaces direct dev sign-in configuration", async () => {
    const response = await fixture!.session.request("/api/browser/session");
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ authenticated: true, ownerId: fixture!.session.ownerId });
    expect((await fetch(`${fixture!.origin}/api/browser/devices`)).status).toBe(401);
  });
  test("connects, verifies engine identity and retains supported capabilities", async () => {
    const status = client!.status;
    expect(status).toMatchObject({ state: "connected", info: { deviceId: fixture!.engines[0]!.deviceId } });
    if (status?.state !== "connected") throw new Error("expected a connected real relay client");
    expect(status.info.capabilities).not.toContain("web-client");
    expect(status.info.capabilities).toContain("message-queue-v1");
    expect(status.info.workspaceScope).toBe("development");
    const info = await client!.call<EngineInfo>(ENGINE_INFO, {});
    expect(info.deviceId).toBe(fixture!.engines[0]!.deviceId);
  });
  test("watches the chat list and sees the genuine fixture's seeded chat", async () => {
    const engine = fixture!.engines[0]!;
    const items: Array<{ generation: number; chats: readonly Chat[] }> = [];
    const watch = client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (item, context) => items.push({ generation: context.generation, chats: item }),
    });
    try {
      await waitUntil(() => items.some(entry => entry.chats.some(chat => chat.id === engine.chatId)), 10_000, "seeded chat on WatchChats");
      const seeded = items.flatMap(entry => entry.chats).find(chat => chat.id === engine.chatId)!;
      expect(seeded.title).toBe(`Chat on ${engine.label}`);
      expect(seeded.deviceId).toBe(engine.deviceId);
      expect(items.every(entry => entry.generation === 1)).toBe(true);
    } finally { watch.cancel(); }
  });
  test("Mutate.createChat reaches the real host and its watch cache", async () => {
    await waitUntil(() => cache!.getSnapshot().chats.loaded && cache!.getSnapshot().chats.rows.length >= 1, 10_000, "initial cache");
    const before = cache!.getSnapshot().chats.rows.length;
    const chatId = "smoke-created-chat";
    await client!.call(MUTATE, { op: "createChat", chatId, deviceId: fixture!.engines[0]!.deviceId });
    await waitUntil(() => cache!.getSnapshot().chats.rows.some(row => row.id === chatId) &&
      cache!.getSnapshot().chats.rows.length === before + 1, 10_000, "new chat on watch cache");
    const sent = cache!.getSnapshot().chats.rows.find(row => row.id === chatId)!;
    expect(sent.id).toBe(chatId);
    expect(sent.title === null || sent.title === "New session").toBe(true);
  });
  test("chat and space streams share the live relay generation", async () => {
    const generations = new Set<number>();
    const seen = { chats: 0, spaces: 0 };
    const chatWatch = client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (_item, context) => { generations.add(context.generation); seen.chats++; },
    });
    const spaceWatch = client!.watch<unknown[]>(WATCH_SPACES, {}, {
      onItem: (_item, context) => { generations.add(context.generation); seen.spaces++; },
    });
    try {
      await waitUntil(() => seen.chats >= 1 && seen.spaces >= 1);
      await client!.call(MUTATE, { op: "createChat", chatId: "smoke-generation-chat", deviceId: fixture!.engines[0]!.deviceId });
      await waitUntil(() => seen.chats >= 2, 10_000, "stream after mutation");
      expect([...generations]).toEqual([client!.generation]);
    } finally { chatWatch.cancel(); spaceWatch.cancel(); }
  });
});
