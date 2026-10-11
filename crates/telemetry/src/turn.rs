//! Per-turn analytics: folds the harness `AgentEvent` stream into one
//! `turn.finished` event — outcome, latency, tool mix, subagents and token
//! usage — without retaining any content.
//!
//! Token semantics follow the T3 Code lessons: unknown counts stay absent
//! (never zero-filled), main-agent and subagent usage are kept apart, and
//! `usage_complete` marks the turns that are safe for per-model cost
//! comparisons (usage reported, no subagents, one model).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, ToolCall};

use crate::event::{Event, bucket, classify_error, names};

/// What was asked for, captured when the turn is dispatched.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnMeta {
    pub harness: String,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub sandbox: String,
    pub auto_approve: bool,
    pub attachments: usize,
    pub worktree: bool,
    pub resumed: bool,
    pub zeron_mcp: bool,
    pub prompt_chars: usize,
    /// Harness option selections (`fast=on`, …) — ids only, filtered to
    /// short tokens.
    pub model_options: Map<String, Value>,
}

impl TurnMeta {
    pub fn from_request(harness: HarnessId, request: &RunRequest) -> Self {
        let model_options = request
            .model_options
            .iter()
            .filter(|(k, v)| short_token(k) && v.as_str().is_none_or(short_token))
            .take(16)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        TurnMeta {
            harness: enum_str(&harness),
            model: request.model.clone().filter(|m| !m.is_empty()),
            reasoning: request.reasoning.as_ref().map(enum_str),
            sandbox: enum_str(&request.sandbox),
            auto_approve: request.auto_approve,
            attachments: request.attachments.len(),
            worktree: request.worktree.is_some(),
            resumed: request.resume.is_some(),
            zeron_mcp: request.mcp.is_some(),
            prompt_chars: request.prompt.chars().count(),
            model_options,
        }
    }

    fn apply(&self, mut ev: Event) -> Event {
        ev = ev
            .with("harness", self.harness.clone())
            .with_opt("reasoning", self.reasoning.clone())
            .with("sandbox", self.sandbox.clone())
            .with("auto_approve", self.auto_approve)
            .with("attachments", self.attachments as u64)
            .with("worktree", self.worktree)
            .with("resumed", self.resumed)
            .with("zeron_mcp", self.zeron_mcp)
            .with("prompt_size", bucket(self.prompt_chars as u64));
        if !self.model_options.is_empty() {
            ev = ev.with("model_options", Value::Object(self.model_options.clone()));
        }
        ev
    }
}

/// Serde string form of a unit enum (`"claude-code"`, `"workspace-write"`).
fn enum_str(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        _ => "unknown".to_string(),
    }
}

fn short_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Tokens {
    reports: u32,
    input: u64,
    output: u64,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    reasoning: Option<u64>,
    cost_usd: Option<f64>,
}

impl Tokens {
    fn add(&mut self, ev: &AgentEvent) {
        let AgentEvent::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            cost_usd,
        } = ev
        else {
            return;
        };
        self.reports += 1;
        self.input += input_tokens;
        self.output += output_tokens;
        add_opt(&mut self.cache_read, *cache_read_tokens);
        add_opt(&mut self.cache_write, *cache_write_tokens);
        add_opt(&mut self.reasoning, *reasoning_tokens);
        if let Some(cost) = cost_usd {
            *self.cost_usd.get_or_insert(0.0) += cost;
        }
    }
}

fn add_opt(slot: &mut Option<u64>, value: Option<u64>) {
    if let Some(v) = value {
        *slot.get_or_insert(0) += v;
    }
}

/// Folds one turn. Feed every event with [`TurnTracker::observe`], then
/// [`TurnTracker::finish`] once with the final status.
#[derive(Debug)]
pub struct TurnTracker {
    meta: TurnMeta,
    started: Instant,
    first_output: Option<Duration>,
    resolved_model: Option<String>,
    tools: BTreeMap<String, u32>,
    tool_calls: u32,
    tool_errors: u32,
    subagents_spawned: u32,
    subagent_events: u32,
    subagent_models: BTreeMap<String, u32>,
    steers: u32,
    input_requests: u32,
    error_events: u32,
    error_kind: Option<&'static str>,
    generated_images: u32,
    output_chars: u64,
    reasoning_chars: u64,
    usage: Tokens,
    subagent_usage: Tokens,
    context_tokens: Option<u64>,
    context_window: Option<u64>,
}

impl TurnTracker {
    pub fn new(meta: TurnMeta) -> Self {
        Self::started_at(meta, Instant::now())
    }

