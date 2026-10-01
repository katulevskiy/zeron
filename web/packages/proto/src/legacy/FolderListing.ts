// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { FolderEntry } from "./FolderEntry";

export type FolderListing = { path: string, entries: Array<FolderEntry>,
/**
 * True when the listing hit the entry cap.
 */
truncated: boolean, };
