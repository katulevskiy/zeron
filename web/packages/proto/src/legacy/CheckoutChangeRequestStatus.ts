// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { ChangeRequestSummary } from "./ChangeRequestSummary";

/**
 * Latest successful change request resolution for one checkout and branch.
 *
 * `change_request: None` is an authoritative successful lookup with no match;
 * resolution failures must retain the previous successful snapshot instead.
 */
export type CheckoutChangeRequestStatus = { checkoutId: string, deviceId: string, cwd: string, branch: string, changeRequest: ChangeRequestSummary | null, updatedAt: string, };
