// @vitest-environment jsdom

/**
 * The Devices page lists browser-connected execution engines. Matching the
 * selected engine's own device id is routing information, not evidence that
 * the browser is that device. The mounted page must therefore label the
 * selected host separately and never call a remote host "This device" or
 * "You".
 */

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DevicesSettingsPage } from "../src/routes/settings-devices";

const NOW = 1_800_000_000_000;

const h = vi.hoisted(() => {
  const deviceA = {
    id: "00000000-0000-4000-8000-000000000001",
    name: "Build server",
    platform: "linux",
    version: "0.2.84",
    lastSeenAt: new Date(1_800_000_000_000).toISOString(),
    createdAt: new Date(1_800_000_000_000 - 11 * 86_400_000).toISOString(),
  };
  const deviceB = {
    id: "00000000-0000-4000-8000-000000000002",
    name: "Offline workstation",
    platform: "windows",
    version: "0.2.85",
    lastSeenAt: new Date(1_800_000_000_000 - 3 * 86_400_000).toISOString(),
    createdAt: new Date(1_800_000_000_000 - 9 * 86_400_000).toISOString(),
  };
  const engines = [
    { key: "dev-a", baseUrl: "dev-a", credential: "", label: deviceA.name, sessionId: "owner", pairedAt: 0, deviceId: deviceA.id },
    { key: "dev-b", baseUrl: "dev-b", credential: "", label: deviceB.name, sessionId: "owner", pairedAt: 1, deviceId: deviceB.id },
  ];
  const session = {
    client: {
      engineInfo: { deviceId: deviceA.id as string | null },
      call: async () => ({}),
    },
  };
  const status = { state: "connected" };
  const snapshot = { devices: { rows: [deviceA, deviceB] } };
  const reset = () => {
    session.client.engineInfo.deviceId = deviceA.id;
    status.state = "connected";
    snapshot.devices.rows = [deviceA, deviceB];
  };
  return { deviceA, deviceB, engines, active: "dev-a", session, status, snapshot, reset };
});

vi.mock("../src/state/fleet", () => ({
  useFleet: () => ({
    active: h.active,
    engines: h.engines,
    session: { authenticated: true },
    configurationError: null,
  }),
}));

vi.mock("../src/state/session-provider", () => ({
  useEngineSession: () => h.session,
}));

vi.mock("../src/state/hooks", () => ({
  useEngineStatus: () => h.status,
  useWatchSnapshot: () => h.snapshot,
  useNow: () => NOW,
}));

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
  if (typeof Element.prototype.scrollIntoView !== "function") {
    Element.prototype.scrollIntoView = () => {};
  }
  if (typeof globalThis.requestAnimationFrame !== "function") {
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      callback(0);
      return 0;
    }) as typeof requestAnimationFrame;
  }
});

afterAll(() => {
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

const mounted: Array<() => void> = [];

afterEach(() => {
  while (mounted.length > 0) {
    mounted.pop()!();
  }
  h.active = "dev-a";
  h.reset();
});

function mountPage(): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root: Root | null = createRoot(container);
  mounted.push(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    container.remove();
  });
  act(() => {
    root!.render(createElement(DevicesSettingsPage));
  });
  return container;
}

function rowByName(page: HTMLElement, name: string): HTMLElement {
  const row = Array.from(page.querySelectorAll<HTMLElement>(".device-row")).find(
    (candidate) => candidate.querySelector(".settings-row-title")?.textContent === name,
  );
  if (row === undefined) {
    throw new Error(`Missing device row: ${name}`);
  }
  return row;
}

describe("DevicesSettingsPage — browser engine identity", () => {
  it("renders one row per engine and labels each selected engine without assigning browser identity", () => {
    const pageA = mountPage();
    expect(pageA.querySelectorAll(".settings-row")).toHaveLength(2);
    const selectedA = rowByName(pageA, h.deviceA.name);
    const parkedB = rowByName(pageA, h.deviceB.name);
    expect(selectedA.querySelector(".badge")?.textContent).toBe("Selected engine");
    expect(parkedB.querySelector(".badge")).toBeNull();
    expect(selectedA.querySelector(".settings-meta-line")?.textContent).toContain("Linux");
    expect(selectedA.querySelector(".presence-dot")?.classList).toContain("presence-connected");
    expect(parkedB.querySelector(".settings-meta-line")?.textContent).toContain("Windows");
    expect(parkedB.querySelector(".presence-dot")?.classList).toContain("presence-off");

    h.active = "dev-b";
    h.session.client.engineInfo.deviceId = h.deviceB.id;
    h.status.state = "connected";
    const pageB = mountPage();
    expect(rowByName(pageB, h.deviceA.name).querySelector(".badge")).toBeNull();
    expect(rowByName(pageB, h.deviceB.name).querySelector(".badge")?.textContent).toBe("Selected engine");
    expect(pageB.textContent).not.toContain("This device");
    expect(pageB.textContent).not.toContain("You");
  });

  it("does not invent a selected host when its identity is absent and retains offline presentation", () => {
    h.session.client.engineInfo.deviceId = null;
    h.status.state = "off";
    const page = mountPage();

    expect(page.querySelectorAll(".badge")).toHaveLength(0);
    const offline = rowByName(page, h.deviceB.name);
    expect(offline.querySelector(".settings-meta-line")?.textContent).toContain("Last seen 3d ago");
    expect(offline.querySelector(".presence-dot")?.classList).toContain("presence-off");
    expect(page.textContent).not.toContain("This device");
    expect(page.textContent).not.toContain("You");
  });
});
