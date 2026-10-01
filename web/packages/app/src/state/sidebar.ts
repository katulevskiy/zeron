import { useSyncExternalStore } from "react";
import { SidebarStore, type SidebarState } from "../lib/sidebar-store";

/**
 * The sidebar UI-state singleton. Private space/chat identities and archived
 * disclosure stay in session memory; nonprivate customization remains durable.
 */
export const sidebarStore = new SidebarStore();

const subscribe = (listener: () => void) => sidebarStore.subscribe(listener);
const getSnapshot = () => sidebarStore.getSnapshot();

export function useSidebar(): SidebarState {
  return useSyncExternalStore(subscribe, getSnapshot);
}
