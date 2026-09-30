//! Zeron MCP `create_chat` / `create_chats` calls in the spawner's
//! transcript: which tool calls they are, and which chats they created.
//!
//! Every harness names MCP tools its own way — Claude `mcp__zeron__create_chat`
//! (decoded to `ToolCall::Mcp { server: "zeron", .. }`), Pi `zeron_create_chat`
//! (also decoded to `Mcp`), Codex/Cursor `Mcp { server, tool }`, OpenCode
//! `zeron_create_chat` and ACP titles such as `zeron/create_chat` or
//! `create_chat (zeron MCP Server)` (both left as `ToolCall::Unknown`) — so
//! detection keys on the tool token plus a mention of the Zeron server,
//! never on one spelling.
//!
//! The created chat ids come from the tool result JSON (`chatId`, or
//! `results[].result.chatId` for the batch) when a result is on hand. The
//! doc fold keeps tool OUTPUTS out of the doc (the text lives only in the
//! host's run journal), so a synced transcript usually has none; then the
//! chats' own provenance (`spawnedByChatId`) is matched back to the calls by
//! time: [`attribute_spawned_chats`].

use std::collections::{HashMap, HashSet};

use zeron_doc::{MessagePart, SessionMessageEntry};
use zeron_proto::ToolCall;

/// Which chat-creation tool a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateChatOp {
    /// `create_chat`: one chat.
    Single,
    /// `create_chats`: a batch, any number of chats.
    Batch,
}

/// `Some` when `call` is the Zeron MCP server's `create_chat`/`create_chats`
/// under any harness's naming scheme.
pub(crate) fn create_chat_op(call: &ToolCall) -> Option<CreateChatOp> {
    let (server, name) = match call {
        ToolCall::Mcp { server, tool, .. } => (Some(server.as_str()), tool.as_str()),
        ToolCall::Unknown { name, .. } => (None, name.as_str()),
        _ => return None,
    };
    let name = name.trim().to_ascii_lowercase().replace('-', "_");
    let (op, bare) = find_op(&name)?;
    let zeron = name.contains("zeron")
        || server.is_some_and(|server| server.to_ascii_lowercase().contains("zeron"));
    // A bare `create_chat` with no server at all (an ACP title that dropped
    // it) is still ours; another MCP server's `create_chat` is not.
    (zeron || (bare && server.is_none())).then_some(op)
}

/// The op token inside a normalized tool name, and whether the name is
/// nothing but the token. The token must stand alone: `create_chat` inside
/// `create_chats` or `create_chat_room` does not count.
fn find_op(name: &str) -> Option<(CreateChatOp, bool)> {
    for (token, op) in [
        ("create_chats", CreateChatOp::Batch),
        ("create_chat", CreateChatOp::Single),
    ] {
        let mut from = 0;
        while let Some(pos) = name[from..].find(token) {
            let start = from + pos;
            let end = start + token.len();
            let before = name[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            let after = name[end..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
            if before && after {
                return Some((op, name == token));
            }
            from = start + 1;
        }
    }
    None
}

/// One chat a create call reported in its result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CreatedChat {
    pub chat_id: String,
    /// `kind: "side"` (or a parent id) → `Some(true)`; `kind: "chat"` →
    /// `Some(false)`; unknown when the result predates `kind`.
    pub side: Option<bool>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub title: Option<String>,
}

/// The chats a `create_chat`/`create_chats` result names, in result order.
/// Accepts the bare result object, the batch's `{results: [{isError,
/// result}]}`, an MCP `CallToolResult` wrapper (`structuredContent`, or
/// `content[].text` holding the JSON), and fenced or double-encoded text.
/// Failed batch entries are skipped; anything unparseable yields nothing.
pub(crate) fn parse_created_chats(output: &str) -> Vec<CreatedChat> {
    let text: String = output
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        collect(&value, &mut out, 0);
    }
    let mut seen = HashSet::new();
    out.retain(|chat: &CreatedChat| seen.insert(chat.chat_id.clone()));
    out
}

