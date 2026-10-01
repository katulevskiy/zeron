import { afterEach, describe, expect, it } from "vitest";
import { encodeScopedId, RpcError } from "@zeron/engine-client";
import { SidebarStateSync, reconcileOwnedIds, type SidebarPreferencesState, type SidebarStateClient } from "../src/lib/sidebar-state-sync";
import { UiSettingsStore } from "../src/state/ui-settings";
import { SidebarStore } from "../src/lib/sidebar-store";
import { advancePrivateSessionGeneration } from "../src/state/private-session-generation";

const key = "engine-a";
const scoped = (id: string) => encodeScopedId(key, id);
const frame = (revision = 1, pins: string[] = []): SidebarPreferencesState => ({ revision, synced: true, initialized: true, pinnedSessionIds: pins, sections: [] });
const cleanup: (() => void)[] = [];
afterEach(() => { cleanup.splice(0).forEach(fn => fn()); });
function setup() {
  const settings = new UiSettingsStore();
  const sidebar = new SidebarStore({ settings });
  let handlers: Parameters<SidebarStateClient["watch"]>[2];
  const calls: { method: string; params: unknown }[] = [];
  let respond: (params: unknown) => Promise<unknown> = async () => ({ sidebarPreferences: frame(2) });
  const client = {
    watch(method: string, _params: unknown, h: typeof handlers) {
      expect(method).toBe("WatchSidebarPreferences"); handlers = h;
      return { method, cancel() {} };
    },
    async call(method: string, params: unknown) { calls.push({ method, params }); return respond(params); },
  } as SidebarStateClient;
  const sync = new SidebarStateSync(settings);
  cleanup.push(() => sync.dispose());
  sync.attach(key, client, "local");
  return { settings, sidebar, sync, calls, client,
    captureFrame: () => { const captured = handlers; return (state: SidebarPreferencesState) => captured.onItem(state, { generation: 1 }); },
    emit: (state: SidebarPreferencesState, generation = 1) => handlers.onItem(state, { generation }),
    end: (error?: RpcError) => handlers.onEnd?.(error),
    respond: (fn: typeof respond) => { respond = fn; },
  };
}
const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

