// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { HarnessId } from "../generated/HarnessId";
import type { ReasoningLevel } from "../generated/ReasoningLevel";
import type { SteeringMode } from "../generated/SteeringMode";

/**
 * What `ListHarnesses` reports per harness.
 */
export type HarnessDescriptor = { id: HarnessId, name: string, supportsSteering: boolean, steeringMode: SteeringMode, reasoningLevels: Array<ReasoningLevel>,
/**
 * Whether the agent's CLI is present on the listing device (the settings
 * enable-gate). Defaults true so catalogs from engines predating the
 * field never read as uninstallable.
 */
installed: boolean,
/**
 * Whether the listing device offers this harness (Settings → Agents).
 * `None` — the catalog came from an engine predating the setting — means
 * "unknown": consumers fall back to detection (see [`descriptor_enabled`]).
 */
enabled?: boolean | null, };
