//! Opt-in import of agent usage that happened OUTSIDE Zeron, read from the
//! CLIs' own local logs, reduced to per-day per-model token totals.
//!
//! Only aggregates leave the device (`external_usage.daily`): day, source,
//! model, origin (how the CLI was launched), token counts, request and
//! session counts. Never paths, project names, session ids or content.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::{Event, names};

mod scan;

/// One day × source × model × origin aggregate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    /// UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// `claude-code` | `codex` | `pi` | `opencode`.
    pub source: String,
    pub model: String,
    /// How the session was launched, as the CLI records it (Claude
    /// `entrypoint`, Codex `originator`) — separates interactive CLI use from
    /// SDK/app-server use such as Zeron's own runs. `unknown` when absent.
    pub origin: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    /// Provider-reported cost when the log carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub requests: u64,
    pub sessions: u64,
}

impl DailyUsage {
    pub fn to_event(&self) -> Event {
        Event::with_payload(names::EXTERNAL_USAGE_DAILY, self)
    }
}

/// Where each CLI keeps its logs. [`Roots::from_home`] gives the defaults.
#[derive(Debug, Clone)]
pub struct Roots {
    pub claude_projects: Option<PathBuf>,
    pub codex_sessions: Option<PathBuf>,
    pub pi_sessions: Option<PathBuf>,
    pub opencode_db: Option<PathBuf>,
}

impl Roots {
    pub fn from_home(home: &Path) -> Self {
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let xdg_data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("share"));
        Roots {
            claude_projects: Some(claude.join("projects")),
            codex_sessions: Some(codex.join("sessions")),
            pi_sessions: Some(home.join(".pi").join("agent").join("sessions")),
            opencode_db: Some(xdg_data.join("opencode").join("opencode.db")),
        }
    }
}

/// Result of one incremental scan.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    /// Completed days (before today, UTC) not reported before — send these.
    pub unreported: Vec<DailyUsage>,
    /// Every aggregate in the retention window, for the local stats view.
    pub window: Vec<DailyUsage>,
    pub files_scanned: u64,
    pub bytes_read: u64,
}

/// Incremental scanner. State (per-file cursors, dedupe ids, aggregates,
/// reported days) persists at `<state_dir>/state.json`, so each scan reads
/// only bytes appended since the last one.
pub struct UsageScanner {
    state_dir: PathBuf,
    roots: Roots,
    state: Option<ScanState>,
}

/// Completed days are reported once they're this many days old, so a log
/// that lands late (a laptop asleep at midnight) still makes it in.
const SETTLE_DAYS: i64 = 2;
/// How far back the first scan after opting in reports.
const BACKFILL_DAYS: i64 = 30;
const STATE_VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ScanState {
    version: u32,
    files: std::collections::HashMap<String, scan::FileCursor>,
    opencode_after_ms: i64,
    aggregates: Vec<DailyUsage>,
    /// Distinct session hashes per aggregate key (for `sessions`).
    sessions: std::collections::HashMap<String, Vec<u64>>,
    /// Dedupe id → day it was seen (pruned with the window).
    seen: std::collections::HashMap<u64, String>,
    /// Aggregate keys (day|source|model|origin) already sent, so a model
    /// first seen late for an already-reported day still goes out.
    reported: std::collections::BTreeSet<String>,
}

impl UsageScanner {
    pub fn new(state_dir: PathBuf, roots: Roots) -> Self {
        UsageScanner {
            state_dir,
            roots,
            state: None,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    fn load(&mut self) -> &mut ScanState {
        if self.state.is_none() {
            let state = std::fs::read(self.state_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ScanState>(&bytes).ok())
                .filter(|state| state.version == STATE_VERSION)
                .unwrap_or_else(|| ScanState {
                    version: STATE_VERSION,
                    ..ScanState::default()
                });
            self.state = Some(state);
        }
        self.state.as_mut().expect("state loaded above")
    }

    fn save(&self) {
        let Some(state) = &self.state else {
            return;
        };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&self.state_dir)?;
            let tmp = self
                .state_dir
                .join(format!("state.json.{}.tmp", std::process::id()));
            std::fs::write(&tmp, serde_json::to_vec(state)?)?;
            std::fs::rename(&tmp, self.state_path())
        };
        if let Err(err) = write() {
            tracing::debug!(%err, "telemetry: usage scan state not saved");
        }
    }

