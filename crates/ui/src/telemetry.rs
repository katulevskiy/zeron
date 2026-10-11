//! Viewport side of anonymous product telemetry.
//!
//! The engine owns collection, consent, sanitising and upload (see the
//! `zeron-telemetry` crate and `docs/telemetry.md`). This module only reports
//! what the user did in the window: call [`track`] from anywhere with a
//! static event name and a few scalar props. Events collect in a small bounded
//! in-memory queue and leave as one `TrackEvents` RPC every ~10 s, at 25
//! queued events, or on quit. Nothing here blocks the UI thread.
//!
//! Props must be bool, number or short tokens: never paths, titles, URLs,
//! prompts, file names, branch or repository names. The engine drops anything
//! else, and [`TrackedEvent::new`] repeats that check so a mistake at a call
//! site fails closed on this side too.
//!
//! Older engines answer `UnknownMethod`; telemetry then goes quiet.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use gpui::{App, Global, Subscription};
use serde_json::{Map, Value, json};
use zeron_rpc::{RpcError, methods};
use zeron_telemetry::external::{DailyUsage, UsageStats};
use zeron_telemetry::{TelemetrySettings, TelemetrySettingsPatch, TelemetryStatus};

use crate::state::EngineHandle;

const SURFACE: &str = "desktop";
const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
const FLUSH_THRESHOLD: usize = 25;
/// Events held while no engine is attached (or a send is failing).
const MAX_QUEUED: usize = 200;
const MAX_BATCH: usize = 100;
const MAX_PROPS: usize = 64;
/// Status is re-read this many ticks apart (the Privacy page polls faster).
const STATUS_EVERY_TICKS: u32 = 6;
const FIRST_SNAPSHOT_DELAY: Duration = Duration::from_secs(30);
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Settings changes reported per tick; a bulk reset would otherwise flood.
const MAX_SETTING_EVENTS: usize = 20;
const MAX_SNAPSHOT_PROPS: usize = 60;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `CamelCase` / `camelCase` to `snake_case`, treating acronyms as one word
/// (`OpenPRView` becomes `open_pr_view`).
pub(crate) fn snake_case(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 4);
    for (ix, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev = ix.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(ix + 1).copied();
            let boundary = match prev {
                Some(p) if p.is_ascii_lowercase() || p.is_ascii_digit() => true,
                Some(p) if p.is_ascii_uppercase() => next.is_some_and(|n| n.is_ascii_lowercase()),
                _ => false,
            };
            if boundary {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Characters the engine accepts in a string prop value.
pub(crate) fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
}

/// `^[a-z][a-z0-9_.]{0,max-1}$`
fn is_identifier(value: &str, max: usize) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && value.len() <= max
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.'))
}

fn clean_scalar(value: &Value) -> Option<Value> {
    match value {
        Value::Bool(_) | Value::Number(_) => Some(value.clone()),
        Value::String(text) if is_token(text) => Some(value.clone()),
        _ => None,
    }
}

/// A prop value the engine would accept: a scalar, or an array of scalars.
fn clean_value(value: &Value) -> Option<Value> {
    match value {
        Value::Array(items) if items.len() <= 32 => items
            .iter()
            .map(clean_scalar)
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        other => clean_scalar(other),
    }
}

/// An action name as a stable event token: `shell::ToggleSidebar` becomes
/// `shell.toggle_sidebar`.
pub(crate) fn normalize_action(raw: &str) -> Option<String> {
    let name = raw
        .split("::")
        .map(snake_case)
        .collect::<Vec<_>>()
        .join(".");
    (is_identifier(&name, 64)).then_some(name)
}

/// The name to report for a keyboard-dispatched action, or `None` for actions
/// that are text editing rather than a feature (cursor movement, delete, ...).
pub(crate) fn shortcut_name(raw: &str) -> Option<String> {
    let (namespace, rest) = raw.split_once("::")?;
    let tracked = match namespace {
        "shell" | "terminal" | "browser" | "zeron" => true,
        "composer" => matches!(rest, "Submit" | "ModifiedSubmit" | "ToggleDictation"),
        _ => false,
    };
    tracked.then(|| normalize_action(raw)).flatten()
}

/// One event as `TrackEvents` expects it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrackedEvent {
    name: String,
    props: Map<String, Value>,
}

impl TrackedEvent {
    /// `None` when the name is not an identifier. Props that would not
    /// survive the engine's sanitiser are dropped here as well.
    fn new<K: Into<String>>(
        name: &str,
        props: impl IntoIterator<Item = (K, Value)>,
    ) -> Option<Self> {
        if !is_identifier(name, 64) {
            return None;
        }
        let props = props
            .into_iter()
            .filter_map(|(key, value)| {
                let key = key.into();
                (is_identifier(&key, 40))
                    .then(|| clean_value(&value).map(|value| (key, value)))
                    .flatten()
            })
            .take(MAX_PROPS)
            .collect();
        Some(Self {
            name: name.to_owned(),
            props,
        })
    }