describe("upstream sidebar preferences bridge", () => {
  it("uses authoritative explicit-empty state and never imports cache on attach", () => {
    const s = setup();
    s.settings.update({ sidebarPinnedSessionIdsByProfile: { local: [scoped("stale")] } }, "immediate");
    s.emit(frame());
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local ?? []).toEqual([]);
    expect(s.calls).toEqual([]);
  });
  it("sends per-pin intents with actual serde op, not fork ordered-list replacement", async () => {
    const s = setup(); s.emit(frame());
    s.respond(async () => ({ sidebarPreferences: frame(2, ["one"]) }));
    s.sidebar.setChatPinned("local", scoped("one"), true);
    await flush();
    expect(s.calls).toEqual([{ method: "Mutate", params: { op: "changeSidebarPin", change: { action: "pin", sessionId: "one", after: null, before: null } } }]);
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local).toEqual([scoped("one")]);
  });
  it("rebases pending intent onto concurrent remote pins and rejects older watch frames", async () => {
    const s = setup(); s.emit(frame());
    let resolve!: (reply: unknown) => void;
    s.respond(() => new Promise(r => { resolve = r; }));
    s.sidebar.setChatPinned("local", scoped("mine"), true);
    s.emit(frame(2, ["remote"]));
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local).toEqual([scoped("remote"), scoped("mine")]);
    resolve({ sidebarPreferences: frame(3, ["remote", "mine"]) }); await flush();
    s.emit(frame(1));
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local).toEqual([scoped("remote"), scoped("mine")]);
  });
  it("reorder emits only item moves, retaining foreign ownership", async () => {
    const s = setup(); s.emit(frame(1, ["a", "b"]));
    const foreign = encodeScopedId("engine-b", "foreign");
    s.respond(async () => ({ sidebarPreferences: frame(2, ["b", "a"]) }));
    s.sidebar.replacePinsByProfile({ local: [scoped("b"), foreign, scoped("a")] }); await flush();
    expect(s.calls[0]).toEqual({ method: "Mutate", params: { op: "changeSidebarPin", change: { action: "move", sessionId: "b", after: null, before: "a" } } });
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local).toContain(foreign);
  });
  it("unknown method is explicitly unavailable, not local-only success", async () => {
    const s = setup(); s.end(new RpcError("unknown-method", "Unknown method: WatchSidebarPreferences"));
    expect(s.sync.getSnapshot()[key]).toBe("unavailable");
    s.sidebar.setChatPinned("local", scoped("one"), true); await flush();
    expect(s.calls).toEqual([]);
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local ?? []).toEqual([]);
  });
  it("unknown/uninitialized preferences cannot be overwritten by local edits", () => {
    const s = setup(); s.emit({ ...frame(), synced: false, initialized: false });
    s.sidebar.setChatPinned("local", scoped("one"), true);
    expect(s.sync.getSnapshot()[key]).toBe("loading");
    expect(s.calls).toEqual([]);
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local ?? []).toEqual([]);
  });
  it("reconnect accepts lower revision on new generation without pushing stale cached pins", () => {
    const s = setup(); s.emit(frame(99, ["old"])); s.emit(frame(1, ["new"]), 2);
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local).toEqual([scoped("new")]);
    expect(s.calls).toEqual([]);
  });
  it("profile detach and owner boundary reject late frames and mutation replies", async () => {
    const s = setup(); s.emit(frame());
    let resolve!: (reply: unknown) => void;
    s.respond(() => new Promise(r => { resolve = r; }));
    s.sidebar.setChatPinned("local", scoped("mine"), true);
    advancePrivateSessionGeneration(); s.sidebar.resetPrivateState();
    s.emit(frame(3, ["secret"])); resolve({ sidebarPreferences: frame(4, ["secret"]) }); await flush();
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile).toEqual({});
    expect(s.calls).toHaveLength(1);
    s.sync.detach(key); s.emit(frame(5, ["secret"]));
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile).toEqual({});
  });
  it("creates sections through supported individual section intent", async () => {
    const s = setup(); s.emit(frame());
    s.respond(async params => {
      const { change: { change } } = params as { change: { change: { id: string; name: string } } };
      return { sidebarPreferences: { ...frame(2), sections: [{ ...change, session_ids: [], collapsed: false }] } };
    });
    const id = s.sidebar.createSection("local", "Focus"); await flush();
    expect(s.calls[0]).toEqual({ method: "Mutate", params: { op: "changeSidebarPin", change: { action: "section", change: { action: "create", id, name: "Focus" } } } });
    expect(s.settings.getSnapshot().sidebarSectionsByProfile.local?.[0]?.name).toBe("Focus");
  });
  it("rolls back failed mutation and exposes error instead of silently keeping local write", async () => {
    const s = setup(); s.emit(frame()); s.respond(async () => { throw new Error("offline"); });
    s.sidebar.setChatPinned("local", scoped("one"), true); await flush();
    expect(s.sync.getSnapshot()[key]).toBe("error");
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local ?? []).toEqual([]);
    s.emit(frame(2, ["remote"]), 2);
    expect(s.calls).toHaveLength(1);
  });
  it("profile reattachment ignores the previous attachment's late frames", () => {
    const s = setup(); s.emit(frame(5, ["old-profile"]));
    const lateFrame = s.captureFrame();
    s.sync.attach(key, s.client, "profile-two");
    s.emit(frame(1, ["new-profile"]));
    lateFrame(frame(999, ["secret-from-old-profile"]));
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile["profile-two"]).toEqual([scoped("new-profile")]);
    expect(s.calls).toEqual([]);
  });
  it("unsupported mutation stays unavailable even if its read-only watch continues", async () => {
    const s = setup(); s.emit(frame());
    s.respond(async () => { throw new RpcError("unknown-method", "Unknown method: Mutate"); });
    s.sidebar.setChatPinned("local", scoped("one"), true); await flush();
    s.emit(frame(2));
    expect(s.sync.getSnapshot()[key]).toBe("unavailable");
    s.sidebar.setChatPinned("local", scoped("two"), true); await flush();
    expect(s.calls).toHaveLength(1);
    expect(s.settings.getSnapshot().sidebarPinnedSessionIdsByProfile.local ?? []).toEqual([]);
  });
  it("owned reconciliation preserves unrelated engine positions", () => {
    expect(reconcileOwnedIds(["a", "foreign", "b"], ["c", "d"], id => id !== "foreign")).toEqual(["c", "foreign", "d"]);
  });
});
