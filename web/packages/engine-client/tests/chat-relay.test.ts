import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, type Browser, type Page } from "playwright";
import { afterEach, beforeEach, expect, test } from "vitest";
import { decodeDeviceFrame, RPC_KIND } from "../src/device-frame";
import { encodeScopedId } from "../src/scoped-id";
import { startBrowserRelayFixture, type BrowserRelayFixture } from "./helpers/browser-relay";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
const evidence = resolve(repository, ".scratch/pr526-unslop/evidence/chat08");
let fixture: BrowserRelayFixture | undefined;
let browser: Browser | undefined;
beforeEach(async () => {
  fixture = await startBrowserRelayFixture({ assetsDirectory: resolve(repository, "web/packages/app/dist"), mockDelayMs: 700 });
  browser = await chromium.launch({ headless: true });
  received.length = 0;
});
afterEach(async () => {
  try { await browser?.close(); }
  finally {
    await fixture?.stop();
    browser = undefined;
    fixture = undefined;
  }
});

interface SentCall { deviceId: string; method: string; params?: Record<string, unknown> }
const received: unknown[] = [];
function observeCalls(page: Page): SentCall[] {
  const calls: SentCall[] = [];
  page.on("websocket", socket => {
    const match = /\/api\/browser\/device\/([^/]+)\/ws/.exec(socket.url());
    if (!match) return;
    socket.on("framereceived", ({ payload }) => {
      if (typeof payload === "string") return;
      const decoded = decodeDeviceFrame(payload);
      if (decoded.header.k !== RPC_KIND) return;
      for (const line of new TextDecoder().decode(decoded.payload).trim().split("\n")) {
        received.push({ deviceId: match[1], frame: JSON.parse(line) });
      }
    });
    socket.on("framesent", ({ payload }) => {
      if (typeof payload === "string") return; // relay heartbeats
      const decoded = decodeDeviceFrame(payload);
      if (decoded.header.k !== RPC_KIND) return;
      for (const line of new TextDecoder().decode(decoded.payload).trim().split("\n")) {
        const frame = JSON.parse(line) as { method?: string; params?: Record<string, unknown> };
        if (frame.method) calls.push({ deviceId: match[1]!, method: frame.method, params: frame.params });
      }
    });
  });
  return calls;
}
async function snapshot(page: Page, name: string): Promise<void> {
  await mkdir(evidence, { recursive: true });
  await page.screenshot({ path: resolve(evidence, `${name}.png`), fullPage: true, animations: "disabled", timeout: 5_000 });
}
async function openChat(page: Page, engine: { deviceId: string; chatId: string }): Promise<void> {
  await page.goto(`${fixture!.origin}/chat/${encodeURIComponent(encodeScopedId(engine.deviceId, engine.chatId))}`);
  await page.locator("textarea.composer-input").waitFor({ state: "visible", timeout: 30_000 });
}
async function send(page: Page, prompt: string): Promise<void> {
  const input = page.locator("textarea.composer-input");
  await input.fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
}
async function dropRelay(page: Page, deviceId: string): Promise<number> {
  return page.evaluate(id => {
    const sockets = (window as unknown as { __relayTestSockets: Array<{ socket: WebSocket }> }).__relayTestSockets;
    let index = sockets.length - 1;
    while (index >= 0 && !(sockets[index]!.socket.url.includes(`/device/${id}/`) && sockets[index]!.socket.readyState === WebSocket.OPEN)) index--;
    if (index < 0) throw new Error("No real device socket to drop");
    sockets[index]!.socket.close(1000, "test relay interruption");
    return index;
  }, deviceId);
}

