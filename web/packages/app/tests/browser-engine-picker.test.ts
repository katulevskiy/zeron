// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { Device } from "@zeron/proto";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DeviceChip } from "../src/components/composer-footer";
import { NewThreadTargetSelectors } from "../src/components/composer/new-thread-selectors";
import { PHONE_QUERY } from "../src/state/media";

const h = vi.hoisted(() => ({ phone: false, now: 1_800_000_000_000 }));
vi.mock("../src/state/fleet", () => ({
  useFleetRegistry: () => ({ engines: [] }),
  engineStatesOf: () => new Map(),
  useFleetSnapshot: () => null,
}));
vi.mock("../src/state/session-provider", () => ({ useEngineSession: () => null }));
vi.mock("../src/state/hooks", () => ({ useNow: () => h.now }));
vi.mock("../src/state/sidebar", () => ({
  useSidebar: () => ({ spaceFilter: null, lastSpaceId: null }),
  sidebarStore: {},
}));

const engineA: Device = {
  id: "engine-a", name: "Build server", platform: "linux", version: "1.0",
  lastSeenAt: new Date(h.now).toISOString(), createdAt: new Date(h.now).toISOString(),
};
const engineB: Device = {
  ...engineA, id: "engine-b", name: "Offline workstation", platform: "windows",
  lastSeenAt: new Date(h.now - 3 * 86_400_000).toISOString(),
};
let root: Root | null = null;
let host: HTMLDivElement | null = null;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: query === PHONE_QUERY && h.phone, media: query, onchange: null,
    addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
  Element.prototype.scrollIntoView ??= () => {};
});
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host?.remove();
  host = null;
  h.phone = false;
  document.body.replaceChildren();
});
afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});
function mount(element: ReturnType<typeof createElement>): void {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => root!.render(element));
}
function press(element: HTMLElement): void {
  act(() => {
    element.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 }));
    element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
  });
}

describe.each([false, true])("browser engine picker (phone=%s)", (phone) => {
  it.each([engineA, engineB])("marks $name as selected, never as the browser's device", (selected) => {
    h.phone = phone;
    mount(createElement(DeviceChip, {
      devices: [engineA, engineB], effectiveDevice: selected, ownDeviceId: engineA.id, now: h.now,
    }));
    const trigger = host!.querySelector<HTMLButtonElement>("#picker-device")!;
    expect(trigger.textContent).toContain(selected.name);
    press(trigger);
    const tag = document.querySelector<HTMLElement>(".picker-row-tag")!;
    expect(tag?.textContent).toBe("Selected engine");
    expect(tag?.closest("button")?.textContent).toContain(selected.name);
    expect(document.querySelector(".picker-list")!.textContent).not.toMatch(/\bYou\b|This device/);
    expect(document.querySelectorAll(".picker-row-offline")).toHaveLength(1);
  });

  it("uses Select engine on the canvas with no known host", () => {
    h.phone = phone;
    mount(createElement(NewThreadTargetSelectors));
    expect(host!.querySelector("#picker-device")!.textContent).toContain("Select engine");
    expect(host!.textContent).not.toContain("This device");
  });
});
