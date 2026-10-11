//! Opt-in import of agent usage that happened OUTSIDE Zeron, read from the
//! CLIs' own local logs, reduced to per-day per-model token totals.
//!
//! Only aggregates leave the device (`external_usage.daily`): day, source,
//! model, origin (how the CLI was launched), token counts, request and
//! session counts. Never paths, project names, session ids or content.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::{Event, names};

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
/// reported days) persists under `state_dir`, so each scan reads only bytes
/// appended since the last one.
pub struct UsageScanner {
    #[allow(dead_code)]
    state_dir: PathBuf,
    #[allow(dead_code)]
    roots: Roots,
}

impl UsageScanner {
    pub fn new(state_dir: PathBuf, roots: Roots) -> Self {
        UsageScanner { state_dir, roots }
    }

    /// Scans new log data. Blocking file IO — call from `spawn_blocking`.
    pub fn scan(&mut self) -> ScanReport {
        ScanReport::default()
    }

    /// Marks the given days reported so they're never sent twice.
    pub fn mark_reported(&mut self, _days: &[DailyUsage]) {}
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
