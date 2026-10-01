//! The scout: a short, tool-less model run that reads the end of a chat's
//! conversation and says what else the work needs on the new device — files
//! the user mentioned but no tool touched yet ("use the specs in
//! ~/Documents/specs"), and advice for the agent once it lands ("the dev
//! server was running on :3000; restart it with `npm run dev`").
//!
//! It runs in the same isolation as automatic titles (a scratch folder, no
//! tools, the harness's cheapest model) while the workspace is already being
//! copied, has a minute, and is best-effort: a move never waits on it or
//! fails because of it. Everything it proposes is validated like a
//! harvested path (exists, not a secret store, size cap).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use zeron_doc::MessageRole;
use zeron_proto::{HarnessId, ReasoningLevel, RunRequest, SandboxLevel};

use crate::registry::HarnessRegistry;

const TIMEOUT: Duration = Duration::from_secs(60);
/// Conversation text the scout reads, newest last.
const TRANSCRIPT_BUDGET: usize = 24_000;
const MAX_NOTES: usize = 6;
const MAX_NOTE_CHARS: usize = 300;
const MAX_INCLUDES: usize = 24;

const INSTRUCTIONS: &str = "\
You are helping move an AI coding agent's session from one computer to another. \
The project folder is copied in full already, and so are the files listed under \
\"Already carried\". Read the end of the conversation and decide:
1. Which OTHER files or folders outside the project the agent will still need on \
the new computer (things the user pointed it at, inputs it was about to use, \
outputs it produced elsewhere). Only absolute paths or paths starting with ~/. \
Never credentials, keys or whole home folders.
2. Short notes for the agent after the move: processes it should restart, setup \
it must redo (installing dependencies, starting services), anything it was in the \
middle of. At most 6 notes, one sentence each.
Reply with JSON only, no prose and no code fence:
{\"include\":[{\"path\":\"~/Documents/specs\",\"reason\":\"…\"}],\"notes\":[\"…\"]}";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScoutAdvice {
    pub include: Vec<PathBuf>,
    pub notes: Vec<String>,
}

#[derive(Deserialize)]
struct RawAdvice {
    #[serde(default)]
    include: Vec<RawInclude>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawInclude {
    Path(String),
    Item { path: String },
}

pub struct ScoutInput<'a> {
    pub harness: HarnessId,
    pub workspace_root: Option<&'a Path>,
    pub carried: &'a [PathBuf],
    pub processes: &'a [String],
    pub transcript: &'a [zeron_doc::SessionMessageEntry],
}

/// Ask the scout. `None` = no title-capable harness here, a timeout, or an
/// answer that didn't parse — the move goes on with what it has.
pub async fn ask(registry: &Arc<HarnessRegistry>, input: ScoutInput<'_>) -> Option<ScoutAdvice> {
    let harness_id = pick_harness(registry, input.harness)?;
    let harness = registry.resolve(harness_id).ok()?;
    let lease = Arc::new(registry.execution_lease(harness_id).await);
    let model = tokio::time::timeout(
        Duration::from_secs(10),
        registry.discover_models_with_lease(harness_id, lease.clone()),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .and_then(|models| crate::titles::cheapest_model(&models));
    let scratch = tempfile::tempdir().ok()?;
    let request = RunRequest {
        mcp: None,
        prompt: prompt(&input),
        harness: Some(harness_id),
        model,
        reasoning: Some(ReasoningLevel::Low),
        model_options: serde_json::Map::new(),
        cwd: scratch.path().to_string_lossy().into_owned(),
        sandbox: SandboxLevel::ReadOnly,
        auto_approve: false,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
    };
    let raw = tokio::time::timeout(
        TIMEOUT,
        crate::titles::collect_text(harness.as_ref(), request, Some(lease)),
    )
    .await
    .ok()?
    .map_err(|error| tracing::debug!(%error, "move scout run failed"))
    .ok()?;
    parse(&raw)
}

fn pick_harness(registry: &HarnessRegistry, chat_harness: HarnessId) -> Option<HarnessId> {
    if zeron_harness::supports_titles(chat_harness) && chat_harness != HarnessId::Mock {
        return Some(chat_harness);
    }
    if chat_harness == HarnessId::Mock {
        // Tests drive the mock end to end.
        return Some(chat_harness);
    }
    registry
        .enabled_set()
        .into_iter()
        .find(|id| zeron_harness::supports_titles(*id) && *id != HarnessId::Mock)
}

fn prompt(input: &ScoutInput<'_>) -> String {
    let mut out = String::from(INSTRUCTIONS);
    out.push_str("\n\nProject folder: ");
    match input.workspace_root {
        Some(root) => out.push_str(&root.display().to_string()),
        None => out.push_str("(none: only the files below travel)"),
    }
    out.push_str("\nAlready carried:\n");
    if input.carried.is_empty() {
        out.push_str("(nothing else)\n");
    }
    for path in input.carried.iter().take(200) {
        out.push_str("- ");
        out.push_str(&path.display().to_string());
        out.push('\n');
    }
    if !input.processes.is_empty() {
        out.push_str("Processes running in the project on the old computer (they do not move):\n");
        for process in input.processes.iter().take(20) {
            out.push_str("- ");
            out.push_str(process);
            out.push('\n');
        }
    }
    out.push_str("\nEnd of the conversation (JSON, oldest first):\n");
    out.push_str(&transcript_tail(input.transcript, TRANSCRIPT_BUDGET));
    out
}

/// The newest messages that fit `budget` characters, as `[{role, text}]`.
fn transcript_tail(entries: &[zeron_doc::SessionMessageEntry], budget: usize) -> String {
    let mut picked: Vec<serde_json::Value> = Vec::new();
    let mut used = 0usize;
    for entry in entries.iter().rev() {
        let mut text = String::new();
        for part in &entry.parts {
            match part {
                zeron_doc::MessagePart::Text { text: body, .. } => {
                    text.push_str(body);
                    text.push('\n');
                }
                zeron_doc::MessagePart::Tool { call, .. } => {
                    text.push_str("[tool] ");
                    text.push_str(&serde_json::to_string(call).unwrap_or_default());
                    text.push('\n');
                }
                _ => {}
            }
        }
        if text.trim().is_empty() {
            continue;
        }
        let text: String = text.chars().take(4000).collect();
        used += text.len();
        if used > budget && !picked.is_empty() {
            break;
        }
        let role = match entry.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            _ => "system",
        };
        picked.push(serde_json::json!({ "role": role, "text": text }));
    }
    picked.reverse();
    serde_json::to_string(&picked).unwrap_or_default()
}

/// The scout's JSON, found even when wrapped in a code fence or prose.
fn parse(raw: &str) -> Option<ScoutAdvice> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let raw: RawAdvice = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let include = raw
        .include
        .into_iter()
        .map(|item| match item {
            RawInclude::Path(path) | RawInclude::Item { path } => PathBuf::from(path.trim()),
        })
        .filter(|path| !path.as_os_str().is_empty())
        .take(MAX_INCLUDES)
        .collect();
    let notes = raw
        .notes
        .into_iter()
        .map(|note| note.trim().chars().take(MAX_NOTE_CHARS).collect::<String>())
        .filter(|note| !note.is_empty())
        .take(MAX_NOTES)
        .collect();
    Some(ScoutAdvice { include, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advice_parses_through_fences_and_both_include_shapes() {
        let raw = "Sure:\n```json\n{\"include\":[{\"path\":\"~/Documents/specs\",\"reason\":\"inputs\"},\"/data/x.csv\",{\"path\":\"  \"}],\"notes\":[\"Restart the dev server with npm run dev.\",\"  \"]}\n```";
        let advice = parse(raw).unwrap();
        assert_eq!(
            advice.include,
            vec![PathBuf::from("~/Documents/specs"), PathBuf::from("/data/x.csv")]
        );
        assert_eq!(advice.notes, vec!["Restart the dev server with npm run dev."]);
        assert!(parse("no json here").is_none());
    }

    #[test]
    fn the_transcript_tail_keeps_the_newest_messages_within_budget() {
        let entry = |id: &str, role, text: &str| zeron_doc::SessionMessageEntry {
            duration_ms: None,
            id: id.into(),
            role,
            parts: vec![zeron_doc::MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 0,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
        };
        let entries = vec![
            entry("1", MessageRole::User, &"old ".repeat(100)),
            entry("2", MessageRole::Assistant, "middle"),
            entry("3", MessageRole::User, "newest"),
        ];
        let tail = transcript_tail(&entries, 50);
        assert!(tail.contains("newest") && tail.contains("middle"));
        assert!(!tail.contains("old old"));
        let order: Vec<serde_json::Value> = serde_json::from_str(&tail).unwrap();
        assert_eq!(order[0]["text"], "middle\n");
    }
}
