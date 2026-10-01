// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { GitFileStatus } from "./GitFileStatus";

/**
 * Latest status only: never contains file content or a patch. `complete = false`
 * means unavailable/partial, not clean. Revision covers only these statuses.
 */
export type CheckoutGitStatus = { checkoutId: string, deviceId: string, revision: string, complete: boolean, files: Array<GitFileStatus>, };
