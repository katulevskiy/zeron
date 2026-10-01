// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

/**
 * Isolated-worktree directive riding [`RunRequest`]. The worktree is created
 * by the HOST while draining the queued Run — not by the sender over a
 * blocking CreateWorktree RPC — so the send path stays durable: a lost relay
 * frame can't wedge the composer on "Sending…" while the session runs anyway
 * (2026-08-18 user report).
 */
export type WorktreeSpec = {
/**
 * The repo whose worktree to create (the space's folder on the host).
 */
repoPath: string,
/**
 * Base ref the fresh `zeron/<name>` branch is created off.
 */
base: string,
/**
 * Owning project used to resolve host-local setup Actions. Optional for
 * wire compatibility with clients that only request worktree creation.
 */
spaceId?: string | null, };
