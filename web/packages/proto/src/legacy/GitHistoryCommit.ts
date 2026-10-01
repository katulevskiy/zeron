// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { GitHistoryRef } from "./GitHistoryRef";

/**
 * One topologically ordered row in the repository history graph.
 */
export type GitHistoryCommit = { sha: string, parentShas: Array<string>, subject: string, authorName: string, authorEmail: string, authoredAt: string, refs: Array<GitHistoryRef>, };
