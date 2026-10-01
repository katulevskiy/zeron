// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * One `SubscribeTerminal` stream item. `seq` is a per-terminal monotonic counter
 * used for replay resumption (`afterSeq`).
 */
export type TerminalEvent = { "type": "data", seq: number, data: string, } | { "type": "exit", seq: number, exitCode: number, signal?: string | null, };
