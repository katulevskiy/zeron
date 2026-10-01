// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { DiffFileSummary } from "./DiffFileSummary";

/**
 * Working-tree diff for a checkout — latest-only sidecar, 3MiB patch cap.
 */
export type CheckoutDiff = { checkoutId: string, deviceId: string, cwd: string, patch: string, files: Array<DiffFileSummary>, additions: number, deletions: number,
/**
 * True when the patch was truncated at the byte cap ("Partial snapshot").
 */
truncated: boolean, checksum: string, updatedAt: string, };
