# Continue in the cloud (Cloudflare, bring your own account)

A **cloud box** is a Zeron device that runs in a Cloudflare Container in the
user's own Cloudflare account. It sleeps when idle and wakes when work is sent
to it. Moving a chat there (and back) is the ordinary session move
([`session-move.md`](session-move.md)); the box only adds the ability to be
asleep, to wake on demand, and to keep its state in R2 while asleep.

Plan: [`plans/2026-09-30-agent-mobility-and-policy.md`](plans/2026-09-30-agent-mobility-and-policy.md),
Part 2. Platform research (2026-10-01) shaped these choices:

- **No `@cloudflare/containers` dependency.** The `Container` class is being
  retired (maintained only until 2026-12-31). The Worker uses `ctx.container`
  directly.
- **The `default` scheduling policy, which is GA.** It pulls a public Docker
  Hub image, so nothing has to be pushed into the user's registry. The
  `durable_object` policy and snapshots are beta; later.
- **Code running inside a container is not activity.** The Durable Object
  keeps the box awake by polling the engine every 60 s and renewing the
  inactivity timeout itself.
- **Plain REST from Rust.** No wrangler, Docker or Node is needed on the
  user's machine.

## Components

```
user's desktop engine ──REST──▶ Cloudflare API (provision: R2, Worker, container app, secrets)
any engine ──signed HTTPS──▶ Worker `zeron-cloud` ──▶ DO `ZeronBox` ──ctx.container──▶ container
                                                    ▲                                      │
                                                    └──── R2Gateway (r2.zeron.internal) ◀──┘ checkpoints
container engine ──WSS──▶ Zeron edge (device credential) ── like any device
```

| Piece | Where |
| --- | --- |
| `CloudProvider` trait + Cloudflare implementation (REST) | `crates/cloud` |
| Worker template (`ZeronBox` DO, `R2Gateway`, signed routes) | `cloud/worker/` (prebuilt ESM, embedded in the binary) |
| Container image (Debian trixie-slim, Node, git, agent CLIs, non-root `zeron`, `tini`) | `cloud/image/Dockerfile`, pushed by `.github/workflows/cloud-image.yml` |
| `zeron cloud-boot`, control server, checkpoints | `crates/engine/src/cloud/` |
| Device credentials (enrollment) | `edge/src` + `crates/localedge` + `crates/engine/src/auth.rs` |
| Box records (synced) | registry kind `cloudBoxes` |

## Contracts

### 1. Box record (registry kind `cloudBoxes`, one row per box, private to the user)

```json
{ "id": "<device id of the box>", "provider": "cloudflare", "name": "Cloud",
  "accountId": "…", "workerUrl": "https://zeron-cloud.<sub>.workers.dev",
  "wakeKey": "<base64 32 random bytes>", "instanceType": "standard-2",
  "idleMinutes": 15, "createdAt": 0, "state": "asleep|waking|running|checkpointing|error",
  "stateAt": 0, "error": null }
```

- Written by the provisioning device, and later by any of the user's devices
  (state).
- `wakeKey` is the HMAC key for the Worker's routes. The registry room is
  private to the user, so this is the same trust boundary as the user's own
  devices.
- The box's device row appears once the box engine first connects. Pickers
  show a box from its record before that.

### 2. Worker routes (called by any of the user's engines)

`POST {workerUrl}/v1/boxes/{deviceId}/wake | /stop | /status`

- **Signature.** Headers `x-zeron-timestamp` (ms) and `x-zeron-signature` =
  hex(HMAC-SHA256(wakeKey, `{METHOD}\n{path}\n{timestamp}\n{hex(sha256(body))}`)).
  The Worker rejects a timestamp more than 5 minutes from its own clock.
- **Keys.** The Worker reads per-box keys from the secret `ZERON_BOXES`:
  a JSON `{deviceId: {wakeKey, credential}}`.
- **wake** starts the container if it's stopped. Body `{}`. Reply
  `{state:"waking"|"running"}`. The engine inside dials the edge on its own.
- **stop** asks the engine for a final checkpoint, then stops the container.
  Reply `{state}`.
- **status** replies `{state, startedAt?, lastActivityAt?, busy?}`.

### 3. Durable Object `ZeronBox` (one per box, `getByName(deviceId)`)

