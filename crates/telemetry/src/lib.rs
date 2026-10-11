//! zeron-telemetry — anonymous product analytics, owned by the engine.
//!
//! Shape (see `docs/telemetry.md` for the event catalog and data policy):
//!
//! * [`Telemetry`] is a cheap `Clone` handle. [`Telemetry::capture`] stamps
//!   an [`Event`] with identity, timestamp and uuid and `try_send`s it to a
//!   bounded channel — it never blocks, never awaits, never touches disk.
//! * One background task ([`transport`]) batches, gzips and POSTs to
//!   PostHog's `/batch/`, retries with backoff, and spools to disk offline.
//! * High-frequency signals (every RPC) aggregate in-process
//!   ([`Telemetry::record`]) and leave as one hourly `rollup` event.
//! * Turn analytics come from [`turn::TurnTracker`], fed the same
//!   `AgentEvent` stream the engine journals.
//! * Out-of-app agent usage (Claude Code / Codex / Pi / OpenCode logs) is
//!   aggregated per day by [`external`], only when the user opts in.
//!
//! Collection is on by default with a visible notice and is switched off by
//! the in-app toggle, `zeron telemetry disable`, `ZERON_TELEMETRY=0`, or the
//! `DO_NOT_TRACK=1` convention. Debug builds collect locally (for the
//! in-app "recent events" view) but never upload unless explicitly asked.

pub mod event;
pub mod external;
mod identity;
mod rollup;
mod transport;
pub mod turn;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};

pub use event::{Event, bucket, classify_error, names};
pub use identity::hash;

use identity::Identity;
use rollup::Rollup;
use transport::{Command, Envelope, WorkerConfig};

/// PostHog US ingestion. Override with `ZERON_TELEMETRY_ENDPOINT` (e.g. a
/// self-hosted instance or a local capture server for testing).
pub const DEFAULT_ENDPOINT: &str = "https://us.i.posthog.com";
/// Public, write-only project key (the same project as zeron.sh's download
/// tracking). Override with `ZERON_TELEMETRY_KEY`.
pub const DEFAULT_API_KEY: &str = "phc_yAQTUdUM9vQMcHpGmMcNJom6vfrN3r3eo5avDSnt8S9R";

const CHANNEL_CAPACITY: usize = 4096;
const SETTINGS_FILE: &str = "settings.json";

/// How the process hosting the engine was launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Headed,
    Headless,
    Cli,
}

/// What the engine knows at startup.
#[derive(Debug, Clone)]
pub struct TelemetryOptions {
    /// The device root (`~/.zeron`); state lives under `telemetry/`.
    pub data_dir: PathBuf,
    pub run_mode: RunMode,
    /// `local` | `synced` | `development`.
    pub workspace_scope: String,
    /// Hashed before it leaves the device; lets "devices per user" be counted.
    pub device_id: String,
    pub app_version: String,
}

/// User-controlled, persisted at `<data_dir>/telemetry/settings.json` and
/// shared by every viewport attached to this engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelemetrySettings {
    /// Anonymous product usage events.
    pub enabled: bool,
    /// Daily per-model token totals from other agent CLIs' local logs.
    pub usage_import: bool,
    /// The first-run notice was seen (any viewport); hides the banner.
    pub notice_acknowledged: bool,
}

impl Default for TelemetrySettings {
    fn default() -> Self {
        TelemetrySettings {
            enabled: true,
            usage_import: false,
            notice_acknowledged: false,
        }
    }
}

/// A partial settings update (RPC `SetTelemetrySettings`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelemetrySettingsPatch {
    pub enabled: Option<bool>,
    pub usage_import: Option<bool>,
    pub notice_acknowledged: Option<bool>,
}

/// Everything a viewport needs to render the privacy page truthfully.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryStatus {
    pub settings: TelemetrySettings,
    /// Events are being recorded (settings + environment allow it).
    pub collecting: bool,
    /// Recorded events are being uploaded (release build or explicit opt-in).
    pub uploading: bool,
    /// Set when the environment pins collection (`ZERON_TELEMETRY`,
    /// `DO_NOT_TRACK`) or a debug build suppresses upload.
    pub override_reason: Option<String>,
    pub distinct_id: String,
    pub account_linked: bool,
    pub endpoint: Option<String>,
    pub sent: u64,
    pub dropped: u64,
    pub last_error: Option<String>,
    /// The most recent events exactly as serialized for upload, newest last.
    pub recent: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct Stats {
    sent: AtomicU64,
    dropped: AtomicU64,
    last_error: Mutex<Option<String>>,
}

