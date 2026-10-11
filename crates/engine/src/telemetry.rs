//! Engine side of product telemetry (`zeron-telemetry`): process lifecycle,
//! identity, the daily heartbeat, per-RPC rollups and product events, the
//! opt-in external usage import, and the telemetry RPC surface.
//!
//! Event sources report through [`zeron_telemetry::global`]; this module
//! starts it once per process and owns the background loops tied to an
//! [`crate::EngineRuntime`].

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use zeron_proto::WorkspaceScope;
use zeron_rpc::{RpcError, RpcReply, methods};
use zeron_telemetry::external::{Roots, UsageScanner, UsageStats, ZeronLedger};
use zeron_telemetry::turn::model_token;
use zeron_telemetry::{Event, Telemetry, TelemetryOptions, TelemetrySettingsPatch, names};

pub use zeron_telemetry::RunMode;

use crate::auth::{Auth, AuthState};
use crate::registry::HarnessRegistry;
use crate::workspace_host::WorkspaceHost;

const HEARTBEAT_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const HEARTBEAT_FIRST_AFTER: Duration = Duration::from_secs(60);
const USAGE_SCAN_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const USAGE_SCAN_FIRST_AFTER: Duration = Duration::from_secs(120);
const EXIT_FLUSH: Duration = Duration::from_secs(3);
const MAX_TRACKED_PER_CALL: usize = 100;

/// Starts (once per process) and installs the global telemetry handle. A
/// later runtime in the same process (headed sign-in swaps Local → Synced)
/// reuses it and only updates the workspace-scope context.
pub fn start(data_dir: &Path, run_mode: RunMode, scope: WorkspaceScope, device_id: &str) {
    let existing = zeron_telemetry::global();
    if existing.state_dir().is_some() {
        existing.set_context("workspace_scope", Some(scope_str(scope).into()));
        return;
    }
    let telemetry = Telemetry::start(TelemetryOptions {
        data_dir: data_dir.to_path_buf(),
        run_mode,
        workspace_scope: scope_str(scope).to_string(),
        device_id: device_id.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    });
    if !zeron_telemetry::install_global(&telemetry) {
        return;
    }
    let status = telemetry.status();
    tracing::info!(
        collecting = status.collecting,
        uploading = status.uploading,
        reason = status.override_reason.as_deref().unwrap_or(""),
        "telemetry: anonymous usage data {} (disable: Settings → Privacy, `zeron telemetry disable`, or ZERON_TELEMETRY=0)",
        if status.collecting { "on" } else { "off" },
    );
    telemetry.capture(
        Event::new(names::APP_STARTED)
            .with("first_run", telemetry.first_run())
            .with("usage_import", status.settings.usage_import),
    );
}

/// `app.stopped` plus a bounded flush. Call once when the process is about
/// to exit (headless shutdown, headed quit) — not on runtime swaps.
pub async fn process_exit() {
    let telemetry = zeron_telemetry::global();
    telemetry
        .capture(Event::new(names::APP_STOPPED).with("uptime_secs", telemetry.uptime().as_secs()));
    telemetry.flush(EXIT_FLUSH).await;
}

fn scope_str(scope: WorkspaceScope) -> &'static str {
    match scope {
        WorkspaceScope::Local => "local",
        WorkspaceScope::Synced => "synced",
        WorkspaceScope::Development => "development",
    }
}

/// Background loops owned by one runtime; aborted when it drops.
pub struct TelemetryTasks(Vec<tokio::task::JoinHandle<()>>);

impl Drop for TelemetryTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// What the heartbeat reads; cloned out of the engine core.
#[derive(Clone)]
pub(crate) struct HeartbeatSources {
    pub registry: Arc<HarnessRegistry>,
    pub workspace: WorkspaceHost,
    pub device_id: String,
}

pub(crate) fn spawn_runtime_tasks(auth: Auth, sources: HeartbeatSources) -> TelemetryTasks {
    let mut tasks = vec![
        tokio::spawn(watch_auth(auth)),
        tokio::spawn(heartbeat_loop(sources)),
    ];
    tasks.push(tokio::spawn(usage_import_loop()));
    TelemetryTasks(tasks)
}

