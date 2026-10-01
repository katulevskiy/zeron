// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { MessagePart } from "./MessagePart";
import type { MessageRole } from "./MessageRole";
import type { MessageStatus } from "./MessageStatus";

/**
 * One entry in the doc's `messages` list (`SessionMessageEntry` in TS).
 */
export type SessionMessageEntry = { id: string, role: MessageRole, parts: Array<MessagePart>,
/**
 * Epoch millis.
 */
createdAt: number, deviceId: string, status?: MessageStatus | null, continuationOf?: string | null,
/**
 * Wall-clock length of this assistant turn, stamped when the segment
 * finishes. Absent on user rows, live streams, and docs written before
 * the field existed.
 */
durationMs?: number | null, };
