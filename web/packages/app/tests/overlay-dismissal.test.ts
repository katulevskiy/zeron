// @vitest-environment jsdom
import { act, createElement, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { RbDialogGlass } from "../src/components/base/dialog";
import { ESCAPE_PRIORITY, installEscapeLadder, registerEscapeSurface, resolveShellEscape } from "../src/state/escape";
import { overlayKeyboard } from "../src/state/keymap";
import { CommandPalette } from "../src/components/command-palette";
import { AddSpacePalette } from "../src/components/add-space-palette";
import { commandPaletteStore } from "../src/state/command-palette";
import { addSpaceStore } from "../src/state/add-space";
import { sendInterrupt } from "../src/lib/composer-actions";

// Navigation is an external boundary; all palette stores and wrappers are real.
vi.mock("@tanstack/react-router", () => ({ useNavigate: () => () => Promise.resolve() }));
vi.hoisted(() => { HTMLImageElement.prototype.decode = async () => {}; });

let phone = false;
let root: Root;
let host: HTMLDivElement;
const cleanups: (() => void)[] = [];
beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.matchMedia = ((query: string) => ({ matches: query.startsWith("(max-width") && phone, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as typeof window.matchMedia;
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};
  installEscapeLadder();
});
afterEach(async () => {
  await act(async () => root?.unmount());
  host?.remove();
  for (const cleanup of cleanups.splice(0)) cleanup();
  vi.restoreAllMocks();
  commandPaletteStore.forceClose();
  addSpaceStore.forceClose();
  phone = false;
});
async function mount(node: ReturnType<typeof createElement>) {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(node));
}
async function pressEscape() {
  await act(async () => {
    (document.activeElement ?? document.body).dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
  });
}
function observeInterrupt() {
  const rpc = vi.fn();
  const listener = (event: KeyboardEvent) => {
    const result = resolveShellEscape({ key: event.key, blockingOverlay: false, escapeStopsActiveAgent: true, route: "chat", interrupting: false, indicator: "working", selectedChatId: "chat-1" });
    if (result.kind === "interruptChat") void sendInterrupt({ call: rpc }, result.chatId);
  };
  window.addEventListener("keydown", listener);
  cleanups.push(() => window.removeEventListener("keydown", listener));
  return rpc;
}

// A caller-controlled exit window deliberately survives Base UI's instant
// jsdom unmount. overlayOpen is the public contract, not a guessed CSS timer.
function Surface({ priority, close }: { priority: number; close: () => void }) {
  const [status, setStatus] = useState("closed");
  return createElement("div", null,
    createElement("button", { id: "opener", onClick: () => setStatus("open") }, "Open"),
    createElement("button", { id: "drain", onClick: () => setStatus("closed") }, "Exit complete"),
    createElement(RbDialogGlass, {
      open: status === "open", overlayOpen: status !== "closed", overlaySource: "test-glass", escapePriority: priority,
      ariaLabel: "Test palette", finalFocus: () => document.getElementById("opener"),
      onOpenChange: (open) => { if (!open) { close(); setStatus("closing"); } },
      children: createElement("input", { "aria-label": "Search" }),
    }),
  );
}
async function click(id: string) {
  await act(async () => { (document.getElementById(id) as HTMLButtonElement).click(); await new Promise((resolve) => setTimeout(resolve, 30)); });
}
describe.each([false, true])("glass dismissal (phone=%s)", (isPhone) => {
  it("closes once, restores focus and swallows repeated Escape until exit completes", async () => {
    phone = isPhone;
    const close = vi.fn();
    const rpc = observeInterrupt();
    await mount(createElement(Surface, { priority: ESCAPE_PRIORITY.commandPalette, close }));
    document.getElementById("opener")!.focus();
    await click("opener");
    expect(document.querySelector('.rb-dialog-card[aria-label="Test palette"]')).not.toBeNull();
    await pressEscape();
    expect(close).toHaveBeenCalledTimes(1);
    await pressEscape();
    await pressEscape();
    expect(close).toHaveBeenCalledTimes(1);
    expect(rpc).not.toHaveBeenCalled();
    expect(overlayKeyboard.owns()).toBe(true);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 50)); });
    expect(document.activeElement?.id).toBe("opener");
    await click("drain");
    expect(overlayKeyboard.owns()).toBe(false);
    await pressEscape();
    expect(rpc).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ chatId: "chat-1", command: { kind: "interrupt" } }));
  });
  it("honors command-before-blocker and blocker-before-add-project priority", async () => {
    phone = isPhone;
    const blocker = vi.fn(() => true);
    cleanups.push(registerEscapeSurface(ESCAPE_PRIORITY.blockingOverlay, blocker));
    const close = vi.fn();
    await mount(createElement(Surface, { priority: ESCAPE_PRIORITY.commandPalette, close }));
    await click("opener");
    await pressEscape();
    expect(close).toHaveBeenCalledTimes(1);
    expect(blocker).not.toHaveBeenCalled();
    await act(async () => root.render(createElement(Surface, { priority: ESCAPE_PRIORITY.addSpace, close })));
    await click("opener");
    await pressEscape();
    expect(blocker).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledTimes(1);
    expect(document.querySelector('.rb-dialog-card[aria-label="Test palette"][data-open]')).not.toBeNull();
  });
});

