// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { CommandBasedOn } from "./CommandBasedOn";
import type { SessionCommandPayload } from "./SessionCommandPayload";
import type { SessionCommandStatus } from "./SessionCommandStatus";

export type SessionCommandEntry = { id: string, payload: SessionCommandPayload, issuedBy: string,
/**
 * Epoch millis.
 */
issuedAt: number, basedOn: CommandBasedOn | null,
/**
 * Epoch millis; defaults to issued_at + COMMAND_DEFAULT_TTL_MS when absent.
 */
expiresAt: number | null, status: SessionCommandStatus, resolution: string | null, };