    /// Scans new log data. Blocking file IO — call from `spawn_blocking`.
    pub fn scan(&mut self) -> ScanReport {
        self.scan_at(chrono::Utc::now())
    }

    fn scan_at(&mut self, now: chrono::DateTime<chrono::Utc>) -> ScanReport {
        let day = |offset: i64| {
            (now - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string()
        };
        let horizon = day(RETENTION_DAYS);
        let roots = self.roots.clone();

        let mut paths: Vec<(PathBuf, scan::Kind)> = Vec::new();
        let mut collect = |root: &Option<PathBuf>, depth: usize, kind: scan::Kind| {
            if let Some(root) = root {
                let mut found = Vec::new();
                scan::jsonl_files(root, depth, &mut found);
                paths.extend(found.into_iter().map(|p| (p, kind)));
            }
        };
        collect(&roots.claude_projects, 4, scan::Kind::Claude);
        collect(&roots.codex_sessions, 4, scan::Kind::Codex);
        collect(&roots.pi_sessions, 2, scan::Kind::Pi);

        let state = self.load();
        let horizon_time =
            std::time::SystemTime::from(now - chrono::Duration::days(RETENTION_DAYS));
        let jobs: Vec<_> = paths
            .into_iter()
            .filter_map(|(path, kind)| {
                let meta = std::fs::metadata(&path).ok()?;
                // Untouched since before the window: nothing in it can count.
                if meta.modified().is_ok_and(|m| m < horizon_time) {
                    return None;
                }
                let key = path.to_string_lossy().into_owned();
                let cursor = state.files.get(&key).cloned().unwrap_or_default();
                (meta.len() != cursor.offset).then_some((path, kind, cursor))
            })
            .collect();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .min(4);
        let scans = scan::scan_parallel(jobs, &horizon, threads);

        let mut report = ScanReport::default();
        let mut records = Vec::new();
        for file in scans {
            report.files_scanned += 1;
            report.bytes_read += file.bytes;
            state
                .files
                .insert(file.path.to_string_lossy().into_owned(), file.cursor);
            records.extend(file.records);
        }
        if let Some(db) = roots.opencode_db.as_deref().filter(|db| db.exists()) {
            let (found, cursor) = scan::scan_opencode(db, state.opencode_after_ms, &horizon);
            state.opencode_after_ms = cursor;
            records.extend(found);
        }

        let mut index: std::collections::HashMap<String, usize> = state
            .aggregates
            .iter()
            .enumerate()
            .map(|(i, d)| (agg_key(d), i))
            .collect();
        for record in records {
            if let Some(id) = record.dedupe
                && state.seen.insert(id, record.day.clone()).is_some()
            {
                continue;
            }
            let key = scan::aggregate_key(&record);
            let i = *index.entry(key.clone()).or_insert_with(|| {
                state.aggregates.push(DailyUsage {
                    day: record.day.clone(),
                    source: record.source.to_string(),
                    model: record.model.clone(),
                    origin: record.origin.clone(),
                    ..DailyUsage::default()
                });
                state.aggregates.len() - 1
            });
            let row = &mut state.aggregates[i];
            row.requests += 1;
            row.input_tokens += record.input;
            row.output_tokens += record.output;
            row.cache_read_tokens += record.cache_read;
            row.cache_write_tokens += record.cache_write;
            row.reasoning_tokens += record.reasoning;
            if let Some(cost) = record.cost_usd {
                *row.cost_usd.get_or_insert(0.0) += cost;
            }
            let sessions = state.sessions.entry(key).or_default();
            if !sessions.contains(&record.session) {
                sessions.push(record.session);
                row.sessions = sessions.len() as u64;
            }
        }

        // Prune everything outside the window.
        state.aggregates.retain(|d| d.day >= horizon);
        let live: std::collections::HashSet<String> =
            state.aggregates.iter().map(agg_key).collect();
        state.sessions.retain(|k, _| live.contains(k));
        state.seen.retain(|_, d| *d >= horizon);
        state.reported.retain(|k| live.contains(k));

        let settled = day(SETTLE_DAYS - 1);
        let backfill = day(BACKFILL_DAYS);
        report.unreported = state
            .aggregates
            .iter()
            .filter(|d| {
                d.day < settled && d.day >= backfill && !state.reported.contains(&agg_key(d))
            })
            .cloned()
            .collect();
        report
            .unreported
            .sort_by(|a, b| (&a.day, &a.source, &a.model).cmp(&(&b.day, &b.source, &b.model)));
        report.window = state.aggregates.clone();
        report
            .window
            .sort_by(|a, b| (&b.day, &a.source, &a.model).cmp(&(&a.day, &b.source, &b.model)));
        self.save();
        report
    }

    /// Marks the given aggregates reported so they're never sent twice.
    pub fn mark_reported(&mut self, days: &[DailyUsage]) {
        let state = self.load();
        for d in days {
            state.reported.insert(agg_key(d));
        }
        self.save();
    }
}

fn agg_key(d: &DailyUsage) -> String {
    format!("{}|{}|{}|{}", d.day, d.source, d.model, d.origin)
}

/// What the "Usage" view shows. Local only — this never leaves the device
/// except as the aggregate `external_usage.daily` / `turn.finished` events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    pub import_enabled: bool,
    /// Out-of-app usage (empty unless the import is enabled and has run).
    pub external: Vec<DailyUsage>,
    /// Zeron's own turns, `source` = harness id, `origin` = `zeron`.
    pub zeron: Vec<DailyUsage>,
    /// RFC 3339 time of the last completed external scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scan_at: Option<String>,
}

