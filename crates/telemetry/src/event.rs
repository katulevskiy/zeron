//! The event model: a name plus a flat property map, built only through the
//! typed constructors in this crate (or [`Event::ui`], which sanitizes).
//!
//! Names are a closed catalog ([`names`]) so a dashboard query and the code
//! that feeds it can't drift. Properties are product metadata and normalized
//! measurements only — never prompts, paths, titles, URLs or credentials.

use std::borrow::Cow;

use serde::Serialize;
use serde_json::{Map, Value};

/// Every event name the engine emits. UI events arrive as `ui.<name>`.
pub mod names {
    pub const APP_STARTED: &str = "app.started";
    pub const APP_STOPPED: &str = "app.stopped";
    pub const APP_HEARTBEAT: &str = "app.heartbeat";
    pub const TURN_STARTED: &str = "turn.started";
    pub const TURN_FINISHED: &str = "turn.finished";
    pub const CHAT_CREATED: &str = "chat.created";
    pub const SPACE_CREATED: &str = "space.created";
    pub const REPO_ADDED: &str = "repo.added";
    pub const WORKTREE_CREATED: &str = "worktree.created";
    pub const WORKTREE_REMOVED: &str = "worktree.removed";
    pub const TERMINAL_OPENED: &str = "terminal.opened";
    pub const PROJECT_ACTION_RUN: &str = "project_action.run";
    pub const SOURCE_CONTROL_ACTION: &str = "source_control.action";
    pub const UPLOAD_COMMITTED: &str = "upload.committed";
    pub const HARNESS_INSTALLED: &str = "harness.installed";
    pub const HARNESS_TOGGLED: &str = "harness.toggled";
    pub const HARNESS_UPDATED: &str = "harness.updated";
    pub const LOCAL_IMPORT_FINISHED: &str = "local_import.finished";
    pub const AUTH_CHANGED: &str = "auth.changed";
    pub const AGENT_ACCOUNT_ACTIVATED: &str = "agent_account.activated";
    pub const REMOTE_RELAY: &str = "remote.relay";
    pub const ROLLUP: &str = "rollup";
    pub const EXTERNAL_USAGE_DAILY: &str = "external_usage.daily";
    pub const TELEMETRY_SETTINGS_CHANGED: &str = "telemetry.settings_changed";
    /// Prefix for viewport-reported events (`ui.settings_changed`, …).
    pub const UI_PREFIX: &str = "ui.";
}

/// One product event, before identity/common properties are stamped on.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub(crate) name: Cow<'static, str>,
    pub(crate) props: Map<String, Value>,
}

impl Event {
    pub fn new(name: &'static str) -> Self {
        Event {
            name: Cow::Borrowed(name),
            props: Map::new(),
        }
    }

    /// An event whose properties are a serializable struct's fields.
    /// Non-object payloads land under `value`.
    pub fn with_payload(name: &'static str, payload: &impl Serialize) -> Self {
        let props = match serde_json::to_value(payload) {
            Ok(Value::Object(map)) => map,
            Ok(Value::Null) | Err(_) => Map::new(),
            Ok(other) => Map::from_iter([("value".to_string(), other)]),
        };
        Event {
            name: Cow::Borrowed(name),
            props,
        }
    }

    /// Builder: sets one property.
    pub fn with(mut self, key: &'static str, value: impl Into<Value>) -> Self {
        self.props.insert(key.to_string(), value.into());
        self
    }

