# Contributor memory findings follow-up — 2026-10-03

This extends [PR #762](https://github.com/zeronsh/zeron/pull/762), starting at
`dd35d26f580249ae76705ec0f25ba7f7b916bd1d`. The input was the supplied
`memory-performance-findings-with-charts` directory: its Markdown report and
three charts. Report SHA-256:
`d9a7b7798be54f8a3bac0256fec8adb7a2bd8c1d731d2053327a33ae48ae372c`.
The report compares 0.2.98, 0.2.99 and 0.2.102, including eight synthetic chats,
six turns per chat, and 256,000-byte responses. Its original benchmark drivers
and raw traces were not included. The experiments below are new regressions and
allocation experiments, not reruns of those original measurements.

## Findings and decisions

| Contributor observation | Assessment and change |
| --- | --- |
| Eight persistent providers survived deleting their chats; deleting their space finally stopped them | Reproduced on the original PR. Centralized permanent teardown for chat and space deletion, including imported registry tombstones. |
| Allocator trim reduced glibc residency; filesystem access events previously drove background work | The existing trim and access-event filtering fixes are already present. No allocator/arena change: the report shows a CPU tradeoff, not a reason to lower arenas universally. |
| Five times the text accompanied 17.4 times engine CPU | Removed one verified full-history decode from user-message insertion. Full streaming publication and CRDT history remain separate profiling/protocol work. |
| Wallpaper source/effect/preload caches and renderer tiles have different lifetimes | Added CPU byte targets and explicit renderer image retirement after all consumers release their references; stopped speculative loading when artwork is removed. |
| Link caches bounded text parts by count but not bytes; metadata probes grew with unique paths | Added byte/count targets with transparent eviction and bounded weak reuse for oversized visible trees. |
| Pi stream queues allow large theoretical payload retention | No demonstrated resident payload volume or blocked consumer trace in the supplied data. Kept ordered transport semantics; a byte-backpressure design needs a separate realistic workload. |
| Voice model caching, pending MCP turn metadata, subagent lifetime | Existing lazy/idle behavior and subagent recovery are present. No measured native-model leak or substantial pending-turn retention. Kept legitimate live subagents protected. |

## Deleted-chat lifetime

`DeleteChat` previously removed a document from the host cache while leaving
`SessionsEngine`'s persistent run, provider, pending input, cached request,
harness session, status and subagent transcript ownership alive. A late final
message could also implicitly claim the deleted chat again. Snapshot exports
already queued before deletion could restore its local snapshot.

The teardown now:

- Signals permanent run cancellation synchronously, including parked runs,
  before waiting for provider settlement. This releases pending questions and
  follows existing harness interrupt/process cleanup. Independent chats continue.
- Forgets per-chat request, harness-session, event-hub and status caches. Guards
  prevent late dispatch, session writes and implicit host claims from reviving an
  explicit tombstone. A missing registry row still permits create/nudge races.
- Retires document command drains, sync cancellation tokens, chat2 subscriptions
  and clients, and open internal subagent documents. Late child frames cannot
  reopen a document under a tombstoned parent.
- Serializes snapshot deletion against legacy exports and chat2 persistence
  exports. An orphaned writer can settle its in-memory document but cannot
  restore the deleted snapshot.
- Watches a dedicated, sorted tombstone list, including deletes of unseen rows.
  Unchanged live-chat lists no longer hide deletions; ordinary streaming metadata
  does not wake teardown. Publications are ordered under the registry lock.
- Closes stale deleted journals at restart without reopening a document or
  interrupting recovery of unrelated chats. Existing journal retention and
  durable sync history are not a new disk-compaction policy.

The regression starts eight persistent harness sessions carrying 256 KiB text,
opens internal subagent transcripts and, on Unix, starts real `sleep` child
processes. It deletes the eight chats while retaining a ninth survivor. The eight-provider parked-deletion version of this regression was also run on
the original PR and failed to converge after ten seconds; it preceded the added
internal-document assertions. The deleted sessions remained alive. The fix releases the
providers and child processes, clears metadata, rejects late reopening and
leaves the survivor running. Additional tests cover active runs, space deletion,
two engines synchronizing through the real mock registry protocol, restart
recovery, and in-flight/late snapshot writes.

## History lookup measurement

`write_user_message` formerly materialized every message and text/tool payload
just to ask whether an ID already existed. `SessionDoc::has_message` scans
scalar IDs instead. Missing/malformed IDs retain the existing torn-entry salvage
behavior; legacy value-map entries are supported. This keeps idempotency while
making ordinary lookup allocation independent of historical payload size.

[Archived samples](performance/memory-followup-2026-10-03.json) compare the exact
former expression against the new lookup in the same binary and document.
Three Linux **debug** runs; eight absent-ID lookups per sample; 256,000 text
bytes per entry. Times are medians and describe only those eight lookups.

| Entries | Text bytes | Former peak requested Rust bytes | New peak | Former time | New time |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 8 | 2,048,000 | 4,114,616 | 667 | 7.326 ms | 0.018 ms |
| 32 | 8,192,000 | 16,458,366 | 667 | 49.593 ms | 0.059 ms |
| 128 | 32,768,000 | 65,833,402 | 667 | 64.927 ms | 0.223 ms |

The 667-byte sample includes diagnostic JSON bookkeeping. Construction is
outside the baseline; the old path is temporary allocation, not a permanent
leak. This does not measure RSS, native/GPU allocations, production throughput,
or explain the contributor's whole-engine CPU ratio. The scan remains linear
in message count, and pathological torn entries still require salvage decoding.
Reproduce with:

```sh
cargo run --locked -p zeron-engine --example memory-audit -- message-lookup
```

## Wallpaper and link ownership

Wallpaper sources retain at most four cached sources with a **64 MiB accounted
CPU-pixel target**, and at most two effect variants per source. Preloads use a
separate 64 MiB target, reserving space for a worst-case 2048-pixel proxy before
starting another job. Large wallpapers therefore need fewer speculative images.
Decoder allocation is limited to 128 MiB and dimensions to 16,384 per side;
original files remain unchanged. Smaller images retain native resolution:
`thumbnail(2048, 2048)` also upscaled tiny inputs, turning a 4×4 source into
approximately 36 MiB of proxy/source/effect pixels. Its new retained size is
144 pixel bytes.

Renderer atlas tiles survive simply dropping a `RenderImage` Arc. A retirement
pool retains one reference to each generated image, then calls `App::drop_image`
for the current and other windows once caches, preloads, readiness crossfades
and other-window consumers release theirs. This protects active consumers and
retires unused renderer entries. The test cycles 100 simulated 1536×1536 images,
checks the byte target before the four-entry limit, checks crossfade and
second-window ownership, and checks weak references disappear after retirement.
These are ownership regressions, not measured native GPU residency. The targets
exclude active crossfades, other live consumers, in-flight decoding, allocator
and graphics-driver overhead; they are not a global process/GPU memory cap.

The inline-code link cache retains at most 32 parts and 8 MiB of conservatively
accounted source/linked tree storage. Oversized visible parts can reuse a bounded
weak memo while callers retain them, without pinning their full payload after
the caller drops it. Metadata probes retain at most 4,096 entries and 1 MiB of
accounted path/entry storage. Evicted probes are queried again rather than
skipping file resolution. Tests cover many unique paths, large code histories,
oversized weak lifetimes and rechecking an evicted negative probe.

## Validation and remaining evidence

Local validation uses Linux x86-64, Rust 1.98.1, locked dependencies and serial
regression execution. **2,113 passed; six existing ignores**:

| Suite | Passed | Ignored |
| --- | ---: | ---: |
| Document library | 132 | 0 |
| Engine library | 385 | 2 |
| UI library | 1,527 | 0 |
| Eight engine integration suites | 69 | 4 |

The integration suites are `chat_deletion` (seven tests), `message_queue`,
`restart_resume`, `session_publication`, `workspace_sync`, `subagent_idle_reap`,
`turn_quiesce` and `side_chats`. The final UI and deletion runs used an isolated
reflinked build cache to avoid concurrent worktrees replacing dependency artifacts.
Commands and successful log hashes are archived with the allocation samples.
 The previous PR's Windows/native-CI results apply to its
previous revision; they do not validate this follow-up on Windows or macOS.

Memory CI now includes document-library regressions, the deleted-chat integration
suite and the new lookup scenario under Memcheck. Local Valgrind is unavailable,
so no new native Memcheck/Massif result is claimed here. Actual GPU retirement
and headed production residency still need platform capture. Full long-history
publication, protocol-safe CRDT compaction, Pi payload backpressure, aggregate
cache residency and ONNX native allocations remain profiling candidates.

These changes fix verified ownership and allocation mechanisms. They do not
attribute or guarantee elimination of the reported 4 GiB production footprint.
