# Telemetry

Zeron sends anonymous product usage data so we can see which features are
used, which aren't, where turns fail, and how agents, models and remotes are
actually used. This page lists everything that is sent and how to turn it off.

## Turning it off

Any one of these stops collection. Anything recorded but not yet sent is
discarded, never sent later.

- **Settings → Privacy → Share anonymous usage data**
- `zeron telemetry disable`
- `ZERON_TELEMETRY=0` in the engine's environment (wins over the setting;
  any value other than `1`/`true`/`on`/`yes` means off)
- `DO_NOT_TRACK=1` ([consoledonottrack.com](https://consoledonottrack.com)),
  which wins over everything, including `ZERON_TELEMETRY=1`

A settings file that exists but can't be read counts as off.

Headless engines print a notice on stderr at every start until it is
acknowledged (`zeron telemetry ack`, or OK on the desktop notice). The
telemetry RPCs answer only local viewports: a remote device on the same
account can't read or change this machine's telemetry.

`zeron telemetry status` shows the current state and why.
`zeron telemetry events` prints the most recent events exactly as they are
sent; Settings → Privacy shows the same list.

Development builds and CI runs (`CI` set) record events locally (for that
list) but never upload unless `ZERON_TELEMETRY=1` or
`ZERON_TELEMETRY_ENDPOINT` is set.

## What is never sent

Prompts, responses, reasoning, tool input or output, file contents, file or
folder paths, repository, branch, project, chat or device names, URLs,
titles, error messages, credentials, emails, account ids, IP-derived person
profiles (`$process_person_profile` is always false).

Enforcement is structural, not by convention:

- Engine events are built only from typed fields (enums, counts, durations,
  token numbers). Text is only ever measured (byte counts) or classified
  (error kind), never copied.
- Events from viewports are sanitized by the engine: names must be short
  identifiers, property values must be booleans, numbers or short
  identifier-like tokens. Anything with a space, slash, `:` or `@` is dropped.
- Model ids that look like absolute paths, URLs, emails or free text become
  `custom` (slash-separated ids such as `openrouter/org/model` are kept —
  that's how providers name models). Tool names are reported only from a
  fixed list of built-in tools; MCP tools are `mcp` and anything else is
  `other`. Error messages become a coarse kind (`rate_limit`, `auth`,
  `network`, `context_overflow`, …). RPC method names that aren't plain
  identifiers roll up as `unknown`.
- `crates/engine/tests/telemetry.rs` runs a turn full of secrets through the
  real engine and asserts none of them appears in anything recorded.

## Identity

- `distinct_id`: salted SHA-256 (truncated to 128 bits) of a random
  per-install UUID (`~/.zeron/telemetry/install-id`) — or, when signed in, of
  the Zeron account id, so one person's devices count as one user. The raw
  ids never leave the device.
- `install_hash`, `device_hash`: same scheme, for install and device counts.
- `app_session`: random per process.

## Event catalog

Every event carries: `app_version`, `os`, `arch`, `build`
(`release`/`debug`), `run_mode` (`headed`/`headless`), `workspace_scope`
(`local`/`synced`/`development`), `install_hash`, `device_hash`,
`app_session`, `account_linked`.

| Event | When | Properties |
| --- | --- | --- |
| `app.started` | engine process start | `first_run`, `usage_import` |
| `app.stopped` | engine process exit | `uptime_secs` |
| `app.heartbeat` | ≤ once a day | installed / enabled harnesses; chat counts (total, active 7d, archived, side, hosted here, worktree); active chats by harness, model, sandbox, reasoning; spaces (total, here, git); devices (total, seen 7d, by platform); `auto_update` |
| `turn.started` | a prompt reaches an agent | `harness`, `model`, `reasoning`, `sandbox`, `auto_approve`, `attachments`, `worktree`, `resumed`, `zeron_mcp`, `prompt_size` (bucket), `model_options` (ids only) |
| `turn.finished` | the turn ends | everything in `turn.started` plus `outcome` (`completed`/`interrupted`/`errored`), `duration_ms`, `ttft_ms`, `tool_calls`, `tools` (counts by kind), `tool_errors`, `subagents_spawned`, `subagent_models`, `steers`, `input_requests`, `error_events`, `error_kind`, `generated_images`, `output_bytes`, `reasoning_bytes`, `context_tokens`, `context_window`, `context_pct`, `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `reasoning_tokens`, `cost_usd`, `total_tokens`, `subagent_*_tokens`, `usage_reported`, `usage_complete` |
| `chat.created` | new chat | `harness`, `model`, `side`, `in_space`, `custom_cwd`, `remote` |
| `space.created` | new space | `git`, `remote` |
| `repo.added` | add / clone / create repo | `via` |
| `worktree.created`, `worktree.removed`, `terminal.opened`, `project_action.run`, `local_import.finished` | the action | `remote` |
| `upload.committed` | attachment uploaded | `remote` |
| `harness.installed`, `harness.toggled`, `harness.updated`, `agent_account.activated` | the action | `harness`, `enabled` |
| `source_control.action` | PR comment, discard, fetch, switch ref | `action` |
| `auth.changed` | sign-in state changes | `state` |
| `remote.relay` | a relay session that served remote clients ends | `session_secs`, `clients`, `failed` |
| `rollup` | hourly | `metric` (`rpc`, `rpc_relay`, `rpc_forwarded`, `relay_session`), `calls`, `errors`, `avg_ms`, `max_ms` per key (RPC method name; `Mutate` split by op) |
| `telemetry.settings_changed` | privacy settings changed (only while on) | `usage_import`, `notice_acknowledged` |
| `external_usage.daily` | opt-in, once per settled day | `day`, `source`, `model`, `origin`, `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`, `reasoningTokens`, `costUsd`, `requests`, `sessions` |
| `ui.*` | desktop app interactions | see below |

### Desktop app (`ui.*`)

Reported by the viewport over the `TrackEvents` RPC and sanitized by the
engine; every one carries `surface` (`desktop`, …). Keyboard shortcuts (the
action, never the keys), command palette activations, panel toggles,
settings pages viewed, settings changed (key and enum/bool value only),
a daily snapshot of appearance/behaviour settings, and the use of features
such as new session, model picker, side chats, PR detail, voice and
attachments.

## Token usage

`input_tokens` is always **uncached** input; cache reads and writes are
separate. A count the agent doesn't report is absent, never zero. Usage from
subagents is reported separately (`subagent_*`), and `usage_complete` marks
the turns that are safe for per-model comparisons (usage reported, no
subagents, no model reroute). Compare aggregate ratios by dividing summed
totals, not by averaging per-turn ratios.

A turn's usage covers every model request in it (Codex and OpenCode report
per request; Zeron sums them). Claude's cost is the per-turn delta of the
CLI's cumulative `total_cost_usd`, which includes background subagent spend
between turns; the first turn after `--resume` may include earlier spend.
`steers` counts prompts delivered into a turn already producing output.

| Agent | input | cache read | cache write | reasoning | cost |
| --- | --- | --- | --- | --- | --- |
| Claude Code | ✓ | ✓ | ✓ | ✓ (thinking) | ✓ |
| Codex | ✓ | ✓ | ✓ | ✓ | |
| OpenCode | ✓ | ✓ | ✓ | ✓ | ✓ |
| Pi | ✓ | ✓ | ✓ | when reported | ✓ |
| Cursor | ✓ | ✓ | ✓ | when reported | |
| ACP (Devin, Grok, …) | as reported¹ | when reported | when reported | when reported | |

¹ Some ACP adapters report input including cached tokens.

## Usage from other agent CLIs (opt-in)

Off by default. When on (Settings → Privacy, or `zeron telemetry import on`),
the engine reads the CLIs' own local logs every six hours:

- Claude Code: `~/.claude/projects/**/*.jsonl` (`$CLAUDE_CONFIG_DIR`)
- Codex: `~/.codex/sessions/**/*.jsonl` (`$CODEX_HOME`)
- Pi: `~/.pi/agent/sessions/**/*.jsonl`
- OpenCode: `~/.local/share/opencode/opencode.db` (read-only)

It reduces them to per-day, per-model token totals and sends one
`external_usage.daily` event per day/agent/model/origin once the day is two
days old (backfilling 30 days on first opt-in). `origin` is how the CLI says
it was launched (Claude `entrypoint` such as `cli` or `sdk-cli`, Codex
`originator`), which separates interactive use from SDK use such as Zeron's
own runs.

Scanning is incremental (byte cursors per file), parallel, and prefiltered
with `memchr` so only usage-bearing lines are parsed. `zeron telemetry usage
--scan` runs the same scan locally and prints the table without sending
anything.

## Local usage view

Zeron keeps per-day per-model totals of its own turns in
`~/.zeron/telemetry/zeron-usage.json` (90 days) for Settings → Privacy →
Usage and `zeron telemetry usage`. This file never leaves the device and is
kept regardless of the telemetry setting.

## Delivery

Events are queued in memory (never blocking the caller), batched (50 events
or 30 s), gzip-compressed and POSTed to PostHog's `/batch/` endpoint. Each
event carries the uuid it was minted with, so a retried batch PostHog already
stored deduplicates. Failed batches back off exponentially and are spooled to
`~/.zeron/telemetry/spool.jsonl` (bounded to 4 MB) for the next successful
send — unless collection has been turned off in the meantime, in which case
the spool is deleted.

Configuration: `ZERON_TELEMETRY_ENDPOINT` (self-hosted PostHog or a local
capture server), `ZERON_TELEMETRY_KEY` (project key),
`ZERON_TELEMETRY_DEBUG=1` (also append every event to
`~/.zeron/logs/telemetry.jsonl`).

## Code map

- `crates/telemetry` — events, sanitizers, identity, batching transport,
  rollups, turn tracker, external log scanner, local ledger.
- `crates/engine/src/telemetry.rs` — lifecycle, heartbeat, RPC rollups and
  product events, the telemetry RPC methods.
- `crates/engine/src/sessions.rs` — turn begin/observe/finish hooks.
- `crates/harness` — each driver reports cache/reasoning/cost on `Usage`.
- `crates/rpc/src/device_room.rs` — relay session reports.
- `crates/ui/src/telemetry.rs` — viewport events; Settings → Privacy.
- `apps/zeron/src/telemetry_cli.rs` — `zeron telemetry …`.
