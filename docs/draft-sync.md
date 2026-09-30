# Synced composer drafts

Status: DESIGN · 2026-09-29
Prior art: `docs/chat2-sync.md` (the room protocol this reuses), `docs/registry-sync.md`.

## Why

A chat's transcript is already synced across every device on an account. The
text being typed into its composer is not: `Composer.drafts` is an in-memory
`HashMap<String, String>` per window. This adds a per-chat **draft** that every
device of the same user edits live, Google-Docs style: concurrent typing from
several devices merges (no last-writer-wins clobbering), and the draft follows
you from device to device.

## Goals

- Typing in an existing chat's composer updates a shared draft; other devices
  show it live and can type into it at the same time.
- Sending consumes the draft on every device, and leaves **no history behind**:
  the draft doc is thrown away, not cleared.
- Desktop first (embedded engine and IPC daemon). The primitive is built so the
  thin client / iOS / Android can adopt it later without redesign.

## Non-goals

- The new-chat canvas (it has no chat id yet).
- Attachments and appshots (they stay local).
- Remote-cursor / selection display of other devices.
- iOS / Android UI wiring, `zeron-client` and the mobile FFI.
- `crates/localedge` (not in `main` yet). When it lands it needs the same room
  ported next to `chat2` (routes + tables); the wire contract below is what it
  must match.

## Why a separate doc (not a field in the session doc)

Session docs keep their whole op history until a trim, and the project's
priority is small session docs. Typing history in the transcript doc would
bloat it and cannot be removed per field. A draft is its own tiny Loro doc in
its own room, so discarding it deletes its history entirely.

## Design

### 1. Room (`edge/`)

New Durable Object `DraftRoom`, copied from `ChatRoom` and reusing
`chat-frames.ts`, `chat-log.ts` and `blobs.ts` unchanged. Same binary frames,
row log, ack/dup semantics, checkpoint endpoint and per-device push quota.
Removed: tail/diff sidecars and the R2 backup alarm.

- Binding `DRAFT_ROOMS`, migration `v5` (`new_sqlite_classes: ["DraftRoom"]`).
- Worker route `/draft/:orgId/:chatId/{ws,checkpoint,rows,epoch,discard,stats}`.
  It requires `auth.orgId === orgId` (403 otherwise) and names the room
  `draft1/{orgId}/{userId}/{chatId}`, so a user can only reach their own rooms
  and there is no ownership-claim race. `chatId` is validated with `ID_RE`.
- **Epoch.** `meta.epoch` (integer, starts at 1). `GET /epoch` returns it.
  `ws`, `rows` and `checkpoint` take `?epoch=N`; a mismatch is rejected
  (`409 epoch_mismatch` / close `4411`) so a stale device can never push into a
  newer epoch.
- **Discard.** `POST /discard?epoch=N`: if `N` equals the current epoch,
  delete all rows, checkpoint blobs and idle state, set `epoch = N + 1`, close
  every socket with `4411 "draft discarded"`. A stale `N` is a no-op returning
  the current epoch (idempotent, so retries and races are safe).
- **Idle expiry.** Every write (re)arms an alarm 30 days out; when it fires on
  an idle room the DO calls `ctx.storage.deleteAll()`. Abandoned drafts never
  accumulate.
- Room size is tiny, so `MAX_CHECKPOINT_BYTES` and `MAX_ROW_BYTES` are lowered
  (checkpoint 256 KiB, row 64 KiB) and reject oversized drafts instead of
  growing.

### 2. Doc and pure primitive (`crates/doc`)

- `DraftDoc`: one `LoroDoc` with a single root `LoroText`. Snapshot/import,
  `text()`, byte-range `splice`, and a subscription that turns remote imports
  into ordered `TextSplice { start, delete, insert }` events in **UTF-8 byte
  offsets** (matching `ComposerInput`).
- `TextSplice::from_diff(old, new)`: single contiguous splice from common
  prefix/suffix (how typing changes text).
- `transform_offset` / `transform_range` and `rebase_splice(splice, over)`:
  the position math shared by the engine (rebasing a stale `EditDraft`) and the
  UI (moving the caret through a remote edit). Pure functions, no I/O, no gpui
  — this is the piece mobile reuses.
- An empty-string doc is the "no draft" state.

### 3. Engine (`crates/engine`)

New `draft_host.rs`, next to `doc_host.rs`, owning one `DraftHandle` per chat
with a watcher or unsynced edits.

- Reuses `zeron_sync::ChatClient` as the room client with a draft sink,
  persister and URL provider. Drafts are **not** in `chat_outbox` /
  `chat_sync_jobs`, so the chat sync scheduler never mistakes them for chats.
- Join loop: `GET /epoch` → if it differs from the local epoch, reset the local
  doc to empty and adopt it → run `ChatClient` on `?epoch=N`. On
  `Disconnected` or `ServerReset`, re-check the epoch before redialing.