fn collect(value: &serde_json::Value, out: &mut Vec<CreatedChat>, depth: usize) {
    use serde_json::Value;
    if depth > 6 {
        return;
    }
    match value {
        Value::Object(map) => {
            if map.get("isError").and_then(Value::as_bool) == Some(true) {
                return;
            }
            if let Some(chat_id) = map
                .get("chatId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                let text = |key: &str| {
                    map.get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                };
                let side = match map.get("kind").and_then(Value::as_str) {
                    Some("side") => Some(true),
                    Some("chat") => Some(false),
                    _ => map
                        .get("parentChatId")
                        .map(|parent| !parent.is_null())
                        .filter(|side| *side),
                };
                out.push(CreatedChat {
                    chat_id: chat_id.to_owned(),
                    side,
                    device_id: text("deviceId"),
                    device_name: text("deviceName"),
                    title: text("title"),
                });
                return;
            }
            for key in ["structuredContent", "results", "result"] {
                if let Some(inner) = map.get(key) {
                    let before = out.len();
                    collect(inner, out, depth + 1);
                    if out.len() > before {
                        return;
                    }
                }
            }
            if let Some(Value::Array(content)) = map.get("content") {
                for part in content {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        collect(&Value::String(text.to_owned()), out, depth + 1);
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, out, depth + 1);
            }
        }
        Value::String(text) => {
            if let Ok(inner) = serde_json::from_str::<Value>(text.trim()) {
                collect(&inner, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// A settled, successful create call in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateCallSite {
    pub part_id: String,
    pub entry_id: String,
    /// The carrying message's start (epoch ms).
    pub at_ms: i64,
    pub op: CreateChatOp,
}

/// The create calls in `entries` whose chats must be recovered from
/// provenance, and the chat ids other calls' results already name (those
/// are spoken for). Running and failed calls create nothing to link.
pub(crate) fn create_call_sites(
    entries: &[SessionMessageEntry],
) -> (Vec<CreateCallSite>, HashSet<String>) {
    let mut sites = Vec::new();
    let mut claimed = HashSet::new();
    for entry in entries {
        for part in &entry.parts {
            let MessagePart::Tool {
                id,
                call,
                is_error,
                resolved,
                output,
                ..
            } = part
            else {
                continue;
            };
            let Some(op) = create_chat_op(call) else {
                continue;
            };
            if !*resolved || *is_error {
                continue;
            }
            let named = output.as_deref().map(parse_created_chats).unwrap_or_default();
            if named.is_empty() {
                sites.push(CreateCallSite {
                    part_id: id.clone(),
                    entry_id: entry.id.clone(),
                    at_ms: entry.created_at,
                    op,
                });
            } else {
                claimed.extend(named.into_iter().map(|chat| chat.chat_id));
            }
        }
    }
    (sites, claimed)
}

/// Match the chats a spawner's agent created (`spawned`: `(chat id,
/// created_at ms)` of every chat whose `spawnedByChatId` is the spawner) to
/// the create calls in its transcript, keyed by the call's part id.
///
/// A chat belongs to the last message that started at or before it was
/// created. Within one message, `create_chat` calls take one chat each in
/// order; a single `create_chats` batch takes whatever the `create_chat`s
/// around it leave. Anything ambiguous (two batches in one message) stays
/// unlinked rather than guessed — the card then falls back to the plain
/// chip. Deleted chats simply leave their call unlinked.
pub(crate) fn attribute_spawned_chats(
    sites: &[CreateCallSite],
    spawned: &[(String, i64)],
) -> HashMap<String, Vec<String>> {
    // Consecutive calls of one message form a group.
    let mut groups: Vec<(i64, Vec<&CreateCallSite>)> = Vec::new();
    let mut last_entry: Option<&str> = None;
    for site in sites {
        if last_entry == Some(site.entry_id.as_str())
            && let Some((_, calls)) = groups.last_mut()
        {
            calls.push(site);
        } else {
            groups.push((site.at_ms, vec![site]));
        }
        last_entry = Some(site.entry_id.as_str());
    }
    let mut spawned: Vec<&(String, i64)> = spawned.iter().collect();
    spawned.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut buckets: Vec<Vec<&str>> = vec![Vec::new(); groups.len()];
    for (chat_id, created_ms) in spawned {
        if let Some(group) = groups.iter().rposition(|(at, _)| *at <= *created_ms) {
            buckets[group].push(chat_id);
        }
    }
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for ((_, calls), chats) in groups.iter().zip(buckets) {
        let batches: Vec<usize> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.op == CreateChatOp::Batch)
            .map(|(ix, _)| ix)
            .collect();
        let mut chats = chats.into_iter();
        let mut give = |call: &CreateCallSite, ids: Vec<&str>| {
            if !ids.is_empty() {
                out.entry(call.part_id.clone())
                    .or_default()
                    .extend(ids.into_iter().map(str::to_owned));
            }
        };
        match batches.as_slice() {
            [] => {
                for call in calls {
                    give(call, chats.next().into_iter().collect());
                }
            }
            [batch] => {
                let (before, rest) = calls.split_at(*batch);
                let after = &rest[1..];
                for call in before {
                    give(call, chats.next().into_iter().collect());
                }
                let remaining: Vec<&str> = chats.collect();
                let keep = remaining.len().saturating_sub(after.len());
                give(rest[0], remaining[..keep].to_vec());
                for (call, chat) in after.iter().zip(&remaining[keep..]) {
                    give(call, vec![chat]);
                }
            }
            [first, ..] => {
                for call in &calls[..*first] {
                    give(call, chats.next().into_iter().collect());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp(server: &str, tool: &str) -> ToolCall {
        ToolCall::Mcp {
            server: server.into(),
            tool: tool.into(),
            input: None,
        }
    }

    fn unknown(name: &str) -> ToolCall {
        ToolCall::Unknown {
            name: name.into(),
            input: None,
        }
    }

    #[test]
    fn create_chat_calls_are_recognized_under_every_naming_scheme() {
        use CreateChatOp::{Batch, Single};
        // Claude (`mcp__zeron__create_chat`), Pi (`zeron_create_chat`),
        // Codex and Cursor all decode to `Mcp { server, tool }`.
        assert_eq!(create_chat_op(&mcp("zeron", "create_chat")), Some(Single));
        assert_eq!(create_chat_op(&mcp("zeron", "create_chats")), Some(Batch));
        assert_eq!(create_chat_op(&mcp("Zeron", "create_chat")), Some(Single));
        // OpenCode and ACP titles stay `Unknown`.
        for name in [
            "zeron_create_chat",
            "mcp__zeron__create_chat",
            "zeron/create_chat",
            "zeron.create_chat",
            "create_chat (zeron MCP Server)",
            "Zeron: create-chat",
            "create_chat",
        ] {
            assert_eq!(create_chat_op(&unknown(name)), Some(Single), "{name}");
        }
        for name in ["zeron_create_chats", "mcp__zeron__create_chats", "create_chats"] {
            assert_eq!(create_chat_op(&unknown(name)), Some(Batch), "{name}");
        }
        // Not ours: other servers, other zeron tools, lookalike tokens, and
        // ordinary tool kinds.
        assert_eq!(create_chat_op(&mcp("github", "create_chat")), None);
        assert_eq!(create_chat_op(&mcp("zeron", "list_chats")), None);
        assert_eq!(create_chat_op(&mcp("zeron", "create_chat_room")), None);
        assert_eq!(create_chat_op(&unknown("slack_create_chat")), None);
        assert_eq!(create_chat_op(&unknown("zeron_recreate_chat")), None);
        assert_eq!(create_chat_op(&unknown("Agent: create_chat")), None);
        assert_eq!(
            create_chat_op(&ToolCall::Exec {
                command: "zeron create_chat".into()
            }),
            None
        );
    }

    #[test]
    fn single_results_name_their_chat() {
        let output = serde_json::to_string_pretty(&serde_json::json!({
            "chatId": "c-1",
            "kind": "chat",
            "deviceId": "gpu",
            "deviceName": "GPU box",
            "title": "Train the tokenizer",
            "parentChatId": null,
            "spawnedByChatId": "coordinator",
        }))
        .unwrap();
        assert_eq!(
            parse_created_chats(&output),
            [CreatedChat {
                chat_id: "c-1".into(),
                side: Some(false),
                device_id: Some("gpu".into()),
                device_name: Some("GPU box".into()),
                title: Some("Train the tokenizer".into()),
            }]
        );
        // A side chat without `kind` (older server) reads its parent.
        let side = parse_created_chats(r#"{"chatId":"s-1","parentChatId":"coordinator"}"#);
        assert_eq!(side[0].side, Some(true));
        // Fenced text and a CallToolResult wrapper resolve the same way.
        let fenced = format!("```json\n{output}\n```");
        assert_eq!(parse_created_chats(&fenced)[0].chat_id, "c-1");
        let wrapped = serde_json::json!({
            "content": [{ "type": "text", "text": output }],
        })
        .to_string();
        assert_eq!(parse_created_chats(&wrapped)[0].chat_id, "c-1");
    }

    #[test]
    fn batch_results_name_every_created_chat_and_skip_failures() {
        let output = serde_json::json!({
            "results": [
                { "index": 0, "isError": false, "result": { "chatId": "a", "kind": "chat", "deviceName": "GPU box" } },
                { "index": 1, "isError": true, "result": "device offline" },
                { "index": 2, "isError": false, "result": { "chatId": "b", "kind": "side" } },
            ]
        })
        .to_string();
        let chats = parse_created_chats(&output);
        assert_eq!(
            chats.iter().map(|c| c.chat_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(chats[0].side, Some(false));
        assert_eq!(chats[1].side, Some(true));
        // structuredContent carries the same batch.
        let structured = serde_json::json!({ "structuredContent": serde_json::from_str::<serde_json::Value>(&output).unwrap() }).to_string();
        assert_eq!(parse_created_chats(&structured).len(), 2);
        // Error text and truncated summaries name nothing.
        assert!(parse_created_chats("device gpu is offline").is_empty());
        assert!(parse_created_chats("{…").is_empty());
        assert!(parse_created_chats(r#"{"isError":true,"chatId":"x"}"#).is_empty());
    }

    fn site(part: &str, entry: &str, at: i64, op: CreateChatOp) -> CreateCallSite {
        CreateCallSite {
            part_id: part.into(),
            entry_id: entry.into(),
            at_ms: at,
            op,
        }
    }

    fn spawned(chats: &[(&str, i64)]) -> Vec<(String, i64)> {
        chats.iter().map(|(id, at)| ((*id).into(), *at)).collect()
    }

    fn ids(map: &HashMap<String, Vec<String>>, part: &str) -> Vec<String> {
        map.get(part).cloned().unwrap_or_default()
    }

    #[test]
    fn spawned_chats_attribute_to_the_message_that_created_them() {
        use CreateChatOp::{Batch, Single};
        let sites = [
            site("t1", "e1", 100, Single),
            site("t2", "e2", 200, Batch),
            site("t3", "e3", 300, Single),
        ];
        // `late` predates nothing it could belong to: before any call.
        let map = attribute_spawned_chats(
            &sites,
            &spawned(&[
                ("early", 50),
                ("one", 110),
                ("batch-a", 210),
                ("batch-b", 215),
                ("three", 320),
            ]),
        );
        assert_eq!(ids(&map, "t1"), ["one"]);
        assert_eq!(ids(&map, "t2"), ["batch-a", "batch-b"]);
        assert_eq!(ids(&map, "t3"), ["three"]);
        assert!(!map.values().flatten().any(|id| id == "early"));

        // A deleted chat leaves its own call empty without shifting others.
        let map = attribute_spawned_chats(&sites, &spawned(&[("batch-a", 210), ("three", 320)]));
        assert!(ids(&map, "t1").is_empty());
        assert_eq!(ids(&map, "t2"), ["batch-a"]);
        assert_eq!(ids(&map, "t3"), ["three"]);
    }

    #[test]
    fn calls_sharing_one_message_split_its_chats_in_order() {
        use CreateChatOp::{Batch, Single};
        // create_chat, create_chats, create_chat in one message.
        let sites = [
            site("s1", "e", 100, Single),
            site("b", "e", 100, Batch),
            site("s2", "e", 100, Single),
        ];
        let map = attribute_spawned_chats(
            &sites,
            &spawned(&[("c1", 101), ("c2", 102), ("c3", 103), ("c4", 104)]),
        );
        assert_eq!(ids(&map, "s1"), ["c1"]);
        assert_eq!(ids(&map, "b"), ["c2", "c3"]);
        assert_eq!(ids(&map, "s2"), ["c4"]);

        // Two batches in one message are ambiguous: only the leading
        // create_chat links.
        let sites = [
            site("s1", "e", 100, Single),
            site("b1", "e", 100, Batch),
            site("b2", "e", 100, Batch),
        ];
        let map = attribute_spawned_chats(&sites, &spawned(&[("c1", 101), ("c2", 102)]));
        assert_eq!(ids(&map, "s1"), ["c1"]);
        assert!(ids(&map, "b1").is_empty() && ids(&map, "b2").is_empty());
    }

    #[test]
    fn call_sites_skip_running_failed_and_self_describing_calls() {
        let tool = |id: &str, tool: &str, resolved: bool, is_error: bool, output: Option<&str>| {
            MessagePart::Tool {
                id: id.into(),
                call: mcp("zeron", tool),
                is_error,
                resolved,
                output: output.map(str::to_owned),
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
            }
        };
        let entry = SessionMessageEntry {
            id: "e".into(),
            role: zeron_doc::MessageRole::Assistant,
            parts: vec![
                tool("running", "create_chat", false, false, None),
                tool("failed", "create_chat", true, true, None),
                tool("named", "create_chat", true, false, Some(r#"{"chatId":"known"}"#)),
                tool("bare", "create_chats", true, false, None),
                tool("other", "list_chats", true, false, None),
            ],
            created_at: 42,
            device_id: "dev".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        let (sites, claimed) = create_call_sites(std::slice::from_ref(&entry));
        assert_eq!(sites, [site("bare", "e", 42, CreateChatOp::Batch)]);
        assert_eq!(claimed, HashSet::from(["known".to_owned()]));
    }
}
