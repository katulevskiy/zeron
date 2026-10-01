// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { SidebarSectionChange } from "./SidebarSectionChange";

export type SidebarPinChange = { "action": "pin", sessionId: string, after: string | null, before: string | null, } | { "action": "move", sessionId: string, after: string | null, before: string | null, } | { "action": "unpin", sessionId: string, } | { "action": "section", change: SidebarSectionChange, };
