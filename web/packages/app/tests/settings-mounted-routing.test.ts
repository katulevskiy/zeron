// @vitest-environment jsdom

import { act, createElement, useSyncExternalStore } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentAccountsSnapshot, HarnessDescriptor } from "@zeron/proto";
import { AccountsSettingsPage } from "../src/routes/settings-accounts";
import { AgentsSettingsPage } from "../src/routes/settings-agents";

// Only the fleet/session boundary is replaced. Pages, RPC helpers, picker,
// portaled menu rows and switches are production components.
const fixture = vi.hoisted(() => {
  const ids = ["123e4567-e89b-12d3-a456-426614174000", "123e4567-e89b-12d3-a456-426614174001"];
  const listeners = new Set<() => void>();
  const calls: Array<{ engine: string; method: string; params: unknown }> = [];
  const accounts = (id: string): AgentAccountsSnapshot => ({ accounts: [{ id: `account-${id}`, harness: "codex", email: `${id}@example.test`, active: false, switchable: true, planLabel: null, usageWindows: [] }], warnings: [] });
  const harnesses = (id: string, enabled = false): HarnessDescriptor[] => [{ id: "codex", name: `Codex ${id}`, installed: true, enabled, supportsSteering: false, steeringMode: "step-boundary", reasoningLevels: [] }];
  let delayed: { method: string; resolve: ((value: unknown) => void) | null } | null = null;
  const clients = Object.fromEntries(ids.map((id) => [id, {
    engineInfo: { deviceId: id },
    call: async (method: string, params: unknown) => {
      calls.push({ engine: id, method, params });
      if (fixture.state.offline.includes(id)) throw new Error("Engine offline");
      if (id === ids[0] && delayed?.method === method) {
        return new Promise((resolve) => { delayed!.resolve = resolve; });
      }
      if (method === "ListAgentAccounts" || method === "ActivateAgentAccount") return accounts(id);
      if (method === "ListHarnesses") return harnesses(id);
      if (method === "SetHarnessEnabled") return harnesses(id, (params as { enabled: boolean }).enabled);
      if (method === "GetTitleSettings") return { harness: null, model: null };
      return {};
    },
  }]));
  const engines = ids.map((id, i) => ({ baseUrl: "https://relay.example.test", key: id, endpoint: `wss://relay.example.test/devices/${id}`, label: i === 0 ? "Desktop" : "Laptop", deviceId: id }));
  let state = { active: ids[0] as string | null, engines, offline: [] as string[] };
  return { ids, calls, clients, accounts, harnesses, listeners,
    get state() { return state; },
    update(next: Partial<typeof state>) { state = { ...state, ...next }; listeners.forEach((listener) => listener()); },
    get delayed() { return delayed; }, set delayed(value: typeof delayed) { delayed = value; },
  };
});

vi.mock("../src/state/fleet", () => ({
  useFleet: () => useSyncExternalStore((listener) => { fixture.listeners.add(listener); return () => fixture.listeners.delete(listener); }, () => fixture.state),
  useFleetRegistry: () => ({ engines: fixture.state.engines.map((engine) => ({ key: engine.key, state: fixture.state.offline.includes(engine.key) ? "off" : "connected", devices: { rows: [] }, info: { deviceId: engine.deviceId } })) }),
  setActiveDevice: (id: string) => { if (fixture.state.engines.some((engine) => engine.key === id)) fixture.update({ active: id }); },
}));
vi.mock("../src/state/session-provider", () => ({
  useEngineSession: () => {
    const state = useSyncExternalStore((listener) => { fixture.listeners.add(listener); return () => fixture.listeners.delete(listener); }, () => fixture.state);
    const id = state.active;
    return id === null || !state.engines.some((engine) => engine.key === id) ? null : {
      client: fixture.clients[id], engine: state.engines.find((engine) => engine.key === id), catalog: { loadHarnesses: async () => {} },
    };
  },
}));
vi.mock("../src/state/hooks", () => ({ useNow: () => 0 }));

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = vi.fn((media: string) => ({ matches: false, media, addEventListener: vi.fn(), removeEventListener: vi.fn() })) as unknown as typeof window.matchMedia;
  fixture.delayed = null;
  fixture.calls.length = 0;
  fixture.update({ active: fixture.ids[0]!, engines: fixture.ids.map((id, i) => ({ baseUrl: "https://relay.example.test", key: id, endpoint: `wss://relay.example.test/devices/${id}`, deviceId: id, label: i === 0 ? "Desktop" : "Laptop" })), offline: [] });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
});

async function pickLaptop() {
  await act(async () => { (container.querySelector(".settings-engine-indicator") as HTMLButtonElement).click(); });
  const rows = Array.from(document.querySelectorAll<HTMLButtonElement>(".settings-engine-menu .menu-row"));
  expect(rows.map((row) => row.textContent)).toEqual(["Desktop", "Laptop"]);
  await act(async () => rows[1]!.click());
  expect(container.querySelector(".settings-engine-indicator-label")?.textContent).toBe("Engine Laptop");
}

