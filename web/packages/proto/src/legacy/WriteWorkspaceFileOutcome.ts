// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { WorkspaceFileConflictReason } from "./WorkspaceFileConflictReason";
import type { WorkspaceFileWriteResult } from "./WorkspaceFileWriteResult";

export type WriteWorkspaceFileOutcome = { "status": "written", file: WorkspaceFileWriteResult, } | { "status": "conflict", reason: WorkspaceFileConflictReason, currentContentHash?: string | null, currentModifiedAt?: string | null, };
