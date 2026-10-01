import { afterAll, beforeAll, expect, test, vi } from "vitest";
import { WebSocket as NodeWebSocket } from "ws";
import { EngineClient, RelaySocket, encodeScopedId, parseScopedId } from "../src/index";
import { startBrowserRelayFixture, type BrowserRelayFixture } from "./helpers/browser-relay";
import { statusWhen, waitUntil } from "./helpers/ws";
import { SidebarStateSync } from "../../app/src/lib/sidebar-state-sync";
import { SidebarStore } from "../../app/src/lib/sidebar-store";
import { UiSettingsStore } from "../../app/src/state/ui-settings";

let fixture: BrowserRelayFixture;
const clients: EngineClient[] = [];
const syncs: SidebarStateSync[] = [];
const sockets: NodeWebSocket[] = [];
beforeAll(async () => {
  fixture = await startBrowserRelayFixture();
  // Only the native socket implementation changes in Node. Production
  // RelaySocket/EngineClient, codecs, bridge, edge and engine are all real.
  vi.stubGlobal("WebSocket", class extends NodeWebSocket {
    constructor(url: string) {
      super(url, { headers: {
        Origin: fixture.origin,
        Cookie: fixture.session.browserCookies().map(c => `${c.name}=${c.value}`).join("; "),
      } });
      sockets.push(this);
    }
  });
});
afterAll(async () => {
  syncs.forEach(s => s.dispose()); clients.forEach(c => c.close());
  sockets.forEach(s => s.terminate()); vi.unstubAllGlobals();
  await fixture?.stop();
});
async function viewport(index: number) {
  const engine = fixture.engines[index]!;
  const client = new EngineClient({ endpoint: fixture.session.relayUrl(engine.deviceId), credential: "", expectedDeviceId: engine.deviceId, engineKey: engine.deviceId, webSocket: url => new RelaySocket(url), backoff: { initialMs: 25, maxMs: 100, jitterMs: 1 } });
  clients.push(client);
  client.connect();
  await statusWhen(client, status => status.state === "connected", 15000);
  const settings = new UiSettingsStore();
  const sidebar = new SidebarStore({ settings });
  const sync = new SidebarStateSync(settings); syncs.push(sync);
  const profile = `development:${engine.deviceId}`;
  sync.attach(engine.deviceId, client, profile);
  await waitUntil(() => sync.getSnapshot()[engine.deviceId] === "ready", 20000, "actual upstream sidebar readiness");
  return { client, settings, sidebar, sync, profile, key: engine.deviceId,
    id: (raw: string) => encodeScopedId(engine.deviceId, raw),
    pins: () => (settings.getSnapshot().sidebarPinnedSessionIdsByProfile[profile] ?? []).map(id => parseScopedId(id).rawId),
    sections: () => settings.getSnapshot().sidebarSectionsByProfile[profile] ?? [],
  };
}

test("fork RPCs fail on actual engine; upstream sidebar synchronizes two real cookie-relay viewports", async () => {
  const desktop = await viewport(0);
  const phone = await viewport(1);
  for (const method of ["SetSidebarPins", "SetSidebarSections", "WatchSidebarState"]) {
    await expect(desktop.client.call(method, {})).rejects.toMatchObject({ kind: "unknown-method" });
    console.log(`RED actual EngineCore: ${method} => unknown-method`);
  }
  const a = fixture.engines[0]!.chatId;
  const b = fixture.engines[1]!.chatId;
  desktop.sidebar.setChatPinned(desktop.profile, desktop.id(a), true);
  phone.sidebar.setChatPinned(phone.profile, phone.id(b), true);
  const rawPins = (v: typeof desktop) => v.pins();
  await waitUntil(() => rawPins(desktop).includes(a) && rawPins(desktop).includes(b) && rawPins(phone).includes(a) && rawPins(phone).includes(b), 20000, "concurrent pins converge without whole-list replacement");
  expect(rawPins(desktop)).toEqual(rawPins(phone));
  desktop.sidebar.replacePinsByProfile({ [desktop.profile]: [desktop.id(b), desktop.id(a)] });
  await waitUntil(() => JSON.stringify(rawPins(phone)) === JSON.stringify([b, a]), 15000, "pin order crosses registry and relay");
  const sectionId = desktop.sidebar.createSection(desktop.profile, "Focus");
  expect(sectionId).not.toBeNull();
  await waitUntil(() => phone.sections().some(s => s.name === "Focus"), 15000, "section create");
  const phoneSection = phone.sections().find(s => s.name === "Focus")!;
  const desktopSection = desktop.sections().find(s => s.name === "Focus")!;
  desktop.sidebar.renameSection(desktop.profile, desktopSection.id, "Concurrent name");
  phone.sidebar.setSectionCollapsed(phone.profile, phoneSection.id, true);
  await waitUntil(() => [desktop, phone].every(v => v.sections().some(s => s.name === "Concurrent name" && s.collapsed)), 15000, "unrelated concurrent section fields retained");
  phone.sidebar.assignSidebarSection(phone.profile, phone.id(b), phoneSection.id);
  await waitUntil(() => desktop.sections().some(s => s.sessionIds.includes(desktop.id(b))), 15000, "section assignment");
  await waitUntil(() => !rawPins(desktop).includes(b) && !rawPins(phone).includes(b), 15000, "section/pin exclusive location");
  // Actual host outage while the other viewport edits the same workspace.
  const host = fixture.engines[0]!;
  await host.disconnect(); await fixture.waitForDevice(fixture.session, host, false);
  await waitUntil(() => desktop.client.status?.state !== "connected", 15000, "viewport notices host disconnect");
  phone.sidebar.setChatPinned(phone.profile, phone.id(a), false);
  await waitUntil(() => rawPins(phone).length === 0, 15000, "remote unpin during outage");
  desktop.sidebar.setChatPinned(desktop.profile, desktop.id(a), true);
  await host.reconnect(); await fixture.waitForDevice(fixture.session, host);
  await waitUntil(() => desktop.sync.getSnapshot()[desktop.key] === "ready" && rawPins(desktop).length === 0, 20000, "reconnect reconciles authoritative state, never stale cache import");
  phone.sidebar.deleteSection(phone.profile, phoneSection.id);
  await waitUntil(() => desktop.sections().length === 0 && phone.sections().length === 0, 15000, "section delete");
  console.log(`GREEN production bridge: two real engines ${desktop.key} / ${phone.key}; concurrent pins, order, section create/rename/collapse/assign/delete and host reconnect converged`);
});
