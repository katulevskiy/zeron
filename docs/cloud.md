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
| Container image (Debian trixie-slim, Node, git, agent CLIs, non-root `zeron`, `tini`) | `cloud/image/Dockerfile` |
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

Every request needs `x-zeron-control: $ZERON_CONTROL_TOKEN`.

- `GET /zeron/cloud/status` returns `{busy, lastActivityAt, runs, moves,
  terminals}`. `busy` is true while any run, move, terminal or connected
  viewer is active.
- `POST /zeron/cloud/checkpoint?final=0|1` returns `{seq, objects, bytes}`.
  A final checkpoint pauses drains, as a move does.
- On SIGTERM: final checkpoint within the 15-minute grace, then exit.

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
| `GET /HEAD` | Latest sequence |

- **Objects:** zstd tar packs of small files (≤ 64 MiB each), or ≤ 64 MiB
  chunks of large files, named by the SHA-256 of their content.
- **Manifest:** `{seq, createdAt, roots: {name: {path, entries: [{rel, kind,
  mode, size, mtimeMs, target?, chunks: [sha], pack?: sha}]}}}`.
- **Roots:** `data` (Zeron data dir; SQLite via `VACUUM INTO` copies),
  `harness` (`~/.claude`, `~/.codex`, …), and one root per workspace
  (including `.git` and regenerable folders; the box keeps its build caches).
- **Restore order at boot:** `data` and `harness` first, workspaces lazily
  (a chat's workspace before its first run).

### 6. Enrollment: device credentials

- `POST {edge}/cloud/devices` (WorkOS-authenticated, org-scoped token). Body
  `{name?}` (1–80 characters, default "Cloud"). Returns `{deviceId,
  credential}` once: a fresh UUID and 32 random bytes, base64url. A token
  without `org_id` gets 403.
- `POST {edge}/auth/device-token` (no bearer). Body `{deviceId, credential}`.
  Returns `{token, expiresAt}` (`expiresAt` in epoch ms): an ES256 JWT with
  `iss: "zeron-edge"`, `sub` = user id, `org_id`, `did`, `kind: "cloud"`,
  `iat`, and `exp` 30 minutes later. Errors: 400 malformed body; 401
  `{error: "invalid_credential" | "revoked"}`; 429 `{error: "rate_limited",
  retryAfter}` with a `Retry-After` header.
- `GET {edge}/.well-known/zeron-device-jwks.json` publishes the public key
  (`kid` = the key's JWK thumbprint unless the secret names one).
- `DELETE {edge}/cloud/devices/{deviceId}` (WorkOS, the owner only; 403 for
  anyone else, 404 if unknown) revokes. Idempotent, returns `{revokedAt}`.

**Storage.** The record lives in the device's own DeviceRoom (`d2/{deviceId}`,
table `device_credential`): `{userId, orgId, name, salt, sha256(salt ‖
credential), createdAt, revokedAt}` plus a failure counter. The exchange knows
only the device id, and that room is already the per-device identity anchor,
so there is no new Durable Object class, binding, or migration. Minting also
claims the room for the user. The Worker reaches the record on internal
`/credential/*` paths that no public route forwards to. There is no per-user
list of boxes on the edge: the `cloudBoxes` registry rows are that list.

**Verification.** Digests are compared in constant time. After 10 failures
inside 10 minutes, every attempt for that device (right or wrong) gets 429
until the window ends. Unknown device ids write nothing.

**Signing key.** The wrangler secret `ZERON_DEVICE_JWT_KEY`: a private P-256
key as a JWK (JSON) or PKCS8 PEM. Unset, the enrollment and exchange routes
answer 501 and device tokens are rejected.

**Using the token.** `verifyToken` checks the unverified `iss` first, in every
auth mode: a `zeron-edge` token is verified against the device key only (a
forged one fails closed rather than falling through to WorkOS or dev-bearer
parsing). It maps to the owner's own `userId`/`orgId`, so every room name the
Worker derives (`reg1/{org}/{user}`, `blob/{user}/…`, `preview1/{org}/{user}`)
is the owner's. Device tokens may reach only:

| Route | Allowed |
| --- | --- |
| `/device/{did}/ws` | `role=host` only, own device |
| `/device/{other}/ws` | `role=client` (dialing the owner's other devices) |
| `/device/{id}/status`, `/device/{id}/nudge` | any of the owner's devices |
| `/device/{did}/sidecar/*` | own device only |
| `/registry/{org}/…`: `ws`, `rows`, `push`, `stats` | yes (not `reset`, not `push-target`) |
| `/chat2/{id}/…`: `ws`, `checkpoint`, `rows`, `tail`, `diff`, `stats` | yes (not `reset`) |
| `/blob/*`, `/preview/{org}/ws`, `POST /diff/{chatId}` | yes |
| everything else (`/cloud/*`, `/auth/orgs`, `/session/*`, `/workspace/*`, …) | 403 |

This is wider than "registry and chat rooms" on purpose: a box is the source
of a move back to a desktop, and a move source dials the target's device room
as a client and signals P2P through the preview room; tool-output blobs and
the legacy diff slot are part of hosting a chat. The registry room already
holds the box's `wakeKey`, so the box is inside the user's trust boundary;
what the scope withholds is managing credentials or memberships, operator
resets, and hosting any device but itself.

**Revocation.** New exchanges fail at once. The DeviceRoom closes the box's
live host socket (code 4403) and refuses host joins made with a device token
of a revoked credential. Other routes keep accepting an already-issued token
until it expires (at most 30 minutes); checking them would cost a DO round
trip per request.

**Local edge.** `crates/localedge` mirrors the routes, scope, failure limit,
and revocation. The owner is the shared-secret holder, so tokens carry
`sub`/`org_id` = `local`. Its tokens are HS256 under a random key kept in the
edge database (the JWKS is empty), and every request re-checks revocation (one
SQLite read). In development a box may also use the shared secret instead.

**Engine side.** With `ZERON_DEVICE_ID` + `ZERON_DEVICE_CREDENTIAL` set
(`zeron headless` reads them, then removes the credential from its environment
before any thread starts, so agents and terminals never inherit it):

- The device id is pinned: `{data_dir}/device-id` is rewritten to it before
  the store opens (`zeron_engine::pin_device_id`).
- `Auth` runs in device mode: WorkOS and dev bearers are ignored. Its bearer
  comes from `/auth/device-token`, renewed 5 minutes before expiry. Failures
  back off from 1 s to 60 s (or the edge's `Retry-After`), shared by every
  consumer.
- The scope is `Synced` for the token's `sub`/`org_id`. Startup waits for the
  first token unless `device-session.json` (written after each token) already
  names the owner, in which case the box opens its profile offline.
- Nothing interactive: sign-in RPCs fail, sign-out is ignored, and a rejected
  or revoked credential is logged ("re-enrol the box") and retried, never
  turned into a sign-out.

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
