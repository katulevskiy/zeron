# Contribution Manager

A standalone Rust + SQLite service with an embedded vanilla JavaScript frontend. It follows Zeron's palette, fonts and logo. GitHub remains the source for PRs, native reviews, checks and final merges; the service coordinates scope decisions, task claims and revision-specific validation.

## Website flow

Navigation is explicit: **Overview**, **Pull requests**, **Bugs**, **Features**, **My work**, and **Starred**. Every PR has a visible next step, remaining requirements, claimed work and a five-step journey: scope → review → validation → integration → merge. Actions requiring input open focused dialogs. Claims and stars are one-click actions. Native dialogs support Escape, keyboard focus and mobile layouts.

The review board supports dragging a PR to open the relevant action; it never assigns a computed readiness state or bypasses authorization. Dashboard sections can be reordered by dragging or by using their move buttons. Guests can save stars locally; authenticated stars persist privately in SQLite. Starred includes a Resolved filter for following shipped changes.

The maintainer hotfix dialog combines low-risk urgent triage with accepting the scope. Independent review, required platform tests, risk rules and CI remain enforced. Authors cannot verify their own changes. For imported mirrors, the original contribution author is used for independence; the fork's PR publisher is retained separately as its GitHub author.

## Identity and authority

Public visitors can read the queue and submit bug reports or feature proposals. Guest reports require a display name, clearly identify the author as an unverified guest, and are stored durably in SQLite. The frontend saves the guest's preferred name in localStorage. A GitHub session supplies the username for authenticated reports; form fields cannot forge it. Reports initially enter the local triage inbox and do not silently create GitHub issues.

Live sign-in uses the GitHub App authorization flow with single-use state, a browser cookie, a server session, CSRF protection and stable GitHub user IDs. The access token is used to identify the user and then discarded. Installation credentials check repository permission server-side. Static usernames never elevate live authority:

| GitHub access               | Workflow capabilities                                           |
| --------------------------- | --------------------------------------------------------------- |
| Guest                       | Read and submit named reports                                   |
| Read / ordinary contributor | Claims, demand/urgency signals, personal stars and reports      |
| Triage                      | Scope/priority/platform triage and low-risk direction decisions |
| Write                       | Triage, code review and platform validation                     |
| Maintain / Admin            | All workflow capabilities and risk/scope decisions              |

Each authenticated request verifies the current principal's access. Mutations fail closed when GitHub cannot verify access; logout still works. Native reviewer permissions and stored grants refresh during synchronization. Live branch rules and final merge decisions remain on GitHub.

## Fork-only development preview

`DEV_PREVIEW=true` is accepted **only** for `katulevskiy/zeron`, with a server-side configuration check. **Try a role** lets visitors view guest, contributor, triager, reviewer, validator and maintainer capabilities. The preview uses real imported PR context copied to a separate `*.preview.sqlite` database. Its claims, evidence and stars stay in the sandbox, and no preview request writes to GitHub. It cannot start GitHub synchronization or use real agent credentials. A separate preview cookie preserves the user's actual GitHub login when returning to the live view.

The deployment runs in live mode and never seeds illustrative records. The old `--demo` flag remains solely for local tests and development fixtures.

## Build and run

```sh
cd apps/review
cargo build --release --locked
# Local fixtures only:
target/release/zeron-review --demo
```

For a live service, provide a policy file and process environment. The binary does not automatically load `.env`. See `.env.example`. `core-tests` is the pilot's required independent check. Missing, skipped, failed, pending and older revision/base checks do not count as passing. A historical upstream result does not attest to a fork mirror. Final GitHub branch rules remain the integration authority.

```sh
PUBLIC_URL=https://your-host.example POLICY_PATH=/path/policy.json \
DATA_PATH=/var/lib/contribution-manager/review.sqlite target/release/zeron-review
```

## Register the GitHub App

Set a private `SETUP_TOKEN` of at least 32 characters and a writable `GITHUB_CREDENTIALS_PATH`. The owner opens `PUBLIC_URL/setup/github?token=OWNER_SETUP_TOKEN`. The page submits a preconfigured manifest named **Contribution Manager** to GitHub. The owner registers it and installs it on the selected repository. The callback validates browser state, converts the registration code, verifies ownership and saves the credentials and RSA key with mode 0600. Installation setup verifies access to the configured repository.

The manifest requests Metadata read, Contents read, Pull requests read, Issues write and Checks write. It subscribes to PR, native review, issue/comment and check events. It does not request repository Administration or Contents write. GitHub receives the OAuth callback at `/auth/callback` and sends signed webhooks to `/webhooks/github`.

