// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { ProjectActionIcon } from "./ProjectActionIcon";

/**
 * A saved project Action: a named shell command scoped to one space.
 */
export type ProjectAction = { id: string, name: string, command: string, icon: ProjectActionIcon, runOnWorktreeCreate: boolean, };
