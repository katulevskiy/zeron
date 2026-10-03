# Memory hardening — 2026-10-02

Branch `codex/memory-hardening` starts from upstream main
`1c83cda221bd18a3b55ea49e5b25ec2c148955bc` (0.2.102).
The [comprehensive baseline audit](memory-audit-2026-10-02.md) records source
evidence, reproducible growth mechanisms, and the archived before measurements.

## Changes

* Chat sync retains a 256 KiB / 32-batch window over its durable SQLite outbox.
  A single legal large row can exceed that target. Stable batch IDs, ordinal
  order, checkpoint-only rows, and failed-write recovery are preserved. Healthy
  ACKs refill the window; HTTP fallback drains successive windows. Transport
  copies share payload ownership. Offline history recovery imports bounded pages.
* Checkpoint catch-up stops reading after 256 KiB / 32 frames while its fetch is
  pending. Existing bounded socket queues propagate backpressure. Checkpoint
  downloads reject bodies over the server's 32 MiB limit.
* PTY readers use a 256 KiB queue; output batches cap at 16 KiB. Viewers share
  a 32-event broadcast ring and 1 MiB replay. Slow viewers receive a visible gap
  and parser reset. Closing Windows ConPTY continues draining discarded output
  so bounded backpressure cannot deadlock native cleanup. Subscriber admission
  caps at 16 per terminal.
* Quiet, detached transcript mirrors are cleared on the idle sweep. The warm
  document navigation cache shrinks from 12 to 4; watched documents, writers,
  active sync, and failed persistence remain protected.
* Journal tail, last-event, and reopen scans retain one line instead of loading
  and decoding the entire historical file. Replay skips old payloads before
  deserializing them. Full requested replay is still proportional to its result.
* Dropped unary RPC futures and harness requests remove pending registrations.
  Production UI streams use scoped subscriptions. Reused server request IDs
  abort the previous task rather than orphaning it.
* File watcher notifications coalesce into one pending kick. Deleted chats
  release UI terminal tabs after registry hydration.
* User images are decoded with allocation/dimension limits, normalized into
  PNG previews up to 2048 pixels per side (SVG viewport up to 512), and charged
  for decoded CPU/GPU pixels. Only rendered transcript rows shield user images;
  shield removal immediately trims the cache. Animated read-back previews show
  the first frame; the durable original file is preserved.
* Font books own font bytes instead of leaking each registered font forever.
  A regression checks repeated owner destruction using weak references.

## Validation and measurements

Local verification uses Windows x86-64, Rust 1.99.0, release, locked dependencies.
The diagnostic allocator measures **requested live Rust bytes**, excluding RSS,
native/C allocations, GPU allocations, allocator overhead, and child processes.
The original baseline binary is retained outside the repository and its SHA-256
matches the archived JSON. Before/after numbers will be recorded after the final
diagnostic run.

[Memory checks](../.github/workflows/memory-checks.yml) runs Linux core regressions,
Valgrind Memcheck on nine finite offline scenarios plus font ownership, and
Massif on document, sync, catch-up, and complete backend replacement workloads.
Its artifacts retain the exact commit, binary hash, versions, XML errors,
allocation samples, and Massif stacks. Definite/indirect leaks and invalid memory
access fail the check; still-reachable allocations require the retention analysis
and cannot be dismissed solely because Memcheck passes.

## Remaining limits

These changes address reproduced mechanisms, not an attribution of any specific
reporter's 4 GB RSS. A connected production account and reporter workload are
still needed for that attribution. This environment cannot run native macOS
Instruments; browser/GPU/ONNX/subprocess memory is outside these allocator samples.

Full CRDT operation history and pinned active documents can still dominate RAM.
Uncoordinated history compaction could fork sync lineage or discard offline edits,
so it is intentionally deferred to a protocol-aware migration. The registry
metadata outbox, aggregate checkout diff caches, full requested RPC/journal
results, and completed session configuration also remain candidates for further
budgeting. Unsaved edits are retained on disk failure rather than discarded to
meet a RAM target. Cache limits are targets, not a global process RSS ceiling.
