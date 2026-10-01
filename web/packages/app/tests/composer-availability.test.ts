// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { methods } from "@zeron/engine-client";
import type { Chat, ChatConfig, HarnessDescriptor, Model } from "@zeron/proto";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { PickerCatalog } from "../src/state/picker-catalog";
import type { EngineSession } from "../src/state/engine-session";
import type { WatchCacheSnapshot } from "@zeron/engine-client";
import { PHONE_QUERY } from "../src/state/media";
import { chatDrafts, composerDefaults } from "../src/lib/composer-draft";
import { sidebarStore } from "../src/state/sidebar";

type ComposerComponentType = (typeof import("../src/components/composer"))["Composer"];
let ComposerImpl: ComposerComponentType;

const h = vi.hoisted(() => ({
  now: Date.parse("2026-10-01T00:00:00.000Z"),
  snapshot: null as unknown,
  session: null as unknown,
}));

// Mount the real Composer, ComposerPickers and PickerCatalog over a controlled
// RPC boundary so discovery and durable-call destinations can be asserted.
vi.mock("../src/state/fleet", () => ({
  useFleetSnapshot: () => h.snapshot,
  useFleetRegistry: () => ({ engines: [] }),
  engineStatesOf: () => new Map(),
}));
vi.mock("../src/state/session-provider", () => ({ useEngineSession: () => h.session }));
vi.mock("../src/state/hooks", () => ({
  useEngineStatus: () => ({ state: "connected" }),
  useNow: () => h.now,
  useWatchSnapshot: () => null,
}));

const CODEX: HarnessDescriptor = {
  id: "codex",
  name: "Codex",
  supportsSteering: false,
  steeringMode: "turn-boundary",
  reasoningLevels: ["medium"],
  installed: true,
  enabled: true,
};
const DISABLED_CODEX: HarnessDescriptor = { ...CODEX, installed: false, enabled: false };
const CLAUDE: HarnessDescriptor = {
  id: "claude-code",
  name: "Claude Code",
  supportsSteering: true,
  steeringMode: "step-boundary",
  reasoningLevels: ["low", "medium", "high"],
  installed: true,
  enabled: true,
};
const CLAUDE_MODEL: Model = {
  id: "haiku",
  label: "Haiku",
  reasoningLevels: [],
  options: [],
};
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

class Deferred<T> {
  readonly promise: Promise<T>;
  #resolve!: (value: T) => void;
  #reject!: (reason?: unknown) => void;
  #settled = false;

  constructor() {
    this.promise = new Promise<T>((resolve, reject) => {
      this.#resolve = resolve;
      this.#reject = reject;
    });
  }

  get settled(): boolean {
    return this.#settled;
  }

