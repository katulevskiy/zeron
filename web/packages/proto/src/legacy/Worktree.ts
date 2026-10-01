// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

export type Worktree = { repoPath: string, path: string, branch: string,
/**
 * Generated worktree folder name (`zeron/<name>` is its branch).
 */
name: string,
/**
 * Canonical checkout identity (device-scoped hash of the git dir).
 */
checkoutId?: string | null, };