- No per-edit outbox: after each (re)join catches up, if local changes are
  unacknowledged the host pushes one full-snapshot row (small; merge is
  idempotent).
- Local persistence: a snapshot under store doc id `draft/{chatId}` so a draft
  survives an app restart offline; deleted on discard.
- Commits are debounced to 250 ms. The edge allows 300 pushes per device per
  minute, and continuous typing at 250 ms is ~240.
- Send/clear: `ClearDraft` empties the local doc, `POST /discard?epoch=N`,
  adopts `N + 1` and deletes the local snapshot. If offline, a `pending_discard`
  flag is kept and the discard runs on the next join. Discard is idempotent.
- Handles are pinned in the LRU while they have a watcher or unacknowledged
  changes.
- Draft rooms count against the process-wide socket budget; only chats with an
  open composer watcher join (not every chat).

RPC (`crates/rpc`, `crates/proto`), all served by the **local** engine and
deliberately not `forwardable` (drafts are per-user replicated state; every
device runs its own room client):

- `WatchDraft { chatId }` → stream of `{ revision, text }` first, then
  `{ revision, splices: [TextSplice] }` per change (local echoes excluded).
- `EditDraft { chatId, baseRevision, splice }` → `{ revision }`. If
  `baseRevision` is behind, the engine rebases the splice over the splices since
  then (bounded log); if the base is too old it returns `resync` and the UI
  reloads via `WatchDraft`.
- `ClearDraft { chatId }`.
- New capability constant in `proto::workspace::capabilities`; the UI only
  uses drafts when the engine advertises it, so older daemons keep working with
  the current local-only drafts.

### 4. Composer (`crates/ui`)

- `AppState` runs a draft watch per chat with an open composer (like
  `spawn_queue_watch`) and hands `(revision, splices)` to the composer.
- Local user edits: on `Edited`, compute `TextSplice::from_diff(old, new)` and
  send `EditDraft` (coalesced). Programmatic changes — loading a draft on
  navigation, clear on submit, restore after a failed send, queue-edit
  recovery, applying a remote splice — are flagged and never echoed back as
  user edits.
- `ComposerInput::apply_remote_splice(splice)`: edits `content`, transforms
  `selected_range`, `selection_reversed`, `marked_range`, `drag_unit` and mention/
  slash token ranges through it; bumps `edit_revision` (so pending paste
  rewrites abort); refreshes the projection; does **not** set `follow_cursor`.
  Remote splices are deferred while an IME composition is active and applied
  right after it commits.
- Undo/redo stacks hold whole-content snapshots, so they are cleared when a
  remote edit lands (documented trade-off).
- Send: the existing clear-on-submit path also calls `ClearDraft`. A failed send
  restores the text as a fresh draft.
- The existing `drafts` map stays as the fallback for engines without the
  capability and for the new-chat canvas.

## Failure modes

| Case | Behaviour |
|---|---|
| Offline while typing | Local doc keeps the edits; merged on next join. |
| Two devices type at once | Loro merges; both edits survive. |
| Device offline during send, reconnects later | Epoch mismatch → drops its old copy; never resurrects the sent text. |
| Discard races with a remote keystroke | The few ms of text typed after the sender's discard is lost (accepted). |
| Stale `EditDraft` position | Rebased, or `resync`. |
| Rate limit / oversize | Client backs off (existing quota retry); oversized drafts are rejected, the local text is kept. |
| Old daemon without the capability | Falls back to today's local-only drafts. |

## Testing

- `zeron-doc`: property/unit tests for `from_diff`, `transform_offset`,
  `rebase_splice`; two-writer concurrent-insert merge; byte offsets with
  multi-byte and combining characters.
- `edge`: unit tests for frame/epoch behaviour; `test/workerd` tests for
  discard, epoch mismatch, idle-expiry alarm and privacy across users; a live
  `scripts/draft-check.mjs` against `wrangler dev`.
- `engine`: integration test with a loopback fake room and two engines —
  live sync, concurrent typing, offline edit + reconnect, discard on send with an
  offline third device, restart persistence, LRU pinning.
- `ui`: gpui tests for caret preservation through remote splices before, inside
  and after the caret; IME deferral; programmatic edits not echoed; capability
  fallback.
- PR verification: `cargo test` for the touched crates, `cargo clippy`, and
  `npm run typecheck && npm test` in `edge/`.

## Extending to mobile / thin client

Everything above the UI is reusable: `DraftDoc` and the splice math live in
`zeron-doc` with no I/O; the room client is the generic `ChatClient`. A later
change adds a draft room next to `Room` in `zeron-client`, exposes
`draft` state and `draft_edit`/`draft_clear` through the FFI, and maps a native
text view's edits to `TextSplice`s.
