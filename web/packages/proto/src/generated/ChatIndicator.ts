// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

/**
 * Display status for a chat row/tab: the four user-facing states plus a
 * distinct Errored. Derived — never stored.
 */
export type ChatIndicator = "working" | "awaitingInput" | "errored" | "completed" | "idle";
