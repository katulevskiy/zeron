// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { encodeScopedId } from "@zeron/engine-client";
import type { Chat, Device, Space } from "@zeron/proto";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
type ComposerComponent = (typeof import("../src/components/composer"))["Composer"];
type TargetSelectorsComponent = (typeof import("../src/components/composer/new-thread-selectors"))["NewThreadTargetSelectors"];
let ComposerImpl: ComposerComponent;
let TargetSelectorsImpl: TargetSelectorsComponent;
import type { PickerCatalog } from "../src/state/picker-catalog";
import type { EngineSession } from "../src/state/engine-session";
import { PHONE_QUERY } from "../src/state/media";
import { chatDrafts, composerDefaults } from "../src/lib/composer-draft";
import { sidebarStore } from "../src/state/sidebar";

const h = vi.hoisted(() => ({
  phone: false,
  now: Date.parse("2026-10-01T00:00:00.000Z"),
  snapshot: null as unknown,
  session: null as unknown,
}));

// These are the live browser data boundaries; the target components, policy,
// composer defaults, and sidebar state remain the real implementations.
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
// The draft-preservation assertion concerns the composer surface, not its
// unrelated harness/model catalog UI.
vi.mock("../src/components/composer-pickers", () => ({ ComposerPickers: () => null }));

const ENGINE_A = "engine_a";
const ENGINE_B = "engine_b";
const DEVICE_A = encodeScopedId(ENGINE_A, "device-a");
const DEVICE_B = encodeScopedId(ENGINE_B, "device-b");
const PROJECT_A = encodeScopedId(ENGINE_A, "project-a");
const PROJECT_B = encodeScopedId(ENGINE_B, "project-b");
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

const deviceA: Device = {
  id: DEVICE_A,
  name: "Engine A",
  platform: "linux",
  version: "1.0",
  createdAt: new Date(h.now).toISOString(),
  lastSeenAt: new Date(h.now).toISOString(),
};
const deviceB: Device = {
  ...deviceA,
  id: DEVICE_B,
  name: "Engine B",
};
const projectA: Space = {
  id: PROJECT_A,
  deviceId: DEVICE_A,
  name: "Project A",
  path: "/workspace/project-a",
  gitDetected: false,
  createdAt: new Date(h.now).toISOString(),
};
const projectB: Space = {
  ...projectA,
  id: PROJECT_B,
  deviceId: DEVICE_B,
  name: "Project B",
  path: "/workspace/project-b",
};

const loadableEmpty = {
  rows: [],
  loaded: false,
  error: null,
  errorKind: null,
  loading: false,
  generation: 0,
};
const catalog = {
  subscribe: () => () => {},
  getHarnesses: () => loadableEmpty,
  subscribeModels: () => () => {},
  getModels: () => loadableEmpty,
  loadModels: async () => {},
} as unknown as PickerCatalog;
const session = {
  engine: { key: ENGINE_A, endpoint: "wss://engine-a.invalid", label: "Engine A", deviceId: DEVICE_A },
  client: {
    state: "connected",
    status: { state: "connected" },
    engineInfo: { deviceId: "device-a", capabilities: [] },
    onStatus: () => () => {},
    call: async () => ({}),
  },
  cache: {},
  catalog,
  transcripts: {},
} as unknown as EngineSession;

let root: Root | null = null;
let host: HTMLDivElement | null = null;

beforeAll(async () => {
  HTMLImageElement.prototype.decode ??= () => Promise.resolve();
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: query === PHONE_QUERY && h.phone,
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
  const [composer, selectors] = await Promise.all([
    import("../src/components/composer"),
    import("../src/components/composer/new-thread-selectors"),
  ]);
  ComposerImpl = composer.Composer;
  TargetSelectorsImpl = selectors.NewThreadTargetSelectors;
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

beforeEach(() => {
  h.snapshot = {
    devices: { rows: [deviceA, deviceB] },
    spaces: { rows: [projectA, projectB] },
    statuses: { rows: [] },
  };
  h.session = session;
  chatDrafts.reset();
  composerDefaults.update({ device: DEVICE_A, project: PROJECT_A, noProject: false });
  sidebarStore.resetPrivateState();
  sidebarStore.setSpaceFilter(PROJECT_A);
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host?.remove();
  host = null;
  document.body.replaceChildren();
  chatDrafts.reset();
  composerDefaults.update({ device: null, project: null, noProject: false });
  sidebarStore.resetPrivateState();
  h.phone = false;
});

function mount(element: ReturnType<typeof createElement>): HTMLDivElement {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root!.render(element));
  return host;
}

function mountSelectors(): HTMLDivElement {
  return mount(createElement(TargetSelectorsImpl));
}

function mountComposer(): HTMLDivElement {
  const chat = {
    id: "",
    branch: null,
    config: null,
    cwd: null,
    deviceId: null,
    spaceId: null,
  } as unknown as Chat;
  const props = {
    session,
    chat,
    catalog,
    transcript: null,
    availableWidth: 720,
  } as Parameters<ComposerComponent>[0];
  return mount(createElement(ComposerImpl, props));
}

function setTarget(device: string | null, project: string | null, noProject: boolean): void {
  composerDefaults.update({ device, project, noProject });
}

function trigger(id: "picker-device" | "picker-project"): HTMLElement {
  const element = host?.querySelector<HTMLElement>(`#${id}`);
  if (element === null || element === undefined) throw new Error(`Missing picker trigger #${id}`);
  return element;
}

function pickerInput(ariaLabel: "Search devices" | "Search projects"): HTMLInputElement {
  const element = document.querySelector<HTMLInputElement>(`input[aria-label="${ariaLabel}"]`);
  if (element === null) throw new Error(`Missing picker input ${ariaLabel}`);
  return element;
}

function rowContaining(text: string): HTMLElement {
  const element = Array.from(document.querySelectorAll<HTMLElement>(".picker-list .menu-row"))
    .find((row) => row.textContent?.includes(text));
  if (element === undefined) throw new Error(`Missing picker row containing ${JSON.stringify(text)}`);
  return element;
}

function press(element: HTMLElement): void {
  act(() => {
    element.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

function pressKey(element: HTMLElement, key: string): void {
  act(() => element.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true })));
}

function expectTarget(device: string | null, project: string | null, noProject: boolean): void {
  expect(composerDefaults.getSnapshot()).toMatchObject({ device, project, noProject });
}

function setTextareaValue(textarea: HTMLTextAreaElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setter === undefined) throw new Error("HTMLTextAreaElement.value setter is unavailable");
  setter.call(textarea, value);
  act(() => textarea.dispatchEvent(new Event("input", { bubbles: true })));
}

