// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * CLI plan rate-limit window (accounts settings meters) — NOT app token accounting.
 */
export type AgentUsageWindow = { label: string,
/**
 * 0.0..=1.0
 */
usedFraction: number, resetsAt: string | null, };