pub(crate) struct Inner {
    collecting: AtomicBool,
    env_override: Option<EnvOverride>,
    upload_endpoint: Option<String>,
    settings: Mutex<TelemetrySettings>,
    state_dir: Option<PathBuf>,
    identity: RwLock<Identity>,
    tx: Option<mpsc::Sender<Command>>,
    rollup: Rollup,
    stats: Stats,
    recent: Mutex<VecDeque<Value>>,
    common: Map<String, Value>,
    started: Instant,
}

impl Inner {
    fn envelope(&self, event: Event) -> Envelope {
        let identity = self
            .identity
            .read()
            .map(|id| id.clone())
            .unwrap_or_else(|_| Identity::new("poisoned"));
        let mut props = event.props;
        props.insert("account_linked".into(), identity.account_linked.into());
        Envelope {
            name: event.name,
            props,
            distinct_id: identity.distinct,
            timestamp: chrono::Utc::now(),
            uuid: uuid::Uuid::new_v4(),
        }
    }
}

/// Cheap, cloneable handle. All methods are safe to call from any thread and
/// are no-ops once collection is off.
#[derive(Clone)]
pub struct Telemetry {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Telemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Telemetry")
            .field("collecting", &self.is_collecting())
            .finish_non_exhaustive()
    }
}

/// Knobs for tests and embedders; production uses [`Telemetry::start`].
#[derive(Debug, Clone)]
pub struct Tuning {
    pub endpoint: Option<String>,
    pub api_key: String,
    pub batch_size: usize,
    pub flush_every: Duration,
    pub rollup_every: Duration,
    pub debug_log: bool,
}

