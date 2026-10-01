import { useMemo, useSyncExternalStore } from "react";
import type { Chat, Device, Space } from "@zeron/proto";
import {
  EngineRegistry,
  BrowserApiError,
  browserActivity,
  browserLogout,
  type BrowserSession,
  encodeScopedId,
  projectRegistrySnapshot,
  fetchBrowserDevices,
  fetchBrowserSession,
  startBrowserLogin,
  RelaySocket,
  relayDeviceUrl,
  type BrowserDevice,
  type EngineRegistrySnapshot,
  type RowSet,
} from "@zeron/engine-client";
import type { ChatStatus, ConnectivitySlot, WatchCacheSnapshot } from "@zeron/engine-client";
import type { OwnedEngine } from "../lib/owned-engine";
import { fleetDeviceRows } from "../lib/devices";
import { fleetSpaceRows } from "../lib/view";
import { beginPrivateSession, endPrivateSession, browserEngineCache } from "./private-session";

/**
 * The engine fleet is the edge's owner-scoped device list (PR #319's
 * browser-session model): the visitor signs in with WorkOS once —
 * through an explicit login redirect — and every owned engine connected
 * to the edge appears here on its own. No manual pairing, no pasted
 * addresses, no client-held credentials: the HttpOnly session cookie
 * authenticates the device relay WebSocket at its upgrade.
 *
 * `edgeFleet` is the session + device store; `engineRegistry` is the fleet
 * supervisor — one supervised relay connection per device, all driven
 * simultaneously, merging every engine's rows under scoped ids.
 */

export type BrowserFleetStatus = "checking" | "ready" | "signed-out" | "signing-in" | "logout-failed" | "error";
export interface EdgeFleetState {
  readonly status: BrowserFleetStatus;
  readonly session: BrowserSession;
  readonly devices: readonly BrowserDevice[];
  readonly active: string | null;
  readonly error: string | null;
}

const EMPTY: EdgeFleetState = {
  status: "checking",
  session: { authenticated: false },
  devices: [],
  active: null,
  error: null,
};

let state: EdgeFleetState = EMPTY;
const listeners = new Set<() => void>();

function setState(next: Partial<EdgeFleetState>): void {
  state = { ...state, ...next };
  for (const listener of listeners) {
    listener();
  }
}

