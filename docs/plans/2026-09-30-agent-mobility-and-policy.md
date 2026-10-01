# Agent mobility and policy

Status: plan. Part 1 (moving a session between devices) is being built first;
the other parts are designed here so that it doesn't paint them into a corner.

This plan covers five features that share three foundations:

| Foundation | What it is | Used by |
| --- | --- | --- |
| **Agent policy** | What an agent may do: a permission mode and a sandbox | permission modes, plan mode, presets, cloud |
| **Agent presets** | A named bundle of harness, model, policy, instructions and tools | personalities, plan → execute, delegation |
| **Session mobility** | Where a chat runs, and moving it, live, between hosts | moving between PCs, continuing in the cloud |

Decisions already made:

1. The default permission mode stays **Bypass** (today's behaviour).
2. Cloud is **bring your own Cloudflare account**.
3. A move uses the target device's own agent login when it has one. When it
   doesn't, the source device sends its OAuth credentials over the
   device-to-device channel.
4. A move **starts immediately** and **finishes at the next safe step**, with
   a **Move now** option. Everything the work needs travels with it, including
   ignored files and files outside the repository. An agent decides what
   "everything" is. Transfers are rsync-style: only what differs is sent.
5. Sandboxing should behave the same on every OS (see Part 4).

---

## Part 1: Moving a live session between devices

### What the user sees

A chat runs on the laptop. The user clicks **Move to…** in the chat header and
picks the desktop. The agent keeps working while the desktop prepares and
copies the workspace. At the next safe step the run pauses, the last changes
are copied, and the chat continues on the desktop. The transcript shows a
*Moved from Laptop → Desktop* divider. The sidebar, every device and the phone
now show the desktop as the chat's host. **Move back** works the same way and
is fast, because only what changed travels.

A banner shows the progress while a move runs: *Preparing · Copying 214 MB
(40%) · Waiting for the current step (Bash: cargo build, 4m) · Finishing*.
It has **Move now**, which interrupts the current step, and **Cancel**.

### Why not WebAssembly or process snapshots

The work isn't Zeron's code. It's `claude`, `codex`, Node and native toolchains
running bash, git and compilers. WASI can't run those binaries. A WASM snapshot
only covers the module's own memory, not child processes. Checkpointing a
process (CRIU) is Linux-only, can't cross macOS ↔ Linux, and wouldn't carry the
live connection to the model API anyway.

Model calls are stateless, though. An agent can be rebuilt completely from
three things: **the conversation**, **the workspace**, and **the harness's own
session file**. Processes are disposable. So a move pauses at a safe point,
moves that state, and resumes natively. It follows the VM live-migration
pattern: **copy ahead while running**, then a short **stop-and-copy** of the
last delta.

### Actors

- **Source** (A): the engine hosting the chat. It drives the move, because it
  has the data and is the only device allowed to change the chat's host. That
  serialises moves without any server-side lock.
- **Target** (B): the receiving engine. It decides *where* things land on its
  own disk. The source never names an absolute path on the target.
- **Initiator**: any UI (A's, B's, a phone). It calls `StartMove` on the host
  (`targetDeviceId` = the host).

### Phases

```
idle ──StartMove──▶ preparing ──▶ copying ──▶ waiting ──▶ finishing ──▶ handover ──▶ resuming ──▶ done
                       │             │            │            │            │
                       └──────── any failure before handover: abort; A keeps (or resumes) the chat
```

1. **Preparing** (agent still running)
   - The target checks it can take the chat and reports:
     - whether the harness is installed and signed in;
     - free disk space;
     - the repository it will use, found by root commit and remote URL, with
       its known commit tips;
     - landing roots;
     - its lineage from an earlier move of the same chat.
   - The source builds the **transfer plan** (below). The deterministic part is
     ready in seconds. The **scout agent** runs alongside it and adds what the
     deterministic part can't know.
2. **Copying**: the bulk transfer (rsync-style sync) and the git bundle go to
   the target's staging area while the agent keeps working. Files the agent
   edits mid-copy are skipped; the final delta covers them.
3. **Waiting**: wait for a **safe point**, meaning no tool is running and no
   native subagent is live. A run parked between turns is already safe. A
   pending question is safe, and the question is asked again after the move.
   **Move now** skips the wait. At the safe point the source pauses the chat's
   command drain and interrupts the run.
4. **Finishing**: the source does a final plan and sync (usually KBs). This
   carries the harness session files and the journal, and fixes the git state:
   new commits as a bundle, and the index as a staged patch.
5. **Handover**: the source calls `MoveCommit` on the target. The target:
   1. applies staging to the landing roots (git objects, refs, index, files,
      deletions);
   2. imports the harness session;
   3. creates or finds its space;
   4. imports the processed-command ledger for the chat;
   5. writes the chat row's new placement in one registry `Update` op
      (`deviceId`, `spaceId`, `cwd`, `branch`, `checkoutId`,
      `harnessSessionId`, `harnessSessionCwd`).

   From that write on, the target is the host. If the commit reply is lost,
   the source reads the registry row: if the row names the target, the move
   happened.
6. **Resuming**: the target writes a `Moved` seam into the transcript. If the
   run was mid-turn, it dispatches a short visible *Continue where you left
   off.* The hidden **move note** is prepended to what the harness receives:
   - path mappings;
   - processes that were running on the source and didn't come along;
   - a question that was pending;
   - the scout's notes.

   If the chat was idle, the note waits and is prepended to the next prompt.
7. **Done**: the source keeps its copy of the workspace, and both sides record
   **lineage**. The source records the hashes of what left; the target records
   where everything landed. A later move in either direction then sends only
   what changed, and the first move back lands exactly where the work started.

A failure before handover aborts the move. If the source had interrupted a run,
it resumes the run locally with a note that the move was cancelled. The
target's staging area is removed.

### The transfer plan

Files travel under **logical roots**. The target maps each root to a folder it
chose:

| Root | Contents | Target landing |
| --- | --- | --- |
| `ws` | The workspace tree (see below) | Lineage landing, else a git worktree or clone, else the same home-relative path if free, else `~/Zeron/Moved/<name>` |
| `x<n>` | Extra paths outside the workspace (screenshots in `~/Desktop`, a dataset in `/data`) | The same home-relative path if free or lineage-owned, else `~/Zeron Transfers/Moves/<chat>/…` |
| `hs` | The harness's native session files | Imported into the harness's own store by its adapter |
| `up` | Attachments referenced by the transcript | The target's uploads folder, plus an alias so old messages still render |
| `jr` | The run journal | `{store}/journals/` |
| `git` | The git bundle and the staged patch | Staging only; consumed by apply |

**The workspace tree:**
- **Git:** the repository root containing the chat's cwd. **Non-git:** the cwd
  itself.
- Everything except `.git` and *regenerable heavy folders* is included:
  `node_modules`, `target`, `.venv`, `venv`, `__pycache__`, `dist`, `build`,
  `.next`, `.turbo`, `.gradle`, `.cache`, `DerivedData`, `Pods`.
- Ignored files are included (`.env`, local config).
- A file over 512 MB, or a total over the size guard, needs the scout's or the
  user's explicit confirmation.
- If the cwd *is* the home folder, or the tree is unreasonably large, the
  workspace is reduced to the files the session actually used.

**Git state** travels as git, not as files:
- objects as a bundle of what the target lacks (`git bundle create … HEAD
  --not <target tips the source knows>`);
- the branch name and HEAD;
- the index as `git diff --cached --binary`.

The working tree is then plain file sync, compared by content. Both sides
apply the same exclusion rules, so their file sets line up.

**Extra paths** come from two places:
- **Deterministic harvest:** every absolute or `~` path in the run journal's
  tool calls (`file_path`, `path`, path-like arguments to shell commands, image
  attachments) that exists, is outside the workspace, and isn't a secret store
  (`~/.ssh`, keychains, `~/.aws`, credential files).
- **The scout agent:** a one-shot, tool-less run with the chat's harness and
  its cheapest model, sharing the title generator's isolation. It gets a
  condensed transcript tail, the harvested candidates with sizes, and the
  workspace summary. It returns JSON:
  ```
  { "include": [{ "path", "reason" }],
    "exclude": [{ "path", "reason" }],
    "notes": [ "…for the agent after the move…" ] }
  ```
  The engine validates every suggestion (exists, size, not a secret store).
  The scout has 60 s and is best-effort: without it the move still carries
  the workspace and the harvested paths.

### Rsync-style sync (`zeron-transfer`)

The device transfer from #670 gains a **sync mode** used only by moves:
- **Tickets.** The target's move service registers a ticket for the round
  (peer, move id, staging folder, basis roots). An offer carrying that ticket
  is accepted without asking, because the move itself was the user's decision.
  It lands in the ticket's staging folder. An unknown ticket is refused. The
  sender never chooses where files land.
- **Content hashes up front.** Each file entry carries its SHA-256, computed
  with a `(size, mtime, inode)` cache so unchanged files aren't re-read. If the
  basis file at the landing already has that content, the receiver reports it
  `done`/`unchanged` in `Accept.have` and its bytes never travel.
- **Block deltas** for large files (≥ 4 MiB): the entry also carries per-block
  hashes. The receiver seeds the `.part` from the basis file, copying the
  blocks whose hashes match and reporting them in `have`. Only differing 1 MiB
  blocks travel. This reuses #670's resume machinery unchanged.
- **Tolerant mode** for the copy-ahead round. A file that changes while it is
  sent is `Skip`ped instead of failing the transfer. The final round, taken
  while the agent is stopped, carries it.

Apply then renames staged files over the landing, atomically per file. Before
overwriting or deleting a landing file that changed locally since lineage
recorded it (the user edited the old copy), the old content is backed up to
`~/.zeron/move-backups/<move>/` and reported.

### Harness sessions

A new `SessionPort` per harness: `export(session_id, cwd)` lists the native
files, and `import(files, old_cwd, new_cwd)` places them in the target
harness's store and rewrites absolute cwd fields.

| Harness | Native store | Approach |
| --- | --- | --- |
| Claude | `$CLAUDE_CONFIG_DIR/projects/<encoded cwd>/<id>.jsonl` (+ `<id>/`) | Native: re-encode the new cwd and rewrite each record's `cwd` |
| Codex | `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*-<thread>.jsonl` | Native: same relative path, rewrite `cwd` in `session_meta` |
| Pi | `~/.pi/agent/…/<session>.jsonl` + Zeron index | Native: place the file, rewrite the index and header cwd |
| Cursor | `~/.zeron/cursor-state/agents/<uuid>/` + `by-agent` marker | Native: copy the store, rewrite the marker path and cwd |
| Grok | `~/.grok/sessions/<urlencoded cwd>/<id>/` | Native: re-key the folder by the new cwd |
| OpenCode, Devin, Hermes, Antigravity | Opaque server or CLI stores | **Transcript replay**: a fresh session gets the conversation once |

Replay generalises the side-chat history bootstrap (`fork_history_prompt`). A
moved chat whose session couldn't be carried tombstones its resume id and
marks the doc so the next run replays history once.

### Host change in the engine

These are correctness fixes the move relies on:
- `ingest_relayed_command` checks that this device still hosts the chat (today
  a stale former host would run a relayed command).
- **Became-host watcher**, modelled on `spawn_cutover_watcher`. When a chat row
  starts naming this device, it opens the chat and drains commands and queue.
  A former host stops draining.
- **Placement** is one targeted registry `Update`, not `set_chat_host`'s
  whole-row upsert, which would clobber concurrent title/config/seen writes.
- **Ledger handover.** The source pauses drains and waits for its `executing`
  set to empty. The processed-command ids for the chat then travel in
  `MoveCommit` and are imported into the target's ledger, so nothing runs
  twice.
- **Safe-point signal.** `RunHandle` exposes a `watch` of `{tools_in_flight,
  inputs_pending, subagents_live, parked}`, derived where `drive_run` already
  computes the quiesce gate.

### Credentials (decision 3)

During preparing, the target reports `signedIn` for the chat's harness. If the
target is not signed in, the source exports the harness credential through the
agent-account credential-slot code (Keychain on macOS, files elsewhere). It
travels inside the move's encrypted P2P/relay channel and is imported into the
target's slot. The source keeps its own.

The UI says which login the target will use. OAuth refresh tokens can rotate,
so two devices sharing one grant may make one of them sign in again later. The
move banner states this when it happens.

### Status everywhere

The host writes a `move` field on the chat row (move id, target, phase,
detail, bytes, started, error), throttled to one write per second. The
desktop, iOS and Android all render the banner from the row they already sync.
No new subscription is needed.

### Surfaces

- **Desktop:**
  - **Move to…** in the chat header and the session context menu;
  - a device picker that shows online engines, harness availability and
    sign-in;
  - the banner, with **Move now** and **Cancel**;
  - the transcript's `Moved` divider.
- **MCP:** `move_chat {chat?, device, when}`, so an agent can move itself or
  another chat ("continue this on the GPU box").
- **RPC:**
  - `StartMove`, `MoveNow` and `CancelMove` (forwardable to the host);
  - `MovePrepare`, `MoveCommit` and `MoveAbort` (engine to engine);
  - capability `session-move-v1`.

### Testing

- **Unit tests:**
  - plan building (exclusions, harvest, secret-store refusal);
  - the hash cache;
  - sync-mode transfer: tickets, unchanged files, block seeding, tolerant
    skips, overwrite plus backup;
  - git ops against temporary repositories: bundle, worktree landing, branch
    rules, index patch, a round trip back;
  - each `SessionPort` against recorded fixtures.
- **Two-engine end-to-end** on `zeron local-edge` with the mock harness:
  - move an idle chat;
  - move a chat mid-run, at a safe point and with Move now;
  - move it back and check that only the delta travels;
  - cancel during the copy;
  - a target crash before commit.

  Each checks the files, the git state, the row placement, the seam, and the
  continuation run on the target.
- **Live, ignored tests** with the real Claude and Codex CLIs, recalling a
  codeword across a move.

---

## Part 2: Continuing in the cloud (Cloudflare, bring your own account)

A cloud box is just another Zeron device, short-lived and headless, so Part 1's
protocol moves work there and back unchanged.

- **Image:** Alpine, plus the static musl `zeron headless` from #639, plus the
  agent CLIs, git and common toolchains. Published with each release.
- **Provisioning:**
  - the user connects their Cloudflare account (an API token with Workers,
    Containers and R2 scopes);
  - Zeron deploys a small Worker + Durable Object + Container app into
    *their* account;
  - the Durable Object starts and stops the container;
  - the engine enrolls as a device with a scoped token minted by the Zeron
    edge, marked `kind: cloud`.
- **Durability:** container disk is ephemeral. The engine checkpoints the
  workspace and harness state to the user's R2 bucket on idle and before
  sleep, using the same plan and sync code, and rehydrates on start.
- **Lifecycle:** "Continue in cloud" is "Move to…" with a cloud target that
  may still need starting. It goes to sleep after an idle window, and the user
  can **Move back** at any time.
- **`CloudProvider` trait:** provision, start, stop, destroy, status, cost.
  Cloudflare first; Fly or Hetzner later. A plain VPS running `zeron
  headless` already works today.
- **Things to verify before building:**
  - current Containers instance sizes;
  - no nested containers, no GPU;
  - outbound networking for model APIs and package registries.

## Part 3: Agent presets ("personalities")

`AgentPreset { id, name, description, icon, harness, model, reasoning, options,
policy, instructions, tools {allow, deny}, target (device / cloud / any),
worktree, may_spawn, fallbacks }`.

- **Storage:** user-level presets are synced registry rows. Project-level
  presets are `.zeron/agents/*.md` (frontmatter + body, the same format as
  `.claude/agents`, so those can be imported).
- **Where presets are used:**
  - the new-session picker;
  - MCP `create_chat { agent }` plus `list_agents`, where descriptions tell
    orchestrators when to use each preset;
  - "execute with" on an approved plan.
- **Snapshotting:** a chat stores the preset id plus a resolved snapshot, so
  editing a preset never silently changes a running chat.
- **Spawn ceiling:** a spawned chat gets the lower of its preset's policy and
  its spawner's. Raising it needs the user.
- **Cross-device delegation** already exists in open #671 (top-level or side
  chats on any device) and #646 (any provider delegates to any other).
  Presets plug into both.

