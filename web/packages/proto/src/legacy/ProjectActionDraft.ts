// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { ProjectActionIcon } from "./ProjectActionIcon";

/**
 * Client-supplied Action payload (create, or full replace by id).
 */
export type ProjectActionDraft = { name: string, command: string, icon: ProjectActionIcon, runOnWorktreeCreate: boolean, };
