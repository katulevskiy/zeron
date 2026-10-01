// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { SidebarSection } from "./SidebarSection";

/**
 * Section intents share the pin queue so moves cannot overtake one another.
 */
export type SidebarSectionChange = { "action": "create", id: string, name: string, } | { "action": "rename", id: string, name: string, } | { "action": "collapse", id: string, collapsed: boolean, } | { "action": "delete", id: string, } | { "action": "assign", sessionId: string, sectionId: string | null, } | { "action": "import", sections: Array<SidebarSection>, };