/// Keeps attribution on the signed-in account (hashed) and reports
/// sign-in/out transitions.
async fn watch_auth(auth: Auth) {
    let mut state = auth.watch_state();
    let mut last_kind = None;
    loop {
        let (account, kind) = {
            let current = state.borrow();
            match &*current {
                AuthState::SignedIn { user, .. } => (Some(user.id.clone()), "signed_in"),
                AuthState::NeedsOrganization { user } => {
                    (Some(user.id.clone()), "needs_organization")
                }
                AuthState::SignedOut => (None, "signed_out"),
            }
        };
        let telemetry = zeron_telemetry::global();
        telemetry.set_account(account.as_deref());
        if last_kind.is_some_and(|last| last != kind) {
            telemetry.capture(Event::new(names::AUTH_CHANGED).with("state", kind));
        }
        last_kind = Some(kind);
        if state.changed().await.is_err() {
            return;
        }
    }
}

async fn heartbeat_loop(sources: HeartbeatSources) {
    tokio::time::sleep(HEARTBEAT_FIRST_AFTER).await;
    loop {
        let telemetry = zeron_telemetry::global();
        if telemetry.is_collecting() && heartbeat_due(&telemetry) {
            let sources = sources.clone();
            if let Ok(event) = tokio::task::spawn_blocking(move || heartbeat(&sources)).await {
                telemetry.capture(event);
                mark_heartbeat(&telemetry);
            }
        }
        tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    }
}

/// At most one heartbeat per ~day per install, across restarts.
fn heartbeat_due(telemetry: &Telemetry) -> bool {
    let Some(dir) = telemetry.state_dir() else {
        return false;
    };
    let last = std::fs::read_to_string(dir.join("last-heartbeat"))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());
    let now = chrono::Utc::now().timestamp();
    last.is_none_or(|last| now - last >= HEARTBEAT_EVERY.as_secs() as i64 - 600)
}

fn mark_heartbeat(telemetry: &Telemetry) {
    if let Some(dir) = telemetry.state_dir() {
        let _ = std::fs::write(
            dir.join("last-heartbeat"),
            chrono::Utc::now().timestamp().to_string(),
        );
    }
}

/// A snapshot of how this install is set up and used: which harnesses are
/// installed/enabled, how chats spread over harnesses, models, sandbox and
/// reasoning settings, and the shape of the workspace (spaces, devices).
/// Counts and enum values only.
pub(crate) fn heartbeat(sources: &HeartbeatSources) -> Event {
    let descriptors = sources.registry.descriptors();
    let installed: Vec<Value> = descriptors
        .iter()
        .filter(|d| d.installed)
        .map(|d| enum_value(&d.id))
        .collect();
    let enabled: Vec<Value> = descriptors
        .iter()
        .filter(|d| crate::registry::descriptor_enabled(d))
        .map(|d| enum_value(&d.id))
        .collect();

    let week_ago = chrono::Utc::now() - chrono::Duration::days(7);
    let chats = sources.workspace.read_chats().unwrap_or_default();
    let mut active = 0u64;
    let mut archived = 0u64;
    let mut side = 0u64;
    let mut hosted_here = 0u64;
    let mut worktree = 0u64;
    let mut by_harness = BTreeMap::<String, u64>::new();
    let mut by_model = BTreeMap::<String, u64>::new();
    let mut by_sandbox = BTreeMap::<String, u64>::new();
    let mut by_reasoning = BTreeMap::<String, u64>::new();
    for chat in &chats {
        if chat.archived {
            archived += 1;
        }
        if chat.parent_chat_id.is_some() {
            side += 1;
        }
        if chat.device_id == sources.device_id {
            hosted_here += 1;
        }
        if chat.checkout_id.is_some() {
            worktree += 1;
        }
        let recent = chat.last_message_at.is_some_and(|at| at >= week_ago);
        if !recent {
            continue;
        }
        active += 1;
        if let Some(config) = &chat.config {
            *by_harness.entry(enum_string(&config.harness)).or_default() += 1;
            if let Some(model) = config.model.as_deref().and_then(model_token) {
                *by_model.entry(model).or_default() += 1;
            }
            *by_sandbox.entry(enum_string(&config.sandbox)).or_default() += 1;
            let reasoning = config
                .reasoning
                .as_ref()
                .map(enum_string)
                .unwrap_or_else(|| "default".into());
            *by_reasoning.entry(reasoning).or_default() += 1;
        }
    }

    let spaces = sources.workspace.read_spaces().unwrap_or_default();
    let spaces_here = spaces
        .iter()
        .filter(|s| s.device_id == sources.device_id)
        .count();
    let spaces_git = spaces.iter().filter(|s| s.git_detected).count();
    let devices = sources.workspace.read_devices().unwrap_or_default();
    let devices_seen = devices
        .iter()
        .filter(|d| d.last_seen_at.is_some_and(|at| at >= week_ago))
        .count();
    let mut platforms = BTreeMap::<String, u64>::new();
    for device in &devices {
        *platforms.entry(device.platform.clone()).or_default() += 1;
    }

    Event::new(names::APP_HEARTBEAT)
        .with("harnesses_installed", installed)
        .with("harnesses_enabled", enabled)
        .with("chats_total", chats.len() as u64)
        .with("chats_active_7d", active)
        .with("chats_archived", archived)
        .with("chats_side", side)
        .with("chats_hosted_here", hosted_here)
        .with("chats_worktree", worktree)
        .with("active_chats_by_harness", counts(by_harness))
        .with("active_chats_by_model", counts(by_model))
        .with("active_chats_by_sandbox", counts(by_sandbox))
        .with("active_chats_by_reasoning", counts(by_reasoning))
        .with("spaces_total", spaces.len() as u64)
        .with("spaces_here", spaces_here as u64)
        .with("spaces_git", spaces_git as u64)
        .with("devices_total", devices.len() as u64)
        .with("devices_seen_7d", devices_seen as u64)
        .with("device_platforms", counts(platforms))
        .with(
            "auto_update",
            std::env::var("ZERON_AUTO_UPDATE").is_ok_and(|v| v == "1"),
        )
}