## Part 4: Permission modes and sandboxing

Today every harness effectively runs in bypass mode:
- Claude auto-allows `can_use_tool`.
- Codex is pinned to `danger-full-access` and `never`.
- OpenCode answers "once".
- ACP forces a bypass mode.
- Cursor ignores approvals.
- Pi has no permission gate.

Policy model:
- **`PermissionMode`:** `Plan`, `Ask`, `AcceptEdits`, `Auto`, `Bypass`
  (default).
- **`Sandbox`:** `Off` (default), `WorkspaceWrite`, `ReadOnly`, plus network on
  or off.

Enforcement, strongest first:
1. **Native harness modes** where they exist: Claude `--permission-mode` and
   live `set_permission_mode`; Codex per-turn `approvalPolicy` and
   `sandboxPolicy`; ACP mode options.
2. **Zeron's policy engine** answers the approval requests that already reach
   it: Claude `can_use_tool`, Codex `requestApproval`, OpenCode
   `permission.asked`, ACP `request_permission`.
   - `Auto` uses rules: safe reads and in-workspace edits are allowed;
     destructive or out-of-workspace actions ask.
   - "Always allow" in the question panel writes a rule.
   - A cheap-model classifier can come later.
3. **An OS sandbox around the harness process**, so the same policy holds
   everywhere (decision 5). A WASM sandbox can't host the native agent
   binaries, so the uniform layer is the *policy*, with two kinds of backend:
   - **Native backends:** macOS Seatbelt, Linux Landlock + seccomp (or
     bubblewrap), Windows AppContainer.
   - An optional **Linux microVM backend:** Virtualization.framework on macOS,
     WSL2 on Windows, namespaces on Linux. It behaves identically on every OS,
     because the agent always runs on the same Linux.