    fn to_json(&self) -> Value {
        json!({ "name": self.name, "props": self.props })
    }
}

fn batch_params(events: &[TrackedEvent]) -> Value {
    json!({
        "surface": SURFACE,
        "events": events.iter().map(TrackedEvent::to_json).collect::<Vec<_>>(),
    })
}

/// Settings that never become telemetry: window and layout state, anything
/// holding ids, paths or device names, and per-key maps. Widths and heights
/// are drag-driven and would only add noise.
fn skip_setting(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "window_geometry",
        "open_tabs",
        "keymap",
        "space_filter",
        "settings_section",
        "sidebar_collapsed",
        "right_pane_open",
        "terminal_open",
        "dictation_input",
        "codex_voice",
        "codex_voice_device",
        "new_thread_composer_background",
        "pull_request_stars",
        "wallpaper_folder",
        "wallpaper_source",
        "wallpaper_history",
        "wallpaper_color",
        "theme_selection",
        "accent_color",
    ];
    EXACT.contains(&key)
        || key.starts_with("last_")
        || key.contains("_by_")
        || key.ends_with("_width")
        || key.ends_with("_widths")
        || key.ends_with("_height")
        || key.ends_with("_order")
}

/// A settings value worth reporting: a bool, a number or a short enum token.
/// Device-local font names collapse to `installed`.
/// Settings whose string values are enum ids (safe to report). Any other
/// string-valued setting is reported as changed without its value, and is
/// left out of the snapshot: a new free-form field can't leak by default.
const ENUM_STRING_SETTINGS: &[&str] = &[
    "composer_send_behavior",
    "sidebar_organization",
    "sidebar_sort",
    "appshot_destination",
    "appearance",
    "git_history_author_display",
    "ui_font_family",
    "terminal_font_family",
    "code_font_family",
    "pull_request_destination",
    "accent",
    "surface",
    "new_thread_background_effect",
    "reduce_motion",
];

fn settings_scalar(key: &str, value: &Value) -> Option<Value> {
    match value {
        Value::String(text) if text.starts_with("installed:") => Some(json!("installed")),
        Value::String(_) if !ENUM_STRING_SETTINGS.contains(&key) => None,
        other => clean_scalar(other),
    }
}

/// Top-level settings keys that differ, as `(snake_case key, value)`. The
/// value is present only for bool, number or short enum values; keys on the
/// skip list are never reported.
pub(crate) fn settings_changes(before: &Value, after: &Value) -> Vec<(String, Option<Value>)> {
    let (Value::Object(before), Value::Object(after)) = (before, after) else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = before.keys().chain(after.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .filter_map(|key| {
            let snake = snake_case(key);
            if skip_setting(&snake) || !is_identifier(&snake, 40) {
                return None;
            }
            let value = after.get(key).and_then(|v| settings_scalar(&snake, v));
            Some((snake, value))
        })
        .take(MAX_SETTING_EVENTS)
        .collect()
}

/// Flattened top-level scalar settings for the periodic snapshot.
pub(crate) fn settings_snapshot(settings: &Value) -> Vec<(String, Value)> {
    let Value::Object(map) = settings else {
        return Vec::new();
    };
    let mut entries: Vec<(String, Value)> = map
        .iter()
        .filter_map(|(key, value)| {
            let snake = snake_case(key);
            if skip_setting(&snake) || !is_identifier(&snake, 40) {
                return None;
            }
            settings_scalar(&snake, value).map(|value| (snake, value))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.truncate(MAX_SNAPSHOT_PROPS);
    entries
}

fn apply_patch(settings: &mut TelemetrySettings, patch: &TelemetrySettingsPatch) {
    settings.enabled = patch.enabled.unwrap_or(settings.enabled);
    settings.usage_import = patch.usage_import.unwrap_or(settings.usage_import);
    settings.notice_acknowledged = patch
        .notice_acknowledged
        .unwrap_or(settings.notice_acknowledged);
}

/// Whether the first-run banner should show for this engine status.
pub(crate) fn notice_needed(status: Option<&TelemetryStatus>) -> bool {
    status.is_some_and(|status| status.collecting && !status.settings.notice_acknowledged)
}

// ---------------------------------------------------------------------------
// Usage aggregation (Privacy page)
// ---------------------------------------------------------------------------

/// Days the Usage table covers, today included.
pub(crate) const USAGE_WINDOW_DAYS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct UsageRow {
    pub source: String,
    pub model: String,
    /// Turns Zeron ran itself, as opposed to other CLIs' local logs.
    pub in_zeron: bool,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: Option<f64>,
}

impl UsageRow {
    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens
    }
}

/// The last [`USAGE_WINDOW_DAYS`] days summed per source and model, biggest
/// first. Zeron's own turns and external logs stay on separate rows even when
/// they share a source id.
pub(crate) fn aggregate_usage(stats: &UsageStats, today: chrono::NaiveDate) -> Vec<UsageRow> {
    let first = (today - chrono::Duration::days(USAGE_WINDOW_DAYS - 1))
        .format("%Y-%m-%d")
        .to_string();
    let mut rows: BTreeMap<(bool, String, String), UsageRow> = BTreeMap::new();
    let groups = [(true, &stats.zeron), (false, &stats.external)];
    for (in_zeron, days) in groups {
        for day in days.iter().filter(|day: &&DailyUsage| day.day >= first) {
            let row = rows
                .entry((in_zeron, day.source.clone(), day.model.clone()))
                .or_insert_with(|| UsageRow {
                    source: day.source.clone(),
                    model: day.model.clone(),
                    in_zeron,
                    ..Default::default()
                });
            row.requests += day.requests;
            row.input_tokens += day.input_tokens;
            row.output_tokens += day.output_tokens;
            row.cache_read_tokens += day.cache_read_tokens;
            if let Some(cost) = day.cost_usd {
                *row.cost_usd.get_or_insert(0.0) += cost;
            }
        }
    }
    let mut rows: Vec<UsageRow> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b.total_tokens()
            .cmp(&a.total_tokens())
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.model.cmp(&b.model))
    });
    rows
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

