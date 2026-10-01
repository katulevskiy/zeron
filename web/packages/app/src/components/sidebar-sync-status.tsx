import type { SidebarSyncStatus } from "../lib/sidebar-state-sync";

/** A failed/unsupported bridge is never advertised as successful local sync. */
export function SidebarSyncStatusNotice({ statuses }: { statuses: Readonly<Record<string, SidebarSyncStatus>> }) {
  const values = Object.values(statuses);
  const message = values.includes("unavailable")
    ? "Sidebar sync unavailable on this engine. Upgrade the engine to edit pins and sections."
    : values.includes("error")
      ? "Sidebar change could not be confirmed. Reconnect before editing pins and sections."
      : values.includes("offline")
        ? "Sidebar offline. Pins and sections are read-only until reconnection."
        : values.includes("loading")
          ? "Waiting for workspace sidebar preferences. Pins and sections are read-only."
          : null;
  return message === null ? null : <div role="status" className="sidebar-notice">{message}</div>;
}
