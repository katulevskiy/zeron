// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * One queued attachment a `QueueCommand` asks the engine to deliver: the
 * bytes are already committed to THIS device's uploads dir under
 * `{id8}-{sanitize(file_name)}`; the transfer pushes them to the chat's
 * host device by upload identity (never by arbitrary path).
 */
export type AttachmentTransfer = { uploadId: string, fileName: string, };
