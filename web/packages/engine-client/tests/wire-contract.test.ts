import { afterAll, beforeAll, describe, expect, test, vi } from "vitest";
import type {
  Chat, ChatConfig, Device, EngineInfo, Model, Session, SessionCommandPayload,
  SessionMessageEntry, Space, TranscriptUpdate,
} from "@zeron/proto";
import { CORE_METHODS, UNSUPPORTED_LEGACY_METHODS, UPSTREAM_METHODS as M } from "@zeron/proto";
import * as browserMethods from "../src/methods";
import { EngineClient, type WatchHandle } from "../src/client";
import { decodeServerMessage, type ServerFrame } from "../src/codec";
import { startBrowserRelayFixture, type BrowserRelayFixture, type BrowserRelayEngine } from "./helpers/browser-relay";
import { trackedCookieRelay } from "./helpers/relay-client";
import { statusWhen, waitUntil } from "./helpers/ws";

let fixture: BrowserRelayFixture | undefined;
let engine: BrowserRelayEngine | undefined;
let client: EngineClient | undefined;
const frames: ServerFrame[] = [];

beforeAll(async () => {
  fixture = await startBrowserRelayFixture({ engineLabels: ["wire-contract"] });
  engine = fixture.engines[0]!;
  const tracked = trackedCookieRelay(fixture.session);
  client = new EngineClient({
    endpoint: fixture.session.relayUrl(engine.deviceId),
    credential: "",
    expectedDeviceId: engine.deviceId,
    webSocket: (url) => {
      const socket = tracked.factory(url);
      socket.addEventListener("message", (event) => {
        const decoded = decodeServerMessage(String(event.data));
        frames.push(...decoded.frames);
      });
      return socket;
    },
    callTimeoutMs: 10_000,
  });
  client.connect();
  await statusWhen(client, (status) => status.state === "connected");
});

afterAll(async () => {
  client?.close();
  await fixture?.stop();
  vi.unstubAllGlobals();
});

function firstItem<T>(method: string, params: unknown = {}, ready: (item: T) => boolean = () => true): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { watch.cancel(); reject(new Error(`${method}: no item`)); }, 10_000);
    const watch = client!.watch<T>(method, params, {
      onItem: (item) => { if (ready(item)) { clearTimeout(timer); watch.cancel(); resolve(item); } },
      onEnd: (error) => { clearTimeout(timer); reject(error ?? new Error(`${method}: ended without an item`)); },
    });
  });
}

function entries(update: TranscriptUpdate): SessionMessageEntry[] {
  return "reset" in update ? update.reset : update.upsert.map((upsert) => upsert.entry);
}

