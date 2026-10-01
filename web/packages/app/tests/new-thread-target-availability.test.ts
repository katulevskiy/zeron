// @vitest-environment jsdom

import { act, createElement } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { encodeScopedId } from "@zeron/engine-client";
import type { Chat, Device } from "@zeron/proto";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { composerDefaults } from "../src/lib/composer-draft";
import { sidebarStore } from "../src/state/sidebar";

const h = vi.hoisted(() => ({
  snapshot: null as unknown,
  session: null as unknown,
  now: Date.parse("2026-10-01T00:00:00.000Z"),
}));

vi.mock("../src/state/fleet", () => ({
  useFleetSnapshot: () => h.snapshot,
  useFleetRegistry: () => ({ engines: [] }),
  engineStatesOf: () => new Map(),
}));
vi.mock("../src/state/session-provider", () => ({ useEngineSession: () => h.session }));
vi.mock("../src/state/hooks", () => ({ useNow: () => h.now, useWatchSnapshot: () => null }));
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: ReactNode }) => createElement("a", null, children),
  useNavigate: () => () => Promise.resolve(),
  useRouterState: (options: { select: (state: { location: { pathname: string } }) => unknown }) =>
    options.select({ location: { pathname: "/" } }),
}));
vi.mock("../src/components/composer", () => ({
  Composer: ({
    session,
    chat,
    targetUnavailable = false,
  }: {
    session: { client: { state: string } };
    chat: { spaceId?: string | null };
    targetUnavailable?: boolean;
  }) =>
    createElement("div", {
      "data-composer-session-state": session.client.state,
      "data-composer-space-id": chat.spaceId ?? "",
      "data-composer-target-unavailable": String(targetUnavailable),
    }),
}));
vi.mock("../src/routes/index-page", () => ({ NewThreadCanvas: () => null }));

const ENGINE_A = "engine_a";
const DEVICE_A = encodeScopedId(ENGINE_A, "device-a");
const PROJECT_A = encodeScopedId(ENGINE_A, "project-a");

const deviceA: Device = {
  id: DEVICE_A,
  name: "Engine A",
  platform: "linux",
  version: "1.0",
  createdAt: new Date(h.now).toISOString(),
  lastSeenAt: new Date(h.now).toISOString(),
};

let TargetSelectors: (typeof import("../src/components/composer/new-thread-selectors"))["NewThreadTargetSelectors"];
let ConversationPage: (typeof import("../src/routes/chat-page"))["ConversationPage"];
let root: Root | null = null;
let host: HTMLDivElement | null = null;
let catalogLoad: ReturnType<typeof vi.fn>;

beforeAll(async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  window.matchMedia = (() => ({
    matches: false,
    media: "",
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
  HTMLImageElement.prototype.decode ??= () => Promise.resolve();
  HTMLCanvasElement.prototype.getContext = () => null;
  TargetSelectors = (await import("../src/components/composer/new-thread-selectors")).NewThreadTargetSelectors;
  ConversationPage = (await import("../src/routes/chat-page")).ConversationPage;
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

beforeEach(() => {
  h.snapshot = {
    devices: { rows: [deviceA], loaded: true },
    // The project was deleted from its owning engine after being remembered.
    spaces: { rows: [], loaded: true },
    chats: { rows: [], loaded: true },
    statuses: { rows: [], loaded: true },
    connectivity: { value: null, loaded: false, error: null },
  };
  h.session = {
    engine: { key: ENGINE_A, endpoint: "wss://engine-a.invalid", label: "Engine A", deviceId: DEVICE_A },
    client: {
      state: "connected",
      status: { state: "connected", info: { deviceId: "device-a" } },
      engineInfo: { deviceId: "device-a" },
    },
    catalog: { loadHarnesses: (catalogLoad = vi.fn(async () => {})) },
    transcripts: {},
  };
  composerDefaults.update({ device: DEVICE_A, project: PROJECT_A, noProject: false });
  sidebarStore.resetPrivateState();
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host?.remove();
  host = null;
  composerDefaults.update({ device: null, project: null, noProject: false });
  sidebarStore.resetPrivateState();
  document.body.replaceChildren();
});

function mount(element: ReactNode): HTMLDivElement {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root!.render(element));
  return host;
}

function unmount(): void {
  act(() => root?.unmount());
  root = null;
  host?.remove();
  host = null;
}

function setProjectRows(loaded: boolean): void {
  (h.snapshot as { spaces: { rows: unknown[]; loaded: boolean } }).spaces = { rows: [], loaded };
}

describe("unavailable new-thread targets", () => {
  it("keeps a missing selected project visible instead of relabeling it as no project", () => {
    const mounted = mount(createElement(TargetSelectors));
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("Selected project unavailable");
    expect(composerDefaults.getSnapshot()).toMatchObject({ device: DEVICE_A, project: PROJECT_A, noProject: false });
  });

  it("keeps a selected project loading distinct from no project and blocks catalog loading", () => {
    setProjectRows(false);
    const selectors = mount(createElement(TargetSelectors));
    expect(selectors.querySelector("#picker-project")?.textContent).toContain("Selected project loading");
    unmount();
    const mounted = mount(createElement(ConversationPage));
    expect(mounted.querySelector("[data-composer-target-unavailable]")?.getAttribute("data-composer-target-unavailable")).toBe("true");
    expect(catalogLoad).not.toHaveBeenCalled();
  });

  it("renders an unavailable canvas target even when another engine has cached chat rows", () => {
    h.session = null;
    const cachedChat: Chat = {
      id: encodeScopedId("engine_b", "cached-chat"),
      deviceId: encodeScopedId("engine_b", "device-b"),
      title: "Cached engine B chat",
      archived: false,
      cwd: null,
      branch: null,
      checkoutId: null,
      config: null,
      lastMessagePreview: null,
      lastMessageAt: null,
      createdAt: new Date(h.now).toISOString(),
    };
    (h.snapshot as { chats: { rows: Chat[]; loaded: boolean } }).chats = { rows: [cachedChat], loaded: true };
    const mounted = mount(createElement(ConversationPage));
    expect(mounted.textContent).toContain("Engine unavailable. Sending is disabled until the host connects.");
    expect(mounted.querySelector("[data-composer-session-state]")).toBeNull();
  });

  it("keeps the persistent composer mounted with an explicit unresolved-target submission gate", () => {
    const mounted = mount(createElement(ConversationPage));
    expect(mounted.querySelector("[data-composer-session-state]")?.getAttribute("data-composer-session-state")).toBe("connected");
    expect(mounted.querySelector("[data-composer-target-unavailable]")?.getAttribute("data-composer-target-unavailable")).toBe("true");
    expect(mounted.querySelector("[data-composer-space-id]")?.getAttribute("data-composer-space-id")).toBe(PROJECT_A);
    expect(mounted.textContent).toContain("Selected project unavailable. Choose another project before sending.");
  });
});
