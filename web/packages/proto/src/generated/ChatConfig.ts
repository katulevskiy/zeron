// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { HarnessId } from "./HarnessId";
import type { ReasoningLevel } from "./ReasoningLevel";
import type { SandboxLevel } from "./SandboxLevel";

export type ChatConfig = { harness: HarnessId, model: string | null, reasoning: ReasoningLevel | null, modelOptions: Record<string, unknown>, sandbox: SandboxLevel, };
