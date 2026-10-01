// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

export type FinishQueueEditOutcome = { "outcome": "committed" } | { "outcome": "cancelled" } | { "outcome": "discarded" } | { "outcome": "released" } | { "outcome": "conflict", currentText: string, } | { "outcome": "lost" } | { "outcome": "missing" };