The sandbox also confines each harness's own subagents, because they share
its process tree.

Other parts:
- `HarnessDescriptor` declares a capability matrix, so pickers grey out modes a
  harness can't honour.
- `RunRequest.auto_approve` becomes `policy`.
- Approvals reach any device through the existing question bridge plus push
  notifications.

## Part 5: Plan mode

- **Native where it exists:**
  - Claude: `--permission-mode plan`, and `ExitPlanMode` arrives through
    `can_use_tool` carrying the plan.
  - OpenCode: its `plan` agent.
  - ACP: plan-like modes.
  - Codex: its `/plan`, which is currently rejected as unmapped.
- **Emulated elsewhere:** read-only policy + an instruction + a Zeron MCP
  `submit_plan` tool. Every harness already has the Zeron MCP server.
- **The plan is a transcript part,** `MessagePart::Plan { revision, markdown,
  status }`, with these actions:
  - **Approve → run in mode X**;
  - **Refine** with comments;
  - **Execute with preset Y**, e.g. plan with Opus and execute with Codex.

## Order of work

| # | Work | Depends on |
| --- | --- | --- |
| 1 | **Session move** (this branch): transfer sync mode, plan + scout, git landing, session ports, host handover, UI, MCP | #670 (stacked) |
| 2 | Move credentials (decision 3) | 1 |
| 3 | Policy model + capability matrix + Zeron-answered approvals | — |
| 4 | Plan mode | 3 |
| 5 | Auto-mode rules | 3 |
| 6 | Sandbox backends (native, then microVM) | 3 |
| 7 | Agent presets | 3, #671, #646 |
| 8 | Cloud provider: Cloudflare (BYO) + image + R2 checkpoints | 1, #639 |