fn counts(map: BTreeMap<String, u64>) -> Value {
    Value::Object(map.into_iter().map(|(k, v)| (k, Value::from(v))).collect())
}

fn enum_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn enum_string(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        _ => "unknown".into(),
    }
}

// ---------------------------------------------------------------------------
// Usage: Zeron's own turns (always, local) + opt-in external import
// ---------------------------------------------------------------------------

struct UsageState {
    ledger: Mutex<ZeronLedger>,
    scanner: Mutex<Option<UsageScanner>>,
    last: Mutex<
        Option<(
            chrono::DateTime<chrono::Utc>,
            Vec<zeron_telemetry::external::DailyUsage>,
        )>,
    >,
    scan_now: tokio::sync::Notify,
}

fn usage_state() -> &'static UsageState {
    static STATE: OnceLock<UsageState> = OnceLock::new();
    STATE.get_or_init(|| {
        let telemetry = zeron_telemetry::global();
        let state_dir = telemetry.state_dir().map(Path::to_path_buf);
        let scanner = state_dir.as_ref().and_then(|dir| {
            let home = crate::repos::session_home_dir().ok()?;
            Some(UsageScanner::new(
                dir.join("external"),
                Roots::from_home(&home),
            ))
        });
        UsageState {
            ledger: Mutex::new(ZeronLedger::open(state_dir.as_deref())),
            scanner: Mutex::new(scanner),
            last: Mutex::new(None),
            scan_now: tokio::sync::Notify::new(),
        }
    })
}

/// Called with every finished turn: captured for telemetry (when on) and
/// always folded into the local usage ledger.
pub(crate) fn turn_finished(event: Event) {
    let telemetry = zeron_telemetry::global();
    if telemetry.state_dir().is_some()
        && let Ok(mut ledger) = usage_state().ledger.lock()
    {
        ledger.record_turn(&event);
    }
    telemetry.capture(event);
}

async fn usage_import_loop() {
    let state = usage_state();
    tokio::select! {
        _ = tokio::time::sleep(USAGE_SCAN_FIRST_AFTER) => {}
        _ = state.scan_now.notified() => {}
    }
    loop {
        if zeron_telemetry::global().settings().usage_import {
            run_usage_scan().await;
        }
        tokio::select! {
            _ = tokio::time::sleep(USAGE_SCAN_EVERY) => {}
            _ = state.scan_now.notified() => {}
        }
    }
}