// A genuine browser, native WebSockets, production RelaySocket/EngineClient,
// actual cookie Durable Objects and EngineCore. Only the external harness is scripted.
test("rendered durable send, streamed response, relay reconnect, dedup, selected-device and offline recovery", async () => {
  const context = await browser!.newContext({ viewport: { width: 1440, height: 960 } });
  await context.addCookies(fixture!.session.browserCookies());
  // Keep references to the REAL native sockets solely to drop a connection and
  // redeliver an already received old frame. No RPC response/server is fabricated.
  await context.addInitScript(() => {
    const NativeSocket = window.WebSocket;
    const sockets: Array<{ socket: WebSocket; frames: ArrayBuffer[] }> = [];
    Object.assign(window, { __relayTestSockets: sockets });
    window.WebSocket = class extends NativeSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        const captured = { socket: this as WebSocket, frames: [] as ArrayBuffer[] };
        sockets.push(captured);
        this.addEventListener("message", event => { if (event.data instanceof ArrayBuffer) captured.frames.push(event.data.slice(0)); });
      }
    };
  });
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const calls = observeCalls(page);
  const [a, b] = fixture!.engines;
  try {
    await openChat(page, a!);
    await send(page, "Relay durable prompt A");
    await page.getByText(`Reply from ${a!.label}.`, { exact: false }).first().waitFor({ timeout: 30_000 });
    // Settle each viewport before dropping its real relay socket so a fast
    // recovery cannot race resizing and erase the reconnect screenshot.
    await page.setViewportSize({ width: 390, height: 844 });
    const oldSocketIndex = await dropRelay(page, a!.deviceId);
    await page.locator(".engine-offline-strip").waitFor({ state: "visible", timeout: 40_000 });
    await snapshot(page, "phone-reconnecting");
    await page.setViewportSize({ width: 1440, height: 960 });
    console.log("Real relay closed; waiting for streamed durable recovery");
    await page.getByText("Deterministic fixture response.", { exact: false }).first().waitFor({ timeout: 30_000 });
    await expect.poll(() => calls.filter(call => call.deviceId === a!.deviceId && call.method === "WatchDocMessages").length).toBeGreaterThan(1);
    await page.locator(".engine-offline-strip").waitFor({ state: "hidden", timeout: 30_000 });
    // Refuse only this real relay's fresh upgrade attempts in Chromium's
    // network stack while photographing its outage. Cookie/session HTTP
    // stays live, and no RPC endpoint or reply is substituted.
    const network = await context.newCDPSession(page);
    await network.send("Network.enable");
    await network.send("Network.setBlockedURLs", { urls: [fixture!.session.relayUrl(a!.deviceId)] });
    await dropRelay(page, a!.deviceId);
    // Native close handshakes can wait for Chromium's TCP close deadline.
    await page.locator(".engine-offline-strip").waitFor({ state: "visible", timeout: 75_000 });
    await snapshot(page, "desktop-reconnecting");
    await network.send("Network.setBlockedURLs", { urls: [] });
    await network.detach();
    await expect.poll(() => calls.filter(call => call.deviceId === a!.deviceId && call.method === "WatchDocMessages").length).toBeGreaterThan(2);
    await page.locator(".engine-offline-strip").waitFor({ state: "hidden", timeout: 30_000 });
    expect(await page.locator(".row-user").count()).toBe(1);
    expect(calls.filter(call => call.method === "QueueCommand")).toHaveLength(1);
    expect(calls.find(call => call.method === "QueueCommand")?.params).toMatchObject({ chatId: a!.chatId, command: { kind: "run", request: { prompt: "Relay durable prompt A" } } });
    // Old real frames from the previous dial must not regress the new baseline.
    const textBeforeStale = await page.locator(".row-user, .row-md").allTextContents();
    await page.evaluate(index => {
      const entry = (window as unknown as { __relayTestSockets: Array<{ socket: WebSocket; frames: ArrayBuffer[] }> }).__relayTestSockets[index]!;
      // The capture listener remains attached; iterate a frozen copy rather
      // than the live array that receives these deliberately replayed events.
      for (const frame of entry.frames.slice()) entry.socket.dispatchEvent(new MessageEvent("message", { data: frame }));
    }, oldSocketIndex);
    await page.waitForTimeout(150);
    expect(await page.locator(".row-user, .row-md").allTextContents()).toEqual(textBeforeStale);
    await snapshot(page, "desktop-transcript");
    console.log("Real relay recovered transcript without duplicate send");

    await openChat(page, b!);
    await send(page, "Selected device prompt B");
    await page.getByText(`Reply from ${b!.label}. Deterministic fixture response.`, { exact: false }).first().waitFor({ timeout: 30_000 });
    await page.locator(".composer-send-stop").waitFor({ state: "hidden", timeout: 15_000 });
    await page.locator("textarea.composer-input").fill("Next prompt on the selected engine");
    expect(calls.filter(call => call.method === "QueueCommand").map(call => call.deviceId)).toEqual([a!.deviceId, b!.deviceId]);
    expect(await page.locator(".row-user").count()).toBe(1);
    await page.setViewportSize({ width: 390, height: 844 });
    await snapshot(page, "phone-transcript");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.locator("textarea.composer-input").fill("");

    await b!.disconnect();
    await fixture!.waitForDevice(fixture!.session, b!, false);
    await page.locator(".engine-offline-strip").waitFor({ state: "visible", timeout: 15_000 });
    // Wait past discovery's poll so the offline registry contract is exercised.
    await page.waitForTimeout(12_000);
    expect(await page.locator(".row-user").count()).toBe(1);
    await page.locator("textarea.composer-input").fill("Do not send while offline");
    expect(await page.getByRole("button", { name: "Send", exact: true }).isDisabled()).toBe(true);
    await page.getByText("Offline — sending is disabled until the host returns.", { exact: true }).waitFor();
    await snapshot(page, "phone-offline");
    await page.setViewportSize({ width: 1440, height: 960 });
    await snapshot(page, "desktop-offline");
    await b!.reconnect();
    await fixture!.waitForDevice(fixture!.session, b!);
    await page.locator(".engine-offline-strip").waitFor({ state: "hidden", timeout: 30_000 });
    expect(await page.locator(".row-user").count()).toBe(1);
    expect(calls.filter(call => call.method === "QueueCommand")).toHaveLength(2);
    await snapshot(page, "desktop-recovered");
    // With every owned host unavailable, keep the selected cached transcript
    // rather than swapping the whole page to a blank loading/failed gate.
    await Promise.all([a!.disconnect(), b!.disconnect()]);
    await Promise.all([fixture!.waitForDevice(fixture!.session, a!, false), fixture!.waitForDevice(fixture!.session, b!, false)]);
    await page.waitForTimeout(12_000);
    expect(await page.locator(".row-user").count()).toBe(1);
    expect(await page.getByRole("button", { name: "Send", exact: true }).isDisabled()).toBe(true);
    await snapshot(page, "desktop-all-offline");
    await page.setViewportSize({ width: 390, height: 844 });
    await snapshot(page, "phone-all-offline");
    await Promise.all([a!.reconnect(), b!.reconnect()]);
    await Promise.all([fixture!.waitForDevice(fixture!.session, a!), fixture!.waitForDevice(fixture!.session, b!)]);
    await page.locator(".engine-offline-strip").waitFor({ state: "hidden", timeout: 30_000 });
    expect(await page.locator(".row-user").count()).toBe(1);
    expect(calls.filter(call => call.method === "QueueCommand")).toHaveLength(2);
    expect(errors).toEqual([]);
  } catch (error) {
    await snapshot(page, "failure").catch(() => {});
    await writeFile(resolve(evidence, "failure.txt"), `${String(error)}\n${await page.locator("body").innerText()}\n${JSON.stringify({ errors, calls }, null, 2)}`);
    throw error;
  } finally {
    await writeFile(resolve(evidence, "wire-calls.json"), JSON.stringify(calls, null, 2));
    await writeFile(resolve(evidence, "wire-received.json"), JSON.stringify(received, null, 2));
  }
}, 240_000);


