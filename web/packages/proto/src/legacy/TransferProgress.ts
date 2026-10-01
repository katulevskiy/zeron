// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * One in-flight queued-attachment transfer (the `WatchTransfers` stream):
 * raw-byte progress of the engine-side relay leg pushing staged bytes to a
 * remote host. An entry appears when a file's chunks start moving, updates
 * per landed chunk, and disappears when the host commits it (or the attempt
 * fails — the retry re-adds it). Keyed by the send-minted uploadId, so the
 * sender's thumbnails can resolve their `pending://{uploadId}/…` refs to a
 * real percent instead of an indeterminate spinner.
 */
export type TransferProgress = { uploadId: string, fileName: string,
/**
 * Raw bytes the host has acknowledged so far.
 */
done: number,
/**
 * Total raw bytes of the staged file.
 */
total: number, };
