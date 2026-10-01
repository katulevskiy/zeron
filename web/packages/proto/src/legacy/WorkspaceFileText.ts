// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { WorkspaceLineEnding } from "./WorkspaceLineEnding";
import type { WorkspaceReadOnlyReason } from "./WorkspaceReadOnlyReason";
import type { WorkspaceTextEncoding } from "./WorkspaceTextEncoding";

export type WorkspaceFileText = {
/**
 * Identity of the checkout this snapshot was read from.
 */
checkoutId: string, path: string, text?: string | null, contentHash?: string | null, size: number, modifiedAt?: string | null, encoding: WorkspaceTextEncoding, lineEnding?: WorkspaceLineEnding | null, readOnlyReason?: WorkspaceReadOnlyReason | null, truncated: boolean, };
