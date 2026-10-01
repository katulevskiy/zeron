// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * An open PTY session on the owning device (`OpenTerminal` reply).
 */
export type TerminalSession = { id: string, cwd: string,
/**
 * Shell basename (`zsh`, `bash`, …) for the tab label.
 */
shell: string, };
