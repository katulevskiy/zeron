// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

/**
 * A file modification carried inline on a tool result (ACP
 * `ToolCallContent::Diff`). `old_text: None` means a new file.
 */
export type ToolDiff = { path: string, oldText?: string | null, newText: string, };
