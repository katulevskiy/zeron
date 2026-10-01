// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { ProjectActionRun } from "./ProjectActionRun";

/**
 * Result of creating a worktree. The worktree remains flattened so this is
 * wire-compatible with both legacy callers and legacy engine replies.
 */
export type CreateWorktreeOutcome = { setupAction?: ProjectActionRun | null, setupError?: string | null, repoPath: string, path: string, branch: string,
/**
 * Generated worktree folder name (`zeron/<name>` is its branch).
 */
name: string,
/**
 * Canonical checkout identity (device-scoped hash of the git dir).
 */
checkoutId?: string | null, };
