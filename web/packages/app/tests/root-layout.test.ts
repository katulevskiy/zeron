// @vitest-environment jsdom
import { act, createElement, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";
import { RootLayout } from "../src/routes/root-layout";

const h = vi.hoisted(() => ({
  authenticated: true,
  clientStates: ["offline", "offline"],
  loaded: false,
  retry: vi.fn(),
}));
vi.mock("@tanstack/react-router", () => ({ Outlet: () => createElement("main", null, "Cached chat page") }));
vi.mock("../src/state/session-provider", () => ({
  EngineSessionProvider: ({ children }: { children: ReactNode }) => children,
  useEngineSession: () => null,
  useEngineRetry: () => h.retry,
}));
vi.mock("../src/state/hooks", () => ({ useEngineStatus: () => null }));
vi.mock("../src/state/fleet", () => ({
  useFleet: () => ({ session: { authenticated: h.authenticated }, engines: [{ key: "a" }, { key: "b" }], status: "signed-out" }),
  useFleetRegistry: () => ({ engines: h.clientStates.map((state, index) => ({
    key: String(index), state: state === "connected" ? "connected" : state === "reconnecting" ? "reconnecting" : "off",
    lastError: state === "parked" ? "Engine identity changed" : "Engine is offline",
    chats: { loaded: h.loaded }, spaces: { loaded: false },
  })) }),
  engineRegistry: { clientFor: (key: string) => ({ state: h.clientStates[Number(key)] }) },
  signOut: vi.fn(), retryBrowserSession: vi.fn(), signIn: vi.fn(),
}));
vi.mock("../src/components/browser-session-gate", () => ({ BrowserSessionGate: () => createElement("div", null, "Sign in gate") }));
vi.mock("../src/components/gate-card", () => ({ GateCard: ({ error }: { error: string }) => createElement("div", { role: "alert" }, error) }));

const container = document.createElement("div");
let root: ReturnType<typeof createRoot>;
afterEach(() => { act(() => root.unmount()); container.innerHTML = ""; h.authenticated = true; h.loaded = false; });
function render(states: string[]) {
  h.clientStates = states;
  root = createRoot(container);
  act(() => root.render(createElement(RootLayout)));
}

test("all owned hosts offline still render the safe cached page before rows load", () => {
  render(["offline", "offline"]);
  expect(container.textContent).toBe("Cached chat page");
});
test("an offline host and a parked host do not gate the offline page", () => {
  render(["offline", "parked"]);
  expect(container.textContent).toBe("Cached chat page");
});
test("every fatal parked host fails closed even with cached rows", () => {
  h.loaded = true;
  render(["parked", "parked"]);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe("Engine identity changed");
  expect(container.querySelector("main")).toBeNull();
});
test("first online dial still waits for verified rows", () => {
  render(["reconnecting", "reconnecting"]);
  expect(container.querySelector(".gate-loading")).not.toBeNull();
});
test("offline entries never bypass browser authentication", () => {
  h.authenticated = false;
  render(["offline", "offline"]);
  expect(container.textContent).toBe("Sign in gate");
  expect(container.querySelector("main")).toBeNull();
});
