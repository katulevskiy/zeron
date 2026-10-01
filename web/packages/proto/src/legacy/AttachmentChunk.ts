// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

/**
 * `ReadAttachmentChunk` reply.
 */
export type AttachmentChunk = { name: string, mimeType: string,
/**
 * Base64 of this chunk's byte range.
 */
data: string, nextOffset: number, done: boolean, };
