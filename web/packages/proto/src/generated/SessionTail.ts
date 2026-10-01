// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { SessionMessageEntry } from "./SessionMessageEntry";

/**
 * Tail sidecar shape (`SessionTail` in TS).
 */
export type SessionTail = { chatId: string, schemaVersion: number, messages: Array<SessionMessageEntry>, totalMessages: number, updatedAt: number, };
