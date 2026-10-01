import { describe, expect, it } from "vitest";
import type { EngineEntrySnapshot, EngineConnectionState, EngineRegistrySnapshot } from "@zeron/engine-client";
import { encodeScopedId, parseScopedId } from "@zeron/engine-client";
import type { FleetState, OwnedEngine } from "../src/lib/owned-engine";
import { engineConnection, settingsDeviceName, settingsEngineKey, settingsEngineLabel } from "../src/lib/settings-engine";

function owned(key: string): OwnedEngine {
  return {
    key,
    endpoint: `wss://relay.example.test/api/browser/devices/${key}/relay`,
    label: "Web on Windows",
    deviceId: key,
  };
}

function fleetOf(active: string | null, engines: readonly OwnedEngine[]): FleetState {
  return { active, engines, configurationError: null };
}

function entryOf(state: EngineConnectionState, lastError: string | null): EngineEntrySnapshot {
  return {
    key: "device-a",
    info: null,
    state,
    lastError,
    generation: 1,
    chats: { rows: [], loaded: false, error: null },
    spaces: { rows: [], loaded: false, error: null },
    devices: { rows: [], loaded: false, error: null },
    sessions: { rows: [], loaded: false, error: null },
  };
}

describe("settingsEngineLabel", () => {
  it("names the active edge device only when the fleet is plural", () => {
    const a = { ...owned("device-a"), label: "Workstation" };
    const b = { ...owned("device-b"), label: "Laptop" };
    expect(settingsEngineLabel(fleetOf(null, []))).toBe(null);
    expect(settingsEngineLabel(fleetOf("device-a", [a]))).toBe(null);
    expect(settingsEngineLabel(fleetOf("device-a", [a, b]))).toBe("Workstation");
    expect(settingsEngineLabel(fleetOf("device-b", [a, b]))).toBe("Laptop");
    expect(settingsEngineLabel(fleetOf("device-a", [a, { ...b, label: "" }]))).toBe("Workstation");
    expect(settingsEngineLabel(fleetOf("device-b", [a, { ...b, label: "" }]))).toBe("device-b");
    expect(settingsEngineLabel(fleetOf("missing", [a, b]))).toBe(null);
  });

  it("keeps a relay UUID separate from its transport URL and display label", () => {
    const id = "123e4567-e89b-12d3-a456-426614174000";
    const other = "123e4567-e89b-12d3-a456-426614174001";
    const a = { ...owned(id), label: "Desktop" };
    const b = { ...a, key: other, deviceId: other, label: "Laptop" };
    expect(settingsEngineKey(a)).toBe(id);
    expect(settingsEngineLabel(fleetOf(other, [a, b]))).toBe("Laptop");
    expect(settingsDeviceName({ ...a, label: "  " })).toBe(id);
    expect(settingsEngineKey({ ...a, endpoint: "wss://another-relay.example.test/relay" })).toBe(id);
    const registry = { engines: [{ ...entryOf("connected", null), key: id, devices: { rows: [{ id, name: "Own device" }] } }] } as unknown as EngineRegistrySnapshot;
    expect(settingsDeviceName(a, registry)).toBe("Own device");
  });
  it("routes scoped chat identities by OwnedEngine key, never by a shared relay origin", () => {
    const a = owned("123e4567-e89b-12d3-a456-426614174000");
    const b = owned("123e4567-e89b-12d3-a456-426614174001");
    const fleet = fleetOf(a.key, [a, b]);
    const chatA = encodeScopedId(a.key, "same-chat");
    const chatB = encodeScopedId(b.key, "same-chat");
    expect(new URL(a.endpoint).origin).toBe(new URL(b.endpoint).origin);
    expect(chatA).not.toBe(chatB);
    const route = parseScopedId(chatB);
    expect(route.rawId).toBe("same-chat");
    expect(fleet.engines.find((engine) => engine.key === route.engine)).toBe(b);
    expect(Object.keys(b).sort()).toEqual(["deviceId", "endpoint", "key", "label"]);
  });
  it("shows the engine's own WatchDevices name instead of an edge UUID, including after a rename", () => {
    const id = "123e4567-e89b-12d3-a456-426614174000";
    const other = "123e4567-e89b-12d3-a456-426614174001";
    const fleet = fleetOf(id, [{ ...owned(id), label: id }, { ...owned(other), label: other }]);
    const own = { id, name: "Work Laptop" };
    const registry = { engines: [
      { ...entryOf("connected", null), key: id, info: { deviceId: id }, devices: { rows: [own, { id: other, name: "Wrong peer copy" }] } },
      { ...entryOf("connected", null), key: other, info: { deviceId: other }, devices: { rows: [{ id: other, name: "Server" }] } },
    ] } as unknown as EngineRegistrySnapshot;
    expect(settingsEngineLabel(fleet, registry)).toBe("Work Laptop");
    expect(settingsEngineLabel(fleetOf(other, fleet.engines), registry)).toBe("Server");
    const renamed = { ...registry, engines: registry.engines.map((entry) =>
      entry.key === id ? { ...entry, devices: { rows: [{ id, name: "Renamed Laptop" }] } } : entry,
    ) } as EngineRegistrySnapshot;
    expect(settingsEngineLabel(fleet, renamed)).toBe("Renamed Laptop");
    const peerOnly = { ...registry, engines: registry.engines.map((entry) =>
      entry.key === id ? { ...entry, devices: { rows: [{ id: other, name: "Wrong peer copy" }] } } : entry,
    ) } as EngineRegistrySnapshot;
    expect(settingsEngineLabel(fleet, peerOnly)).toBe(id);
    expect(settingsEngineLabel(fleetOf(id, [{ ...fleet.engines[0]!, label: "Edge Name" }, fleet.engines[1]!]), peerOnly))
      .toBe("Edge Name");
  });
});

describe("engineConnection (the folded engine-drawer row mapping)", () => {
  it("engineConnectionLabelsParkedAndIdentityChanged", () => {
    const cases: readonly [EngineEntrySnapshot | null, string, string, boolean][] = [
      // No registry entry yet (engine just paired, spawn pending).
      [null, "dot-connecting", "Starting…", false],
      [entryOf("connected", null), "dot-connected", "Connected", false],
      [entryOf("reconnecting", null), "dot-reconnecting", "Reconnecting…", false],
      // Parked without identity in the reason: revoked Session.
      [entryOf("off", "handshake refused: 4401"), "dot-parked", "Session revoked", true],
      // Parked with identity in the reason: the engine changed underneath.
      [entryOf("off", "engine identity mismatch"), "dot-parked", "Engine changed", true],
    ];
    for (const [entry, dot, label, pairable] of cases) {
      expect(engineConnection(entry)).toEqual({ dot, label, pairable });
    }
  });
});
