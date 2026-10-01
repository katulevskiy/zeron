import { encodeScopedId, parseScopedId } from "@zeron/engine-client";
import { UPSTREAM_METHODS as methods } from "@zeron/proto";
import type { SidebarPreferencesState, SidebarPinChange, SidebarSectionChange } from "@zeron/proto";
export type { SidebarPreferencesState } from "@zeron/proto";
import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import type { SidebarSection, UiSettingsStore } from "../state/ui-settings";
import { privateSessionGeneration, isCurrentPrivateSession } from "../state/private-session-generation";

export type SidebarStateClient = Pick<EngineClient, "call" | "watch"> & Partial<Pick<EngineClient, "onStatus">>;
export type SidebarSyncStatus = "loading" | "ready" | "offline" | "unavailable" | "error";
interface Bridge {
  key: string;
  client: SidebarStateClient;
  profile: string;
  session: number;
  watch: WatchHandle | null;
  unsubscribeStatus: (() => void) | null;
  timer: ReturnType<typeof setTimeout> | null;
  generation: number;
  confirmed: SidebarPreferencesState | null;
  pending: SidebarPinChange[];
  running: boolean;
  status: SidebarSyncStatus;
}

export function reconcileOwnedIds(current: readonly string[], next: readonly string[], owner: (id: string) => boolean): string[] {
  const out: string[] = [];
  let index = 0;
  for (const id of current) {
    if (!owner(id)) out.push(id);
    else if (index < next.length) out.push(next[index++]!);
  }
  out.push(...next.slice(index));
  return [...new Set(out)];
}
function owns(id: string, key: string): boolean {
  try { return parseScopedId(id).engine === key; } catch { return false; }
}
function raw(id: string): string { return parseScopedId(id).rawId; }
function sectionRaw(id: string): string { return id.startsWith("engine:v1:") ? raw(id) : id; }
function project(state: SidebarPreferencesState, changes: readonly SidebarPinChange[]): SidebarPreferencesState {
  const next = structuredClone(state);
  for (const change of changes) {
    if (change.action === "section") {
      const c = change.change;
      const section = "id" in c ? next.sections.find(s => s.id === c.id) : undefined;
      if (c.action === "create" && !section) next.sections.push({ id: c.id, name: c.name, session_ids: [], collapsed: false });
      if (c.action === "rename" && section) section.name = c.name;
      if (c.action === "collapse" && section) section.collapsed = c.collapsed;
      if (c.action === "delete") next.sections = next.sections.filter(s => s.id !== c.id);
      if (c.action === "assign") {
        next.pinnedSessionIds = next.pinnedSessionIds.filter(id => id !== c.sessionId);
        for (const s of next.sections) {
          s.session_ids = s.session_ids.filter(id => id !== c.sessionId);
          if (s.id === c.sectionId) { s.session_ids.push(c.sessionId); s.collapsed = false; }
        }
      }
      continue;
    }
    if (change.action === "move" && !next.pinnedSessionIds.includes(change.sessionId)) continue;
    next.pinnedSessionIds = next.pinnedSessionIds.filter(id => id !== change.sessionId);
    if (change.action === "unpin") continue;
    const before = change.before === null ? -1 : next.pinnedSessionIds.indexOf(change.before);
    const after = change.after === null ? -1 : next.pinnedSessionIds.indexOf(change.after);
    next.pinnedSessionIds.splice(before >= 0 ? before : after >= 0 ? after + 1 : next.pinnedSessionIds.length, 0, change.sessionId);
    if (change.action === "pin") for (const s of next.sections) s.session_ids = s.session_ids.filter(id => id !== change.sessionId);
  }
  return next;
}

/** Upstream workspace watch plus per-item intents, never ordered-list replacement.
 * Cache is only a projection: attach/reconnect never imports stale local settings.
 * Pending user intents rebase on new frames; mutation replies reject older frames.
 */