async fn run_usage_scan() {
    let report = tokio::task::spawn_blocking(|| {
        let state = usage_state();
        let mut guard = state.scanner.lock().ok()?;
        let scanner = guard.as_mut()?;
        let started = Instant::now();
        let report = scanner.scan();
        tracing::debug!(
            files = report.files_scanned,
            bytes = report.bytes_read,
            ms = started.elapsed().as_millis() as u64,
            "telemetry: external usage scan"
        );
        Some(report)
    })
    .await
    .ok()
    .flatten();
    let Some(report) = report else {
        return;
    };
    let telemetry = zeron_telemetry::global();
    // Re-check consent after the (possibly long) scan.
    if telemetry.is_collecting() && telemetry.settings().usage_import {
        for day in &report.unreported {
            telemetry.capture(day.to_event());
        }
        if let Ok(mut guard) = usage_state().scanner.lock()
            && let Some(scanner) = guard.as_mut()
        {
            scanner.mark_reported(&report.unreported);
        }
    }
    if let Ok(mut last) = usage_state().last.lock() {
        *last = Some((chrono::Utc::now(), report.window));
    }
}

fn usage_stats() -> UsageStats {
    let state = usage_state();
    let import_enabled = zeron_telemetry::global().settings().usage_import;
    let (last_scan_at, external) = match state.last.lock().ok().and_then(|l| l.clone()) {
        Some((at, window)) if import_enabled => (Some(at.to_rfc3339()), window),
        _ => (None, Vec::new()),
    };
    UsageStats {
        import_enabled,
        external,
        zeron: state.ledger.lock().map(|l| l.days()).unwrap_or_default(),
        last_scan_at,
    }
}

// ---------------------------------------------------------------------------
// RPC
// ---------------------------------------------------------------------------

/// Where an [`crate::rpc::EngineRpc`] call came from, for rollups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcOrigin {
    /// Localhost IPC / in-process: this machine's viewports, CLI, MCP.
    Local,
    /// Through this device's DeviceRoom host relay: a remote viewport.
    Relay,
}

impl RpcOrigin {
    fn metric(self) -> &'static str {
        match self {
            RpcOrigin::Local => "rpc",
            RpcOrigin::Relay => "rpc_relay",
        }
    }
}

/// The telemetry methods, answered before routing (always local, never
/// forwarded). `None` = not a telemetry method.
pub(crate) async fn handle_rpc(method: &str, params: &Value) -> Option<Result<RpcReply, RpcError>> {
    let telemetry = zeron_telemetry::global();
    Some(match method {
        methods::TELEMETRY_STATUS => RpcReply::value(&telemetry.status()),
        methods::SET_TELEMETRY_SETTINGS => {
            let patch: TelemetrySettingsPatch = match serde_json::from_value(params.clone()) {
                Ok(patch) => patch,
                Err(err) => return Some(Err(RpcError::BadParams(err.to_string()))),
            };
            let turned_on_import = patch.usage_import == Some(true);
            telemetry.update_settings(patch);
            if turned_on_import {
                usage_state().scan_now.notify_one();
            }
            RpcReply::value(&telemetry.status())
        }
        methods::TRACK_EVENTS => {
            #[derive(Deserialize)]
            struct Tracked {
                name: String,
                #[serde(default)]
                props: Map<String, Value>,
            }
            #[derive(Deserialize)]
            struct Params {
                #[serde(default)]
                surface: String,
                #[serde(default)]
                events: Vec<Tracked>,
            }
            let params: Params = match serde_json::from_value(params.clone()) {
                Ok(p) => p,
                Err(err) => return Some(Err(RpcError::BadParams(err.to_string()))),
            };
            let mut accepted = 0u64;
            for tracked in params.events.into_iter().take(MAX_TRACKED_PER_CALL) {
                if let Some(event) = Event::ui(&tracked.name, &params.surface, tracked.props) {
                    telemetry.capture(event);
                    accepted += 1;
                }
            }
            RpcReply::value(&json!({ "accepted": accepted }))
        }
        methods::USAGE_STATS => RpcReply::value(&usage_stats()),
        _ => return None,
    })
}