const cases = [
  ["Accounts", AccountsSettingsPage, "ListAgentAccounts", "ActivateAgentAccount"],
  ["Agents", AgentsSettingsPage, "ListHarnesses", "SetHarnessEnabled"],
] as const;

describe.each(cases)("mounted %s settings", (name, Page, listMethod, actionMethod) => {
  async function action() {
    const control = name === "Accounts"
      ? Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find((button) => button.textContent === "Switch")
      : container.querySelector<HTMLButtonElement>('[role="switch"]');
    expect(control).toBeTruthy();
    await act(async () => control!.click());
    if (name === "Agents") expect(container.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("true");
  }

  it("opens the real two-device picker and routes a control action only to the selected UUID", async () => {
    await act(async () => root.render(createElement(Page)));
    expect(fixture.calls.filter((call) => call.method === listMethod).at(-1)?.engine).toBe(fixture.ids[0]);
    await pickLaptop();
    expect(fixture.calls.filter((call) => call.method === listMethod).at(-1)?.engine).toBe(fixture.ids[1]);
    await action();
    expect(fixture.calls.filter((call) => call.method === actionMethod)).toEqual([
      expect.objectContaining({ engine: fixture.ids[1], params: expect.not.objectContaining({ targetDeviceId: expect.anything() }) }),
    ]);
    for (const call of fixture.calls) expect(call.params).not.toHaveProperty("targetDeviceId");
    expect(fixture.calls.find((call) => call.method === actionMethod)?.params).toMatchObject(
      name === "Accounts" ? { accountId: `account-${fixture.ids[1]}`, harness: "codex" } : { harness: "codex", enabled: true },
    );
  });

  it("renders a single device without an ambiguous picker and routes its action", async () => {
    fixture.update({ engines: fixture.state.engines.slice(0, 1) });
    await act(async () => root.render(createElement(Page)));
    expect(container.querySelector(".settings-engine-indicator")).toBeNull();
    await action();
    expect(fixture.calls.filter((call) => call.method === actionMethod).at(-1)?.engine).toBe(fixture.ids[0]);
  });

  it("does not let the previous engine's late list populate the selected device", async () => {
    fixture.delayed = { method: listMethod, resolve: null };
    await act(async () => root.render(createElement(Page)));
    await pickLaptop();
    const selectedContent = name === "Accounts" ? `${fixture.ids[1]}@example.test` : `Codex ${fixture.ids[1]}`;
    expect(container.textContent).toContain(selectedContent);
    await act(async () => fixture.delayed!.resolve!(name === "Accounts" ? fixture.accounts(fixture.ids[0]!) : fixture.harnesses(fixture.ids[0]!)));
    expect(container.textContent).toContain(selectedContent);
    expect(container.textContent).not.toContain(name === "Accounts" ? `${fixture.ids[0]}@example.test` : `Codex ${fixture.ids[0]}`);
  });

  it("shows the selected offline engine's error instead of loading another device", async () => {
    fixture.update({ active: fixture.ids[1]!, offline: [fixture.ids[1]!] });
    await act(async () => root.render(createElement(Page)));
    expect(container.textContent).toContain("Engine offline");
    expect(container.querySelector(".settings-engine-indicator-label")?.textContent).toBe("Engine Laptop");
    expect(fixture.calls.filter((call) => call.method === listMethod)).toEqual([expect.objectContaining({ engine: fixture.ids[1] })]);
    expect(fixture.calls.every((call) => call.engine === fixture.ids[1])).toBe(true);
  });
  it("preserves selection on reorder, never falls back while offline, and uses an explicit removal fallback", async () => {
    await act(async () => root.render(createElement(Page)));
    await pickLaptop();
    await act(async () => fixture.update({ engines: [...fixture.state.engines].reverse() }));
    expect(container.querySelector(".settings-engine-indicator-label")?.textContent).toBe("Engine Laptop");
    await action();
    expect(fixture.calls.filter((call) => call.method === actionMethod).at(-1)?.engine).toBe(fixture.ids[1]);
    const count = fixture.calls.length;
    await act(async () => fixture.update({ offline: [fixture.ids[1]!] }));
    for (const control of Array.from(container.querySelectorAll<HTMLButtonElement>('button[role="switch"], button.settings-refresh'))) {
      await act(async () => control.click());
    }
    expect(fixture.calls.slice(count).every((call) => call.engine === fixture.ids[1])).toBe(true);
    await act(async () => fixture.update({ active: fixture.ids[0]!, engines: fixture.state.engines.filter((engine) => engine.key === fixture.ids[0]) }));
    expect(container.querySelector(".settings-engine-indicator")).toBeNull();
    await action();
    expect(fixture.calls.filter((call) => call.method === actionMethod).at(-1)?.engine).toBe(fixture.ids[0]);
    await act(async () => fixture.update({ active: null, engines: [] }));
    expect(container.querySelector(".settings-engine-indicator")).toBeNull();
  });
});
