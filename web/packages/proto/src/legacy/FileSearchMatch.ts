// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * A workspace-relative file or directory returned by `SearchFiles`.
 * Contents deliberately never cross this boundary: mentioning a path leaves
 * the harness to read it through its normal workspace tools when needed.
 */
export type FileSearchMatch = { path: string, isDir: boolean, };
