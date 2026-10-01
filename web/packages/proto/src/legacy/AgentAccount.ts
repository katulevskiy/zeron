// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { AgentAuthKind } from "./AgentAuthKind";
import type { AgentUsageWindow } from "./AgentUsageWindow";
import type { HarnessId } from "../generated/HarnessId";

export type AgentAccount = { id: string, harness: HarnessId, email: string | null, planLabel: string | null, active: boolean, usageWindows: Array<AgentUsageWindow>, displayName?: string | null, organization?: string | null,
/**
 * How the CLI is signed in (`oauth` account vs raw `api-key`).
 */
authKind?: AgentAuthKind | null,
/**
 * False for a live login whose credentials we could not read (e.g. macOS
 * Keychain denied) — shown, but not re-activatable.
 */
switchable: boolean,
/**
 * Epoch millis of the slot's last snapshot.
 */
savedAt?: number | null, };
