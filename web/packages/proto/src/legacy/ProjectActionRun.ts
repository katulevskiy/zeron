// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

import type { TerminalSession } from "./TerminalSession";

/**
 * `RunProjectAction` reply: the Action echoed with the managed terminal
 * session running its command.
 */
export type ProjectActionRun = { actionId: string, actionName: string, terminal: TerminalSession, };
