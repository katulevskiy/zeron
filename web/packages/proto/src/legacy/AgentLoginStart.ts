// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { AgentLoginMode } from "./AgentLoginMode";

/**
 * `StartAgentLogin` reply: open `url`, then either paste the code back
 * (`CompleteAgentLogin`) or poll until the browser flow lands (`PollAgentLogin`).
 */
export type AgentLoginStart = { loginId: string, url: string, mode: AgentLoginMode,
/**
 * True when the spawned CLI opens the authorization page itself
 * (the engine could not suppress it) — clients must not open it too.
 */
cliOpensBrowser: boolean, };
