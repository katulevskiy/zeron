// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

/**
 * The fixed data boundary selected when an engine runtime is assembled.
 *
 * Authentication can change while a runtime is alive, but its workspace scope
 * cannot. Switching scopes requires assembling a new runtime.
 */
export type WorkspaceScope = "local" | "synced" | "development";
