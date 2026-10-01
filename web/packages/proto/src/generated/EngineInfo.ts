// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { WorkspaceScope } from "./WorkspaceScope";

/**
 * Stable information about the engine runtime reached by a client.
 */
export type EngineInfo = { deviceId: string, workspaceScope: WorkspaceScope,
/**
 * SDK selected by the owning engine, absent on older versions.
 */
cursorSdkVersion?: string | null,
/**
 * Supported protocol/document features. Missing on older engines.
 */
capabilities?: Array<string>, };