export class SidebarStateSync {
  readonly #settings: UiSettingsStore;
  readonly #bridges = new Map<string, Bridge>();
  readonly #listeners = new Set<() => void>();
  readonly #unsubscribe: () => void;
  #previous: ReturnType<UiSettingsStore["getSnapshot"]>;
  #applying = false;
  #snapshot: Readonly<Record<string, SidebarSyncStatus>> = {};
  constructor(settings: UiSettingsStore) {
    this.#settings = settings;
    this.#previous = settings.getSnapshot();
    this.#unsubscribe = settings.subscribe(() => this.#changed());
  }
  getSnapshot = (): Readonly<Record<string, SidebarSyncStatus>> => this.#snapshot;
  subscribe = (listener: () => void): (() => void) => { this.#listeners.add(listener); return () => { this.#listeners.delete(listener); }; };
  #notify(): void {
    this.#snapshot = Object.fromEntries([...this.#bridges].map(([key, bridge]) => [key, bridge.status]));
    for (const listener of this.#listeners) listener();
  }
  attachedKeys(): readonly string[] { return [...this.#bridges.keys()]; }
  #current(b: Bridge): boolean { return this.#bridges.get(b.key) === b && isCurrentPrivateSession(b.session); }
  attach(key: string, client: SidebarStateClient, profile: string | null): void {
    const old = this.#bridges.get(key);
    if (old?.client === client && old.profile === profile && this.#current(old)) return;
    this.detach(key);
    if (profile === null) return;
    const b: Bridge = { key, client, profile, session: privateSessionGeneration(), watch: null, unsubscribeStatus: null, timer: null, generation: 0, confirmed: null, pending: [], running: false, status: "loading" };
    this.#bridges.set(key, b);
    b.unsubscribeStatus = client.onStatus?.(status => {
      if (!this.#current(b) || b.status === "unavailable") return;
      if (status.state === "reconnecting" || status.state === "closed" || status.state === "parked") {
        b.status = "offline";
        this.#notify();
      }
      // Connected is not readiness: wait for authoritative preferences.
    }) ?? null;
    this.#notify();
    this.#watch(b);
  }
  detach(key: string): void {
    const b = this.#bridges.get(key);
    if (!b) return;
    this.#bridges.delete(key);
    b.watch?.cancel();
    b.unsubscribeStatus?.();
    if (b.timer) clearTimeout(b.timer);
    this.#notify();
  }
  resetPrivateState(): void { for (const key of this.attachedKeys()) this.detach(key); this.#previous = this.#settings.getSnapshot(); }
  dispose(): void { this.resetPrivateState(); this.#unsubscribe(); }
  #watch(b: Bridge): void {
    if (!this.#current(b)) return;
    b.watch = b.client.watch<SidebarPreferencesState>(methods.WATCH_SIDEBAR_PREFERENCES, {}, {
      onItem: (state, context) => {
        if (!this.#current(b)) return;
        if (context.generation !== b.generation) { b.generation = context.generation; b.confirmed = null; }
        this.#frame(b, state);
        void this.#push(b);
      },
      onEnd: error => {
        if (!this.#current(b)) return;
        b.status = error?.kind === "unknown-method" ? "unavailable" : "offline";
        this.#notify();
        if (b.status !== "unavailable") b.timer = setTimeout(() => { b.timer = null; this.#watch(b); }, 2000);
      },
    }, { ackTimeoutMs: 0 });
  }
  #frame(b: Bridge, state: SidebarPreferencesState): void {
    if (!this.#current(b) || !Array.isArray(state.pinnedSessionIds) || !Array.isArray(state.sections)) return;
    if (b.confirmed && state.revision < b.confirmed.revision) return;
    b.confirmed = state;
    if (b.status !== "unavailable") b.status = state.synced || state.initialized ? "ready" : "loading";
    this.#render(b);
    this.#notify();
  }
  #render(b: Bridge): void {
    if (!b.confirmed || !this.#current(b)) return;
    const state = project(b.confirmed, b.pending);
    const settings = this.#settings.getSnapshot();
    const pins = reconcileOwnedIds(settings.sidebarPinnedSessionIdsByProfile[b.profile] ?? [], state.pinnedSessionIds.map(id => encodeScopedId(b.key, id)), id => owns(id, b.key));
    const arrivals: SidebarSection[] = state.sections.map(s => ({ id: encodeScopedId(b.key, s.id), name: s.name, collapsed: s.collapsed, sessionIds: s.session_ids.map(id => encodeScopedId(b.key, id)) }));
    const currentSections = settings.sidebarSectionsByProfile[b.profile] ?? [];
    const order = reconcileOwnedIds(currentSections.map(s => s.id), arrivals.map(s => s.id), id => owns(id, b.key) || !id.startsWith("engine:v1:"));
    const sections = order.map(id => arrivals.find(s => s.id === id) ?? currentSections.find(s => s.id === id)!);
    this.#applying = true;
    try {
      this.#settings.update({ sidebarPinnedSessionIdsByProfile: { ...settings.sidebarPinnedSessionIdsByProfile, [b.profile]: pins }, sidebarSectionsByProfile: { ...settings.sidebarSectionsByProfile, [b.profile]: sections } }, "immediate");
    } finally { this.#previous = this.#settings.getSnapshot(); this.#applying = false; }
  }
  #changed(): void {
    const previous = this.#previous;
    const next = this.#settings.getSnapshot();
    this.#previous = next;
    if (this.#applying) return;
    for (const b of this.#bridges.values()) {
      if (!this.#current(b)) continue;
      // Unknown/unsupported engine state must never turn local cache into truth.
      if (!b.confirmed || b.status !== "ready") {
        this.#applying = true;
        try {
          this.#settings.update({
            sidebarPinnedSessionIdsByProfile: { ...next.sidebarPinnedSessionIdsByProfile, [b.profile]: previous.sidebarPinnedSessionIdsByProfile[b.profile] ?? [] },
            sidebarSectionsByProfile: { ...next.sidebarSectionsByProfile, [b.profile]: previous.sidebarSectionsByProfile[b.profile] ?? [] },
          }, "immediate");
        } finally { this.#previous = this.#settings.getSnapshot(); this.#applying = false; }
        continue;
      }
      const oldPins = (previous.sidebarPinnedSessionIdsByProfile[b.profile] ?? []).filter(id => owns(id, b.key));
      const pins = (next.sidebarPinnedSessionIdsByProfile[b.profile] ?? []).filter(id => owns(id, b.key));
      const oldSections = (previous.sidebarSectionsByProfile[b.profile] ?? []).filter(s => owns(s.id, b.key) || !s.id.startsWith("engine:v1:"));
      const sections = (next.sidebarSectionsByProfile[b.profile] ?? []).filter(s => owns(s.id, b.key) || !s.id.startsWith("engine:v1:"));
      const assigned = new Set(sections.flatMap(s => s.sessionIds));
      for (const id of oldPins) if (!pins.includes(id) && !assigned.has(id)) b.pending.push({ action: "unpin", sessionId: raw(id) });
      let working = oldPins.filter(id => pins.includes(id));
      for (let i = 0; i < pins.length; i++) {
        const id = pins[i]!;
        if (working[i] === id) continue;
        b.pending.push({ action: oldPins.includes(id) ? "move" : "pin", sessionId: raw(id), after: i > 0 ? raw(pins[i - 1]!) : null, before: working.filter(v => v !== id)[i] ? raw(working.filter(v => v !== id)[i]!) : null });
        working = working.filter(v => v !== id); working.splice(i, 0, id);
      }
      const section = (change: SidebarSectionChange) => b.pending.push({ action: "section", change });
      for (const old of oldSections) if (!sections.some(s => s.id === old.id)) section({ action: "delete", id: sectionRaw(old.id) });
      for (const s of sections) {
        const old = oldSections.find(v => v.id === s.id);
        if (!old) section({ action: "create", id: sectionRaw(s.id), name: s.name });
        else if (old.name !== s.name) section({ action: "rename", id: sectionRaw(s.id), name: s.name });
        if ((old?.collapsed ?? false) !== s.collapsed) section({ action: "collapse", id: sectionRaw(s.id), collapsed: s.collapsed });
        for (const id of s.sessionIds) if (owns(id, b.key) && !old?.sessionIds.includes(id)) section({ action: "assign", sessionId: raw(id), sectionId: sectionRaw(s.id) });
      }
      for (const old of oldSections) for (const id of old.sessionIds) {
        if (owns(id, b.key) && !assigned.has(id) && !pins.includes(id) && sections.some(s => s.id === old.id)) section({ action: "assign", sessionId: raw(id), sectionId: null });
      }
      this.#render(b);
      void this.#push(b);
    }
  }
  async #push(b: Bridge): Promise<void> {
    if (!this.#current(b) || b.running || b.status !== "ready") return;
    b.running = true;
    try {
      while (this.#current(b) && b.status === "ready" && b.pending.length) {
        const change = b.pending[0]!;
        const generation = b.generation;
        const reply = await b.client.call<{ sidebarPreferences: SidebarPreferencesState }>(methods.MUTATE, { op: "changeSidebarPin", change });
        if (!this.#current(b)) return;
        b.pending.shift();
        if (generation === b.generation) this.#frame(b, reply.sidebarPreferences);
        else this.#render(b);
      }
    } catch (error) {
      if (!this.#current(b)) return;
      // Transport uncertainty is not permission to retry an old write on reconnect.
      b.pending = [];
      b.status = (error as { kind?: string }).kind === "unknown-method" ? "unavailable" : "error";
      this.#render(b);
      this.#notify();
    } finally { b.running = false; }
  }
}
