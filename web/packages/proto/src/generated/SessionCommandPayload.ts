// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

import type { RunRequest } from "./RunRequest";
import type { UserInputAnswer } from "./UserInputAnswer";

export type SessionCommandPayload = { "kind": "run", request: RunRequest,
/**
 * Client-minted message id for the optimistic user entry (dedup key).
 */
messageId: string, } | { "kind": "steer", prompt: string, messageId: string | null, } | { "kind": "interrupt", } | { "kind": "respondInput", requestId: string, answers: Array<UserInputAnswer>, };