test("rendered failed revocation stays gated; retry performs real logout", async () => {
  const context = await browser!.newContext({ viewport: { width: 1440, height: 960 } });
  await context.addCookies(fixture!.session.browserCookies());
  const page = await context.newPage();
  await openChat(page, fixture!.engines[0]!);
  await send(page, "Private prompt before logout");
  await page.getByText("Deterministic fixture response.", { exact: false }).first().waitFor({ timeout: 30_000 });
  await page.locator("textarea.composer-input").fill("Private unsent draft");
  // Drop only the actual logout request. No session/auth response is substituted.
  await page.route("**/api/browser/logout", route => route.abort("failed"));
  await page.getByRole("button", { name: /^Account menu:/ }).click();
  await page.getByRole("menuitem", { name: "Sign out", exact: true }).click();
  await page.getByRole("heading", { name: "Sign-out incomplete" }).waitFor({ timeout: 15_000 });
  expect(await page.locator("textarea.composer-input").count()).toBe(0);
  expect(await page.locator("body").innerText()).not.toContain("Private prompt before logout");
  expect(await (await fixture!.session.request("/api/browser/session")).json()).toMatchObject({ authenticated: true });
  await snapshot(page, "desktop-revocation-failed");
  await page.getByRole("button", { name: "Retry session check", exact: true }).click();
  await page.getByRole("heading", { name: "Sign-out incomplete" }).waitFor();
  await page.unroute("**/api/browser/logout");
  const revoked = page.waitForResponse(response => response.url().endsWith("/api/browser/logout") && response.request().method() === "POST");
  await page.getByRole("button", { name: "Retry sign-out", exact: true }).click();
  expect((await revoked).status()).toBe(200);
  await page.getByRole("heading", { name: "Sign in to Zeron" }).waitFor({ timeout: 15_000 });
  const session = await (await fixture!.session.request("/api/browser/session")).json();
  expect(session).toMatchObject({ authenticated: false });
  await snapshot(page, "desktop-signed-out");
  await page.setViewportSize({ width: 390, height: 844 });
  await snapshot(page, "phone-signed-out");
  await page.reload();
  await page.getByRole("heading", { name: "Sign in to Zeron" }).waitFor();
  expect(await page.locator("textarea.composer-input").count()).toBe(0);
});

