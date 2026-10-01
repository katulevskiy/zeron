// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { ProjectAction } from "./ProjectAction";
import type { ProjectActionDraft } from "./ProjectActionDraft";

/**
 * The full Action state for one space: saved actions plus import offers
 * from the project file (`zeron.json`), and any project-file issue.
 */
export type ProjectActionsSnapshot = { spaceId: string, actions: Array<ProjectAction>, importableActions: Array<ProjectActionDraft>, projectFileIssue: string | null, };
