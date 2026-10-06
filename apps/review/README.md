# Zeron review workspace

A standalone contributor website plus a GitHub App integration. GitHub owns code, PRs, native reviews, issue contents, and merge permissions. The workspace owns task claims, accepted scope, validation evidence, community signals, and an audit trail.

This is a pilot implementation. It does not merge PRs, grant repository access, automatically close old work, or execute contributor code. Merging its source does not deploy it or install the GitHub App.

## Run a demo

The backend is Rust with Axum and SQLite. The frontend is plain JavaScript and CSS, using Zeron’s own fonts, icon, and dark palette. There is no frontend framework, npm runtime, or separate static server. The small HTTP service is an independent Cargo workspace; it does not build the desktop app.

Requires a current stable Rust toolchain. Build once, then run the standalone binary:

```sh
cd apps/review
cargo build --release --locked
target/release/zeron-review --demo
```

Open http://localhost:3080. Choose a demo identity in the header. Sample scenarios exercise direction decisions, claims, review findings, platform validation, and the final review packet. All sample evidence is illustrative. SQLite persists the sandbox in `data/demo.sqlite`. To reset, stop the demo and remove its database and WAL files.

To show a read-only snapshot of actual upstream PRs/issues in the sandbox, use an authenticated GitHub CLI:

```sh
target/release/zeron-review import --demo --github
```

Restart the demo after importing. Imported checks are context, not a fresh integration attestation; they do not automatically make a PR ready. All subsequent sandbox mutations stay local. Demo mode disables webhooks and GitHub writes regardless of environment settings.

For a server demo:

```sh
PUBLIC_URL=https://review-demo.example.com docker compose up --build -d
```

Put an HTTPS reverse proxy in front of `127.0.0.1:3080`, forwarding normal requests to the service. Set `PUBLIC_URL` to the exact external origin; it is used for cookies, origin checks, callbacks, and PR links. Demo identities intentionally let visitors try roles in a shared sandbox; use a separate database for live operation.

## What a maintainer must configure

1. Review [WORKFLOW.md](WORKFLOW.md), choose the initial trusted people, and set their exact GitHub logins in `policy.json`. At least one maintainer and one required CI check are mandatory in live mode. Use the check names that actually apply to this repository. For changes needing no platform validation, triage explicitly with an empty platform list.
2. Register a GitHub App owned by the organization. Set the homepage to the website, the OAuth callback to `PUBLIC_URL/auth/callback`, and the webhook URL to `PUBLIC_URL/webhooks/github`.
3. Grant **Issues: read/write**, **Pull requests: read**, **Checks: read/write**, and **Contents: read**, with mandatory Metadata read. Subscribe to **Pull request**, **Pull request review**, **Issues**, **Issue comment**, **Check run**, and **Check suite** events. The app needs neither Contents write nor repository Administration permissions.
4. Install the App on this repository. Generate a private key and webhook secret, and retrieve the App ID, installation ID, client ID, and client secret. Put the key in `github-app.pem`; copy `.env.example` to `.env` and populate these values. Keep writeback disabled initially. Ensure the container's non-root `review` user (UID 10001) can read the mounted key, without making it publicly readable.
5. Run `docker compose -f compose.live.yaml up --build -d`. Live mode imports open work on startup and reconciles every 15 minutes. Public visitors can read; authenticated users can claim and express demand; policy grants control attestations and decisions. OAuth identifies users without retaining their GitHub access tokens.
6. Check imported data, sign-in, roles, and webhook deliveries. Set `GITHUB_WRITEBACK=true` and recreate the service when ready to publish `review:*` labels, a single maintained summary comment, and the `zeron-review/readiness` check. The initial writeback covers every imported item, so enabling it is an explicit rollout decision.
7. After a pilot, optionally require the App's readiness check in branch rules. Keep native required reviews and integration checks. Configure native stale-review handling and a merge queue or up-to-date branch requirement separately. A workspace approval is not a replacement for GitHub's required-review permissions.

