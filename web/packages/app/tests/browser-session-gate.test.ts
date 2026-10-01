// @vitest-environment jsdom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { BrowserFleetStatus } from "../src/state/fleet";

const h = vi.hoisted(() => ({
  status: "checking" as BrowserFleetStatus,
  authenticated: false,
  error: null as string | null,
  providerMounts: 0,
  signIn: vi.fn(async () => {}),
  retry: vi.fn(async () => {}),
  signOut: vi.fn(async () => {}),
}));
vi.mock("../src/state/fleet", () => ({
  useFleet: () => ({ session: { authenticated: h.authenticated }, status: h.status, configurationError: h.error, engines: [] }),
  useFleetRegistry: () => ({ engines: [] }),
  signIn: h.signIn, retryBrowserSession: h.retry, signOut: h.signOut,
}));
vi.mock("../src/state/session-provider", () => ({
  EngineSessionProvider: () => { h.providerMounts += 1; return createElement("div", null, "Private engine viewport"); },
  useEngineRetry: () => () => {}, useEngineSession: () => null,
}));
import { RootLayout } from "../src/routes/root-layout";

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  h.status = "checking"; h.authenticated = false; h.error = null; h.providerMounts = 0;
  vi.clearAllMocks();
  container = document.createElement("div"); document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(() => { act(() => root.unmount()); container.remove(); });
function render() { act(() => root.render(createElement(RootLayout))); }
function click(label: string) {
  const button = Array.from(container.querySelectorAll("button")).find((item) => item.textContent === label);
  expect(button).toBeDefined();
  act(() => button!.click());
}

test("session checking never mounts or renders the private engine subtree", () => {
  render();
  expect(container.textContent).toContain("Checking your session");
  expect(container.textContent).not.toContain("Private engine viewport");
  expect(h.providerMounts).toBe(0);
});

test("missing backend is a recoverable gate, not an authenticated viewport", () => {
  h.status = "error"; h.error = "Browser API did not return JSON. Check the backend configuration.";
  render();
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("backend configuration");
  expect(h.providerMounts).toBe(0);
  click("Retry session check"); expect(h.retry).toHaveBeenCalledOnce();
});

test("failed revocation offers a retry without exposing private state or a false success", () => {
  h.status = "logout-failed"; h.error = "Sign-out is incomplete: HTTP 500.";
  render();
  expect(container.textContent).toContain("Sign-out incomplete");
  expect(container.textContent).not.toContain("Sign in with WorkOS");
  expect(h.providerMounts).toBe(0);
  click("Retry sign-out"); expect(h.signOut).toHaveBeenCalledOnce();
});

test("signed-out login is explicit and authenticated users can mount the engine subtree", () => {
  h.status = "signed-out";
  render();
  expect(h.providerMounts).toBe(0);
  expect(h.signIn).not.toHaveBeenCalled();
  click("Sign in with WorkOS"); expect(h.signIn).toHaveBeenCalledOnce();
  h.authenticated = true; h.status = "ready";
  render();
  expect(container.textContent).toContain("Private engine viewport");
});
