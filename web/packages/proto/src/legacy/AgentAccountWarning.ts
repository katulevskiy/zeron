// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { HarnessId } from "../generated/HarnessId";

/**
 * A per-harness detection warning (e.g. Keychain denied reading the live login).
 */
export type AgentAccountWarning = { harness: HarnessId, message: string, };
