// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

/**
 * Immutable-at-run-start repository context owned by one conversation.
 *
 * This is deliberately separate from the live checkout snapshot: another
 * chat may change the branch at the same checkout without changing which
 * branch this conversation belongs to.
 */
export type ConversationSourceContext = { checkoutId: string, repoRoot: string, cwd: string, branch: string, headSha?: string | null, observedAt: string, };
