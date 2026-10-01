// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { WorkspaceFileChangeKind } from "./WorkspaceFileChangeKind";

export type WorkspaceFileChange = { kind: WorkspaceFileChangeKind, path: string, oldPath?: string | null, };