// jsdom has no Web Animations API. Only this platform seam is supplied:
// Base UI observes the real data-closed state and waits for `finished`.
// No store lifecycle, dismissal, focus, or keyboard handler is mocked.
function holdExit() {
  let finish = () => {};
  const finished = new Promise<void>((resolve) => { finish = resolve; });
  Object.defineProperty(Element.prototype, "getAnimations", {
    configurable: true,
    value(this: Element) { return this.hasAttribute("data-closed") ? [{ finished }] : []; },
  });
  cleanups.push(() => { delete (Element.prototype as { getAnimations?: unknown }).getAnimations; });
  return finish;
}
async function settle() {
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 60)); });
}
describe.each([false, true])("mounted palettes (phone=%s)", (isPhone) => {
  it.each([
    ["Command palette", CommandPalette, commandPaletteStore],
    ["New project", AddSpacePalette, addSpaceStore],
  ] as const)("opens %s, consumes exit Escape, drains and restores composer focus", async (label, Component, store) => {
    phone = isPhone;
    const finish = holdExit();
    const rpc = observeInterrupt();
    const close = vi.spyOn(store, "close");
    await mount(createElement("div", null,
      createElement("textarea", { id: "composer" }),
      createElement("button", { id: "palette-open", onClick: () => store.open() }, "Open"),
      createElement(Component),
    ));
    document.getElementById("composer")!.focus();
    await click("palette-open");
    await settle();
    const popup = document.querySelector(`.rb-dialog-card[aria-label="${label}"]`)!;
    expect(popup).not.toBeNull();
    expect(document.activeElement).toBe(popup.querySelector("input"));
    close.mockClear();
    await pressEscape();
    expect(store.getSnapshot().status).toBe("closing");
    expect(close).toHaveBeenCalledTimes(1);
    await pressEscape();
    await pressEscape();
    expect(close).toHaveBeenCalledTimes(1);
    expect(store.getSnapshot().status).toBe("closing");
    expect(rpc).not.toHaveBeenCalled();
    expect(overlayKeyboard.owns()).toBe(true);
    await act(async () => finish());
    await settle();
    expect(store.getSnapshot().status).toBe("closed");
    expect(document.querySelector(`.rb-dialog-card[aria-label="${label}"]`)).toBeNull();
    expect(document.activeElement?.id).toBe("composer");
    expect(overlayKeyboard.owns()).toBe(false);
    await pressEscape();
    expect(rpc).toHaveBeenCalledTimes(1);
  });
  it.each([
    ["Command palette", CommandPalette, commandPaletteStore],
    ["New project", AddSpacePalette, addSpaceStore],
  ] as const)("routes %s scrim dismissal through the same close lifecycle", async (_label, Component, store) => {
    phone = isPhone;
    const finish = holdExit();
    const rpc = observeInterrupt();
    await mount(createElement(Component));
    await act(async () => store.open());
    await settle();
    const close = vi.spyOn(store, "close");
    const backdrop = document.querySelector(".modal-glass-backdrop")!;
    await act(async () => {
      backdrop.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
      backdrop.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
      backdrop.dispatchEvent(new MouseEvent("click", { bubbles: true, button: 0 }));
    });
    expect(store.getSnapshot().status).toBe("closing");
    await pressEscape();
    expect(close).toHaveBeenCalledTimes(1);
    expect(rpc).not.toHaveBeenCalled();
    await act(async () => finish());
    await settle();
    expect(store.getSnapshot().status).toBe("closed");
  });
  it("dismisses the mounted add-project Close button exactly once", async () => {
    phone = isPhone;
    const finish = holdExit();
    const rpc = observeInterrupt();
    await mount(createElement(AddSpacePalette));
    await act(async () => addSpaceStore.open());
    await settle();
    const close = vi.spyOn(addSpaceStore, "close");
    await act(async () => (document.querySelector('.add-space-back') as HTMLButtonElement).click());
    expect(addSpaceStore.getSnapshot().status).toBe("closing");
    await pressEscape();
    expect(close).toHaveBeenCalledTimes(1);
    expect(rpc).not.toHaveBeenCalled();
    await act(async () => finish());
    await settle();
    expect(addSpaceStore.getSnapshot().status).toBe("closed");
  });
});

function NestedSurfaces() {
  const [outer, setOuter] = useState(true);
  const [inner, setInner] = useState(false);
  return createElement(RbDialogGlass, {
    open: outer, ariaLabel: "Outer", escapePriority: ESCAPE_PRIORITY.addSpace,
    onOpenChange: setOuter,
    children: createElement("div", null,
      createElement("button", { id: "nested-opener", onClick: () => setInner(true) }, "Rename"),
      createElement(RbDialogGlass, {
        open: inner, ariaLabel: "Inner", escapePriority: ESCAPE_PRIORITY.renameDialog,
        onOpenChange: setInner, finalFocus: () => document.getElementById("nested-opener"),
        children: createElement("input", { "aria-label": "Rename" }),
      }),
    ),
  });
}
it("nested rename closes before add-project and returns focus within the parent", async () => {
  const rpc = observeInterrupt();
  await mount(createElement(NestedSurfaces));
  await click("nested-opener");
  await settle();
  expect(document.querySelector('[aria-label="Inner"][data-open]')).not.toBeNull();
  await pressEscape();
  await settle();
  expect(document.querySelector('[aria-label="Inner"][data-open]')).toBeNull();
  expect(document.querySelector('[aria-label="Outer"][data-open]')).not.toBeNull();
  expect(document.activeElement?.id).toBe("nested-opener");
  await pressEscape();
  await settle();
  expect(document.querySelector('[aria-label="Outer"][data-open]')).toBeNull();
  expect(rpc).not.toHaveBeenCalled();
});
