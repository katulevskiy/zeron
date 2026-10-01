// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * One row of `ListRefs`: a branch plus its checkout state — whether it is
 * the repo's current (main-checkout) branch and whether it is materialized
 * as a linked worktree. Drives the composer's ref picker (`current` /
 * `worktree` tags) and the checkout-kind selector.
 */
export type RepoRef = { name: string,
/**
 * Checked out in the repo's MAIN folder right now.
 */
current: boolean,
/**
 * Path of the linked worktree this branch is checked out in, if any.
 */
worktreePath?: string | null, };