  resolve(value: T): void {
    if (this.#settled) return;
    this.#settled = true;
    this.#resolve(value);
  }

  reject(reason: unknown): void {
    if (this.#settled) return;
    this.#settled = true;
    this.#reject(reason);
  }
}

class FakeCache {
  #chats: Chat[] = [];
  readonly #listeners = new Set<() => void>();

  getSnapshot(): WatchCacheSnapshot {
    return { chats: { rows: this.#chats } } as unknown as WatchCacheSnapshot;
  }

  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  addChat(id: string): void {
    this.#chats = [...this.#chats, { id } as Chat];
    for (const listener of this.#listeners) listener();
  }
}

interface RpcCall {
  readonly method: string;
  readonly params: unknown;
}

class FakeClient {
  readonly state = "connected";
  readonly status = { state: "connected" };
  readonly engineInfo = { deviceId: "device-a", capabilities: [] as string[] };
  readonly calls: RpcCall[] = [];
  readonly harnessReply = new Deferred<HarnessDescriptor[]>();
  readonly cache: FakeCache;
  readonly models = new Map<string, Model[]>([["claude-code", [CLAUDE_MODEL]]]);

  constructor(cache: FakeCache) {
    this.cache = cache;
  }

  onStatus(_listener: (status: { state: string }) => void): () => void {
    return () => {};
  }

  call<T>(method: string, params?: unknown): Promise<T> {
    this.calls.push({ method, params });
    if (method === methods.LIST_HARNESSES) {
      return this.harnessReply.promise as Promise<T>;
    }
    if (method === methods.LIST_MODELS) {
      const harness = (params as { harness: string }).harness;
      return Promise.resolve((this.models.get(harness) ?? []) as T);
    }
    if (method === methods.MUTATE) {
      const request = params as { op?: string; chatId?: string };
      if (request.op === "createChat" && typeof request.chatId === "string") {
        this.cache.addChat(request.chatId);
      }
      return Promise.resolve({} as T);
    }
    if (method === methods.UPLOAD_CHUNK) {
      return Promise.resolve({} as T);
    }
    if (method === methods.UPLOAD_COMMIT) {
      return Promise.resolve({ path: "/uploaded/draft.png" } as T);
    }
    if (method === methods.QUEUE_COMMAND) {
      return Promise.resolve({ commandId: "run-command" } as T);
    }
    return Promise.reject(new Error(`Unexpected test RPC ${method}`));
  }

  durableCalls(): RpcCall[] {
    return this.calls.filter(({ method }) =>
      method === methods.MUTATE ||
      method === methods.UPLOAD_CHUNK ||
      method === methods.UPLOAD_COMMIT ||
      method === methods.QUEUE_COMMAND,
    );
  }
}

interface MountedComposer {
  readonly host: HTMLDivElement;
  readonly client: FakeClient;
  readonly cache: FakeCache;
  readonly catalog: PickerCatalog;
  readonly textarea: HTMLTextAreaElement;
  readonly sendButton: HTMLButtonElement;
  setTargetUnavailable(unavailable: boolean): void;
}

let root: Root | null = null;
let host: HTMLDivElement | null = null;
let mountedCatalog: PickerCatalog | null = null;
let mountedClient: FakeClient | null = null;

beforeAll(async () => {
  // The shell-scoped artwork store prewarms on import; jsdom's Image lacks
  // decode(), so install that browser API before dynamically loading Composer.
  HTMLImageElement.prototype.decode ??= () => Promise.resolve();
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: query === PHONE_QUERY && false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  ComposerImpl = (await import("../src/components/composer")).Composer;
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

beforeEach(() => {
  h.snapshot = {
    devices: { rows: [] },
    spaces: { rows: [] },
    statuses: { rows: [] },
  };
  chatDrafts.reset();
  composerDefaults.update({
    harness: "codex",
    modelByHarness: { codex: { id: "gpt-5", label: "Remembered GPT-5" } },
    modelLabels: { "gpt-5": "Remembered GPT-5" },
    reasoning: "medium",
    modelOptionsByModel: {},
    device: null,
    project: null,
    noProject: false,
  });
  sidebarStore.resetPrivateState();
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host?.remove();
  host = null;
  document.body.replaceChildren();
  mountedCatalog?.dispose();
  mountedCatalog = null;
  if (mountedClient !== null && !mountedClient.harnessReply.settled) {
    mountedClient.harnessReply.resolve([CLAUDE, DISABLED_CODEX]);
  }
  mountedClient = null;
  chatDrafts.reset();
  composerDefaults.update({
    harness: null,
    modelByHarness: {},
    modelLabels: {},
    reasoning: null,
    modelOptionsByModel: {},
    device: null,
    project: null,
    noProject: false,
  });
  sidebarStore.resetPrivateState();
});

function mountComposer(targetUnavailable = false): MountedComposer {
  const cache = new FakeCache();
  const client = new FakeClient(cache);
  const catalog = new PickerCatalog(client);
  const session = {
    engine: { key: "engine_a", endpoint: "wss://engine-a.invalid", label: "Engine A", deviceId: "device-a" },
    client,
    cache,
    catalog,
    transcripts: {},
  } as unknown as EngineSession;
  h.session = session;
  mountedClient = client;
  mountedCatalog = catalog;
  const chat = {
    id: "",
    branch: null,
    config: null,
    cwd: null,
    deviceId: "device-a",
    spaceId: null,
  } as unknown as Chat;
  const props = {
    session,
    chat,
    catalog,
    transcript: null,
    availableWidth: 720,
    targetUnavailable,
  } as Parameters<ComposerComponentType>[0];
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root!.render(createElement(ComposerImpl, props)));
  const textarea = host.querySelector<HTMLTextAreaElement>("textarea.composer-input");
  const sendButton = host.querySelector<HTMLButtonElement>("button.composer-send");
  if (textarea === null || sendButton === null) throw new Error("Fresh Composer controls did not mount");
  return {
    host, client, cache, catalog, textarea, sendButton,
    setTargetUnavailable: (unavailable) => {
      act(() => root!.render(createElement(ComposerImpl, { ...props, targetUnavailable: unavailable })));
    },
  };
}

function setTextareaValue(textarea: HTMLTextAreaElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setter === undefined) throw new Error("HTMLTextAreaElement.value setter is unavailable");
  setter.call(textarea, value);
  act(() => textarea.dispatchEvent(new Event("input", { bubbles: true })));
}

async function pastePng(textarea: HTMLTextAreaElement): Promise<void> {
  const file = new File([PNG], "draft.png", { type: "image/png" });
  Object.defineProperty(file, "arrayBuffer", { value: () => Promise.resolve(PNG.slice().buffer) });
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: { items: [{ kind: "file", getAsFile: () => file }] },
  });
  await act(async () => {
    textarea.dispatchEvent(event);
    await tick();
  });
}

async function stageDraft(mounted: MountedComposer): Promise<void> {
  setTextareaValue(mounted.textarea, "Keep this unsent draft");
  await pastePng(mounted.textarea);
  expect(mounted.textarea.value).toBe("Keep this unsent draft");
  expect(mounted.host.querySelectorAll(".composer-staged-thumb")).toHaveLength(1);
}

function pressEnter(textarea: HTMLTextAreaElement): void {
  act(() => textarea.dispatchEvent(new KeyboardEvent("keydown", {
    key: "Enter",
    code: "Enter",
    bubbles: true,
    cancelable: true,
  })));
}

function pressSend(mounted: MountedComposer): void {
  act(() => mounted.sendButton.dispatchEvent(new MouseEvent("click", {
    bubbles: true,
    cancelable: true,
    detail: 1,
  })));
}

async function tick(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function waitFor(predicate: () => boolean, description: string): Promise<void> {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (predicate()) return;
    await act(async () => tick());
  }
  throw new Error(`Timed out waiting for ${description}`);
}

async function completeHarnessCatalog(mounted: MountedComposer): Promise<void> {
  await act(async () => {
    mounted.client.harnessReply.resolve([CLAUDE, DISABLED_CODEX]);
    await mounted.client.harnessReply.promise;
    await tick();
  });
  await waitFor(() => mounted.catalog.getHarnesses().loaded, "harness catalog completion");
}

async function failHarnessCatalog(mounted: MountedComposer): Promise<void> {
  const caught = mounted.client.harnessReply.promise.catch(() => undefined);
  await act(async () => {
    mounted.client.harnessReply.reject(new Error("selected engine catalog unavailable"));
    await caught;
    await tick();
  });
  await waitFor(() => mounted.catalog.getHarnesses().error !== null, "harness catalog error");
}

async function finishRun(mounted: MountedComposer): Promise<void> {
  await waitFor(
    () => mounted.client.calls.some(({ method }) => method === methods.QUEUE_COMMAND),
    "Run command RPC",
  );
}

function expectIntentStillPresent(mounted: MountedComposer): void {
  expect(composerDefaults.getSnapshot().harness).toBe("codex");
  expect(mounted.textarea.value).toBe("Keep this unsent draft");
  expect(mounted.host.querySelectorAll(".composer-staged-thumb")).toHaveLength(1);
}

describe("fresh composer harness availability", () => {
  it("blocks an unresolved project without losing the draft and resumes after resolution", async () => {
    const mounted = mountComposer(true);
    await stageDraft(mounted);
    await completeHarnessCatalog(mounted);
    await waitFor(() => mounted.catalog.getModels("claude-code").loaded, "offered model catalog");

    expect(mounted.sendButton.disabled).toBe(true);
    pressEnter(mounted.textarea);
    pressSend(mounted);
    await act(async () => tick());
    expect(mounted.client.durableCalls()).toHaveLength(0);
    expectIntentStillPresent(mounted);

    mounted.setTargetUnavailable(false);
    expect(mounted.sendButton.disabled).toBe(false);
    expectIntentStillPresent(mounted);
    pressSend(mounted);
    await finishRun(mounted);
    expect(mounted.client.calls.some(({ method }) => method === methods.QUEUE_COMMAND)).toBe(true);
  });

  it("disables Send while the selected engine's harness catalog is loading", async () => {
    const mounted = mountComposer();
    await stageDraft(mounted);

    expect(mounted.catalog.getHarnesses().loading).toBe(true);
    expect(mounted.sendButton.disabled).toBe(true);
    expectIntentStillPresent(mounted);

    await completeHarnessCatalog(mounted);
    expect(mounted.catalog.getHarnesses().rows.find((row) => row.id === "codex")?.enabled).toBe(false);
  });

  it("makes Enter a no-op with no durable RPC while the catalog is loading", async () => {
    const mounted = mountComposer();
    await stageDraft(mounted);

    expect(mounted.catalog.getHarnesses().loading).toBe(true);
    pressEnter(mounted.textarea);
    await act(async () => tick());

    expect(mounted.client.durableCalls()).toHaveLength(0);
    expectIntentStillPresent(mounted);
  });

  it("disables Send when selected-engine harness discovery errors", async () => {
    const mounted = mountComposer();
    await stageDraft(mounted);
    await failHarnessCatalog(mounted);

    expect(mounted.catalog.getHarnesses().error).not.toBeNull();
    expect(mounted.sendButton.disabled).toBe(true);
    expectIntentStillPresent(mounted);
  });

  it("makes Enter a no-op with no durable RPC after catalog error", async () => {
    const mounted = mountComposer();
    await stageDraft(mounted);
    await failHarnessCatalog(mounted);

    pressEnter(mounted.textarea);
    await act(async () => tick());

    expect(mounted.client.durableCalls()).toHaveLength(0);
    expectIntentStillPresent(mounted);
  });

  it("sends through an offered alternative after catalog completion without losing the draft early", async () => {
    const mounted = mountComposer();
    await stageDraft(mounted);
    expect(mounted.sendButton.disabled).toBe(true);

    await completeHarnessCatalog(mounted);
    await waitFor(() => mounted.catalog.getModels("claude-code").loaded, "alternative model catalog");

    expect(composerDefaults.getSnapshot().harness).toBe("codex");
    expect(mounted.sendButton.disabled).toBe(false);
    expectIntentStillPresent(mounted);

    pressSend(mounted);
    await finishRun(mounted);

    const create = mounted.client.calls.find(({ method, params }) =>
      method === methods.MUTATE && (params as { op?: string }).op === "createChat",
    );
    expect(create).toBeDefined();
    expect((create!.params as { config: ChatConfig }).config.harness).toBe("claude-code");

    const queued = mounted.client.calls.find(({ method }) => method === methods.QUEUE_COMMAND);
    expect(queued).toBeDefined();
    const payload = queued!.params as {
      command: { kind: string; request: { harness: string; prompt: string; attachments: readonly string[] } };
      transfers: readonly { uploadId: string; fileName: string }[];
    };
    expect(payload.command.kind).toBe("run");
    expect(payload.command.request.harness).toBe("claude-code");
    expect(payload.command.request.prompt).toContain("Keep this unsent draft");
    expect(payload.command.request.attachments).toEqual(["/uploaded/draft.png"]);
    expect(payload.transfers).toHaveLength(1);
    expect(payload.transfers[0]!.fileName).toBe("draft.png");
  });
});
