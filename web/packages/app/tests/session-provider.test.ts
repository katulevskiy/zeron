// @vitest-environment jsdom
/** Mounted provider ownership regressions. Cookie discovery supplies opaque owned
 * device configurations; only registry timing/resources and unrelated UI are doubled.
 * Real cookie/discovery/RPC behavior is exercised by chat-relay.test.ts. */
import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { HarnessDescriptor, Model } from "@zeron/proto";
import { encodeScopedId } from "@zeron/engine-client";
import { createChat } from "../src/lib/chat-actions";
import type { OwnedEngine, FleetState } from "../src/lib/owned-engine";
import type { EngineSession } from "../src/state/engine-session";
import { EngineSessionProvider, useEngineRetry, useEngineSession, useEngineSessions } from "../src/state/session-provider";

const h = vi.hoisted(() => {
  class FakeClient {
    readonly calls: Array<{ method: string; params: unknown }> = [];
    readonly deferreds: Array<{ method: string; resolve: (value: unknown) => void; reject: (error: unknown) => void }> = [];
    readonly statusListeners = new Set<(status: { state: string }) => void>();
    closeCalls = 0;
    onStatus(listener: (status: { state: string }) => void): () => void {
      this.statusListeners.add(listener);
      return () => { this.statusListeners.delete(listener); };
    }
    call(method: string, params?: unknown): Promise<unknown> {
      this.calls.push({ method, params });
      return new Promise((resolve, reject) => { this.deferreds.push({ method, resolve, reject }); });
    }
    take(method: string) {
      const index = this.deferreds.findIndex(entry => entry.method === method);
      if (index < 0) throw new Error(`no pending ${method}`);
      return this.deferreds.splice(index, 1)[0]!;
    }
    resolveNext(method: string, value: unknown): void { this.take(method).resolve(value); }
    rejectNext(method: string, error: unknown): void { this.take(method).reject(error); }
    close(): void { this.closeCalls++; }
  }
  class FakeCache {
    readonly listeners = new Set<() => void>();
    get listenerCount(): number { return this.listeners.size; }
    subscribe(listener: () => void): () => void { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
    getSnapshot(): null { return null; }
  }
  class FakeRegistry {
    autoSpawn = true;
    readonly restartCalls: Array<{ key: string; expectedDeviceId?: string | null }> = [];
    readonly entries = new Map<string, { endpoint: string; client: FakeClient; cache: FakeCache }>();
    readonly listeners = new Set<() => void>();
    snapshot = { engines: [] as Array<{ key: string; state: string }>, configurationError: null };
    clientFor(key: string) { return this.entries.get(key)?.client ?? null; }
    watchCacheFor(key: string) { return this.entries.get(key)?.cache ?? null; }
    getSnapshot() { return this.snapshot; }
    subscribe(listener: () => void): () => void { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
    syncFrom(engines: readonly { key: string; endpoint: string }[]): void {
      if (!this.autoSpawn) return;
      for (const engine of engines) {
        if (this.entries.get(engine.key)?.endpoint !== engine.endpoint) this.adopt(engine.key, engine.endpoint);
      }
      for (const key of this.entries.keys()) if (!engines.some(engine => engine.key === key)) this.entries.delete(key);
      this.publishRows();
    }
    adopt(key: string, endpoint: string) {
      const entry = { endpoint, client: new FakeClient(), cache: new FakeCache() };
      this.entries.set(key, entry);
      this.publishRows();
      return entry;
    }
    drop(key: string): void { this.entries.delete(key); this.publishRows(); }
    restart(key: string, expectedDeviceId?: string | null): void {
      this.restartCalls.push({ key, expectedDeviceId });
      const entry = this.entries.get(key);
      if (entry) this.adopt(key, entry.endpoint);
    }
    publishRows(): void {
      this.snapshot = { engines: [...this.entries.keys()].sort().map(key => ({ key, state: "connected" })), configurationError: null };
      for (const listener of this.listeners) listener();
    }
  }
  const cells = {
    fleet: { active: null, engines: [], configurationError: null } as import("../src/lib/owned-engine").FleetState,
    fleetListeners: new Set<() => void>(), registry: new FakeRegistry(), pathname: "/",
    sidebar: { spaceFilter: null as string | null, lastSpaceId: null as string | null },
    sidebarListeners: new Set<() => void>(),
    defaults: { device: null as string | null, project: null as string | null, noProject: false },
    defaultsListeners: new Set<() => void>(),
  };
  return { FakeClient, FakeCache, FakeRegistry, cells };
});
vi.mock("../src/state/fleet", async () => {
  const { useSyncExternalStore } = await import("react");
  return {
    useFleet: () => useSyncExternalStore(listener => { h.cells.fleetListeners.add(listener); return () => { h.cells.fleetListeners.delete(listener); }; }, () => h.cells.fleet),
    useFleetRegistry: () => useSyncExternalStore(listener => h.cells.registry.subscribe(listener), () => h.cells.registry.getSnapshot()),
    get engineRegistry() { return h.cells.registry; },
  };
});
vi.mock("@tanstack/react-router", () => ({
  useRouterState: (options: { select: (state: { location: { pathname: string } }) => unknown }) => options.select({ location: { pathname: h.cells.pathname } }),
  useNavigate: () => () => Promise.resolve(),
}));
vi.mock("../src/state/sidebar", async () => {
  const { useSyncExternalStore } = await import("react");
  return {
    useSidebar: () => useSyncExternalStore(
      (listener) => { h.cells.sidebarListeners.add(listener); return () => { h.cells.sidebarListeners.delete(listener); }; },
      () => h.cells.sidebar,
    ),
  };
});
vi.mock("../src/lib/composer-draft", () => ({
  composerDefaults: {
    subscribe: (listener: () => void) => {
      h.cells.defaultsListeners.add(listener);
      return () => { h.cells.defaultsListeners.delete(listener); };
    },
    getSnapshot: () => h.cells.defaults,
  },
}));
vi.mock("../src/state/ui-settings", () => { const settings = {}; return { useUiSettings: () => settings }; });
vi.mock("../src/lib/sounds", () => ({ playSound: () => {}, sessionSoundEnabled: () => false }));
vi.mock("../src/state/attention-gate", () => ({ appAttentionGate: { shouldPlay: () => false } }));
vi.mock("../src/lib/notifications", () => ({
  ConnectivityNotificationState: class { update(): null { return null; } },
  chatBannerTexts: () => ({ title: "", body: "" }), connectivityBannerTexts: () => ({ title: "", body: "" }),
  echoSendPending: () => false, onChatNotificationClick: () => () => {}, postBanner: () => {},
  sessionNotificationState: () => ({}), soundSince: () => null,
}));

const ONE = "engine_one";
const TWO = "engine_two";
const HARNESS: HarnessDescriptor = { id: "claude-code", name: "Claude", supportsSteering: true, steeringMode: "step-boundary", reasoningLevels: ["medium"], installed: true, enabled: true };
const MODEL: Model = { id: "sonnet", label: "Sonnet", reasoningLevels: ["medium"], options: [] };
let revision = 0;
function publish(fleet: FleetState): void {
  h.cells.fleet = fleet;
  h.cells.registry.syncFrom(fleet.engines);
  for (const listener of h.cells.fleetListeners) listener();
}
async function discover(key: string): Promise<OwnedEngine> {
  const engine: OwnedEngine = { key, endpoint: `ws://edge.test/api/browser/device/${key}/ws?revision=${++revision}`, label: key, deviceId: key };
  await act(async () => { publish({ active: key, engines: [...h.cells.fleet.engines.filter(row => row.key !== key), engine], configurationError: null }); });
  return engine;
}
async function discovered(key: string) {
  const engine = await discover(key);
  return { engine, client: h.cells.registry.clientFor(key)!, cache: h.cells.registry.watchCacheFor(key)! };
}
async function rename(engine: OwnedEngine): Promise<void> {
  await act(async () => { publish({ ...h.cells.fleet, engines: h.cells.fleet.engines.map(row => row.key === engine.key ? { ...row, label: "Renamed host" } : row) }); });
}
interface Probe { session: EngineSession | null; sessions: ReadonlyMap<string, EngineSession>; retry: () => void }
const mounted: Array<{ unmount(): void }> = [];
function mountProvider(strict = false) {
  const observed = { current: { session: null, sessions: new Map(), retry: () => {} } as Probe };
  function ProbeComponent() { observed.current = { session: useEngineSession(), sessions: useEngineSessions(), retry: useEngineRetry() }; return null; }
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const tree = createElement(EngineSessionProvider, null, createElement(ProbeComponent));
  act(() => { root.render(strict ? createElement(StrictMode, null, tree) : tree); });
  let unmounted = false;
  const handle = { observed, unmount() { if (unmounted) return; unmounted = true; act(() => { root.unmount(); }); container.remove(); } };
  mounted.push(handle);
  return handle;
}
function sessionOf(handle: ReturnType<typeof mountProvider>, key: string): EngineSession {
  const session = handle.observed.current.sessions.get(key);
  if (!session) throw new Error(`no session for ${key}`);
  return session;
}
beforeAll(() => { Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true }); });
afterAll(() => { delete (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT; });
beforeEach(() => {
  h.cells.registry = new h.FakeRegistry();
  h.cells.fleet = { active: null, engines: [], configurationError: null };
  h.cells.pathname = "/";
  h.cells.sidebar = { spaceFilter: null, lastSpaceId: null };
  h.cells.defaults = { device: null, project: null, noProject: false };
});
afterEach(() => { for (const handle of mounted.splice(0)) handle.unmount(); vi.restoreAllMocks(); document.body.replaceChildren(); });

function setCanvasTarget(target: Partial<typeof h.cells.defaults>): void {
  h.cells.defaults = { ...h.cells.defaults, ...target };
  for (const listener of h.cells.defaultsListeners) listener();
}

function setSidebarTarget(target: Partial<typeof h.cells.sidebar>): void {
  h.cells.sidebar = { ...h.cells.sidebar, ...target };
  for (const listener of h.cells.sidebarListeners) listener();
}
describe("EngineSessionProvider resource lifetime", () => {
  it("an unavailable scoped route never falls through to a different active engine", async () => {
    await discovered(ONE);
    await discovered(TWO);
    h.cells.pathname = `/chat/${encodeURIComponent(encodeScopedId(ONE, "chat-one"))}`;
    h.cells.registry.drop(ONE);
    const handle = mountProvider();
    expect(handle.observed.current.sessions.has(TWO)).toBe(true);
    expect(handle.observed.current.session).toBeNull();
  });

  it("routes a selected canvas device and its catalog to that device's engine", async () => {
    const one = await discovered(ONE);
    await discovered(TWO);
    await act(async () => { setCanvasTarget({ device: encodeScopedId(ONE, "device-one"), noProject: true }); });
    const handle = mountProvider();
    expect(handle.observed.current.session?.engine.key).toBe(ONE);
    let load!: Promise<void>;
    await act(async () => { load = handle.observed.current.session!.catalog.loadHarnesses(); });
    expect(one.client.calls.map((call) => call.method)).toEqual(["ListHarnesses"]);
    expect(h.cells.registry.clientFor(TWO)!.calls).toHaveLength(0);
    await act(async () => { one.client.resolveNext("ListHarnesses", [HARNESS]); await load; });
  });

  it("routes an explicit canvas project over the active engine", async () => {
    const one = await discovered(ONE);
    await discovered(TWO);
    await act(async () => {
      setCanvasTarget({
        device: encodeScopedId(TWO, "device-two"),
        project: encodeScopedId(ONE, "project-one"),
        noProject: false,
      });
    });
    const handle = mountProvider();
    expect(handle.observed.current.session?.engine.key).toBe(ONE);
    let load!: Promise<void>;
    await act(async () => { load = handle.observed.current.session!.catalog.loadModels("claude-code"); });
    expect(one.client.calls.map((call) => call.method)).toEqual(["ListModels"]);
    expect(h.cells.registry.clientFor(TWO)!.calls).toHaveLength(0);
    await act(async () => { one.client.resolveNext("ListModels", [MODEL]); await load; });
  });

  it("does not let sidebar history override an explicit projectless canvas target", async () => {
    await discovered(ONE);
    await discovered(TWO);
    await act(async () => {
      setSidebarTarget({ spaceFilter: null, lastSpaceId: encodeScopedId(ONE, "project-one") });
      setCanvasTarget({ device: encodeScopedId(TWO, "device-two"), project: null, noProject: true });
    });
    const handle = mountProvider();
    expect(handle.observed.current.session?.engine.key).toBe(TWO);
  });

  it("fails closed when a selected canvas engine is unavailable", async () => {
    await discovered(ONE);
    await discovered(TWO);
    await act(async () => { setCanvasTarget({ device: encodeScopedId(ONE, "device-one"), noProject: true }); });
    h.cells.registry.drop(ONE);
    const handle = mountProvider();
    expect(handle.observed.current.sessions.has(TWO)).toBe(true);
    expect(handle.observed.current.session).toBeNull();
  });

  it("keeps per-engine catalogs and creation requests isolated through A → B → A", async () => {
    const one = await discovered(ONE);
    const two = await discovered(TWO);
    await act(async () => { setCanvasTarget({ device: encodeScopedId(ONE, "device-one"), noProject: true }); });
    const handle = mountProvider();

    let firstLoad!: Promise<void>;
    await act(async () => { firstLoad = handle.observed.current.session!.catalog.loadHarnesses(); });
    expect(one.client.calls.map((call) => call.method)).toEqual(["ListHarnesses"]);

    await act(async () => { setCanvasTarget({ device: encodeScopedId(TWO, "device-two"), noProject: true }); });
    expect(handle.observed.current.session?.engine.key).toBe(TWO);
    let secondLoad!: Promise<void>;
    await act(async () => { secondLoad = handle.observed.current.session!.catalog.loadModels("claude-code"); });
    const created = createChat(handle.observed.current.session!.client, {
      deviceId: encodeScopedId(TWO, "device-two"),
      mintId: () => "chat-two",
    });
    expect(two.client.calls.map((call) => call.method)).toEqual(["ListModels", "Mutate"]);
    expect(one.client.calls.map((call) => call.method)).toEqual(["ListHarnesses"]);

    await act(async () => {
      one.client.resolveNext("ListHarnesses", [HARNESS]);
      await firstLoad;
    });
    expect(handle.observed.current.session?.engine.key).toBe(TWO);
    expect(handle.observed.current.session!.catalog.getHarnesses().rows).toEqual([]);

    await act(async () => {
      two.client.resolveNext("ListModels", [{ ...MODEL, id: "model-two" }]);
      two.client.resolveNext("Mutate", null);
      await Promise.all([secondLoad, created]);
    });
    expect(handle.observed.current.session!.catalog.getModels("claude-code").rows.map((model) => model.id)).toEqual(["model-two"]);

    await act(async () => { setCanvasTarget({ device: encodeScopedId(ONE, "device-one"), noProject: true }); });
    expect(handle.observed.current.session?.engine.key).toBe(ONE);
    expect(handle.observed.current.session!.catalog.getHarnesses().rows.map((harness) => harness.id)).toEqual(["claude-code"]);
  });
  it("discovery metadata refresh preserves the live mounted picker catalog", async () => {
    const { engine, client, cache } = await discovered(ONE);
    const handle = mountProvider();
    const before = sessionOf(handle, ONE);
    const dispose = vi.spyOn(before.catalog, "dispose");
    const listener = vi.fn(); before.catalog.subscribe(listener);
    await rename(engine);
    const after = sessionOf(handle, ONE);
    expect(after.engine.label).toBe("Renamed host");
    expect(after.engine.deviceId).toBe(ONE);
    expect(after.client).toBe(client); expect(after.cache).toBe(cache);
    expect(after.catalog).toBe(before.catalog); expect(dispose).not.toHaveBeenCalled();
    let harness!: Promise<void>, models!: Promise<void>;
    await act(async () => { harness = after.catalog.loadHarnesses(); models = after.catalog.loadModels("claude-code"); });
    expect(client.deferreds.map(row => row.method)).toEqual(["ListHarnesses", "ListModels"]);
    await act(async () => { client.resolveNext("ListHarnesses", [HARNESS]); client.resolveNext("ListModels", [MODEL]); await Promise.all([harness, models]); });
    expect(after.catalog.getHarnesses().rows.map(row => row.id)).toEqual(["claude-code"]);
    expect(after.catalog.getModels("claude-code").rows.map(row => row.id)).toEqual(["sonnet"]);
    expect(listener).toHaveBeenCalled();
  });
  it("registry restart replaces resources without a discovery metadata change", async () => {
    const { client, cache } = await discovered(ONE);
    const handle = mountProvider(); const before = sessionOf(handle, ONE);
    const engines = h.cells.fleet.engines; const dispose = vi.spyOn(before.catalog, "dispose");
    await act(async () => { handle.observed.current.retry(); });
    const after = sessionOf(handle, ONE);
    expect(h.cells.fleet.engines).toBe(engines);
    expect(h.cells.registry.restartCalls).toEqual([{ key: ONE, expectedDeviceId: ONE }]);
    expect(after.engine).toBe(before.engine); expect(after.client).toBe(h.cells.registry.clientFor(ONE));
    expect(after.client).not.toBe(client); expect(after.cache).not.toBe(cache); expect(after.catalog).not.toBe(before.catalog);
    expect(dispose).toHaveBeenCalledTimes(1);
    let load!: Promise<void>; await act(async () => { load = after.catalog.loadHarnesses(); });
    const current = h.cells.registry.clientFor(ONE)!;
    expect(current.calls.map(row => row.method)).toEqual(["ListHarnesses"]); expect(client.calls).toHaveLength(0);
    await act(async () => { current.resolveNext("ListHarnesses", [HARNESS]); await load; });
    expect(after.catalog.getHarnesses().loaded).toBe(true); expect(after.catalog.getHarnesses().rows.map(row => row.id)).toEqual(["claude-code"]);
  });
  it("metadata refresh preserves pending harness and model requests and subscribers", async () => {
    const { engine, client } = await discovered(ONE); const handle = mountProvider(); const catalog = sessionOf(handle, ONE).catalog;
    const harnessListener = vi.fn(), modelListener = vi.fn(); catalog.subscribe(harnessListener); catalog.subscribeModels("claude-code", modelListener);
    let harness!: Promise<void>, models!: Promise<void>;
    await act(async () => { harness = catalog.loadHarnesses(); models = catalog.loadModels("claude-code"); });
    const calls = client.calls.length; await rename(engine);
    expect(sessionOf(handle, ONE).catalog).toBe(catalog); expect(client.calls).toHaveLength(calls);
    await act(async () => { client.resolveNext("ListHarnesses", [HARNESS]); client.resolveNext("ListModels", [MODEL]); await Promise.all([harness, models]); });
    expect(catalog.getHarnesses().rows.map(row => row.id)).toEqual(["claude-code"]); expect(catalog.getModels("claude-code").rows.map(row => row.id)).toEqual(["sonnet"]);
    expect(harnessListener).toHaveBeenCalledTimes(2); expect(modelListener).toHaveBeenCalledTimes(2);
  });
  it("registry row updates preserve unchanged maps, wrappers, catalogs and subscriptions", async () => {
    const { cache } = await discovered(ONE); const handle = mountProvider(); const before = sessionOf(handle, ONE); const map = handle.observed.current.sessions;
    const dispose = vi.spyOn(before.catalog, "dispose"); expect(cache.listenerCount).toBe(1);
    for (let i = 0; i < 3; i++) await act(async () => { h.cells.registry.publishRows(); });
    expect(handle.observed.current.sessions).toBe(map); expect(sessionOf(handle, ONE)).toBe(before);
    expect(sessionOf(handle, ONE).catalog).toBe(before.catalog); expect(dispose).not.toHaveBeenCalled(); expect(cache.listenerCount).toBe(1);
  });
  it("changed transport and device removal dispose only displaced catalogs", async () => {
    const one = await discovered(ONE), two = await discovered(TWO); const handle = mountProvider();
    const firstOne = sessionOf(handle, ONE), firstTwo = sessionOf(handle, TWO);
    const oneDispose = vi.spyOn(firstOne.catalog, "dispose"), twoDispose = vi.spyOn(firstTwo.catalog, "dispose");
    const replacement = await discover(ONE);
    expect(replacement.endpoint).not.toBe(one.engine.endpoint);
    const second = sessionOf(handle, ONE); expect(second.engine.endpoint).toBe(replacement.endpoint);
    expect(second.client).toBe(h.cells.registry.clientFor(ONE)); expect(second.catalog).not.toBe(firstOne.catalog);
    expect(oneDispose).toHaveBeenCalledTimes(1); expect(twoDispose).not.toHaveBeenCalled(); expect(sessionOf(handle, TWO)).toBe(firstTwo);
    const secondDispose = vi.spyOn(second.catalog, "dispose");
    await act(async () => { publish({ ...h.cells.fleet, active: TWO, engines: h.cells.fleet.engines.filter(row => row.key !== ONE) }); });
    expect(handle.observed.current.sessions.has(ONE)).toBe(false); expect(secondDispose).toHaveBeenCalledTimes(1); expect(twoDispose).not.toHaveBeenCalled(); expect(sessionOf(handle, TWO)).toBe(firstTwo);
    let load!: Promise<void>; await act(async () => { load = firstTwo.catalog.loadHarnesses(); });
    await act(async () => { two.client.resolveNext("ListHarnesses", [HARNESS]); await load; });
    expect(firstTwo.catalog.getHarnesses().loaded).toBe(true); expect(two.client.calls.map(row => row.method)).toEqual(["ListHarnesses"]);
  });
  it("late success and failure from replaced catalogs cannot land or notify", async () => {
    const { client } = await discovered(ONE); const handle = mountProvider(); const old = sessionOf(handle, ONE).catalog;
    const harnessListener = vi.fn(), modelListener = vi.fn(); old.subscribe(harnessListener); old.subscribeModels("claude-code", modelListener);
    let harness!: Promise<void>, models!: Promise<void>;
    await act(async () => { harness = old.loadHarnesses(); models = old.loadModels("claude-code"); });
    const harnessCalls = harnessListener.mock.calls.length, modelCalls = modelListener.mock.calls.length;
    await act(async () => { handle.observed.current.retry(); });
    const fresh = sessionOf(handle, ONE).catalog; expect(fresh).not.toBe(old);
    let load!: Promise<void>; await act(async () => { load = fresh.loadHarnesses(); });
    await act(async () => { h.cells.registry.clientFor(ONE)!.resolveNext("ListHarnesses", [HARNESS]); await load; });
    await act(async () => { client.resolveNext("ListHarnesses", [{ ...HARNESS, id: "codex" }]); client.rejectNext("ListModels", new Error("late failure")); await Promise.all([harness, models]); });
    expect(old.getHarnesses().rows).toEqual([]); expect(harnessListener).toHaveBeenCalledTimes(harnessCalls); expect(modelListener).toHaveBeenCalledTimes(modelCalls);
    expect(fresh.getHarnesses().rows.map(row => row.id)).toEqual(["claude-code"]);
  });
  it("missing registry resources recover on publication without changing discovery", async () => {
    h.cells.registry.autoSpawn = false; const engine = await discover(ONE); const handle = mountProvider();
    expect(handle.observed.current.sessions.size).toBe(0); expect(handle.observed.current.session).toBeNull();
    const engines = h.cells.fleet.engines;
    await act(async () => { h.cells.registry.adopt(ONE, engine.endpoint); });
    expect(h.cells.fleet.engines).toBe(engines); const session = sessionOf(handle, ONE); expect(session.client).toBe(h.cells.registry.clientFor(ONE));
    let load!: Promise<void>; await act(async () => { load = session.catalog.loadHarnesses(); });
    await act(async () => { h.cells.registry.clientFor(ONE)!.resolveNext("ListHarnesses", [HARNESS]); await load; });
    expect(session.catalog.getHarnesses().loaded).toBe(true); expect(session.catalog.getHarnesses().rows.map(row => row.id)).toEqual(["claude-code"]);
  });
  it("unmount disposes current catalogs exactly once without closing registry clients", async () => {
    const { engine, client } = await discovered(ONE); const handle = mountProvider(); const first = sessionOf(handle, ONE);
    const firstDispose = vi.spyOn(first.catalog, "dispose"); await rename(engine); expect(sessionOf(handle, ONE).catalog).toBe(first.catalog); expect(firstDispose).not.toHaveBeenCalled();
    await act(async () => { handle.observed.current.retry(); }); const second = sessionOf(handle, ONE);
    expect(second.catalog).not.toBe(first.catalog); expect(firstDispose).toHaveBeenCalledTimes(1);
    const secondDispose = vi.spyOn(second.catalog, "dispose"), newClient = h.cells.registry.clientFor(ONE)!;
    handle.unmount(); expect(firstDispose).toHaveBeenCalledTimes(1); expect(secondDispose).toHaveBeenCalledTimes(1); expect(client.closeCalls).toBe(0); expect(newClient.closeCalls).toBe(0);
  });
  it("strict lifecycle replay leaves the adopted catalog usable", async () => {
    const { engine, client } = await discovered(ONE); const handle = mountProvider(true); const first = sessionOf(handle, ONE); const dispose = vi.spyOn(first.catalog, "dispose");
    await rename(engine); const after = sessionOf(handle, ONE); expect(after.engine.label).toBe("Renamed host"); expect(after.catalog).toBe(first.catalog); expect(dispose).not.toHaveBeenCalled();
    let load!: Promise<void>; await act(async () => { load = after.catalog.loadHarnesses(); });
    await act(async () => { client.resolveNext("ListHarnesses", [HARNESS]); await load; });
    expect(after.catalog.getHarnesses().loaded).toBe(true); expect(after.catalog.getHarnesses().rows.map(row => row.id)).toEqual(["claude-code"]);
  });
});
