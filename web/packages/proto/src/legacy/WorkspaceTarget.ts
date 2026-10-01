// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * Identifies the local checkout used by workspace file operations.
 * Exactly one of `chat_id` and `space_id` must be present.
 */
export type WorkspaceTarget = { chatId?: string | null, spaceId?: string | null, checkoutPath?: string | null, };
