// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { ModelOption } from "./ModelOption";
import type { ReasoningLevel } from "./ReasoningLevel";

export type Model = { id: string, label: string,
/**
 * Short tagline rendered under the name in the model picker (11px muted),
 * mirroring the Electron app's `ModelInfo.description`.
 */
description?: string | null, reasoningLevels: Array<ReasoningLevel>, options: Array<ModelOption>, };