impl Tuning {
    fn from_env() -> (Self, Option<EnvOverride>) {
        let env_override = EnvOverride::detect();
        let explicit_endpoint = env_nonempty("ZERON_TELEMETRY_ENDPOINT");
        let forced_on = matches!(env_override, Some(EnvOverride::ForcedOn));
        // Dev builds never pollute production analytics by accident.
        let endpoint = explicit_endpoint.or_else(|| {
            (!cfg!(debug_assertions) || forced_on).then(|| DEFAULT_ENDPOINT.to_string())
        });
        let tuning = Tuning {
            endpoint,
            api_key: env_nonempty("ZERON_TELEMETRY_KEY")
                .unwrap_or_else(|| DEFAULT_API_KEY.to_string()),
            batch_size: 50,
            flush_every: Duration::from_secs(30),
            rollup_every: Duration::from_secs(60 * 60),
            debug_log: env_truthy("ZERON_TELEMETRY_DEBUG"),
        };
        (tuning, env_override)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvOverride {
    /// `ZERON_TELEMETRY=0|false|off` or `DO_NOT_TRACK=1`.
    ForcedOff(&'static str),
    /// `ZERON_TELEMETRY=1|true|on`: also enables upload from debug builds.
    ForcedOn,
}

impl EnvOverride {
    fn detect() -> Option<Self> {
        if let Some(v) = env_nonempty("ZERON_TELEMETRY") {
            match v.to_ascii_lowercase().as_str() {
                "0" | "false" | "off" | "no" | "disabled" => {
                    return Some(EnvOverride::ForcedOff("ZERON_TELEMETRY"));
                }
                "1" | "true" | "on" | "yes" | "enabled" => return Some(EnvOverride::ForcedOn),
                _ => {}
            }
        }
        if env_truthy("DO_NOT_TRACK") {
            return Some(EnvOverride::ForcedOff("DO_NOT_TRACK"));
        }
        None
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_truthy(key: &str) -> bool {
    env_nonempty(key)
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

impl Telemetry {
    /// A handle that records nothing. Used by tests and by code paths that
    /// run before (or without) an engine.
    pub fn disabled() -> Self {
        Telemetry {
            inner: Arc::new(Inner {
                collecting: AtomicBool::new(false),
                env_override: None,
                upload_endpoint: None,
                settings: Mutex::new(TelemetrySettings {
                    enabled: false,
                    ..TelemetrySettings::default()
                }),
                state_dir: None,
                identity: RwLock::new(Identity::new("disabled")),
                tx: None,
                rollup: Rollup::new(),
                stats: Stats::default(),
                recent: Mutex::new(VecDeque::new()),
                common: Map::new(),
                started: Instant::now(),
            }),
        }
    }

    /// Starts telemetry for an engine. Must be called inside a tokio runtime;
    /// outside one it degrades to [`Telemetry::disabled`].
    pub fn start(opts: TelemetryOptions) -> Self {
        let (tuning, env_override) = Tuning::from_env();
        Self::start_inner(opts, tuning, env_override)
    }

    /// [`Telemetry::start`] with explicit tuning (tests, local capture).
    /// `forced_off` mimics `ZERON_TELEMETRY=0`.
    pub fn start_with(
        opts: TelemetryOptions,
        tuning: Tuning,
        forced_off: Option<&'static str>,
    ) -> Self {
        Self::start_inner(opts, tuning, forced_off.map(EnvOverride::ForcedOff))
    }

    fn start_inner(
        opts: TelemetryOptions,
        tuning: Tuning,
        env_override: Option<EnvOverride>,
    ) -> Self {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Self::disabled();
        };
        let state_dir = opts.data_dir.join("telemetry");
        let settings = load_settings(&state_dir);
        let install_id = identity::load_or_create_install_id(&state_dir);
        let identity = Identity::new(&install_id);
        let collecting =
            settings.enabled && !matches!(env_override, Some(EnvOverride::ForcedOff(_)));

        let mut common = Map::new();
        common.insert("$lib".into(), "zeron".into());
        common.insert("$lib_version".into(), opts.app_version.clone().into());
        common.insert("app_version".into(), opts.app_version.into());
        common.insert("os".into(), std::env::consts::OS.into());
        common.insert("arch".into(), std::env::consts::ARCH.into());
        common.insert(
            "build".into(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .into(),
        );
        common.insert(
            "run_mode".into(),
            serde_json::to_value(opts.run_mode).unwrap_or_default(),
        );
        common.insert("workspace_scope".into(), opts.workspace_scope.into());
        common.insert("device_hash".into(), hash("device", &opts.device_id).into());
        common.insert("install_hash".into(), identity.install.to_string().into());
        common.insert(
            "app_session".into(),
            uuid::Uuid::new_v4().to_string().into(),
        );

        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let inner = Arc::new(Inner {
            collecting: AtomicBool::new(collecting),
            env_override,
            upload_endpoint: tuning.endpoint.clone(),
            settings: Mutex::new(settings),
            state_dir: Some(state_dir.clone()),
            identity: RwLock::new(identity),
            tx: Some(tx),
            rollup: Rollup::new(),
            stats: Stats::default(),
            recent: Mutex::new(VecDeque::new()),
            common,
            started: Instant::now(),
        });
        let cfg = WorkerConfig {
            endpoint: tuning.endpoint,
            api_key: tuning.api_key,
            spool: Some(state_dir.join("spool.jsonl")),
            debug_log: tuning
                .debug_log
                .then(|| opts.data_dir.join("logs").join("telemetry.jsonl")),
            batch_size: tuning.batch_size.max(1),
            flush_every: tuning.flush_every,
            rollup_every: tuning.rollup_every,
        };
        runtime.spawn(transport::run(inner.clone(), rx, cfg));
        Telemetry { inner }
    }

    /// Records one event. Never blocks: when the channel is full (a stuck
    /// network with a burst of events) the event is counted as dropped.
    pub fn capture(&self, event: Event) {
        if !self.is_collecting() {
            return;
        }
        let Some(tx) = &self.inner.tx else {
            return;
        };
        let envelope = self.inner.envelope(event);
        if tx.try_send(Command::Event(Box::new(envelope))).is_err() {
            self.inner.stats.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Aggregates one occurrence of `metric`/`key` into the hourly rollup.
    pub fn record(&self, metric: &'static str, key: &str, elapsed: Option<Duration>, ok: bool) {
        if self.is_collecting() {
            self.inner.rollup.record(metric, key, elapsed, ok);
        }
    }

    /// Shorthand for an un-timed, successful [`Telemetry::record`].
    pub fn count(&self, metric: &'static str, key: &str) {
        self.record(metric, key, None, true);
    }

    pub fn is_collecting(&self) -> bool {
        self.inner.collecting.load(Ordering::Relaxed)
    }

    /// Time since this handle started (process uptime for the engine).
    pub fn uptime(&self) -> Duration {
        self.inner.started.elapsed()
    }

    /// Attributes subsequent events to the signed-in account (hashed), or
    /// back to the install when `None`.
    pub fn set_account(&self, account_id: Option<&str>) {
        if let Ok(mut identity) = self.inner.identity.write() {
            *identity = identity.with_account(account_id);
        }
    }

    /// Where telemetry keeps its own state (cursors, spool), if anywhere.
    pub fn state_dir(&self) -> Option<&Path> {
        self.inner.state_dir.as_deref()
    }

    pub fn settings(&self) -> TelemetrySettings {
        self.inner
            .settings
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// Applies and persists a settings change. Turning collection off drops
    /// everything still buffered or spooled — nothing recorded before the
    /// opt-out is sent after it.
    pub fn update_settings(&self, patch: TelemetrySettingsPatch) -> TelemetrySettings {
        let (before, after) = {
            let Ok(mut settings) = self.inner.settings.lock() else {
                return TelemetrySettings::default();
            };
            let before = settings.clone();
            if let Some(v) = patch.enabled {
                settings.enabled = v;
            }
            if let Some(v) = patch.usage_import {
                settings.usage_import = v;
            }
            if let Some(v) = patch.notice_acknowledged {
                settings.notice_acknowledged = v;
            }
            (before, settings.clone())
        };
        if let Some(dir) = &self.inner.state_dir {
            save_settings(dir, &after);
        }
        let forced_off = matches!(self.inner.env_override, Some(EnvOverride::ForcedOff(_)));
        let collecting = after.enabled && !forced_off && self.inner.tx.is_some();
        let was = self.inner.collecting.swap(collecting, Ordering::Relaxed);
        if was && !collecting {
            self.inner.rollup.drain();
            if let Some(tx) = &self.inner.tx {
                let _ = tx.try_send(Command::Purge);
            }
        }
        if collecting && before != after {
            self.capture(
                Event::new(names::TELEMETRY_SETTINGS_CHANGED)
                    .with("usage_import", after.usage_import)
                    .with("notice_acknowledged", after.notice_acknowledged),
            );
        }
        after
    }

    pub fn status(&self) -> TelemetryStatus {
        let identity = self
            .inner
            .identity
            .read()
            .map(|id| id.clone())
            .unwrap_or_else(|_| Identity::new("poisoned"));
        let collecting = self.is_collecting();
        let override_reason = match self.inner.env_override {
            Some(EnvOverride::ForcedOff(var)) => Some(format!("disabled by {var}")),
            Some(EnvOverride::ForcedOn) => Some("enabled by ZERON_TELEMETRY".to_string()),
            None if self.inner.tx.is_some() && self.inner.upload_endpoint.is_none() => {
                Some("development build: events are recorded locally, not uploaded".to_string())
            }
            None => None,
        };
        TelemetryStatus {
            settings: self.settings(),
            collecting,
            uploading: collecting && self.inner.upload_endpoint.is_some(),
            override_reason,
            distinct_id: identity.distinct.to_string(),
            account_linked: identity.account_linked,
            endpoint: self.inner.upload_endpoint.clone(),
            sent: self.inner.stats.sent.load(Ordering::Relaxed),
            dropped: self.inner.stats.dropped.load(Ordering::Relaxed),
            last_error: self
                .inner
                .stats
                .last_error
                .lock()
                .ok()
                .and_then(|e| e.clone()),
            recent: self
                .inner
                .recent
                .lock()
                .map(|r| r.iter().cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// Pushes everything buffered (including the current rollup window) and
    /// waits up to `timeout` for the send to finish. Used at shutdown.
    pub async fn flush(&self, timeout: Duration) {
        let Some(tx) = &self.inner.tx else {
            return;
        };
        let (ack, done) = oneshot::channel();
        if tx.send(Command::Flush(ack)).await.is_ok() {
            let _ = tokio::time::timeout(timeout, done).await;
        }
    }
}

/// Reads persisted settings without starting anything (CLI, engine-less).
pub fn read_settings(data_dir: &Path) -> TelemetrySettings {
    load_settings(&data_dir.join("telemetry"))
}

/// Writes settings without starting anything (CLI when no engine runs).
pub fn write_settings(data_dir: &Path, settings: &TelemetrySettings) {
    save_settings(&data_dir.join("telemetry"), settings);
}

fn load_settings(dir: &Path) -> TelemetrySettings {
    std::fs::read(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_settings(dir: &Path, settings: &TelemetrySettings) {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{SETTINGS_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
        std::fs::rename(&tmp, dir.join(SETTINGS_FILE))
    };
    if let Err(err) = write() {
        tracing::warn!(%err, "telemetry: settings not saved");
    }
}

#[cfg(test)]
mod tests;
