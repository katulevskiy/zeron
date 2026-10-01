// @vitest-environment jsdom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { expect, it } from "vitest";
import { SidebarSyncStatusNotice } from "../src/components/sidebar-sync-status";
import type { SidebarSyncStatus } from "../src/lib/sidebar-state-sync";

it("mounted sidebar makes loading, offline, failure and unsupported state explicit", async () => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    const cases: [SidebarSyncStatus, RegExp][] = [
      ["loading", /Waiting for workspace sidebar preferences/],
      ["offline", /read-only until reconnection/],
      ["unavailable", /Sidebar sync unavailable/],
      ["error", /could not be confirmed/],
    ];
    for (const [status, message] of cases) {
      await act(async () => { root.render(createElement(SidebarSyncStatusNotice, { statuses: { engine: status } })); });
      expect(container.querySelector('[role="status"]')?.textContent).toMatch(message);
    }
    await act(async () => { root.render(createElement(SidebarSyncStatusNotice, { statuses: { engine: "ready" } })); });
    expect(container.textContent).toBe("");
  } finally {
    await act(async () => root.unmount()); container.remove();
    delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT;
  }
});
