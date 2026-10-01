// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { WorkspaceWritableEncoding } from "./WorkspaceWritableEncoding";
import type { WorkspaceWritableLineEnding } from "./WorkspaceWritableLineEnding";

export type WriteWorkspaceFileRequest = {
/**
 * Must match the read snapshot, even if the chat has since changed cwd.
 */
expectedCheckoutId: string, path: string, text: string, expectedContentHash: string, encoding: WorkspaceWritableEncoding, lineEnding: WorkspaceWritableLineEnding, chatId?: string | null, spaceId?: string | null, checkoutPath?: string | null, };
