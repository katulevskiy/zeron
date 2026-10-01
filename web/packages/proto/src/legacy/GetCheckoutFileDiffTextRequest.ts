// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

export type GetCheckoutFileDiffTextRequest = { checkoutId: string, cwd: string, path: string, mode: string, baseRef?: string | null, chatId?: string | null,
/**
 * Pinned commit for History's per-commit diff scope. When present, the
 * source pair is read from the commit parent and this commit, never from
 * the live working tree.
 */
commitSha?: string | null, diffChecksum: string, };