/// Days of aggregates kept locally (and the backfill horizon on opt-in).
pub const RETENTION_DAYS: i64 = 90;

/// Per-day per-model token totals of Zeron's own turns, persisted at
/// `<state_dir>/zeron-usage.json`. Fed from `turn.finished` properties so
/// it counts exactly what telemetry reports — but it is kept regardless of
/// the telemetry setting, because it never leaves the device.
pub struct ZeronLedger {
    path: Option<PathBuf>,
    days: Vec<DailyUsage>,
}

impl ZeronLedger {
    pub fn open(state_dir: Option<&Path>) -> Self {
        let path = state_dir.map(|dir| dir.join("zeron-usage.json"));
        let days = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        ZeronLedger { path, days }
    }

    /// Folds one `turn.finished` event in. Turns without reported usage
    /// still count as requests; their token columns stay untouched.
    pub fn record_turn(&mut self, turn: &Event) {
        let p = turn.props();
        let get = |k: &str| p.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        let source = p
            .get("harness")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let model = p
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let idx = match self
            .days
            .iter()
            .position(|d| d.day == day && d.source == source && d.model == model)
        {
            Some(idx) => idx,
            None => {
                self.days.push(DailyUsage {
                    day,
                    source,
                    model,
                    origin: "zeron".into(),
                    ..DailyUsage::default()
                });
                self.days.len() - 1
            }
        };
        let row = &mut self.days[idx];
        row.requests += 1;
        row.input_tokens += get("input_tokens");
        row.output_tokens += get("output_tokens");
        row.cache_read_tokens += get("cache_read_tokens");
        row.cache_write_tokens += get("cache_write_tokens");
        row.reasoning_tokens += get("reasoning_tokens");
        if let Some(cost) = p.get("cost_usd").and_then(serde_json::Value::as_f64) {
            *row.cost_usd.get_or_insert(0.0) += cost;
        }
        self.prune();
        self.save();
    }

    pub fn days(&self) -> Vec<DailyUsage> {
        self.days.clone()
    }

    fn prune(&mut self) {
        let horizon = (chrono::Utc::now() - chrono::Duration::days(RETENTION_DAYS))
            .format("%Y-%m-%d")
            .to_string();
        self.days.retain(|d| d.day >= horizon);
    }

    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
            std::fs::write(&tmp, serde_json::to_vec(&self.days)?)?;
            std::fs::rename(&tmp, path)
        };
        if let Err(err) = write() {
            tracing::debug!(%err, "telemetry: usage ledger not saved");
        }
    }
}

#[cfg(test)]
mod ledger_tests {
    use super::*;

    #[test]
    fn ledger_aggregates_turns_per_day_and_model_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = ZeronLedger::open(Some(dir.path()));
        let turn = Event::new(names::TURN_FINISHED)
            .with("harness", "codex")
            .with("model", "gpt-5")
            .with("input_tokens", 10u64)
            .with("output_tokens", 5u64)
            .with("cache_read_tokens", 100u64)
            .with("cost_usd", 0.5);
        ledger.record_turn(&turn);
        ledger.record_turn(&turn);
        ledger.record_turn(&Event::new(names::TURN_FINISHED).with("harness", "codex"));
        let reopened = ZeronLedger::open(Some(dir.path())).days();
        assert_eq!(reopened.len(), 2);
        let gpt = reopened.iter().find(|d| d.model == "gpt-5").unwrap();
        assert_eq!(gpt.requests, 2);
        assert_eq!(gpt.input_tokens, 20);
        assert_eq!(gpt.cache_read_tokens, 200);
        assert_eq!(gpt.cost_usd, Some(1.0));
        assert_eq!(gpt.origin, "zeron");
    }
}

