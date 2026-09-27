// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { AgentsSettingsPage } from "../src/routes/settings-agents";
import { AccountsSettingsPage } from "../src/routes/settings-accounts";

const h = vi.hoisted(() => {
  const calls: Array<{ engine: string; method: string; params: unknown }> = [];
  let active = "engine-a";
  const clients = Object.fromEntries(["engine-a", "engine-b"].map((id) => [id, {
    engineInfo: { deviceId: id },
    call: async (method: string, params: unknown) => {
      calls.push({ engine: id, method, params });
      if (method === "ListHarnesses") return [];
      if (method === "ListAgentAccounts") return { accounts: [], warnings: [] };
      if (method === "GetTitleSettings") return { harness: null, model: null };
      return {};
    },
  }]));
  return { calls, clients, get active() { return active; }, set active(id: string) { active = id; } };
});

vi.mock("../src/state/fleet", () => ({
  useFleet: () => ({ active: h.active, engines: [
    { baseUrl: "engine-a", label: "Desktop", deviceId: "engine-a" },
    { baseUrl: "engine-b", label: "Laptop", deviceId: "engine-b" },
  ], session: { authenticated: true }, configurationError: null }),
  useFleetRegistry: () => ({ engines: [] }),
  setActiveDevice: (id: string) => { h.active = id; },
}));
vi.mock("../src/state/session-provider", () => ({
  useEngineSession: () => ({ client: h.clients[h.active], engine: { baseUrl: h.active }, catalog: { loadHarnesses: async () => {} } }),
}));
vi.mock("../src/state/hooks", () => ({
  useWatchSnapshot: () => ({ devices: { rows: [] } }),
  useNow: () => 0,
}));
vi.mock("../src/components/ui/DeviceSwitcher", async () => {
  const { createElement } = await import("react");
  return { DeviceSwitcher: ({ devices, onTargetChange }: {
    devices: readonly { id: string }[];
    onTargetChange: (id: string) => void;
  }) => createElement("button", { "data-testid": "choose-b", disabled: !devices.some((d) => d.id === "engine-b"), onClick: () => onTargetChange("engine-b") }, "Laptop") };
});
vi.mock("../src/components/settings-engine-indicator", async () => {
  const { createElement } = await import("react");
  return { SettingsEngineIndicator: () => createElement("button", { "data-testid": "choose-b", onClick: () => { h.active = "engine-b"; } }, "Laptop") };
});

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({ matches: false, media: query, addEventListener: () => {}, removeEventListener: () => {} })) as unknown as typeof window.matchMedia;
});
afterAll(() => { delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT; });

describe.each([
  ["Agents", AgentsSettingsPage, "ListHarnesses"],
  ["Accounts", AccountsSettingsPage, "ListAgentAccounts"],
] as const)("%s settings", (_name, Page, method) => {
  it("lists the selected edge engine directly, even when the old engine has no synced devices", async () => {
    h.active = "engine-a";
    h.calls.length = 0;
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    try {
      await act(async () => { root.render(createElement(Page)); });
      expect(h.calls.filter((call) => call.method === method).at(-1)).toMatchObject({ engine: "engine-a" });
      const choices = container.querySelectorAll<HTMLButtonElement>("[data-testid=choose-b]");
      expect(choices).toHaveLength(1);
      await act(async () => { choices[0]!.click(); });
      await act(async () => { root.render(createElement(Page)); });
      const lists = h.calls.filter((call) => call.method === method);
      expect(lists.at(-1)).toMatchObject({ engine: "engine-b" });
      expect(lists.at(-1)?.params).not.toHaveProperty("targetDeviceId");
    } finally {
      await act(async () => { root.unmount(); });
      container.remove();
    }
  });
});