test("rendered real cookie account switch does not reveal previous prompts or drafts", async () => {
  const context = await browser!.newContext({ viewport: { width: 1440, height: 960 } });
  await context.addCookies(fixture!.session.browserCookies());
  const page = await context.newPage();
  const a = fixture!.engines[0]!;
  await openChat(page, a);
  await send(page, "Account A confidential prompt");
  await page.getByText("Deterministic fixture response.", { exact: false }).first().waitFor({ timeout: 30_000 });
  await page.locator("textarea.composer-input").fill("Account A confidential draft");
  const b = await fixture!.addEngine({ label: "account-b-engine", ownerId: "relay-owner-b" });
  const accountB = await fixture!.loginAs(b.ownerId);
  await fixture!.waitForDevice(accountB, b);
  await context.clearCookies();
  await context.addCookies(accountB.browserCookies());
  // Retain this browser's actual local/IndexedDB storage across the owner switch.
  await openChat(page, b);
  await send(page, "Account B owned prompt");
  await page.getByText(`Reply from ${b.label}. Deterministic fixture response.`, { exact: false }).first().waitFor({ timeout: 30_000 });
  const text = await page.locator("body").innerText();
  expect(text).not.toContain("Account A confidential");
  expect(text).not.toContain(`Chat on ${a.label}`);
  expect(await page.locator("textarea.composer-input").inputValue()).toBe("");
  await snapshot(page, "desktop-account-b");
  await page.setViewportSize({ width: 390, height: 844 });
  await snapshot(page, "phone-account-b");
  // An old scoped route is not allowed to select the new owner's unrelated device.
  await page.goto(`${fixture!.origin}/chat/${encodeURIComponent(encodeScopedId(a.deviceId, a.chatId))}`);
  await page.getByText("That chat is not in this engine's list.", { exact: true }).waitFor({ timeout: 15_000 });
  expect(await page.locator("body").innerText()).not.toContain("Account A confidential");
});

