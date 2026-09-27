import { describe, expect, it } from "vitest";
import type { EngineEntrySnapshot, EngineConnectionState, EngineRegistrySnapshot } from "@zeron/engine-client";
import type { FleetState, StoredEngine } from "../src/lib/engine-store";
import { engineConnection, settingsEngineLabel } from "../src/lib/settings-engine";

function stored(baseUrl: string): StoredEngine {
  return {
    baseUrl,
    credential: "credential",
    label: "Web on Windows",
    sessionId: "session",
    pairedAt: 1,
    deviceId: null,
  };
}

function fleetOf(active: string | null, engines: readonly StoredEngine[]): FleetState {
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
    const a = { ...stored("device-a"), label: "Workstation" };
    const b = { ...stored("device-b"), label: "Laptop" };
    expect(settingsEngineLabel(fleetOf(null, []))).toBe(null);
    expect(settingsEngineLabel(fleetOf("device-a", [a]))).toBe(null);
    expect(settingsEngineLabel(fleetOf("device-a", [a, b]))).toBe("Workstation");
    expect(settingsEngineLabel(fleetOf("device-b", [a, b]))).toBe("Laptop");
    expect(settingsEngineLabel(fleetOf("device-a", [a, { ...b, label: "" }]))).toBe("Workstation");
    expect(settingsEngineLabel(fleetOf("device-b", [a, { ...b, label: "" }]))).toBe("device-b");
    expect(settingsEngineLabel(fleetOf("missing", [a, b]))).toBe(null);
  });

  it("shows the engine's own WatchDevices name instead of an edge UUID, including after a rename", () => {
    const id = "123e4567-e89b-12d3-a456-426614174000";
    const other = "123e4567-e89b-12d3-a456-426614174001";
    const fleet = fleetOf(id, [{ ...stored(id), label: id }, { ...stored(other), label: other }]);
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
