import type { EngineEntrySnapshot, EngineRegistrySnapshot } from "@zeron/engine-client";
import type { FleetState } from "./owned-engine";

/** Settings address the discovered engine by opaque registry identity. */
export function settingsEngineKey(engine: FleetState["engines"][number]): string {
  return engine.key;
}

/**
 * The engine-addressing settings vocabulary: which engine the settings
 * pages that silently talk to the *active* one (Remote access, Agents,
 * Accounts) are addressing, and what one paired engine's connection looks
 * like as a row. The desktop has no analog — its settings address the
 * implicit local engine by construction (`remote_access.rs:37-39`); the
 * web's closest concept is `fleet.active`, surfaced and switched by these
 * rows.
 */

/**
 * The engine's own WatchDevices row is the name of record (and reflects
 * renames). Edge discovery can have only the device UUID: older relay hosts
 * don't send a name on their WebSocket registration. Never borrow a peer's
 * copy of this row, which may be stale or identify a different engine.
 */
export function settingsDeviceName(engine: FleetState["engines"][number], registry?: EngineRegistrySnapshot): string {
  const key = settingsEngineKey(engine);
  const entry = registry?.engines.find((candidate) => candidate.key === key);
  const ownId = entry?.info?.deviceId ?? engine.deviceId;
  const name = entry?.devices.rows.find((device) => device.id === ownId)?.name?.trim();
  return name || engine.label.trim() || ownId;
}

/** Name the active engine only when more than one is available. */
export function settingsEngineLabel(fleet: FleetState, registry?: EngineRegistrySnapshot): string | null {
  if (fleet.engines.length < 2 || fleet.active === null) return null;
  const engine = fleet.engines.find((entry) => settingsEngineKey(entry) === fleet.active);
  return engine === undefined ? null : settingsDeviceName(engine, registry);
}

/** One paired engine's connection view off its registry entry state. */
export interface EngineConnectionView {
  /** The shared status-dot class (`dot-connected` and siblings). */
  readonly dot: string;
  /** The connection label for the row's meta line. */
  readonly label: string;
  /** True when the entry is parked — the row offers "Pair again". */
  readonly pairable: boolean;
}

/**
 * The engine-row state mapping: "Starting…" while the registry entry is
 * still pending, "Connected"/"Reconnecting…" off its live state, and parked
 * reads "Engine changed" when the off-reason names identity, "Session
 * revoked" otherwise — parked alone is pairable.
 */
export function engineConnection(entry: EngineEntrySnapshot | null): EngineConnectionView {
  if (entry === null) {
    return { dot: "dot-connecting", label: "Starting…", pairable: false };
  }
  switch (entry.state) {
    case "connected":
      return { dot: "dot-connected", label: "Connected", pairable: false };
    case "reconnecting":
      return { dot: "dot-reconnecting", label: "Reconnecting…", pairable: false };
    case "off":
      return {
        dot: "dot-parked",
        label: (entry.lastError ?? "").includes("identity") ? "Engine changed" : "Session revoked",
        pairable: true,
      };
  }
}
