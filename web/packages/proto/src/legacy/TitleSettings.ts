// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { HarnessId } from "../generated/HarnessId";

/**
 * Per-device automatic session title preferences.
 */
export type TitleSettings = {
/**
 * None follows the session harness, using a supported installed fallback.
 */
harness: HarnessId | null,
/**
 * None selects the cheapest model offered by the selected harness.
 */
model: string | null, };