/** The fleet store: the browser session and the devices the edge reports. */
export const edgeFleet = {
  getSnapshot(): EdgeFleetState {
    return state;
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};

/** The registry singleton: every discovered device, supervised concurrently. */
export const engineRegistry = new EngineRegistry({
  cache: browserEngineCache,
  webSocket: (url) => new RelaySocket(url),
  log: import.meta.env.DEV ? (message, detail) => console.debug("[fleet]", message, detail) : undefined,
});

function deviceConfigs(devices: readonly BrowserDevice[]) {
  return devices.map((device) => ({
    key: device.id,
    endpoint: relayDeviceUrl(device.id),
    // The relay socket authenticates with the browser session cookie at
    // the upgrade; the client holds no credential.
    credential: "",
    expectedDeviceId: device.id,
    available: device.online,
  }));
}

/** Reconcile the supervised set with whatever the edge reports. */
function syncRegistry(): void {
  // Offline is availability, not loss of ownership: keep the supervised
  // entry and its cached rows until discovery actually removes the device.
  engineRegistry.sync(deviceConfigs(state.devices), null);
}

/** Verified discovery entries: opaque device identities and cookie relay endpoints. */
function ownedEngines(devices: readonly BrowserDevice[]): readonly OwnedEngine[] {
  return devices.map((device) => ({
    key: device.id,
    endpoint: relayDeviceUrl(device.id),
    label: device.name ?? device.id,
    deviceId: device.id,
  }));
}

let started = false;
let polling: ReturnType<typeof setInterval> | undefined;
let generation = 0;
let refreshingGeneration: number | null = null;
let scope: string | null = null;
let privateTransition = Promise.resolve();
let pendingLogoutToken: string | undefined;
let lastActivity = -Infinity;
const LOGOUT_PENDING_KEY = "zeron.browser.logout-pending";

function logoutPending(): boolean {
  try { return localStorage.getItem(LOGOUT_PENDING_KEY) === "true"; } catch { return pendingLogoutToken !== undefined; }
}
function markLogoutPending(pending: boolean): void {
  try {
    if (pending) localStorage.setItem(LOGOUT_PENDING_KEY, "true");
    else localStorage.removeItem(LOGOUT_PENDING_KEY);
  } catch { /* The in-memory pending token still prevents local re-entry. */ }
}
function stopPolling(): void {
  if (polling !== undefined) clearInterval(polling);
  polling = undefined;
}
function transitionPrivateSession(nextScope: string | null): Promise<void> {
  // forget tears down each entry synchronously, then drains old cache writes.
  const drained = Promise.all(engineRegistry.getSnapshot().engines.map((engine) => engineRegistry.forget(engine.key)));
  privateTransition = privateTransition.catch(() => {}).then(async () => {
    await drained;
    if (nextScope === null) await endPrivateSession();
    else await beginPrivateSession(nextScope);
  });
  return privateTransition;
}
async function clearSession(status: BrowserFleetStatus, error: string | null): Promise<void> {
  generation += 1;
  refreshingGeneration = null;
  scope = null;
  stopPolling();
  setState({ ...EMPTY, status, error });
  await transitionPrivateSession(null);
}
/** Gate actions report cleanup failures without creating an unhandled promise. */
async function clearSessionForGate(status: BrowserFleetStatus, error: string | null): Promise<void> {
  const cleanup = clearSession(status, error);
  const epoch = generation;
  try {
    await cleanup;
  } catch (cleanupError) {
    if (epoch !== generation) return;
    const detail = cleanupError instanceof Error ? cleanupError.message : String(cleanupError);
    setState({ status: status === "logout-failed" ? status : "error", error: [error, detail].filter(Boolean).join(" ") });
  }
}
function engineListSignature(devices: readonly BrowserDevice[]): string {
  return devices.map((device) => `${device.id}\u0000${device.name ?? ""}\u0000${device.online}`).join("\u0001");
}

/** Recheck the cookie session as well as presence; passive checks never send activity. */
async function refreshSession(): Promise<void> {
  let epoch = generation;
  const initialEpoch = epoch;
  if (refreshingGeneration === epoch) return;
  refreshingGeneration = epoch;
  let verified = false;
  try {
    const session = await fetchBrowserSession();
    if (epoch !== generation) return;
    if (!session.authenticated) {
      pendingLogoutToken = undefined;
      markLogoutPending(false);
      await clearSessionForGate("signed-out", null);
      return;
    }
    if (logoutPending()) {
      pendingLogoutToken = session.csrfToken;
      await clearSessionForGate("logout-failed", "Sign-out is incomplete. Retry sign-out before continuing.");
      return;
    }
    const nextScope = `${session.ownerId}\u0000${session.csrfToken}`;
    if (scope !== nextScope) {
      generation += 1;
      epoch = generation;
      scope = nextScope;
      setState({ ...EMPTY, status: "checking" });
      await transitionPrivateSession(nextScope);
      if (epoch !== generation) return;
    }
    // A concurrent retry can see the new scope while its registry drain
    // is still pending. Keep every caller behind the same reset barrier.
    await privateTransition;
    if (epoch !== generation) return;
    verified = true;
    setState({ session, status: "ready" });
    const devices = await fetchBrowserDevices();
    if (epoch !== generation) return;
    const online = devices.filter((device) => device.online);
    const active = state.active !== null && devices.some((device) => device.id === state.active)
      ? state.active : (online[0]?.id ?? devices[0]?.id ?? null);
    if (state.active !== active || state.error !== null || engineListSignature(devices) !== engineListSignature(state.devices)) {
      setState({ devices, active, error: null });
      syncRegistry();
    }
    if (polling === undefined) polling = setInterval(() => { void refreshSession(); }, 10_000);
  } catch (error) {
    if (epoch !== generation) return;
    const message = error instanceof Error ? error.message : String(error);
    if (error instanceof BrowserApiError && (error.status === 401 || error.status === 403)) {
      await clearSessionForGate("signed-out", "Your session expired. Sign in again to continue.");
    } else if (!verified || (error instanceof BrowserApiError && error.status === 501)) {
      await clearSessionForGate("error", message);
    } else {
      setState({ error: message });
      if (polling === undefined) polling = setInterval(() => { void refreshSession(); }, 10_000);
    }
  } finally {
    if (refreshingGeneration === initialEpoch) refreshingGeneration = null;
  }
}

/** Boot behind a signed-out gate; login requires the visitor's explicit action. */
export function startEdgeFleet(): void {
  if (started) return;
  started = true;
  void refreshSession();
}
export async function retryBrowserSession(): Promise<void> {
  started = true;
  await refreshSession();
}
export async function signIn(): Promise<void> {
  if (logoutPending()) {
    setState({ status: "logout-failed", error: "Sign-out is incomplete. Retry sign-out before continuing." });
    return;
  }
  setState({ status: "signing-in", error: null });
  const epoch = generation;
  try {
    const authorizationUrl = await startBrowserLogin();
    if (epoch !== generation) return;
    const url = new URL(authorizationUrl);
    if (url.protocol !== "https:") throw new Error("Browser login returned an unsafe authorization URL");
    window.location.assign(url.href);
  } catch (error) {
    if (epoch === generation) setState({ status: "error", error: error instanceof Error ? error.message : String(error) });
  }
}

/** Switch only among this authenticated owner's devices. */
export function setActiveDevice(deviceId: string): void {
  if (state.session.authenticated && state.devices.some((device) => device.id === deviceId)) setState({ active: deviceId });
}

/** Clear local private state immediately, but only acknowledge successful remote revocation. */
let signOutJob: Promise<void> | undefined;
export function signOut(): Promise<void> {
  if (signOutJob !== undefined) return signOutJob;
  signOutJob = performSignOut().finally(() => { signOutJob = undefined; });
  return signOutJob;
}
async function performSignOut(): Promise<void> {
  const token = state.session.csrfToken ?? pendingLogoutToken;
  pendingLogoutToken = token;
  markLogoutPending(true);
  const cleanup = clearSession("signed-out", null);
  try {
    await Promise.all([cleanup, browserLogout(token ?? "")]);
    pendingLogoutToken = undefined;
    markLogoutPending(false);
  } catch (error) {
    await cleanup.catch(() => {});
    setState({ status: "logout-failed", error: `Sign-out is incomplete: ${error instanceof Error ? error.message : String(error)}. Retry sign-out.` });
    throw error;
  }
}
export async function stopEdgeFleet(): Promise<void> {
  started = false;
  await clearSession("signed-out", null);
}

/** Real foreground interaction, throttled; polling and synthetic events are excluded. */
function recordActivity(event: Event): void {
  if (!event.isTrusted || !state.session.authenticated || !state.session.csrfToken || document.visibilityState === "hidden") return;
  if (Date.now() - lastActivity < 60_000) return;
  lastActivity = Date.now();
  const epoch = generation;
  void browserActivity(state.session.csrfToken).catch(async (error: unknown) => {
    if (epoch === generation && error instanceof BrowserApiError && (error.status === 401 || error.status === 403)) {
      await clearSessionForGate("signed-out", "Your session expired. Sign in again to continue.");
    }
  });
}

startEdgeFleet();
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => { void stopEdgeFleet(); });
  window.addEventListener("pageshow", (event) => { if (event.persisted) startEdgeFleet(); });
  for (const event of ["pointerdown", "keydown"] as const) document.addEventListener(event, recordActivity, { passive: true });
}

