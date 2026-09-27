// @vitest-environment jsdom

import { act, createElement, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, beforeAll, expect, it, vi } from "vitest";
import { SettingsEngineIndicator } from "../src/components/settings-engine-indicator";

const h = vi.hoisted(() => {
  const ids = ["123e4567-e89b-12d3-a456-426614174000", "123e4567-e89b-12d3-a456-426614174001"];
  let active = ids[0]!;
  const engines = ids.map((id) => ({ baseUrl: id, label: id, deviceId: id }));
  const registry = { engines: ids.map((id, ix) => ({
    key: id, state: "connected", info: { deviceId: id },
    devices: { rows: [{ id, name: ix === 0 ? "Work Laptop" : "Server" }] },
  })) };
  return { ids, engines, registry, get active() { return active; }, set active(value: string) { active = value; } };
});

vi.mock("../src/state/fleet", () => ({
  useFleet: () => ({ active: h.active, engines: h.engines }),
  useFleetRegistry: () => h.registry,
  setActiveDevice: (id: string) => { h.active = id; },
}));
vi.mock("../src/components/ui/PickerCard", async () => {
  const { createElement } = await import("react");
  return { PickerCard: ({ trigger, children }: { trigger: ReactNode; children: ReactNode }) =>
    createElement("div", null, trigger, children) };
});
vi.mock("../src/components/ui/MenuRows", async () => {
  const { createElement } = await import("react");
  return { MenuRow: ({ children, onClick }: { children: ReactNode; onClick: () => void }) =>
    createElement("button", { type: "button", onClick }, children) };
});

beforeAll(() => { (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true; });
afterAll(() => { delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT; });

it("names both settings picker rows and its active trigger from each engine's own registry row", async () => {
  h.active = h.ids[0]!;
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => { root.render(createElement(SettingsEngineIndicator)); });
    expect(container.querySelector(".settings-engine-indicator-label")?.textContent).toBe("Engine Work Laptop");
    expect(Array.from(container.querySelectorAll(".settings-engine-row-host"), (node) => node.textContent))
      .toEqual(["Work Laptop", "Server"]);
    await act(async () => { (container.querySelectorAll("button")[2] as HTMLButtonElement).click(); });
    await act(async () => { root.render(createElement(SettingsEngineIndicator)); });
    expect(container.querySelector(".settings-engine-indicator-label")?.textContent).toBe("Engine Server");
  } finally {
    await act(async () => { root.unmount(); });
    container.remove();
  }
});
