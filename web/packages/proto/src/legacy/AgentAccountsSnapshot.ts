// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { AgentAccount } from "./AgentAccount";
import type { AgentAccountWarning } from "./AgentAccountWarning";

/**
 * Everything the Accounts settings page renders, rebuilt after every mutation.
 */
export type AgentAccountsSnapshot = { accounts: Array<AgentAccount>, warnings: Array<AgentAccountWarning>, };
