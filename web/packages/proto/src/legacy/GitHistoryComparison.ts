// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * Divergence between the checked-out branch and the repository's integration
 * branch. Counts are computed only from locally available refs; callers must
 * fetch explicitly when they want newer remote state.
 */
export type GitHistoryComparison = {
/**
 * The local remote-tracking ref used as the comparison base, e.g.
 * `upstream/main`.
 */
base: string,
/**
 * Commits reachable from HEAD but not from [`Self::base`].
 */
ahead: number,
/**
 * Commits reachable from [`Self::base`] but not from HEAD.
 */
behind: number, };