    pub fn started_at(meta: TurnMeta, started: Instant) -> Self {
        TurnTracker {
            meta,
            started,
            first_output: None,
            resolved_model: None,
            tools: BTreeMap::new(),
            tool_calls: 0,
            tool_errors: 0,
            subagents_spawned: 0,
            subagent_events: 0,
            subagent_models: BTreeMap::new(),
            steers: 0,
            input_requests: 0,
            error_events: 0,
            error_kind: None,
            generated_images: 0,
            output_chars: 0,
            reasoning_chars: 0,
            usage: Tokens::default(),
            subagent_usage: Tokens::default(),
            context_tokens: None,
            context_window: None,
        }
    }

    /// The `turn.started` event for this turn.
    pub fn started_event(&self) -> Event {
        self.meta
            .apply(Event::new(names::TURN_STARTED))
            .with_opt("model", self.meta.model.clone())
    }

    pub fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::SessionStarted { model, .. } => {
                if !model.is_empty() {
                    self.resolved_model = Some(model.clone());
                }
            }
            AgentEvent::TextDelta { text } => {
                self.mark_output();
                self.output_chars += text.len() as u64;
            }
            AgentEvent::ReasoningDelta { text } => {
                self.mark_output();
                self.reasoning_chars += text.len() as u64;
            }
            AgentEvent::GeneratedImage { .. } => self.generated_images += 1,
            AgentEvent::ToolCall { call, .. } => {
                self.mark_output();
                self.tool_calls += 1;
                if call.is_subagent_spawn() {
                    self.subagents_spawned += 1;
                    if let Some(model) = call.subagent_model() {
                        *self.subagent_models.entry(model.to_string()).or_default() += 1;
                    }
                }
                *self.tools.entry(tool_kind(call)).or_default() += 1;
            }
            AgentEvent::ToolResult { is_error, .. } => {
                if *is_error {
                    self.tool_errors += 1;
                }
            }
            AgentEvent::ContextUsage { tokens, window } => {
                if tokens.is_some() {
                    self.context_tokens = *tokens;
                }
                if window.is_some() {
                    self.context_window = *window;
                }
            }
            AgentEvent::Usage { .. } => self.usage.add(event),
            AgentEvent::Error { message } => {
                self.error_events += 1;
                self.error_kind.get_or_insert(classify_error(message));
            }
            AgentEvent::InputRequested { .. } => self.input_requests += 1,
            AgentEvent::Steered { .. } => self.steers += 1,
            AgentEvent::Done { error, .. } => {
                if let Some(error) = error {
                    self.error_kind.get_or_insert(classify_error(error));
                }
            }
            AgentEvent::Subagent { event, .. } => {
                self.subagent_events += 1;
                if matches!(**event, AgentEvent::Usage { .. }) {
                    self.subagent_usage.add(event);
                }
            }
            AgentEvent::AssistantMessageCompleted { .. }
            | AgentEvent::AvailableCommands { .. }
            | AgentEvent::InputResolved { .. }
            | AgentEvent::UserMessage { .. } => {}
        }
    }

    fn mark_output(&mut self) {
        if self.first_output.is_none() {
            self.first_output = Some(self.started.elapsed());
        }
    }

    /// The `turn.finished` event.
    pub fn finish(self, status: DoneStatus) -> Event {
        let outcome = match status {
            DoneStatus::Completed => "completed",
            DoneStatus::Interrupted => "interrupted",
            DoneStatus::Errored => "errored",
        };
        let model = self
            .resolved_model
            .clone()
            .or_else(|| self.meta.model.clone());
        let mut ev = self
            .meta
            .apply(Event::new(names::TURN_FINISHED))
            .with("outcome", outcome)
            .with_opt("model", model.clone())
            .with("duration_ms", self.started.elapsed().as_millis() as u64)
            .with_opt("ttft_ms", self.first_output.map(|d| d.as_millis() as u64))
            .with("tool_calls", self.tool_calls)
            .with("tool_errors", self.tool_errors)
            .with("subagents_spawned", self.subagents_spawned)
            .with("subagent_events", self.subagent_events)
            .with("steers", self.steers)
            .with("input_requests", self.input_requests)
            .with("error_events", self.error_events)
            .with_opt("error_kind", self.error_kind)
            .with("generated_images", self.generated_images)
            .with("output_chars", self.output_chars)
            .with("reasoning_chars", self.reasoning_chars)
            .with_opt("context_tokens", self.context_tokens)
            .with_opt("context_window", self.context_window);
        if let (Some(tokens), Some(window)) = (self.context_tokens, self.context_window)
            && window > 0
        {
            let pct = (tokens as f64 / window as f64 * 1000.0).round() / 10.0;
            ev = ev.with("context_pct", pct);
        }
        if model.is_some() && self.meta.model.is_some() && model != self.meta.model {
            ev = ev.with_opt("model_requested", self.meta.model.clone());
        }
        if !self.tools.is_empty() {
            let tools: Map<String, Value> = self
                .tools
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect();
            ev = ev.with("tools", Value::Object(tools));
        }
        if !self.subagent_models.is_empty() {
            let models: Map<String, Value> = self
                .subagent_models
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect();
            ev = ev.with("subagent_models", Value::Object(models));
        }
        let mixed_models = self.meta.model.is_some()
            && self.resolved_model.is_some()
            && self.meta.model != self.resolved_model;
        ev = ev.with("usage_reported", self.usage.reports > 0).with(
            "usage_complete",
            self.usage.reports > 0 && self.subagents_spawned == 0 && !mixed_models,
        );
        if self.usage.reports > 0 {
            ev = with_tokens(ev, "", &self.usage);
        }
        if self.subagent_usage.reports > 0 {
            ev = with_tokens(ev, "subagent_", &self.subagent_usage);
        }
        ev
    }
}