async function pastePng(textarea: HTMLTextAreaElement): Promise<void> {
  const file = new File([PNG], "draft.png", { type: "image/png" });
  Object.defineProperty(file, "arrayBuffer", {
    value: () => Promise.resolve(PNG.slice().buffer),
  });
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: { items: [{ kind: "file", getAsFile: () => file }] },
  });
  await act(async () => {
    textarea.dispatchEvent(event);
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

describe.each([
  { viewport: "desktop", phone: false },
  { viewport: "phone", phone: true },
])("new-chat target pickers ($viewport)", ({ phone }) => {
  beforeEach(() => {
    h.phone = phone;
  });

  it("pointer-switching from engine A to B clears the foreign project and sidebar filter", () => {
    const mounted = mountSelectors();
    expect(mounted.querySelector("#picker-device")?.textContent).toContain("Engine A");
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("Project A");

    press(trigger("picker-device"));
    press(rowContaining("Engine B"));

    expectTarget(DEVICE_B, null, true);
    expect(sidebarStore.getSnapshot()).toMatchObject({ spaceFilter: null, lastSpaceId: PROJECT_A });
    expect(mounted.querySelector("#picker-device")?.textContent).toContain("Engine B");
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("No project");
  });

  it("keyboard engine selection retains a project owned by that same engine", () => {
    setTarget(DEVICE_B, PROJECT_B, false);
    const mounted = mountSelectors();

    press(trigger("picker-device"));
    pressKey(pickerInput("Search devices"), "Enter");

    expectTarget(DEVICE_B, PROJECT_B, false);
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("Project B");
  });

  it("pointer-selecting project B also selects engine B", () => {
    const mounted = mountSelectors();

    press(trigger("picker-project"));
    press(rowContaining("Project B"));

    expectTarget(DEVICE_B, PROJECT_B, false);
    expect(mounted.querySelector("#picker-device")?.textContent).toContain("Engine B");
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("Project B");
  });

  it("keyboard-selecting project B also selects engine B", () => {
    setTarget(DEVICE_A, null, true);
    const mounted = mountSelectors();

    press(trigger("picker-project"));
    pressKey(pickerInput("Search projects"), "ArrowUp");
    pressKey(pickerInput("Search projects"), "Enter");

    expectTarget(DEVICE_B, PROJECT_B, false);
    expect(mounted.querySelector("#picker-device")?.textContent).toContain("Engine B");
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("Project B");
  });

  it("keyboard No project keeps engine B despite earlier sidebar project history", () => {
    setTarget(DEVICE_B, PROJECT_B, false);
    const mounted = mountSelectors();

    press(trigger("picker-project"));
    pressKey(pickerInput("Search projects"), "ArrowDown");
    pressKey(pickerInput("Search projects"), "Enter");

    expectTarget(DEVICE_B, null, true);
    expect(sidebarStore.getSnapshot()).toMatchObject({ spaceFilter: null, lastSpaceId: PROJECT_A });
    expect(mounted.querySelector("#picker-device")?.textContent).toContain("Engine B");
    expect(mounted.querySelector("#picker-project")?.textContent).toContain("No project");
  });

  it("dismissal leaves the remembered engine and project unchanged", () => {
    mountSelectors();
    press(trigger("picker-device"));
    pressKey(pickerInput("Search devices"), "Escape");

    expectTarget(DEVICE_A, PROJECT_A, false);
    expect(sidebarStore.getSnapshot()).toMatchObject({ spaceFilter: PROJECT_A, lastSpaceId: PROJECT_A });
  });

  it("keeps the real composer draft and staged attachment while the engine target changes", async () => {
    const mounted = mountComposer();
    const textarea = mounted.querySelector<HTMLTextAreaElement>("textarea.composer-input");
    expect(textarea).not.toBeNull();
    setTextareaValue(textarea!, "Unsent draft survives engine switching");
    await pastePng(textarea!);

    expect(textarea!.value).toBe("Unsent draft survives engine switching");
    expect(mounted.querySelectorAll(".composer-staged-thumb")).toHaveLength(1);

    press(trigger("picker-device"));
    press(rowContaining("Engine B"));

    expectTarget(DEVICE_B, null, true);
    expect(sidebarStore.getSnapshot()).toMatchObject({ spaceFilter: null, lastSpaceId: PROJECT_A });
    expect(textarea!.value).toBe("Unsent draft survives engine switching");
    expect(mounted.querySelectorAll(".composer-staged-thumb")).toHaveLength(1);
  });
});