const subscribeFleet = (listener: () => void) => edgeFleet.subscribe(listener);
const getFleetSnapshot = () => edgeFleet.getSnapshot();

export function useFleet(): { active: string | null; engines: readonly OwnedEngine[]; session: BrowserSession; status: BrowserFleetStatus; configurationError: string | null } {
  const snapshot = useSyncExternalStore(subscribeFleet, getFleetSnapshot, getFleetSnapshot);
  // `engines` must keep its identity while the device list is unchanged:
  // the session provider reconciles on `fleet.engines`, and
  // `reconcileEngineSessions` clones every session whose OwnedEngine
  // wrapper identity changed — a fresh array here re-ran that effect on
  // every render, cloning sessions forever (the post-login "Maximum
  // update depth exceeded" loop). Owner transitions replace the device list.
  const engines = useMemo(
    () => ownedEngines(snapshot.devices),
    [snapshot.devices],
  );
  return {
    status: snapshot.status,
    active: snapshot.active,
    engines,
    session: snapshot.session,
    configurationError: snapshot.error,
  };
}

const subscribeRegistry = (listener: () => void) => engineRegistry.subscribe(listener);
const getRegistrySnapshot = () => engineRegistry.getSnapshot();

/** The registry's live snapshot: one entry per device + its rows. */
export function useFleetRegistry(): EngineRegistrySnapshot {
  return useSyncExternalStore(subscribeRegistry, getRegistrySnapshot, getRegistrySnapshot);
}

