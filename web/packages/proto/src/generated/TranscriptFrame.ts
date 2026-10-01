// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { SessionMessageEntry } from "./SessionMessageEntry";
import type { TextAppend } from "./TextAppend";
import type { TranscriptUpsert } from "./TranscriptUpsert";

/**
 * One `WatchDocMessages` stream item.
 */
export type TranscriptFrame = { reset: Array<SessionMessageEntry>, } | { upsert: Array<TranscriptUpsert>, append: Array<TextAppend>, remove: Array<string>,
/**
 * Expected transcript length after applying this frame — the desync
 * tripwire: a consumer that lands elsewhere resubscribes for a reset.
 */
count: number, };
