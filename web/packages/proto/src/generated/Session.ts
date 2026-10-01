// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { SessionStatus } from "./SessionStatus";

/**
 * Live run status for a chat — drives the Working indicator and sidebar status dots.
 * Staleness-checked client-side against `updated_at` so a crashed backend never shows
 * an eternal "Working".
 */
export type Session = {
/**
 * Last successfully completed assistant turn. Retained while the next turn
 * runs so coalesced status watches do not lose normal queue completions.
 * Interrupts, failures and liveness expiry never advance this marker.
 */
lastCompletedTurn: string | null, chatId: string, deviceId: string, status: SessionStatus, startedAt: string | null, updatedAt: string, };