The service exits with status 75 when credentials change; the included systemd unit restarts it to load an immutable, consistent configuration. For a manually run service, restart it yourself after registration/installation. Sign-in becomes available after installation credentials are present. Never deploy a developer's GitHub CLI token.

`GITHUB_WRITEBACK=true` activates only once an installation is configured. Writeback projects state into labels, a maintained summary comment and a readiness check. A transactional outbox retries failures; background drains handle one item per 30-second tick and prioritize recent actions. Initial imports also enter this outbox, so the rollout publishes the backlog gradually. Webhooks update individual items; reconciliation runs every 30 minutes and is disabled until an installation exists.

## Mirror upstream into a pilot fork

The import scripts use an authenticated local GitHub CLI. They do not transfer its credential to the service:

```sh
python3 scripts/mirror-prs.py --output /path/pilot-data --git-dir /path/mirror.git
python3 scripts/export-pilot.py --directory /path/pilot-data
DATA_PATH=/path/review.sqlite POLICY_PATH=/path/policy.json \
  target/release/zeron-review import --file /path/pilot-data/pilot-snapshot.json
```

The mirror script creates actual fork PRs for the current **open** upstream backlog, targeting a dedicated upstream-base snapshot. Original titles, code trees, authors and source links are preserved. Each head has an empty `[skip ci]` commit to avoid launching hundreds of builds; no source code is changed, history is not rewritten, and all pushes are normal pushes. Existing fork PRs are retained. A durable upstream-to-fork manifest prevents duplicate creation when retrying. Reruns add newly open PRs; they do not silently update previously imported code or close mirrors.

The exporter captures real fork checks/reviews and separately attributed upstream checks/reviews. Upstream context is labelled with its import timestamp; it remains a snapshot until refreshed with the exporter. The live App synchronizes the fork. The snapshots never invent approvals, validations or CI success. Closed upstream history is not recreated as hundreds of new open fork PRs.

## Deploy on Exnomic

Use a dedicated service account, `/opt/contribution-manager` release directory, `/var/lib/contribution-manager` data directory and `/etc/contribution-manager.env` (0600). The service listens only on `127.0.0.1:3080`. The included systemd unit limits memory/CPU and grants write access only to its data directory.

Exnomic's public TLS port uses an nginx stream SNI map for sites and VPNs. Add only `zeron.exnomic.com 127.0.0.1:8573;` to that map and install the dedicated site file. Its loopback TLS listener uses the existing wildcard certificate. Back up nginx.conf, run `nginx -t`, then gracefully reload. Never replace the entire Nginx configuration or restart unrelated services. See [DEPLOYMENT.md](DEPLOYMENT.md).

## Agent API

Agents use opaque bearer credentials mapped to accountable GitHub usernames via `AGENT_TOKENS`. They use the same authorization, independence, revision and idempotency checks as the website. Creating additional tokens does not create independent reviewers.

- `GET /api/state`: queue, verified principal/access, capabilities, policy, personal stars and sync health.
- `GET /api/items/:id`: full context and activity, where IDs are `pr:N`, `issue:N` or `report:N`.
- `POST /api/items/:id/:action`: exact `revision` plus action fields. Actions: `triage`, `direction`, `hotfix`, `claim`, `renew`, `release`, `review`, `validate`, `risk-approval`, `vote`.
- `POST /api/reports`: `type` (bug/feature), `title`, `details`, and a guest `name` when signed out.
- `POST /api/stars`: `id`, `starred`; signed-in browser requests require the CSRF token.

Signed PR commands retain `/review claim code-review`, `/review claim validate:PLATFORM`, `/review renew TASK`, and `/review release TASK`. Readiness follows evidence for the current revision, not a hand-set “verified” flag.

## Validation and limits

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
npm ci
npm run check
npm run format:check
npx playwright install chromium
npm run test:ui
```

The frontend and fonts are embedded in the binary, with gzip assets and ETags. Queue snapshots are cached. SQLite WAL runs on a dedicated database thread. Use one instance per database and back up using SQLite's backup API, or stop the service before copying its files. No PR code is executed by the controller.

Stubbed GitHub tests verify integration behavior and authorization; they do not prove a real owner has registered, installed or authorized the GitHub App. Live login/webhook/writeback validation requires completing that owner flow. The permission preview tests UI and sandbox rules, not actual GitHub permission levels.