#[cfg(test)]
mod scanner_tests {
    use super::*;
    use std::io::Write as _;

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-10-10T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn append(path: &Path, lines: &[serde_json::Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    fn claude_line(id: &str, req: &str, ts: &str, input: u64) -> serde_json::Value {
        serde_json::json!({
            "type": "assistant", "timestamp": ts, "requestId": req,
            "sessionId": "s-1", "entrypoint": "cli", "cwd": "/home/alice/secret",
            "message": {
                "id": id, "model": "claude-opus-4-1",
                "content": [{"type": "text", "text": "SECRET CONTENT with \"usage\" inside"}],
                "usage": {
                    "input_tokens": input, "output_tokens": 10,
                    "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 50,
                    "output_tokens_details": {"thinking_tokens": 4}
                }
            }
        })
    }

    fn roots(home: &Path) -> Roots {
        Roots {
            claude_projects: Some(home.join(".claude/projects")),
            codex_sessions: Some(home.join(".codex/sessions")),
            pi_sessions: Some(home.join(".pi/agent/sessions")),
            opencode_db: Some(home.join("opencode.db")),
        }
    }

    #[test]
    fn scans_all_sources_incrementally_with_dedupe() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let claude = home
            .path()
            .join(".claude/projects/-home-alice-secret/s-1.jsonl");
        append(
            &claude,
            &[
                claude_line("msg_1", "req_1", "2026-10-07T10:00:00Z", 100),
                // Same response, second content block: must not double count.
                claude_line("msg_1", "req_1", "2026-10-07T10:00:01Z", 100),
                claude_line("msg_2", "req_2", "2026-10-07T11:00:00Z", 7),
                serde_json::json!({"type": "user", "message": {"content": "hi"}}),
            ],
        );
        // Subagent transcripts live one level deeper.
        append(
            &home
                .path()
                .join(".claude/projects/-p/s-1/subagents/agent-1.jsonl"),
            &[claude_line("msg_3", "req_3", "2026-10-07T12:00:00Z", 1)],
        );
        let codex = home.path().join(".codex/sessions/2026/10/07/rollout.jsonl");
        let count = |total: u64, input: u64| {
            serde_json::json!({"timestamp": "2026-10-07T09:00:00Z", "type": "event_msg", "payload": {
                "type": "token_count", "info": {
                    "total_token_usage": {"total_tokens": total},
                    "last_token_usage": {"input_tokens": input, "cached_input_tokens": 40,
                        "output_tokens": 5, "reasoning_output_tokens": 2}
                }
            }})
        };
        append(
            &codex,
            &[
                serde_json::json!({"type": "session_meta", "payload": {"id": "zz-session-77", "originator": "codex_cli_rs", "cwd": "/secret"}}),
                serde_json::json!({"type": "turn_context", "payload": {"model": "gpt-5.1-codex"}}),
                count(100, 60),
                count(100, 60), // re-emitted unchanged: skipped
                count(180, 70),
            ],
        );
        append(
            &home.path().join(".pi/agent/sessions/slug/a.jsonl"),
            &[
                serde_json::json!({"type": "message", "timestamp": "2026-10-07T08:00:00Z", "message": {
                    "role": "assistant", "model": "glm-4.6", "responseId": "r1",
                    "usage": {"input": 3, "output": 4, "cacheRead": 5, "cacheWrite": 6, "cost": {"total": 0.25}}
                }}),
            ],
        );
        let db = rusqlite::Connection::open(home.path().join("opencode.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);",
        )
        .unwrap();
        let completed = chrono::DateTime::parse_from_rfc3339("2026-10-07T07:00:00Z")
            .unwrap()
            .timestamp_millis();
        db.execute(
            "INSERT INTO message VALUES ('m1', 's', 1, 2, ?1), ('m2', 's', 1, 3, ?2)",
            [
                serde_json::json!({"role": "assistant", "modelID": "kimi-k2", "cost": 0.5,
                    "time": {"completed": completed},
                    "tokens": {"input": 9, "output": 8, "reasoning": 1, "cache": {"read": 2, "write": 3}}})
                .to_string(),
                serde_json::json!({"role": "assistant", "modelID": "kimi-k2", "time": {}}).to_string(),
            ],
        )
        .unwrap();
        drop(db);

        let mut scanner = UsageScanner::new(state.path().to_path_buf(), roots(home.path()));
        let report = scanner.scan_at(now());
        let find = |source: &str| {
            report
                .window
                .iter()
                .find(|d| d.source == source)
                .cloned()
                .unwrap()
        };

        let claude_day = find("claude-code");
        assert_eq!(claude_day.day, "2026-10-07");
        assert_eq!(claude_day.model, "claude-opus-4-1");
        assert_eq!(claude_day.origin, "cli");
        assert_eq!(claude_day.requests, 3, "duplicate content block deduped");
        assert_eq!(claude_day.input_tokens, 108);
        assert_eq!(claude_day.cache_read_tokens, 3000);
        assert_eq!(claude_day.reasoning_tokens, 12);
        assert_eq!(claude_day.sessions, 1);

        let codex_day = find("codex");
        assert_eq!(codex_day.model, "gpt-5.1-codex");
        assert_eq!(codex_day.origin, "codex_cli_rs");
        assert_eq!(codex_day.requests, 2, "unchanged re-emit skipped");
        assert_eq!(codex_day.input_tokens, 20 + 30, "cached input split out");
        assert_eq!(codex_day.cache_read_tokens, 80);

        let pi_day = find("pi");
        assert_eq!(pi_day.cost_usd, Some(0.25));
        let oc = find("opencode");
        assert_eq!(oc.requests, 1, "in-flight message not counted yet");
        assert_eq!(oc.cache_write_tokens, 3);

        // Settled days are reported once.
        assert_eq!(report.unreported.len(), report.window.len());
        scanner.mark_reported(&report.unreported);

        // Appends are picked up incrementally; old bytes aren't re-read.
        append(
            &claude,
            &[claude_line("msg_9", "req_9", "2026-10-07T13:00:00Z", 1)],
        );
        let mut fresh = UsageScanner::new(state.path().to_path_buf(), roots(home.path()));
        let second = fresh.scan_at(now());
        assert_eq!(second.files_scanned, 1);
        assert!(
            second.unreported.is_empty(),
            "already reported day not resent"
        );

        // A model first seen late for an already-reported day still goes out.
        let late = claude_line("msg_10", "req_10", "2026-10-07T14:00:00Z", 1)
            .to_string()
            .replace("claude-opus-4-1", "claude-haiku-4-5");
        append(&claude, &[serde_json::from_str(&late).unwrap()]);
        let third = fresh.scan_at(now());
        assert_eq!(third.unreported.len(), 1);
        assert_eq!(third.unreported[0].model, "claude-haiku-4-5");
        fresh.mark_reported(&third.unreported);
        let claude_day = second
            .window
            .iter()
            .find(|d| d.source == "claude-code")
            .unwrap();
        assert_eq!(claude_day.requests, 4);

        let serialized = serde_json::to_string(&second.window).unwrap();
        for secret in ["SECRET", "alice", "/secret", "zz-session-77", "req_1"] {
            assert!(!serialized.contains(secret), "{secret} leaked");
        }
    }

    #[test]
    fn recent_days_wait_to_settle() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        append(
            &home.path().join(".claude/projects/p/s.jsonl"),
            &[
                claude_line("a", "a", "2026-10-10T01:00:00Z", 1),
                claude_line("b", "b", "2026-10-09T01:00:00Z", 1),
                claude_line("c", "c", "2026-10-08T01:00:00Z", 1),
            ],
        );
        let mut scanner = UsageScanner::new(state.path().to_path_buf(), roots(home.path()));
        let report = scanner.scan_at(now());
        assert_eq!(report.window.len(), 3);
        let days: Vec<_> = report.unreported.iter().map(|d| d.day.as_str()).collect();
        assert_eq!(days, ["2026-10-08"]);
    }

    #[test]
    fn partial_trailing_line_waits_for_the_writer() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude/projects/p/s.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let line = claude_line("a", "a", "2026-10-07T01:00:00Z", 1).to_string();
        std::fs::write(&path, &line[..line.len() / 2]).unwrap();
        let mut scanner = UsageScanner::new(state.path().to_path_buf(), roots(home.path()));
        assert!(scanner.scan_at(now()).window.is_empty());
        std::fs::write(&path, format!("{line}\n")).unwrap();
        assert_eq!(scanner.scan_at(now()).window[0].requests, 1);
    }
}