    /// Builder: sets one property when present; unknown stays absent.
    pub fn with_opt<V: Into<Value>>(self, key: &'static str, value: Option<V>) -> Self {
        match value {
            Some(v) => self.with(key, v),
            None => self,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn props(&self) -> &Map<String, Value> {
        &self.props
    }

    /// A viewport-reported event. Everything a client sends is untrusted:
    /// the name must be a short `snake.case` identifier and properties are
    /// filtered to scalars and short identifier-like strings, so a buggy or
    /// hostile client can't smuggle content (paths, URLs, prose) through.
    pub fn ui(name: &str, surface: &str, props: Map<String, Value>) -> Option<Self> {
        if !is_identifier(name, 64) {
            return None;
        }
        let mut clean = Map::new();
        for (key, value) in props.into_iter().take(MAX_UI_PROPS) {
            if !is_identifier(&key, 40) {
                continue;
            }
            if let Some(value) = sanitize_value(value) {
                clean.insert(key, value);
            }
        }
        clean.insert("surface".into(), Value::String(sanitize_surface(surface)));
        Some(Event {
            name: Cow::Owned(format!("{}{name}", names::UI_PREFIX)),
            props: clean,
        })
    }
}

const MAX_UI_PROPS: usize = 64;
const MAX_UI_STRING: usize = 64;
const MAX_UI_LIST: usize = 32;

fn is_identifier(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.'))
}

/// Short tokens only: identifiers, enum values, model ids, versions. Anything
/// with path separators, spaces, `@` or `:` (URLs, emails, prose) is dropped.
fn is_safe_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_UI_STRING
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
}

fn sanitize_value(value: Value) -> Option<Value> {
    match value {
        Value::Bool(_) | Value::Number(_) => Some(value),
        Value::String(s) if is_safe_token(&s) => Some(Value::String(s)),
        Value::Array(items) => {
            let items: Vec<Value> = items
                .into_iter()
                .filter_map(|v| match v {
                    Value::String(s) if is_safe_token(&s) => Some(Value::String(s)),
                    Value::Bool(_) | Value::Number(_) => Some(v),
                    _ => None,
                })
                .take(MAX_UI_LIST)
                .collect();
            Some(Value::Array(items))
        }
        _ => None,
    }
}

fn sanitize_surface(surface: &str) -> String {
    match surface {
        "desktop" | "ios" | "android" | "web" | "cli" | "mcp" => surface.to_string(),
        _ => "unknown".to_string(),
    }
}

/// Coarse, content-free classification of an error message. The message
/// itself never leaves the device.
pub fn classify_error(message: &str) -> &'static str {
    let m = message.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));
    if has(&[
        "rate limit",
        "rate_limit",
        "usage limit",
        "quota",
        "429",
        "overloaded",
    ]) {
        "rate_limit"
    } else if has(&[
        "unauthorized",
        "401",
        "403",
        "forbidden",
        "not logged in",
        "login",
        "auth",
        "api key",
        "credential",
    ]) {
        "auth"
    } else if has(&[
        "context length",
        "context window",
        "too long",
        "prompt is too long",
        "maximum context",
    ]) {
        "context_overflow"
    } else if has(&[
        "timed out",
        "timeout",
        "connection",
        "network",
        "dns",
        "econn",
        "socket",
        "unreachable",
    ]) {
        "network"
    } else if has(&[
        "not found",
        "no such file",
        "enoent",
        "not installed",
        "spawn",
    ]) {
        "missing_binary"
    } else if has(&["exited", "crash", "signal", "panicked", "killed"]) {
        "process_exit"
    } else {
        "other"
    }
}

/// Order-of-magnitude bucket for a size, so lengths are useful without being
/// fingerprintable.
pub fn bucket(n: u64) -> &'static str {
    match n {
        0 => "0",
        1..=99 => "1-99",
        100..=999 => "100-999",
        1_000..=9_999 => "1k-10k",
        10_000..=99_999 => "10k-100k",
        _ => "100k+",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ui_events_are_prefixed_and_sanitized() {
        let props = json!({
            "pane": "changes",
            "count": 3,
            "on": true,
            "path": "/home/me/secret.rs",
            "url": "https://example.com",
            "prose": "hello world",
            "BadKey": "x",
            "tags": ["a", "/etc/passwd", 2],
            "nested": {"a": 1},
        });
        let Value::Object(props) = props else {
            unreachable!()
        };
        let ev = Event::ui("panel.toggled", "desktop", props).unwrap();
        assert_eq!(ev.name(), "ui.panel.toggled");
        let p = ev.props();
        assert_eq!(p["pane"], "changes");
        assert_eq!(p["count"], 3);
        assert_eq!(p["on"], true);
        assert_eq!(p["tags"], json!(["a", 2]));
        assert_eq!(p["surface"], "desktop");
        for dropped in ["path", "url", "prose", "BadKey", "nested"] {
            assert!(!p.contains_key(dropped), "{dropped} leaked");
        }
    }

    #[test]
    fn ui_event_names_are_validated() {
        assert!(Event::ui("Bad Name", "desktop", Map::new()).is_none());
        assert!(Event::ui("", "desktop", Map::new()).is_none());
        assert!(Event::ui("a/b", "desktop", Map::new()).is_none());
        let ev = Event::ui("ok_name", "toaster", Map::new()).unwrap();
        assert_eq!(ev.props()["surface"], "unknown");
    }

    #[test]
    fn errors_classify_without_content() {
        assert_eq!(classify_error("Claude usage limit reached"), "rate_limit");
        assert_eq!(classify_error("HTTP 401 Unauthorized"), "auth");
        assert_eq!(
            classify_error("prompt is too long: 210000 tokens"),
            "context_overflow"
        );
        assert_eq!(classify_error("connection reset by peer"), "network");
        assert_eq!(classify_error("something odd"), "other");
    }

    #[test]
    fn buckets_are_coarse() {
        assert_eq!(bucket(0), "0");
        assert_eq!(bucket(42), "1-99");
        assert_eq!(bucket(4_200), "1k-10k");
        assert_eq!(bucket(1_000_000), "100k+");
    }
}
