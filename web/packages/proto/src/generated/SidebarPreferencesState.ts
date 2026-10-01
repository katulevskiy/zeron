// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { SidebarSection } from "./SidebarSection";

/**
 * Watch payload for pins. `initialized` records known cached state, including
 * an empty list; `synced` records receipt of an authoritative registry state.
 */
export type SidebarPreferencesState = {
/**
 * Monotonic within one engine attachment, not a cross-device order key.
 * Lets clients reject older watch frames after a mutation response.
 */
revision: number, synced: boolean, initialized: boolean, pinnedSessionIds: Array<string>, sections: Array<SidebarSection>, };