The App's installation credentials synchronize the repository. Its user authorization flow identifies website contributors. These are separate authentication paths. See GitHub's [user-token flow](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app), [repository roles](https://docs.github.com/en/organizations/managing-user-access-to-your-organizations-repositories/managing-repository-roles/repository-roles-for-an-organization), and [branch protection](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches).

## Website and PR flow

The website has review, maintainer, personal work, roadmap, and contributor views. Each item links to GitHub. Its managed PR/issue comment links back to the item and shows remaining actions and active claims. Native GitHub approvals, changes requested, and dismissed reviews synchronize into the evidence history; only configured reviewers contribute to workspace readiness.

Contributors can claim tasks directly in an issue or PR comment:

```text
/review claim code-review
/review claim validate:windows
/review renew validate:windows
/review release validate:windows
/review claim reproduce
```

Commands must be the entire comment and use a required task (`reproduce` is for issues). They execute as the authenticated comment author from a signed webhook. Use the website for detailed triage, scope decisions, and validation records. The App does not parse arbitrary prose as an approval. Failed commands appear as failed webhook deliveries; use the website to see the conflict or permission reason and retry there.

## Agent API

The website and agents share the same domain rules. Configure opaque server-issued tokens in `AGENT_TOKENS`, mapping each token to its accountable GitHub login. Agents inherit that login's capabilities and independence restrictions; creating several agent tokens does not produce independent reviewers.

```sh
curl -H "Authorization: Bearer $REVIEW_AGENT_TOKEN" \
  https://review.example.com/api/state

curl -H "Authorization: Bearer $REVIEW_AGENT_TOKEN" \
  https://review.example.com/api/items/pr:123

curl -X POST -H "Authorization: Bearer $REVIEW_AGENT_TOKEN" \
  -H 'Content-Type: application/json' -H 'Idempotency-Key: review-task-123' \
  --data '{"revision":"CURRENT_HEAD_SHA","task":"code-review","hours":2}' \
  https://review.example.com/api/items/pr:123/claim
```

`GET /api/state` returns work items, blockers, claims, policy requirements, source freshness, and synchronization health. `GET /api/items/:id` adds event history. Filter the state by kind, area, state, or required platform in the client. IDs are `pr:NUMBER` or `issue:NUMBER`.

All mutations are `POST /api/items/:id/:action` with an exact `revision`:

| Action                      | Additional fields                                                                                                                                        |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `claim`, `renew`, `release` | `task`; optional `hours` bounded by policy                                                                                                               |
| `direction`                 | `verdict`: accepted/deferred/rejected/needs-decision; `reason`                                                                                           |
| `triage`                    | `area`, `risk`: low/medium/high, `priority`: urgent/normal/low, `platforms`; optional `blocker`; issues also require `plan`: now/next/later/needs-triage |
| `review`                    | `verdict`: pass/changes; `summary`                                                                                                                       |
| `validate`                  | `platform`, `verdict`: pass/fail, `environment`, `summary`; optional HTTPS `artifact` URL                                                                |
| `risk-approval`             | `reason`                                                                                                                                                 |
| `vote`                      | `dimension`: demand/urgency; `reason`                                                                                                                    |

Mutations return the updated item. Conflicting claims or stale revisions return 409. Missing capabilities or self-verification return 403. An optional idempotency key is bound to principal, route, and request body; reusing it with different data returns 409. Browser sessions use the CSRF token supplied by `/api/state`. Bearer credentials use the same authorization rules without browser CSRF tokens. There is no automatic MCP registration in this pilot; this HTTP API is the integration surface.

## Performance and deployment

The release binary embeds the complete frontend and fonts, with precompressed gzip variants and ETags. Public queue data is serialized once per committed database change; actor and CSRF fields are attached separately. Conditional queue requests return 304 when unchanged. SQLite runs on a dedicated worker thread, with WAL and prepared statements, so database work does not block asynchronous HTTP and GitHub requests.

For a host without Docker, copy `target/release/zeron-review` and `policy.json`, set environment variables, and run the binary from a directory writable for `data/`. Static assets need no separate files. Existing pilot SQLite databases are compatible; a Docker volume from the earlier Node prototype must be writable by the new service user (UID 10001). Environment variables are read from the process; the native binary does not automatically load `.env`. A Linux release must be built for the target architecture (Docker builds for its host). Place one instance behind an HTTPS reverse proxy with persistent storage.

See [PERFORMANCE.md](PERFORMANCE.md) for a reproducible loopback benchmark and its limits.

## Operation and limits

- SQLite WAL supports one small deployment with persistent storage. Deploy one service instance per database. Back up with SQLite's backup command/API rather than copying only the main file while it is running. Stop the service before a plain file backup, or include an appropriate consistent WAL-aware backup.
- Signed webhooks and delivery deduplication synchronize relevant items. A transactional outbox retries failed GitHub writes. Periodic reconciliation repairs missed events and observes closed/merged work. Worker failures are visible in the workspace.
- No title, comment, or PR code is executed. The readiness controller must remain separate from CI/test runners and their credentials. All browser-rendered source text is escaped; source links come from GitHub.
- CI must report each configured check with `success` on the current revision. Skipped, neutral, missing, failed, or pending checks do not count. When a previously observed base changes, unchanged checks become stale and must run again. On first import the App does not independently prove which base a historical build used; **native up-to-date/merge queue checks remain the integration authority**. The app's readiness check represents review evidence, not an unconditional merge guarantee.
- Native branch rules, final merge authority, reviewer nominations, and hosting credentials require owner decisions. This pilot does not automatically grant capabilities from ratings or activity.
- Pilot capability grants apply across the configured repository. Area-specific reviewer grants and nominations are future extensions. Restart after changing `policy.json`; a changed policy re-evaluates existing items and schedules updated GitHub readiness checks.
- The 30-day disposition target is visible, not an automatic close/merge action. Formal linked issue/PR relationships, escalation notifications, full review scheduling, and contributor nominations can follow the pilot.
- Authentication and GitHub App endpoints are implemented but must be exercised against a real installation before rollout. Local tests use isolated fixtures and stubbed GitHub calls; they do not claim a live installation has been verified.

## Validate changes

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

Node 24 or newer is only needed for development checks and browser tests. For an installed Chrome, use `REVIEW_BROWSER_CHANNEL=chrome npm run test:ui`. Browser tests launch an isolated profile and a dedicated test server/database. They use the release binary by default; set `REVIEW_TEST_BINARY=target/debug/zeron-review` after `cargo build --locked` for faster development iteration. GitHub CI runs domain, HTTP, integration-stub, and browser tests for changes to this app. Test artifacts stay under ignored `test-results/`.