/// Rollup key for a call: `Mutate` is split by op so "create chat" and
/// "archive chat" aren't one number.
fn rollup_key<'a>(method: &'a str, params: &'a Value) -> std::borrow::Cow<'a, str> {
    if method == methods::MUTATE
        && let Some(op) = params.get("op").and_then(Value::as_str)
        && op.len() <= 40
        && op.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return format!("Mutate.{op}").into();
    }
    method.into()
}

/// Per-call bookkeeping started before dispatch, finished after.
pub(crate) struct RpcCall {
    origin: RpcOrigin,
    key: String,
    forwarded: bool,
    event: Option<Event>,
    started: Instant,
}

impl RpcCall {
    pub fn begin(origin: RpcOrigin, method: &str, params: &Value, forwarded: bool) -> Option<Self> {
        let telemetry = zeron_telemetry::global();
        if !telemetry.is_collecting() {
            return None;
        }
        Some(RpcCall {
            origin,
            key: rollup_key(method, params).into_owned(),
            forwarded,
            // A forwarded call is the target device's product event, not ours.
            event: (!forwarded)
                .then(|| product_event(method, params))
                .flatten(),
            started: Instant::now(),
        })
    }

    pub fn finish(self, ok: bool) {
        let telemetry = zeron_telemetry::global();
        let metric = if self.forwarded {
            "rpc_forwarded"
        } else {
            self.origin.metric()
        };
        telemetry.record(metric, &self.key, Some(self.started.elapsed()), ok);
        if ok && let Some(event) = self.event {
            let event = event.with("remote", self.origin == RpcOrigin::Relay);
            telemetry.capture(event);
        }
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// The product events worth more than a rollup count: creation of the
/// things users build with, and the settings they change. Only enum-like
/// params are read; ids, paths, names and text never are.
pub(crate) fn product_event(method: &str, params: &Value) -> Option<Event> {
    let token = |key: &str| {
        str_param(params, key)
            .filter(|v| {
                v.len() <= 40
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
            .map(str::to_string)
    };
    Some(match method {
        methods::MUTATE => match str_param(params, "op")? {
            "createChat" => {
                let config = params.get("config");
                Event::new(names::CHAT_CREATED)
                    .with_opt(
                        "harness",
                        config
                            .and_then(|c| c.get("harness"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    )
                    .with_opt(
                        "model",
                        config
                            .and_then(|c| c.get("model"))
                            .and_then(Value::as_str)
                            .and_then(model_token),
                    )
                    .with(
                        "side",
                        params.get("parentChatId").is_some_and(|v| !v.is_null()),
                    )
                    .with(
                        "in_space",
                        params.get("spaceId").is_some_and(|v| !v.is_null()),
                    )
                    .with(
                        "custom_cwd",
                        params.get("cwd").is_some_and(|v| !v.is_null()),
                    )
            }
            "createSpace" => Event::new(names::SPACE_CREATED).with(
                "git",
                params
                    .get("gitDetected")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            _ => return None,
        },
        methods::ADD_REPO => Event::new(names::REPO_ADDED).with("via", "add"),
        methods::CLONE_REPO => Event::new(names::REPO_ADDED).with("via", "clone"),
        methods::CREATE_REPO => Event::new(names::REPO_ADDED).with("via", "create"),
        methods::CREATE_WORKTREE => Event::new(names::WORKTREE_CREATED),
        methods::DELETE_WORKTREE => Event::new(names::WORKTREE_REMOVED),
        methods::OPEN_TERMINAL => Event::new(names::TERMINAL_OPENED),
        methods::RUN_PROJECT_ACTION => Event::new(names::PROJECT_ACTION_RUN),
        methods::UPLOAD_COMMIT => Event::new(names::UPLOAD_COMMITTED)
            .with_opt("mime_class", token("mimeType").map(mime_class)),
        methods::INSTALL_HARNESS => {
            Event::new(names::HARNESS_INSTALLED).with_opt("harness", token("harness"))
        }
        methods::SET_HARNESS_ENABLED => Event::new(names::HARNESS_TOGGLED)
            .with_opt("harness", token("harness"))
            .with_opt("enabled", params.get("enabled").and_then(Value::as_bool)),
        methods::APPLY_HARNESS_UPDATE => {
            Event::new(names::HARNESS_UPDATED).with_opt("harness", token("harness"))
        }
        methods::IMPORT_LOCAL_WORKSPACE => Event::new(names::LOCAL_IMPORT_FINISHED),
        methods::ACTIVATE_AGENT_ACCOUNT => {
            Event::new(names::AGENT_ACCOUNT_ACTIVATED).with_opt("harness", token("harness"))
        }
        methods::POST_CHANGE_REQUEST_COMMENT
        | methods::DISCARD_WORKING_TREE
        | methods::FETCH_ALL
        | methods::SWITCH_REF => Event::new(names::SOURCE_CONTROL_ACTION).with(
            "action",
            match method {
                methods::POST_CHANGE_REQUEST_COMMENT => "comment",
                methods::DISCARD_WORKING_TREE => "discard",
                methods::FETCH_ALL => "fetch",
                _ => "switch_ref",
            },
        ),
        _ => return None,
    })
}

fn mime_class(mime: String) -> String {
    mime.split('/').next().unwrap_or("other").to_string()
}

/// Relay session outcomes → `remote.relay` events.
pub(crate) fn relay_observer() -> zeron_rpc::HostSessionObserver {
    Arc::new(|report: zeron_rpc::HostSessionReport| {
        let telemetry = zeron_telemetry::global();
        telemetry.record(
            "relay_session",
            match (report.connected, report.failed) {
                (false, _) => "dial_failed",
                (true, true) => "dropped",
                (true, false) => "ended",
            },
            Some(report.duration),
            report.connected,
        );
        // Sessions that actually served someone are worth an event each.
        if report.clients > 0 {
            telemetry.capture(
                Event::new(names::REMOTE_RELAY)
                    .with("session_secs", report.duration.as_secs())
                    .with("clients", report.clients)
                    .with("failed", report.failed),
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_events_read_only_enum_params() {
        let ev = product_event(
            methods::MUTATE,
            &json!({
                "op": "createChat",
                "chatId": "c1",
                "spaceId": "s1",
                "cwd": "/home/me/secret",
                "config": {"harness": "codex", "model": "gpt-5", "sandbox": "read-only"},
            }),
        )
        .unwrap();
        assert_eq!(ev.name(), names::CHAT_CREATED);
        let p = ev.props();
        assert_eq!(p["harness"], "codex");
        assert_eq!(p["model"], "gpt-5");
        assert_eq!(p["in_space"], true);
        assert_eq!(p["custom_cwd"], true);
        let json = serde_json::to_string(p).unwrap();
        assert!(!json.contains("secret") && !json.contains("c1"));

        let toggled = product_event(
            methods::SET_HARNESS_ENABLED,
            &json!({"harness": "cursor", "enabled": false}),
        )
        .unwrap();
        assert_eq!(toggled.props()["harness"], "cursor");
        assert_eq!(toggled.props()["enabled"], false);

        assert!(product_event(methods::LIST_REPOS, &json!({})).is_none());
        assert!(product_event(methods::MUTATE, &json!({"op": "archiveChat"})).is_none());
    }

    #[test]
    fn mutate_rollups_split_by_op() {
        assert_eq!(
            rollup_key(methods::MUTATE, &json!({"op": "archiveChat"})),
            "Mutate.archiveChat"
        );
        assert_eq!(
            rollup_key(methods::MUTATE, &json!({"op": "../../x"})),
            methods::MUTATE
        );
        assert_eq!(
            rollup_key(methods::LIST_REPOS, &json!({})),
            methods::LIST_REPOS
        );
    }

    #[tokio::test]
    async fn telemetry_methods_answer_even_when_disabled() {
        let status = handle_rpc(methods::TELEMETRY_STATUS, &Value::Null)
            .await
            .unwrap()
            .unwrap();
        let RpcReply::Value(value) = status else {
            panic!("unary reply expected");
        };
        assert_eq!(value["collecting"], false);
        assert!(
            handle_rpc(methods::LIST_REPOS, &Value::Null)
                .await
                .is_none()
        );
        let tracked = handle_rpc(
            methods::TRACK_EVENTS,
            &json!({"surface": "desktop", "events": [{"name": "panel.toggled", "props": {}}]}),
        )
        .await
        .unwrap()
        .unwrap();
        let RpcReply::Value(value) = tracked else {
            panic!("unary reply expected");
        };
        assert_eq!(value["accepted"], 1);
    }
}
