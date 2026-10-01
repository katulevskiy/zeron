import { useSyncExternalStore } from "react";
import { engineRegistry } from "./fleet";
import { sidebarPinProfileKey } from "../lib/sidebar-pins";
import { SidebarStateSync } from "../lib/sidebar-state-sync";
import { uiSettings } from "./ui-settings";

/** Cookie-discovered engine workspace preferences. Local settings are a cache,
 * never imported into the registry on attach or reconnect. Owner boundaries
 * invalidate outstanding frames and writes via private-session generation.
 */

export const sidebarStateSync = new SidebarStateSync(uiSettings);

export function useSidebarSyncStatus() {
  return useSyncExternalStore(sidebarStateSync.subscribe, sidebarStateSync.getSnapshot, sidebarStateSync.getSnapshot);
}

function syncBridges(): void {
  const snapshot = engineRegistry.getSnapshot();
  const present = new Set<string>();
  for (const engine of snapshot.engines) {
    present.add(engine.key);
    const profileKey = sidebarPinProfileKey(
      engine.info?.workspaceScope ?? null,
      engine.info?.deviceId ?? null,
    );
    // The bridge only starts once the engine's identity has landed (the
    // bucket key needs the scope + device id); re-attaching when it does
    // restarts the bridge with the resolved key.
    const client = engineRegistry.clientFor(engine.key);
    if (client === null) {
      sidebarStateSync.detach(engine.key);
      continue;
    }
    sidebarStateSync.attach(engine.key, client, profileKey);
  }
  for (const key of sidebarStateSync.attachedKeys()) {
    if (!present.has(key)) {
      sidebarStateSync.detach(key);
    }
  }
}

engineRegistry.subscribe(syncBridges);
syncBridges();
