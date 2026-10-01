// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { GitFileState } from "./GitFileState";

export type GitFileStatus = { path: string, oldPath: string | null, index: GitFileState, worktree: GitFileState, };
