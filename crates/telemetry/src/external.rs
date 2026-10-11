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
