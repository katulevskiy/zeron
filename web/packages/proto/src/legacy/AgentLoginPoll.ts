// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { AgentLoginStatus } from "./AgentLoginStatus";

export type AgentLoginPoll = { status: AgentLoginStatus, message?: string | null,
/**
 * a sign-in page that only became known after the start reply (the
 * agent had to install first); the app opens it once.
 */
url?: string | null, };
