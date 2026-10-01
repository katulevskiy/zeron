// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { CheckoutGitStatus } from "./CheckoutGitStatus";

/**
 * Keep unavailable updates inside an object: the RPC envelope uses JSON null
 * for a missing item, so a bare optional snapshot cannot signal invalidation.
 */
export type WorkspaceGitStatusFrame = { status: CheckoutGitStatus | null, };
