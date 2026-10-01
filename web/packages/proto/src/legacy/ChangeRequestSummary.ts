// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { ChangeRequestState } from "./ChangeRequestState";

/**
 * Compact provider-neutral change request metadata for checkout surfaces.
 */
export type ChangeRequestSummary = { provider: string, number: number, title: string, url: string, state: ChangeRequestState, baseRef: string, headRef: string, };