/// Bounded FIFO of events waiting for the next send.
struct Queue {
    events: VecDeque<TrackedEvent>,
    cap: usize,
}

impl Queue {
    fn new(cap: usize) -> Self {
        Self {
            events: VecDeque::new(),
            cap,
        }
    }

    /// Enqueue, dropping the oldest event when full. True when a send is due.
    fn push(&mut self, event: TrackedEvent) -> bool {
        if self.events.len() >= self.cap {
            self.events.pop_front();
        }
        self.events.push_back(event);
        self.events.len() >= FLUSH_THRESHOLD
    }

    fn take(&mut self, max: usize) -> Vec<TrackedEvent> {
        let count = self.events.len().min(max);
        self.events.drain(..count).collect()
    }

    /// Put an unsent batch back in front, dropping its oldest events if the
    /// queue has filled up in the meantime.
    fn restore(&mut self, batch: Vec<TrackedEvent>) {
        for event in batch.into_iter().rev() {
            if self.events.len() >= self.cap {
                break;
            }
            self.events.push_front(event);
        }
    }

    fn clear(&mut self) {
        self.events.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.events.len()
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

struct Tracker {
    engine: Option<EngineHandle>,
    queue: Queue,
    /// Last `collecting` the engine reported; optimistic until it answers.
    collecting: bool,
    /// False once the engine answered `UnknownMethod`.
    available: bool,
    flushing: bool,
    ticks: u32,
    next_snapshot: Instant,
    _keystrokes: Option<Subscription>,
}

impl Global for Tracker {}

impl Tracker {
    fn new() -> Self {
        Self {
            engine: None,
            queue: Queue::new(MAX_QUEUED),
            collecting: true,
            available: true,
            flushing: false,
            ticks: 0,
            next_snapshot: Instant::now() + FIRST_SNAPSHOT_DELAY,
            _keystrokes: None,
        }
    }

    fn accepting(&self) -> bool {
        self.collecting && self.available
    }

    fn record(&mut self, event: TrackedEvent) -> bool {
        if !self.accepting() {
            return false;
        }
        self.queue.push(event)
    }

    fn set_collecting(&mut self, collecting: bool) {
        self.collecting = collecting;
        if !collecting {
            self.queue.clear();
        }
    }

    fn mark_unavailable(&mut self) {
        self.available = false;
        self.queue.clear();
    }

    /// Claim the next batch when a send can start.
    fn begin_flush(&mut self) -> Option<(EngineHandle, Vec<TrackedEvent>)> {
        if self.flushing || self.queue.is_empty() || !self.accepting() {
            return None;
        }
        let engine = self.engine.clone()?;
        self.flushing = true;
        Some((engine, self.queue.take(MAX_BATCH)))
    }

    fn finish_flush(&mut self, batch: Vec<TrackedEvent>, result: Result<(), RpcError>) {
        self.flushing = false;
        match result {
            Ok(()) => {}
            Err(RpcError::UnknownMethod(_)) => self.mark_unavailable(),
            Err(error) => {
                tracing::debug!(%error, "telemetry send failed; will retry");
                self.queue.restore(batch);
            }
        }
    }
}

/// What the engine last told us; the Privacy page and first-run banner read it.
#[derive(Default)]
pub(crate) struct TelemetryState {
    status: Option<TelemetryStatus>,
    usage: Option<UsageStats>,
}

impl Global for TelemetryState {}

fn with_tracker<R>(cx: &mut App, f: impl FnOnce(&mut Tracker) -> R) -> Option<R> {
    cx.has_global::<Tracker>()
        .then(|| f(cx.global_mut::<Tracker>()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install the tracker: keyboard-shortcut observer and the flush timer.
pub fn init(cx: &mut App) {
    let keystrokes = cx.observe_keystrokes(|event, _, cx| {
        if let Some(name) = event
            .action
            .as_ref()
            .and_then(|action| shortcut_name(action.name()))
        {
            track(cx, "shortcut", [("action", Value::from(name))]);
        }
    });
    let mut tracker = Tracker::new();
    tracker._keystrokes = Some(keystrokes);
    cx.set_global(tracker);
    cx.set_global(TelemetryState::default());
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(FLUSH_INTERVAL).await;
            cx.update(tick);
        }
    })
    .detach();
}

/// Record a feature use. Cheap and non-blocking; a no-op when telemetry is
/// off, unavailable, or not installed. Props must be bool, number or short
/// tokens (see the module docs).
pub fn track<K: Into<String>>(
    cx: &mut App,
    name: &'static str,
    props: impl IntoIterator<Item = (K, Value)>,
) {
    if !cx.has_global::<Tracker>() {
        return;
    }
    let Some(event) = TrackedEvent::new(name, props) else {
        return;
    };
    if with_tracker(cx, |tracker| tracker.record(event)).unwrap_or(false) {
        flush(cx);
    }
}

/// The engine this window talks to is up: start sending and learn the
/// consent state. Called on every attach, including account switches.
pub fn attach_engine(handle: EngineHandle, cx: &mut App) {
    with_tracker(cx, |tracker| {
        tracker.engine = Some(handle);
        tracker.available = true;
        tracker.flushing = false;
    });
    refresh_status(cx);
}

/// The engine went away (runtime swap); events queue until the next attach.
pub fn detach_engine(cx: &mut App) {
    with_tracker(cx, |tracker| {
        tracker.engine = None;
        tracker.flushing = false;
    });
}

/// Send what is queued now, if an engine is attached.
pub fn flush(cx: &mut App) {
    let Some((engine, batch)) = with_tracker(cx, Tracker::begin_flush).flatten() else {
        return;
    };
    let params = batch_params(&batch);
    cx.spawn(async move |cx| {
        let result = engine
            .client()
            .call(methods::TRACK_EVENTS, params)
            .await
            .map(|_| ());
        cx.update(|cx| {
            with_tracker(cx, |tracker| tracker.finish_flush(batch, result));
        });
    })
    .detach();
}

/// The final batch as `TrackEvents` params, for the quit hook to send before
/// the engine shuts down.
pub fn drain_for_quit(cx: &mut App) -> Option<Value> {
    let batch = with_tracker(cx, |tracker| {
        if tracker.accepting() {
            tracker.queue.take(MAX_BATCH)
        } else {
            Vec::new()
        }
    })?;
    (!batch.is_empty()).then(|| batch_params(&batch))
}

fn tick(cx: &mut App) {
    report_settings(cx);
    let snapshot_due = with_tracker(cx, |tracker| {
        let due = Instant::now() >= tracker.next_snapshot;
        if due {
            tracker.next_snapshot = Instant::now() + SNAPSHOT_INTERVAL;
        }
        due
    })
    .unwrap_or(false);
    if snapshot_due {
        let snapshot = serde_json::to_value(crate::settings::current(cx)).unwrap_or(Value::Null);
        track(cx, "settings.snapshot", settings_snapshot(&snapshot));
    }
    flush(cx);
    let refresh = with_tracker(cx, |tracker| {
        tracker.ticks = tracker.ticks.wrapping_add(1);
        tracker.ticks % STATUS_EVERY_TICKS == 0
    })
    .unwrap_or(false);
    if refresh {
        refresh_status(cx);
    }
}

/// One `settings.changed` per differing key since the last tick, however
/// many writes it took to get there.
fn report_settings(cx: &mut App) {
    let Some((before, after)) = crate::settings::take_changes_since_report(cx) else {
        return;
    };
    let (Ok(before), Ok(after)) = (serde_json::to_value(before), serde_json::to_value(after))
    else {
        return;
    };
    for (key, value) in settings_changes(&before, &after) {
        let mut props = vec![("key", Value::from(key))];
        props.extend(value.map(|value| ("value", value)));
        track(cx, "settings.changed", props);
    }
}

// ---- engine status -------------------------------------------------------

pub(crate) fn status(cx: &App) -> Option<&TelemetryStatus> {
    cx.try_global::<TelemetryState>()?.status.as_ref()
}

pub(crate) fn usage(cx: &App) -> Option<&UsageStats> {
    cx.try_global::<TelemetryState>()?.usage.as_ref()
}

/// The first-run notice is due: collecting, and no viewport has dismissed it.
pub fn notice_visible(cx: &App) -> bool {
    notice_needed(status(cx))
}

fn current_engine(cx: &mut App) -> Option<EngineHandle> {
    with_tracker(cx, |tracker| tracker.engine.clone()).flatten()
}

fn apply_status(status: TelemetryStatus, cx: &mut App) {
    if !cx.has_global::<TelemetryState>() {
        return;
    }
    let notice_before = notice_visible(cx);
    let collecting = status.collecting;
    cx.global_mut::<TelemetryState>().status = Some(status);
    with_tracker(cx, |tracker| tracker.set_collecting(collecting));
    if notice_visible(cx) != notice_before {
        cx.refresh_windows();
    }
}

fn status_failed(error: RpcError, cx: &mut App) {
    if matches!(error, RpcError::UnknownMethod(_)) {
        with_tracker(cx, Tracker::mark_unavailable);
        if cx.has_global::<TelemetryState>() {
            let notice_before = notice_visible(cx);
            cx.global_mut::<TelemetryState>().status = None;
            if notice_before {
                cx.refresh_windows();
            }
        }
    } else {
        tracing::debug!(%error, "telemetry status unavailable");
    }
}

/// Re-read consent state and the recent-events list from the engine.
pub fn refresh_status(cx: &mut App) {
    let Some(engine) = current_engine(cx) else {
        return;
    };
    cx.spawn(async move |cx| {
        let result = engine
            .client()
            .call_as::<TelemetryStatus>(methods::TELEMETRY_STATUS, json!({}))
            .await;
        cx.update(|cx| match result {
            Ok(status) => apply_status(status, cx),
            Err(error) => status_failed(error, cx),
        });
    })
    .detach();
}

/// Re-read the local usage table.
pub fn refresh_usage(cx: &mut App) {
    let Some(engine) = current_engine(cx) else {
        return;
    };
    cx.spawn(async move |cx| {
        let result = engine
            .client()
            .call_as::<UsageStats>(methods::USAGE_STATS, json!({}))
            .await;
        cx.update(|cx| match result {
            Ok(stats) if cx.has_global::<TelemetryState>() => {
                cx.global_mut::<TelemetryState>().usage = Some(stats);
            }
            Ok(_) => {}
            Err(error) => tracing::debug!(%error, "telemetry usage unavailable"),
        });
    })
    .detach();
}

/// Change engine-held telemetry settings. The local copy updates at once so
/// toggles and the banner respond immediately; the engine's reply replaces it.
pub fn set_settings(patch: TelemetrySettingsPatch, cx: &mut App) {
    let Some(engine) = current_engine(cx) else {
        return;
    };
    if cx.has_global::<TelemetryState>() {
        let notice_before = notice_visible(cx);
        if let Some(status) = cx.global_mut::<TelemetryState>().status.as_mut() {
            apply_patch(&mut status.settings, &patch);
        }
        if notice_visible(cx) != notice_before {
            cx.refresh_windows();
        }
    }
    let params = serde_json::to_value(&patch).unwrap_or_else(|_| json!({}));
    cx.spawn(async move |cx| {
        let result = engine
            .client()
            .call_as::<TelemetryStatus>(methods::SET_TELEMETRY_SETTINGS, params)
            .await;
        cx.update(|cx| match result {
            Ok(status) => apply_status(status, cx),
            Err(error) => {
                tracing::warn!(%error, "telemetry settings update failed");
                // The optimistic copy may be wrong now.
                refresh_status(cx);
            }
        });
    })
    .detach();
}

/// Dismiss the first-run notice for every viewport on this engine.
pub fn acknowledge_notice(cx: &mut App) {
    set_settings(
        TelemetrySettingsPatch {
            notice_acknowledged: Some(true),
            ..Default::default()
        },
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str) -> TrackedEvent {
        TrackedEvent::new(name, [("n", json!(1))]).expect("valid name")
    }

    #[test]
    fn snake_case_handles_camel_pascal_and_acronyms() {
        assert_eq!(snake_case("ToggleSidebar"), "toggle_sidebar");
        assert_eq!(
            snake_case("sidebarShowPullRequest"),
            "sidebar_show_pull_request"
        );
        assert_eq!(snake_case("OpenPRView"), "open_pr_view");
        assert_eq!(snake_case("codexVoice"), "codex_voice");
        assert_eq!(
            snake_case("filesAutosaveDelayMs"),
            "files_autosave_delay_ms"
        );
        assert_eq!(snake_case("already_snake"), "already_snake");
    }

    #[test]
    fn action_names_normalize_to_dotted_snake_case() {
        assert_eq!(
            normalize_action("shell::ToggleSidebar").as_deref(),
            Some("shell.toggle_sidebar")
        );
        assert_eq!(
            normalize_action("terminal::ToggleTerminal").as_deref(),
            Some("terminal.toggle_terminal")
        );
        assert_eq!(normalize_action("Weird Name!"), None);
        assert_eq!(normalize_action(""), None);
    }

    #[test]
    fn shortcuts_skip_text_editing_actions() {
        assert_eq!(
            shortcut_name("shell::NewSession").as_deref(),
            Some("shell.new_session")
        );
        assert_eq!(
            shortcut_name("composer::Submit").as_deref(),
            Some("composer.submit")
        );
        assert_eq!(shortcut_name("composer::Backspace"), None);
        assert_eq!(shortcut_name("composer::WordLeft"), None);
        assert_eq!(shortcut_name("input::MoveLeft"), None);
        assert_eq!(shortcut_name("NoNamespace"), None);
    }

    #[test]
    fn event_props_keep_only_what_the_engine_accepts() {
        let event = TrackedEvent::new(
            "demo",
            [
                ("ok_bool", json!(true)),
                ("ok_num", json!(3.5)),
                ("ok_token", json!("tab-a_1.2+x")),
                ("ok_list", json!(["a", 1, false])),
                ("path", json!("/home/me/secret")),
                ("spaces", json!("two words")),
                ("long", json!("x".repeat(65))),
                ("object", json!({"a": 1})),
                ("null", Value::Null),
                ("mixed_list", json!(["a", "b c"])),
                ("Bad-Key", json!(1)),
            ],
        )
        .unwrap();
        let mut keys: Vec<&str> = event.props.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["ok_bool", "ok_list", "ok_num", "ok_token"]);
    }

    #[test]
    fn event_names_must_be_identifiers() {
        assert!(TrackedEvent::new("panel.toggled", [("a", json!(1))]).is_some());
        assert!(TrackedEvent::new("Panel", [("a", json!(1))]).is_none());
        assert!(TrackedEvent::new("has space", [("a", json!(1))]).is_none());
        assert!(TrackedEvent::new(&"a".repeat(65), [("a", json!(1))]).is_none());
    }

    #[test]
    fn settings_changes_report_allowed_keys_and_values_only() {
        let before = json!({
            "pauseAnimationsInBackground": false,
            "sidebarOrganization": "inOneList",
            "sidebarWidth": 260.0,
            "windowGeometry": {"x": 1},
            "openTabs": ["chat-1"],
            "keymap": {"newSession": "mod-n"},
            "wallpaperFolder": "/home/me/pics",
            "lastSpaceId": "space-1",
            "themeSelection": {"light": "a"},
            "uiFontFamily": "geist",
            "filesAutosaveDelayMs": 800,
            "codeFontFamily": "geistMono",
            "unchanged": true,
        });
        let after = json!({
            "pauseAnimationsInBackground": true,
            "sidebarOrganization": "byProject",
            "sidebarWidth": 300.0,
            "windowGeometry": {"x": 2},
            "openTabs": ["chat-1", "chat-2"],
            "keymap": {"newSession": "mod-t"},
            "wallpaperFolder": "/home/me/other",
            "lastSpaceId": "space-2",
            "themeSelection": {"light": "b"},
            "uiFontFamily": "installed:My Private Font",
            "filesAutosaveDelayMs": 1200,
            "codeFontFamily": "geistMono",
            "unchanged": true,
        });
        let changes = settings_changes(&before, &after);
        assert_eq!(
            changes,
            vec![
                ("files_autosave_delay_ms".to_string(), Some(json!(1200))),
                (
                    "pause_animations_in_background".to_string(),
                    Some(json!(true))
                ),
                ("sidebar_organization".to_string(), Some(json!("byProject"))),
                ("ui_font_family".to_string(), Some(json!("installed"))),
            ]
        );
    }

    #[test]
    fn settings_changes_drop_values_that_are_not_scalars() {
        let before = json!({ "accent": {"kind": "custom", "hex": "#ff0000"}, "note": "free text" });
        let after =
            json!({ "accent": {"kind": "custom", "hex": "#00ff00"}, "note": "other free text" });
        assert_eq!(
            settings_changes(&before, &after),
            vec![("accent".to_string(), None), ("note".to_string(), None)]
        );
        assert!(settings_changes(&json!(null), &after).is_empty());
    }

    /// Every string or nullable field of `UiSettings` must be classified:
    /// an enum id (reportable) or skipped. A new free-form field fails here
    /// instead of silently shipping in `settings.snapshot`.
    #[test]
    fn every_string_setting_is_classified() {
        let defaults = serde_json::to_value(crate::settings::UiSettings::default()).unwrap();
        for (key, value) in defaults.as_object().unwrap() {
            if value.is_string() || value.is_null() {
                let snake = snake_case(key);
                assert!(
                    skip_setting(&snake) || ENUM_STRING_SETTINGS.contains(&snake.as_str()),
                    "classify UiSettings::{snake} in crates/ui/src/telemetry.rs"
                );
            }
        }
        let snapshot = settings_snapshot(&json!({"futureFreeText": "acme corp"}));
        assert!(snapshot.is_empty(), "unclassified strings stay out");
    }

    #[test]
    fn settings_snapshot_flattens_allowed_scalars_within_the_cap() {
        let mut map = Map::new();
        map.insert("soundEnabled".into(), json!(true));
        map.insert("appearance".into(), json!("dark"));
        map.insert("windowGeometry".into(), json!({"x": 1}));
        map.insert("wallpaperFolder".into(), json!("/home/me"));
        map.insert("openTabs".into(), json!(["a"]));
        map.insert("lastSpaceId".into(), json!("abc"));
        map.insert("transcriptWidth".into(), json!(760.0));
        map.insert("freeText".into(), json!("has spaces"));
        for ix in 0..80 {
            map.insert(format!("flag{ix:02}"), json!(true));
        }
        let snapshot = settings_snapshot(&Value::Object(map));
        assert_eq!(snapshot.len(), MAX_SNAPSHOT_PROPS);
        let keys: Vec<&str> = snapshot.iter().map(|(key, _)| key.as_str()).collect();
        for banned in [
            "window_geometry",
            "wallpaper_folder",
            "open_tabs",
            "last_space_id",
            "transcript_width",
            "free_text",
        ] {
            assert!(!keys.contains(&banned), "{banned} leaked");
        }
        assert!(keys.contains(&"appearance"));
        assert!(
            snapshot
                .iter()
                .all(|(_, v)| !v.is_object() && !v.is_array())
        );
    }

    #[test]
    fn queue_is_bounded_and_flags_when_a_send_is_due() {
        let mut queue = Queue::new(MAX_QUEUED);
        for ix in 0..FLUSH_THRESHOLD - 1 {
            assert!(!queue.push(event(&format!("e{ix}"))), "{ix}");
        }
        assert!(queue.push(event("trigger")));
        for _ in 0..MAX_QUEUED * 2 {
            queue.push(event("flood"));
        }
        assert_eq!(queue.len(), MAX_QUEUED);
        // Oldest dropped first: the earliest names are gone.
        assert!(queue.events.iter().all(|e| e.name == "flood"));
    }

    #[test]
    fn take_respects_the_batch_cap_and_restore_keeps_order() {
        let mut queue = Queue::new(10);
        for ix in 0..6 {
            queue.push(event(&format!("e{ix}")));
        }
        let batch = queue.take(4);
        assert_eq!(batch.len(), 4);
        assert_eq!(queue.len(), 2);
        queue.restore(batch);
        let names: Vec<&str> = queue.events.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["e0", "e1", "e2", "e3", "e4", "e5"]);
    }