describe("upstream core contract (no fork project/sidebar requirements)", () => {
  test("browser core method names equal the generated upstream inventory", () => {
    for (const value of CORE_METHODS) {
      const key = Object.entries(M).find(([, method]) => method === value)![0];
      expect(browserMethods[key as keyof typeof browserMethods], key).toBe(value);
    }
    for (const legacy of UNSUPPORTED_LEGACY_METHODS) {
      expect(Object.values(M)).not.toContain(legacy);
    }
  });

  test("identity, watches, chat creation, command decoding and transcript response use real dispatch", async () => {
    const identity = await client!.call<EngineInfo>(M.ENGINE_INFO);
    expect(identity.deviceId).toBe(engine!.deviceId);
    expect(identity.workspaceScope).toBe("development");
    expect(identity.capabilities).not.toContain("web-client");
    expect(identity.capabilities).toContain("message-queue-v1");
    expect(await client!.call(M.LOCAL_DEVICE)).toEqual({ deviceId: identity.deviceId });
    const harnesses = await client!.call<Array<{ id: string }>>(M.LIST_HARNESSES);
    expect(harnesses.some((harness) => harness.id === "mock")).toBe(true);
    const models = await client!.call<Model[]>(M.LIST_MODELS, { harness: "mock" });
    expect(models.some((model) => model.id === "mock-1")).toBe(true);

    const [devices, spaces, chats, sessions] = await Promise.all([
      firstItem<Device[]>(M.WATCH_DEVICES), firstItem<Space[]>(M.WATCH_SPACES),
      firstItem<Chat[]>(M.WATCH_CHATS), firstItem<Session[]>(M.WATCH_SESSIONS),
    ]);
    for (const rows of [devices, spaces, chats, sessions]) expect(Array.isArray(rows)).toBe(true);

    // The relay fixture already seeds wire-contract-chat in its project.
    const chatId = "wire-contract-created-chat";
    expect(chats.map(chat => chat.id)).not.toContain(chatId);
    const config: ChatConfig = { harness: "mock", model: "mock-1", reasoning: null, modelOptions: {}, sandbox: "workspace-write" };
    expect(await client!.call(M.MUTATE, { op: "createChat", chatId, deviceId: identity.deviceId, config })).toEqual({ ok: true });
    // Shared-registry acknowledgement precedes asynchronous watch convergence.
    const created = (await firstItem<Chat[]>(M.WATCH_CHATS, {},
      (rows) => rows.some((chat) => chat.id === chatId && chat.config !== null),
    )).find((chat) => chat.id === chatId)!;
    expect(created.deviceId).toBe(identity.deviceId);
    expect(created.config).toEqual(config);
    expect(created.cwd).toBe("~");
    expect(created.spaceId).toBeUndefined();
    const initial = await firstItem<TranscriptUpdate>(M.WATCH_DOC_MESSAGES, { chatId });
    expect(initial).toHaveProperty("reset", []);
    expect(initial).toHaveProperty("contextUsage");
    expect(await firstItem(M.WATCH_QUEUE, { chatId })).toEqual({ items: [] });

    const updates: TranscriptUpdate[] = [];
    let streamError: unknown;
    const watch: WatchHandle = client!.watch(M.WATCH_DOC_MESSAGES, { chatId }, {
      onItem: (update: TranscriptUpdate) => updates.push(update),
      onEnd: (error) => { streamError = error; },
    });
    try {
      const command: SessionCommandPayload = {
        kind: "run", messageId: "wire-user-1", request: {
          prompt: "hello upstream", harness: "mock", model: "mock-1", reasoning: null,
          modelOptions: {}, cwd: "~", sandbox: "workspace-write", autoApprove: true, resume: null,
        },
      };
      const reply = await client!.call<{ commandId: string }>(M.QUEUE_COMMAND, { chatId, command });
      expect(typeof reply.commandId).toBe("string");
      await waitUntil(() => streamError !== undefined || updates.some((update) =>
        entries(update).some((entry) => entry.role === "assistant" && entry.status === "complete")),
      10_000, "completed response from the real engine and scripted harness");
      expect(streamError).toBeUndefined();
      const received = updates.flatMap(entries);
      expect(received.some((entry) => entry.id === "wire-user-1" && entry.role === "user")).toBe(true);
      expect(received.some((entry) => entry.role === "assistant" && entry.parts.some((part) => part.kind === "text" && part.text.length > 0))).toBe(true);
      // Ordinary upstream watches send data items directly, not an invented unary ack.
      expect(frames.some((frame) => frame.item !== undefined)).toBe(true);
      expect(frames.some((frame) => typeof frame.ok === "object" && frame.ok !== null &&
        (frame.ok as { stream?: unknown }).stream === true)).toBe(false);
    } finally {
      watch.cancel();
    }
    await expect(client!.call(M.QUEUE_COMMAND, { chatId, command: { kind: "not-a-command" } })).rejects.toMatchObject({ kind: "failed" });
  });

  test("fork-only methods remain actual unknown-method errors, not fabricated support", async () => {
    for (const method of UNSUPPORTED_LEGACY_METHODS) {
      await expect(client!.call(method, {})).rejects.toMatchObject({ kind: "unknown-method", method });
    }
    expect(frames.some((frame) => frame.err === "unknown method: PrepareSpacePath")).toBe(true);
  });
});
