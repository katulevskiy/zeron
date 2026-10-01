// RETAINED BROWSER MODEL — not generated or freshness-verified against upstream Rust.
// See ../README.md; do not use this declaration as proof that a method is supported.

export type BeginQueueEditOutcome = { "outcome": "acquired", leaseId: string, text: string, attachments: Array<string>, baseTextHash: string, expiresAtMs: number, } | { "outcome": "locked", ownerDeviceId: string, expiresAtMs: number, } | { "outcome": "missing" };