    #[test]
    fn restore_never_exceeds_the_cap() {
        let mut queue = Queue::new(5);
        for ix in 0..5 {
            queue.push(event(&format!("old{ix}")));
        }
        let batch = queue.take(5);
        for ix in 0..3 {
            queue.push(event(&format!("new{ix}")));
        }
        queue.restore(batch);
        assert_eq!(queue.len(), 5);
        // Newer events win; the restored batch's oldest are the ones dropped.
        let names: Vec<&str> = queue.events.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["old3", "old4", "new0", "new1", "new2"]);
    }

    #[test]
    fn tracker_drops_events_when_not_collecting_or_unavailable() {
        let mut tracker = Tracker::new();
        tracker.record(event("a"));
        assert_eq!(tracker.queue.len(), 1);
        tracker.set_collecting(false);
        assert_eq!(
            tracker.queue.len(),
            0,
            "turning collection off clears the queue"
        );
        tracker.record(event("b"));
        assert_eq!(tracker.queue.len(), 0);
        tracker.set_collecting(true);
        tracker.record(event("c"));
        assert_eq!(tracker.queue.len(), 1);
        tracker.mark_unavailable();
        tracker.record(event("d"));
        assert_eq!(tracker.queue.len(), 0);
    }

    #[test]
    fn tracker_without_engine_buffers_but_does_not_start_a_send() {
        let mut tracker = Tracker::new();
        tracker.record(event("a"));
        assert!(tracker.begin_flush().is_none());
        assert_eq!(tracker.queue.len(), 1, "events wait for an engine");
    }

    #[test]
    fn failed_send_requeues_and_unknown_method_goes_quiet() {
        let mut tracker = Tracker::new();
        tracker.flushing = true;
        tracker.finish_flush(
            vec![event("a"), event("b")],
            Err(RpcError::Transport("down".into())),
        );
        assert!(!tracker.flushing);
        assert_eq!(tracker.queue.len(), 2);

        tracker.flushing = true;
        tracker.finish_flush(
            vec![event("c")],
            Err(RpcError::UnknownMethod("TrackEvents".into())),
        );
        assert!(!tracker.available);
        assert_eq!(tracker.queue.len(), 0);
        tracker.record(event("d"));
        assert_eq!(tracker.queue.len(), 0);
    }

    #[test]
    fn batch_params_carry_the_desktop_surface() {
        let params = batch_params(&[TrackedEvent::new(
            "panel.toggled",
            [("panel", json!("sidebar")), ("open", json!(true))],
        )
        .unwrap()]);
        assert_eq!(params["surface"], "desktop");
        assert_eq!(params["events"][0]["name"], "panel.toggled");
        assert_eq!(params["events"][0]["props"]["panel"], "sidebar");
        assert_eq!(params["events"][0]["props"]["open"], true);
    }

    fn status_with(collecting: bool, acknowledged: bool) -> TelemetryStatus {
        TelemetryStatus {
            settings: TelemetrySettings {
                enabled: true,
                usage_import: false,
                notice_acknowledged: acknowledged,
            },
            collecting,
            uploading: true,
            override_reason: None,
            distinct_id: "anon".into(),
            account_linked: false,
            endpoint: None,
            sent: 0,
            dropped: 0,
            last_error: None,
            recent: Vec::new(),
        }
    }

    #[test]
    fn notice_shows_only_while_collecting_and_unacknowledged() {
        assert!(!notice_needed(None));
        assert!(notice_needed(Some(&status_with(true, false))));
        assert!(!notice_needed(Some(&status_with(true, true))));
        assert!(!notice_needed(Some(&status_with(false, false))));
    }

    #[test]
    fn patches_only_touch_the_fields_they_name() {
        let mut settings = TelemetrySettings::default();
        apply_patch(
            &mut settings,
            &TelemetrySettingsPatch {
                notice_acknowledged: Some(true),
                ..Default::default()
            },
        );
        assert!(settings.enabled && settings.notice_acknowledged && !settings.usage_import);
        apply_patch(
            &mut settings,
            &TelemetrySettingsPatch {
                enabled: Some(false),
                usage_import: Some(true),
                ..Default::default()
            },
        );
        assert!(!settings.enabled && settings.usage_import && settings.notice_acknowledged);
    }

    #[gpui::test]
    fn notice_follows_the_engine_status(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Tracker::new());
            cx.set_global(TelemetryState::default());
            assert!(!notice_visible(cx), "hidden until the engine answers");
            apply_status(status_with(true, false), cx);
            assert!(notice_visible(cx));
            apply_status(status_with(true, true), cx);
            assert!(!notice_visible(cx), "acknowledged on any viewport hides it");
            apply_status(status_with(false, false), cx);
            assert!(
                !notice_visible(cx),
                "nothing to announce when not collecting"
            );
        });
    }

    #[gpui::test]
    fn tracking_follows_the_engine_collecting_flag(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            // Not installed: a silent no-op.
            track(cx, "demo.event", [("n", json!(1))]);

            cx.set_global(Tracker::new());
            cx.set_global(TelemetryState::default());
            let queued = |cx: &App| cx.global::<Tracker>().queue.len();
            track(cx, "demo.event", [("n", json!(1))]);
            assert_eq!(queued(cx), 1, "buffered while no engine is attached");
            // Invalid names and values are dropped before they queue.
            track(cx, "Not Valid", [("n", json!(1))]);
            assert_eq!(queued(cx), 1);

            apply_status(status_with(false, false), cx);
            assert_eq!(queued(cx), 0, "turning collection off discards the buffer");
            track(cx, "demo.event", [("n", json!(1))]);
            assert_eq!(queued(cx), 0);

            apply_status(status_with(true, false), cx);
            track(cx, "demo.event", [("n", json!(1))]);
            assert_eq!(queued(cx), 1);
            let params = drain_for_quit(cx).expect("final batch");
            assert_eq!(params["events"][0]["name"], "demo.event");
            assert!(drain_for_quit(cx).is_none());
        });
    }

    fn day(day: &str, source: &str, model: &str, requests: u64, cost: Option<f64>) -> DailyUsage {
        DailyUsage {
            day: day.into(),
            source: source.into(),
            model: model.into(),
            origin: "cli".into(),
            input_tokens: 100 * requests,
            output_tokens: 10 * requests,
            cache_read_tokens: 5 * requests,
            requests,
            cost_usd: cost,
            ..Default::default()
        }
    }

    #[test]
    fn usage_aggregates_the_last_thirty_days_per_source_and_model() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        let stats = UsageStats {
            import_enabled: true,
            external: vec![
                day("2026-10-10", "codex", "gpt-5", 2, Some(0.5)),
                day("2026-10-01", "codex", "gpt-5", 1, Some(0.25)),
                day("2026-09-11", "codex", "gpt-5", 4, None),
                day("2026-09-10", "codex", "gpt-5", 99, None),
                day("2026-10-02", "claude-code", "opus", 1, None),
            ],
            zeron: vec![day("2026-10-09", "codex", "gpt-5", 3, None)],
            last_scan_at: None,
        };
        let rows = aggregate_usage(&stats, today);
        assert_eq!(rows.len(), 3);
        let external = rows
            .iter()
            .find(|r| !r.in_zeron && r.source == "codex")
            .unwrap();
        assert_eq!(
            external.requests, 7,
            "the day outside the window is excluded"
        );
        assert_eq!(external.input_tokens, 700);
        assert_eq!(external.cost_usd, Some(0.75));
        let opus = rows.iter().find(|r| r.source == "claude-code").unwrap();
        assert_eq!(opus.cost_usd, None);
        assert!(rows.iter().any(|r| r.in_zeron && r.requests == 3));
        // Biggest first.
        assert_eq!(rows[0].requests, 7);
    }
}