const EMPTY_ROWS: RowSet<never> = { rows: [], loaded: false, error: null };
const NEVER_CONNECTED_SLOT: ConnectivitySlot = { value: null, loaded: false, error: null };
const EMPTY_SNAPSHOT: WatchCacheSnapshot = {
  generation: 0,
  capabilities: [],
  chats: EMPTY_ROWS,
  spaces: EMPTY_ROWS,
  devices: EMPTY_ROWS,
  statuses: EMPTY_ROWS,
  connectivity: NEVER_CONNECTED_SLOT,
};

/**
 * The merged view every fleet-aware surface reads: `projected()` over the
 * registry snapshot, shaped exactly like one engine's `WatchCacheSnapshot`
 * so `chatListRows`/`healedSpaceFilter`/`chatPageRow` and friends operate
 * unchanged — just over more rows. Rows carry scoped ids; a request for one
 * is decoded back to its owning engine at the wire boundary.
 */
export function useFleetSnapshot(): WatchCacheSnapshot {
  const registry = useFleetRegistry();
  const active = useFleet().active;
  return useMemo(() => {
    if (registry.engines.length === 0) {
      return EMPTY_SNAPSHOT;
    }
    const projected = projectRegistrySnapshot(registry);
    return {
      generation: registry.engines.reduce((total, engine) => total + engine.generation, 0),
      capabilities:
        registry.engines.find((engine) => engine.key === active)?.info?.capabilities ?? [],
      chats: mergedRowSet(registry.engines.map((engine) => engine.chats), projected.chats),
      spaces: mergedRowSet(registry.engines.map((engine) => engine.spaces), fleetSpaceRows(projected.spaces)),
      devices: mergedRowSet(registry.engines.map((engine) => engine.devices), fleetDeviceRows(registry, projected.devices)),
      statuses: mergedRowSet(registry.engines.map((engine) => engine.sessions), projected.sessions as ChatStatus[]),
      connectivity: NEVER_CONNECTED_SLOT,
    };
  }, [registry, active]);
}

function mergedRowSet<T>(
  parts: readonly RowSet<T>[],
  rows: readonly T[],
): RowSet<T> {
  const loaded = parts.some((part) => part.loaded);
  // A failed PART degrades only its own engine (badge-level, the entry's
  // lastError); the merged list renders whatever rows exist — a parked or
  // reconnecting engine's cached rows must not be blanked by its own
  // stream error (§2.2: last-known rows keep rendering). The error note
  // surfaces only when there is nothing left to show at all.
  const error =
    rows.length === 0 ? (parts.find((part) => part.error !== null)?.error ?? null) : null;
  return { rows, loaded, error };
}

/**
 * The `EngineConnectionState` of the engine a device id resolves to, keyed
 * by device id — the input to `spaceDeviceTag`'s live-presence override
 * (§2.5: a device backed by a supervised engine reports that engine's
 * connection state, which beats the heartbeat heuristic).
 */
export function engineStatesOf(registry: EngineRegistrySnapshot): Map<string, "connected" | "reconnecting" | "off"> {
  return new Map(registry.engines.map((engine) => [engine.key, engine.state]));
}

/**
 * The fleet's "local device" for group promotion: the ACTIVE engine's own
 * device id, scoped so it matches the projected rows' device ids.
 */
export function fleetLocalDeviceId(registry: EngineRegistrySnapshot, active: string | null): string | null {
  const engine = registry.engines.find((entry) => entry.key === active);
  const deviceId = engine?.info?.deviceId ?? null;
  return engine !== undefined && deviceId !== null ? encodeScopedId(engine.key, deviceId) : null;
}