test("rendered relay identity mismatch fails closed before any durable send", async () => {
  const context = await browser!.newContext({ viewport: { width: 1440, height: 960 } });
  await context.addCookies(fixture!.session.browserCookies());
  const [a, b] = fixture!.engines;
  await context.addInitScript(({ expected, other }) => {
    const NativeSocket = window.WebSocket;
    window.WebSocket = class extends NativeSocket {
      constructor(url: string | URL, protocols?: string | string[]) {
        // Deliberately misroute to another ACTUAL owned host, simulating a
        // bad relay/proxy mapping; actual cookie ownership remains enforced.
        super(String(url).replace(`/device/${expected}/`, `/device/${other}/`), protocols);
      }
    };
  }, { expected: a!.deviceId, other: b!.deviceId });
  const page = await context.newPage();
  const calls = observeCalls(page);
  await page.goto(`${fixture!.origin}/chat/${encodeURIComponent(encodeScopedId(a!.deviceId, a!.chatId))}`);
  try {
    await page.locator(".connection-pill-label").filter({ hasText: /^Engine changed$/ }).waitFor({ timeout: 30_000 });
  } catch (error) {
    await snapshot(page, "identity-failure").catch(() => {});
    await writeFile(resolve(evidence, "identity-failure.txt"), `${String(error)}\n${await page.locator("body").innerText()}\n${JSON.stringify({ calls, received }, null, 2)}`);
    throw error;
  }
  await page.getByText(/Engine identity changed/).waitFor({ timeout: 10_000 });
  expect(calls.filter(call => call.method === "QueueCommand")).toHaveLength(0);
  expect(await page.locator("textarea.composer-input").count()).toBe(0);
  await snapshot(page, "desktop-identity-changed");
  await page.setViewportSize({ width: 390, height: 844 });
  await snapshot(page, "phone-identity-changed");
});


test("new chat safely blocks when the real fixture offers only a disabled Mock harness", async () => {
  // This scenario deliberately disables the fixture-only ClaudeCode provider;
  // Mock remains the real registry's non-offered negative control.
  await fixture!.stop();
  fixture = await startBrowserRelayFixture({
    assetsDirectory: resolve(repository, "web/packages/app/dist"),
    mockDelayMs: 700,
    scriptedClaude: false,
  });
  const context = await browser!.newContext({ viewport: { width: 1440, height: 960 } });
  await context.addCookies(fixture!.session.browserCookies());
  const page = await context.newPage();
  const calls = observeCalls(page);
  await page.goto(fixture!.origin);
  await page.getByText("No agents available", { exact: true }).waitFor({ timeout: 30_000 });
  const catalog = received.flatMap(row => {
    const frame = (row as { frame: { ok?: unknown } }).frame;
    return Array.isArray(frame.ok) ? frame.ok : [];
  });
  expect(catalog).toContainEqual(expect.objectContaining({ id: "mock", installed: true, enabled: false }));
  await page.locator("textarea.composer-input").fill("Cannot choose a disabled agent");
  expect(await page.getByRole("button", { name: "Send", exact: true }).isDisabled()).toBe(true);
  await page.locator("textarea.composer-input").press("Enter");
  expect(await page.locator("textarea.composer-input").inputValue()).toBe("Cannot choose a disabled agent");
  await snapshot(page, "desktop-no-agents");
  await page.setViewportSize({ width: 390, height: 844 });
  await snapshot(page, "phone-no-agents");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(calls.filter(call => call.method === "QueueCommand")).toHaveLength(0);
});


test("an initially offline owned host without cached rows shows an explicit unavailable state", async () => {
  const engine = fixture!.engines[1]!;
  await engine.disconnect();
  await fixture!.waitForDevice(fixture!.session, engine, false);
  const context = await browser!.newContext({ viewport: { width: 390, height: 844 } });
  await context.addCookies(fixture!.session.browserCookies());
  const page = await context.newPage();
  const calls = observeCalls(page);
  await page.goto(`${fixture!.origin}/chat/${encodeURIComponent(encodeScopedId(engine.deviceId, engine.chatId))}`);
  await page.getByText("Engine unavailable. Sending is disabled until the host connects.", { exact: true }).waitFor({ timeout: 15_000 });
  expect(await page.locator("textarea.composer-input").count()).toBe(0);
  expect(calls.filter(call => call.method === "QueueCommand")).toHaveLength(0);
  await snapshot(page, "phone-offline-no-cache");
});
