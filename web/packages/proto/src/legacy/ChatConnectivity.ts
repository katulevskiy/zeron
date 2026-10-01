// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

export type ChatConnectivity = { chatId: string, connected: boolean,
/**
 * Local update batches not yet acked by the chat's edge room.
 */
pendingPushes: number, };