- **`start`:** `ctx.container.start({ enableInternet: true, env })` with:
  - `ZERON_DEVICE_ID`, `ZERON_DEVICE_CREDENTIAL`, `ZERON_EDGE_URL`;
  - `ZERON_CONTROL_TOKEN` (random per start, kept in DO storage);
  - `ZERON_CLOUD_CONTROL_PORT=8787`, `ZERON_CHECKPOINT_URL=http://r2.zeron.internal`.

  Then `setInactivityTimeout(idleMinutes)`, register
  `interceptOutboundHttp("r2.zeron.internal", R2Gateway)`, and set a 60 s
  alarm. The constructor sets the timeout again after a DO restart.
- **Alarm:** `GET http://container:8787/zeron/cloud/status` over
  `ctx.container.getTcpPort(8787)`, with header `x-zeron-control`.
  - If `busy`, or `lastActivityAt` is within the idle window: renew the
    timeout and set the alarm again.
  - Otherwise: `POST /zeron/cloud/checkpoint?final=1`, then
    `ctx.container.signal(15)` (SIGTERM) and record `asleep`.

### 4. Engine control server (inside the box, `0.0.0.0:$ZERON_CLOUD_CONTROL_PORT`)

`zeron cloud-boot` (the image's entrypoint) restores `data` and `harness`
from the store's `HEAD` (§5), then starts the headless engine as `zeron
headless` would, with the control server and checkpoints attached. A first
boot finds no `HEAD` and restores nothing. A restore that fails stops the
boot: a box must not start empty and then overwrite its own checkpoint. The
server only runs when `ZERON_CLOUD_CONTROL_PORT` is set, and refuses to start
without `ZERON_CONTROL_TOKEN`.

The image (`cloud/image/Dockerfile`, linux/amd64) is Debian trixie-slim with
`tini` as PID 1, git, openssh-client, curl, ripgrep, python3, Node 24 LTS
(official tarball, checksum-verified), and Claude Code, Codex and OpenCode
installed with npm under the non-root user `zeron` (uid 1000,
`HOME=/home/zeron`, npm prefix `~/.npm-global`, so Zeron can update them).
`IS_SANDBOX=1` lets Claude Code accept permission bypass in a container. The
`zeron` binary is the release's Linux x86_64 build. Its desktop libraries
(gpui, the webview) are installed but unused; a static musl engine-only build
passes `ZERON_LIBC=musl` and skips them. The image build fails if `ldd` finds
a missing library. `.github/workflows/cloud-image.yml` pushes
`docker.io/zeronsh/zeron-cloud:<version>` (and `:latest`) after each release.

Every request needs `x-zeron-control: $ZERON_CONTROL_TOKEN` (compared in
constant time; anything else is `401`).

- `GET /zeron/cloud/status` returns `{busy, lastActivityAt, runs, moves,
  terminals, viewers}`. `busy` is true while any of the counts is nonzero:
  - `runs`: runs working or waiting for an answer;
  - `moves`: moves into or out of this device that haven't finished (from the
    chats' registry rows);
  - `terminals`: open terminals;
  - `viewers`: RPC calls in flight and open RPC streams, from the local IPC
    port and from other devices through the host relay. A device showing a
    chat hosted here keeps streams open to it (files, diffs, terminal
    output), so this is the "connected viewer" signal.

  `lastActivityAt` (epoch ms) is the latest of: the last RPC call, and the
  last time the box was seen busy (sampled every 5 s and on every status
  request). It starts at boot.
- `POST /zeron/cloud/checkpoint?final=0|1` returns `{seq, objects, bytes,
  changed}`: the checkpoint now in force, objects and compressed bytes
  uploaded, and whether anything changed (an unchanged checkpoint writes
  nothing). It flushes open docs to the store first. A final checkpoint first
  holds every hosted chat's commands and queue, as a move does (chats a move
  already holds stay the move's), and keeps them held. If the box is still
  running 10 minutes later, the holds lift.
- An incremental checkpoint runs every 10 minutes. It writes only when
  something changed.
- On SIGTERM: hold every chat, give live runs up to 10 minutes to finish,
  stop the engine gracefully (which settles anything still running), then
  write the final checkpoint and exit. That fits Cloudflare's 15-minute grace.
- Workspaces restore lazily: the first dispatch of a run whose folder belongs
  to a workspace not restored yet restores it first (`SessionsEngine`'s cwd
  preparer, also called before a worktree is created from a project).

### 5. Checkpoint store (`$ZERON_CHECKPOINT_URL`, the R2 gateway)

The gateway confines keys to `boxes/{deviceId}/`. Plain HTTP:

| Request | Meaning |
| --- | --- |
| `PUT /objects/{sha256}` | Upload an object (idempotent) |
| `HEAD /objects/{sha256}` | Exists? |
| `GET /objects/{sha256}` | Download |
| `PUT /manifests/{seq}.json` | Write a manifest |
| `GET /manifests/{seq}.json` | Read a manifest |
| `PUT /HEAD` | `{"seq": n}`, written last, so a checkpoint is atomic |
| `GET /HEAD` | Latest sequence (`404` before the first checkpoint) |

Requests that fail with a transport error or a `5xx` are retried 3 times
with backoff.

- **Objects.** Every object is one zstd frame (level 3), named by the
  SHA-256 of its *uncompressed* payload. A name is known without compressing
  anything, and a download is verified by decompressing and hashing it.
  - Files under 4 MiB travel in **packs**: a tar of members named by their
    content's SHA-256 (mode 0644, mtime 0, so equal content gives an equal
    pack). A pack's raw tar stays under 64 MiB.
  - Larger files are cut into 64 MiB **chunks**, one object each.
- **Manifest.** `{version: 1, seq, createdAt, roots: {name: {path, requires?,
  entries: [{rel, kind, mode, size, mtimeMs, target?, chunks?, pack?}]}}}`.
  - `kind` is `file`, `symlink` or `dir`. Every folder is listed, so a
    restore recreates empty folders and every folder's mode and mtime.
  - `chunks` holds the content hashes: one per 64 MiB chunk, or one for a
    small file. It is empty for empty files.
  - `pack` names the pack holding a small file, whose member is `chunks[0]`.
    Without `pack`, every chunk is an object of its own. A small file whose
    content already exists as an object (the tail chunk of a large file, say)
    points at that object instead of being packed again.
  - `requires` lists roots to restore first: a linked worktree requires its
    main checkout, which holds its git metadata.
- **What is uploaded.** Only content the store lacks:
  - A local index (`{data}/cloud-checkpoints/index.json`, never checkpointed)
    remembers which objects exist and each file's hashes by size, mtime and
    inode. Files changed within 2 s of being hashed aren't cached.
  - The previous manifest says which pack holds which small file.
  - Large chunks are probed with `HEAD` before a `PUT`. Small files are never
    probed one by one.

  A small edit uploads one small pack. A restore seeds the index, so the
  first checkpoint after a wake uploads nothing new.
- **Roots.**
  - `data`: the engine data dir. SQLite databases (found by their header)
    are copied with `VACUUM INTO` from a read-only connection: a consistent
    snapshot while the engine has them open, with the WAL's committed
    transactions included. Left out: `-wal`/`-shm`/`-journal` files, `*.lock`,
    temp files (`*.tmp`, `.tmp*`, `*.tmp-*`), sockets and FIFOs, and the
    folders `logs`, `updates`, `adapters`, `worktrees` (workspace roots),
    `local-edge-server`, `cloud-checkpoints`, and `moves/in|out` (staging).
  - `harness`: these paths under the home folder, logins included (the store
    is the user's own bucket, and a box that forgot its logins on every sleep
    would be useless): `.claude`, `.claude.json`, `.codex`,
    `.config/opencode`, `.local/share/opencode`, `.local/state/opencode`,
    `.pi`, `.grok`, `.gemini`, `.hermes`, `.cursor`, `.config/devin`,
    `.local/share/devin`. Cursor's Zeron-side store is in the data dir.
    - Left out: `node_modules`, `.cache`, `.claude/local`, `.claude/statsig`,
      `.codex/log`, `.codex/tmp`, OpenCode's `bin` and `log`, and
      `.cursor/extensions`.
    - SQLite files get the same `VACUUM INTO` copy as in `data`.
  - one root per workspace, named `ws-<first 16 hex of sha256(path)>`.
    - Workspaces are the git toplevels (else the folders themselves) of the
      chats hosted here and of this device's projects, plus each linked
      worktree's main checkout.
    - Everything is included: `.git` and regenerable folders, so the box keeps
      its build caches. Only `*.lock` files inside `.git` are left out (a
      leftover `index.lock` would wedge the restored repository).
    - A chat working in the home folder gets the `home` root instead. It
      leaves out caches and toolchains (`.cache`, `.npm`, `.npm-global`,
      `.cargo`, `.rustup`, …) and the usual regenerable folders. `/` and
      folders above home are never carried.
    - A workspace from the previous checkpoint is kept while its folder still
      exists, even if no chat lists it right now. A momentarily empty chat
      list can't drop a workspace.
  - Roots never nest: a walk skips any path another root covers.
- **Restore order at boot.** `data`, `harness` and `home` are restored in
  place, merged with what's there, before the engine opens anything. Each file is
  written beside its final name, then renamed, with its mode and mtime. Links
  are recreated. Folder modes and mtimes are set last, deepest first.
  - Workspaces wait for their chat's first run. Until then a checkpoint
    carries their entries forward unchanged.
  - A workspace is restored into `.<name>.zeron-restore` beside it, then
    renamed into place. If its folder already exists with content (a move
    landed there first), that content wins.
- **Not done yet.** Garbage collection of objects no manifest references,
  repacking packs that are mostly dead, and pruning old manifests.

### 6. Enrollment: device credentials

- `POST {edge}/cloud/devices` (WorkOS-authenticated). Body `{name}`. Returns
  `{deviceId, credential}`. The edge stores only the credential's hash, and
  the credential is shown once.
- `POST {edge}/auth/device-token` (no bearer). Body `{deviceId, credential}`.
  Returns `{token, expiresAt}`: an edge-signed JWT (ES256) with `sub` = user
  id, `org_id`, `did`, `kind: "cloud"`, valid for 30 minutes.
- **Using the token.** `verifyToken` accepts both issuers. Tokens with `did`
  may join only `/device/{did}/ws` (host role), the user's registry room and
  chat rooms.
- **Revoking.** `DELETE {edge}/cloud/devices/{deviceId}` (WorkOS) revokes the
  credential.
- **Local edge.** It mirrors these routes; in development the box may use the
  shared token instead.
- **Engine side.** With `ZERON_DEVICE_ID` + `ZERON_DEVICE_CREDENTIAL` set,
  `Auth` gets its bearer from `/auth/device-token` and renews it 5 minutes
  before expiry. The device id is fixed (written to `device-id` before the
  store opens).

### 7. Provisioning (from a desktop engine)

RPCs `CloudConnect {apiToken}`, `CloudProvision {name, instanceType, idleMinutes,
region?}`, `CloudBoxes`, `CloudWake {deviceId}`, `CloudStop {deviceId}` and
`CloudDestroy {deviceId, keepData}`.

The API token is kept like agent credentials (Keychain on macOS, a 0600 file
elsewhere). Provisioning is idempotent and runs in this order:

1. `GET /user/tokens/verify` (or `/accounts/{id}/tokens/verify`), then
   `GET /accounts` (pick the one account) and `GET /accounts/{id}/containers/me`.
2. Enrol: `POST {edge}/cloud/devices` returns `{deviceId, credential}`.
3. `POST /accounts/{id}/r2/buckets` (`zeron-cloud`; 409 means it exists).
4. Upload the Worker: `PUT /accounts/{id}/workers/scripts/zeron-cloud`.
   - Multipart: a `metadata` part plus the module.
   - Bindings: DO `BOX`/`ZeronBox`, R2 `CKPT`, plain text `ZERON_EDGE_URL`.
   - `containers: [{class_name: "ZeronBox"}]`.
   - `migrations: {new_tag: "v1", steps: [{new_sqlite_classes: ["ZeronBox"]}]}`
     on the first upload; `old_tag` from `GET /workers/services/zeron-cloud`
     afterwards.
   - `keep_bindings: ["secret_text"]`.
5. Find the DO namespace id: `GET /accounts/{id}/workers/durable_objects/namespaces`,
   matching the class and script.
6. Create or patch the container app: `POST /accounts/{id}/containers/applications`
   with:
   ```json
   {"name":"zeron-cloud","scheduling_policy":"default",
    "configuration":{"image":"docker.io/zeronsh/zeron-cloud:<version>","instance_type":"<type>"},
    "instances":0,"max_instances":<boxes>,"durable_objects":{"namespace_id":"<ns>"},
    "rollout_active_grace_period":900}
   ```
7. Secret `ZERON_BOXES`: `PUT /accounts/{id}/workers/scripts/zeron-cloud/secrets`.
8. Enable workers.dev: `GET/PUT /accounts/{id}/workers/subdomain`, then
   `POST …/scripts/zeron-cloud/subdomain {"enabled":true}`. This gives
   `workerUrl`.
9. Write the `cloudBoxes` row.

Token permissions (account-scoped):
- Workers Scripts: Edit
- Containers: Edit
- Workers R2 Storage: Edit
- Account Settings: Read

### 8. Moving to a box

- **Picker.** `MoveCandidates` lists boxes from their records, as
  "asleep — wakes when you move".
- **Starting the move.** `StartMove` to a box that is offline first calls
  `wake` and waits up to 120 s for the box's presence, then continues as an
  ordinary move. The banner detail reads "Waking the cloud box".