fn with_tokens(ev: Event, prefix: &'static str, t: &Tokens) -> Event {
    // Static keys keep the catalog greppable.
    let key = |name: &'static str| -> &'static str {
        match (prefix, name) {
            ("", n) => n,
            (_, "input_tokens") => "subagent_input_tokens",
            (_, "output_tokens") => "subagent_output_tokens",
            (_, "cache_read_tokens") => "subagent_cache_read_tokens",
            (_, "cache_write_tokens") => "subagent_cache_write_tokens",
            (_, "reasoning_tokens") => "subagent_reasoning_tokens",
            (_, "cost_usd") => "subagent_cost_usd",
            (_, n) => n,
        }
    };
    let total = t.input + t.output + t.cache_read.unwrap_or(0) + t.cache_write.unwrap_or(0);
    let mut ev = ev
        .with(key("input_tokens"), t.input)
        .with(key("output_tokens"), t.output)
        .with_opt(key("cache_read_tokens"), t.cache_read)
        .with_opt(key("cache_write_tokens"), t.cache_write)
        .with_opt(key("reasoning_tokens"), t.reasoning)
        .with_opt(key("cost_usd"), t.cost_usd);
    if prefix.is_empty() {
        ev = ev.with("total_tokens", total);
    }
    ev
}

/// Content-free tool family. MCP tools collapse to `mcp` — their server and
/// tool names are user configuration and may be private.
pub fn tool_kind(call: &ToolCall) -> String {
    let kind = match call {
        ToolCall::Exec { .. } => "exec",
        ToolCall::ReadFile { .. } => "read",
        ToolCall::WriteFile { .. } => "write",
        ToolCall::EditFile { .. } => "edit",
        ToolCall::ApplyPatch { .. } => "patch",
        ToolCall::Search { .. } => "search",
        ToolCall::Glob { .. } => "glob",
        ToolCall::WebFetch { .. } => "web_fetch",
        ToolCall::WebSearch { .. } => "web_search",
        ToolCall::Todo { .. } => "todo",
        ToolCall::Mcp { .. } => "mcp",
        ToolCall::Unknown { name, .. } => {
            if call.is_subagent_spawn() {
                return "agent".to_string();
            }
            let lower = name.to_ascii_lowercase();
            if lower.starts_with("mcp") || lower.contains("__") {
                return "mcp".to_string();
            }
            return if short_token(&lower) && lower.len() <= 32 {
                format!("other.{lower}")
            } else {
                "other".to_string()
            };
        }
    };
    kind.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::SandboxLevel;

    fn request() -> RunRequest {
        serde_json::from_value(serde_json::json!({
            "prompt": "fix the bug in /secret/path.rs",
            "model": "claude-sonnet-4-5",
            "reasoning": "high",
            "modelOptions": {"fast": "on", "evil": "/etc/passwd"},
            "cwd": "/home/me/project",
            "sandbox": "workspace-write",
            "autoApprove": true,
            "resume": null,
            "attachments": ["/tmp/a.png"],
        }))
        .unwrap()
    }

    #[test]
    fn meta_captures_settings_but_no_content() {
        let meta = TurnMeta::from_request(HarnessId::ClaudeCode, &request());
        assert_eq!(meta.harness, "claude-code");
        assert_eq!(meta.reasoning.as_deref(), Some("high"));
        assert_eq!(meta.sandbox, enum_str(&SandboxLevel::WorkspaceWrite));
        assert_eq!(meta.attachments, 1);
        assert_eq!(
            meta.model_options.len(),
            1,
            "path-like option values dropped"
        );
        let started = TurnTracker::new(meta).started_event();
        let json = serde_json::to_string(started.props()).unwrap();
        assert!(!json.contains("secret"), "prompt leaked: {json}");
        assert!(!json.contains("/home/me"), "cwd leaked: {json}");
    }

    #[test]
    fn folds_a_turn() {
        let meta = TurnMeta::from_request(HarnessId::ClaudeCode, &request());
        let mut t = TurnTracker::new(meta);
        t.observe(&AgentEvent::SessionStarted {
            harness: HarnessId::ClaudeCode,
            model: "claude-sonnet-4-5".into(),
            tools: vec![],
            cwd: "/x".into(),
            session_id: "s".into(),
            assistant_message_id: "a".into(),
        });
        t.observe(&AgentEvent::TextDelta {
            text: "hello".into(),
        });
        t.observe(&AgentEvent::ToolCall {
            id: "1".into(),
            call: ToolCall::Exec {
                command: "rm -rf /".into(),
            },
        });
        t.observe(&AgentEvent::ToolResult {
            id: "1".into(),
            is_error: true,
            output: None,
            diff: None,
        });
        t.observe(&AgentEvent::ContextUsage {
            tokens: Some(50_000),
            window: Some(200_000),
        });
        t.observe(&AgentEvent::usage(100, 20).with_cache(Some(1000), Some(50)));
        t.observe(&AgentEvent::usage(10, 2).with_cost_usd(Some(0.01)));
        let ev = t.finish(DoneStatus::Completed);
        let p = ev.props();
        assert_eq!(ev.name(), names::TURN_FINISHED);
        assert_eq!(p["outcome"], "completed");
        assert_eq!(p["model"], "claude-sonnet-4-5");
        assert_eq!(p["tools"]["exec"], 1);
        assert_eq!(p["tool_errors"], 1);
        assert_eq!(p["input_tokens"], 110);
        assert_eq!(p["output_tokens"], 22);
        assert_eq!(p["cache_read_tokens"], 1000);
        assert_eq!(p["cache_write_tokens"], 50);
        assert_eq!(p["total_tokens"], 110 + 22 + 1000 + 50);
        assert_eq!(p["cost_usd"], 0.01);
        assert_eq!(p["context_pct"], 25.0);
        assert_eq!(p["usage_complete"], true);
        assert!(!p.contains_key("reasoning_tokens"), "unknown stays absent");
        assert!(p.contains_key("ttft_ms"));
        let json = serde_json::to_string(p).unwrap();
        assert!(!json.contains("rm -rf"), "tool input leaked");
        assert!(!json.contains("hello"), "text leaked");
    }

    #[test]
    fn subagents_split_usage_and_mark_incomplete() {
        let meta = TurnMeta::from_request(HarnessId::ClaudeCode, &request());
        let mut t = TurnTracker::new(meta);
        t.observe(&AgentEvent::ToolCall {
            id: "1".into(),
            call: ToolCall::Unknown {
                name: "Agent".into(),
                input: None,
            },
        });
        t.observe(&AgentEvent::Subagent {
            parent_tool_use_id: "1".into(),
            event: Box::new(AgentEvent::usage(7, 3)),
        });
        let p = t.finish(DoneStatus::Errored).props().clone();
        assert_eq!(p["subagents_spawned"], 1);
        assert_eq!(p["tools"]["agent"], 1);
        assert_eq!(p["subagent_input_tokens"], 7);
        assert_eq!(p["usage_reported"], false);
        assert_eq!(p["usage_complete"], false);
        assert!(!p.contains_key("input_tokens"));
    }

    #[test]
    fn errors_are_classified_not_copied() {
        let meta = TurnMeta::from_request(HarnessId::Codex, &request());
        let mut t = TurnTracker::new(meta);
        t.observe(&AgentEvent::Error {
            message: "429 rate limit for org secret-org".into(),
        });
        let p = t.finish(DoneStatus::Errored).props().clone();
        assert_eq!(p["error_kind"], "rate_limit");
        assert!(!serde_json::to_string(&p).unwrap().contains("secret-org"));
    }

    #[test]
    fn mcp_and_unknown_tools_are_generalized() {
        let mcp = ToolCall::Unknown {
            name: "mcp__private_server__do_thing".into(),
            input: None,
        };
        assert_eq!(tool_kind(&mcp), "mcp");
        let other = ToolCall::Unknown {
            name: "NotebookEdit".into(),
            input: None,
        };
        assert_eq!(tool_kind(&other), "other.notebookedit");
        let weird = ToolCall::Unknown {
            name: "has spaces/and slashes".into(),
            input: None,
        };
        assert_eq!(tool_kind(&weird), "other");
    }
}
