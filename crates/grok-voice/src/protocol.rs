//! Realtime JSON. Seeds are conversation items only: never `response.create`.

use base64::Engine as _;
use serde_json::{Value, json};
use zeron_proto::voice::VoiceRole;

pub(crate) const FRAME_SAMPLES: usize = 480;

pub(crate) struct ListedTool<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SeedTurn {
    pub role: VoiceRole,
    pub text: String,
}

pub(crate) enum ServerEvent {
    SessionReady,
    Audio(Vec<i16>),
    TranscriptDelta {
        item_id: Option<String>,
        delta: String,
    },
    TranscriptDone {
        item_id: Option<String>,
        transcript: Option<String>,
    },
    UserTranscript {
        item_id: Option<String>,
        transcript: String,
    },
    Conversation(String),
    Function {
        name: String,
        call_id: String,
        arguments: Value,
    },
    Error(String),
    Ignore,
}

pub(crate) fn session_update_message(instructions: &str, tools: &[ListedTool<'_>]) -> Value {
    json!({
        "type": "session.update",
        "session": {
            "instructions": instructions,
            "turn_detection": {"type": "server_vad"},
            "resumption": {"enabled": true},
            "audio": {
                "input": {"format": {"type": "audio/pcm", "rate": 24_000}},
                "output": {"format": {"type": "audio/pcm", "rate": 24_000}}
            },
            "tools": tools.iter().map(|tool| json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })).collect::<Vec<_>>()
        }
    })
}

pub(crate) fn select_seeds(turns: &[SeedTurn]) -> Vec<SeedTurn> {
    let mut picked = Vec::new();
    let mut chars = 0usize;
    for turn in turns.iter().rev() {
        if picked.len() >= 24 {
            break;
        }
        let len = turn.text.chars().count();
        if chars + len > 8_000 {
            if picked.is_empty() && len > 0 {
                let text: String = turn
                    .text
                    .chars()
                    .rev()
                    .take(8_000)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                picked.push(SeedTurn {
                    role: turn.role,
                    text,
                });
            }
            break;
        }
        chars += len;
        picked.push(turn.clone());
    }
    picked.reverse();
    picked
}

pub(crate) fn seed_frames(turns: &[SeedTurn]) -> Vec<Value> {
    select_seeds(turns).iter().map(seed_frame).collect()
}

fn seed_frame(turn: &SeedTurn) -> Value {
    let (role, kind) = match turn.role {
        VoiceRole::User => ("user", "input_text"),
        VoiceRole::Assistant => ("assistant", "text"),
    };
    json!({
        "type": "conversation.item.create",
        "item": {
            "type": "message",
            "role": role,
            "content": [{"type": kind, "text": turn.text}]
        }
    })
}

pub(crate) fn append_audio(samples: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let value = if sample.is_finite() {
            (sample.clamp(-1.0, 1.0) * 32_767.0) as i16
        } else {
            0
        };
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let audio = base64::engine::general_purpose::STANDARD.encode(bytes);
    json!({"type": "input_audio_buffer.append", "audio": audio}).to_string()
}

pub(crate) fn function_output(call_id: &str, output: &str) -> String {
    json!({
        "type": "conversation.item.create",
        "item": {
            "type": "function_call_output",
            "call_id": call_id,
            "output": output
        }
    })
    .to_string()
}

pub(crate) fn response_create() -> String {
    json!({"type": "response.create"}).to_string()
}

pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

pub(crate) fn parse_server_event(value: &Value) -> ServerEvent {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "session.created" | "session.updated" => ServerEvent::SessionReady,
        "response.output_audio.delta" | "response.audio.delta" => value
            .get("delta")
            .and_then(Value::as_str)
            .and_then(decode_pcm16)
            .map(ServerEvent::Audio)
            .unwrap_or(ServerEvent::Ignore),
        "response.output_audio_transcript.delta" | "response.audio_transcript.delta" => {
            let Some(delta) = value.get("delta").and_then(Value::as_str) else {
                return ServerEvent::Ignore;
            };
            if delta.is_empty() {
                return ServerEvent::Ignore;
            }
            ServerEvent::TranscriptDelta {
                item_id: event_item_id(value),
                delta: delta.to_string(),
            }
        }
        "response.output_audio_transcript.done" | "response.audio_transcript.done" => {
            ServerEvent::TranscriptDone {
                item_id: event_item_id(value),
                transcript: value
                    .get("transcript")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            let Some(transcript) = value.get("transcript").and_then(Value::as_str) else {
                return ServerEvent::Ignore;
            };
            ServerEvent::UserTranscript {
                item_id: event_item_id(value),
                transcript: transcript.to_string(),
            }
        }
        "conversation.created" => conversation_id(value)
            .map(ServerEvent::Conversation)
            .unwrap_or(ServerEvent::Ignore),
        "response.function_call_arguments.done" => function_call(value)
            .map(|(name, call_id, arguments)| ServerEvent::Function {
                name,
                call_id,
                arguments,
            })
            .unwrap_or(ServerEvent::Ignore),
        "error" => ServerEvent::Error(error_message(value)),
        _ => ServerEvent::Ignore,
    }
}

fn decode_pcm16(encoded: &str) -> Option<Vec<i16>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if bytes.len() < 2 {
        return None;
    }
    let pcm: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    if pcm.is_empty() { None } else { Some(pcm) }
}

fn event_item_id(value: &Value) -> Option<String> {
    ["item_id", "itemId"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .or_else(|| value.pointer("/item/id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn conversation_id(value: &Value) -> Option<String> {
    value
        .pointer("/conversation/id")
        .and_then(Value::as_str)
        .or_else(|| value.get("conversation_id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn function_call(value: &Value) -> Option<(String, String, Value)> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/item/name").and_then(Value::as_str))?;
    let call_id = value
        .get("call_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("callId").and_then(Value::as_str))
        .or_else(|| value.pointer("/item/call_id").and_then(Value::as_str))
        .or_else(|| value.pointer("/item/callId").and_then(Value::as_str))?;
    if name.is_empty() || call_id.is_empty() {
        return None;
    }
    let arguments = match value
        .get("arguments")
        .or_else(|| value.pointer("/item/arguments"))
    {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        Some(other) => other.clone(),
        None => Value::Null,
    };
    Some((name.to_string(), call_id.to_string(), arguments))
}

fn error_message(value: &Value) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("error").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

pub(crate) fn mentions_conversation(message: &str) -> bool {
    message.to_ascii_lowercase().contains("conversation")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_update_carries_instructions_and_tools_without_a_voice() {
        let parameters = json!({"type": "object"});
        let tools = [ListedTool {
            name: "lookup",
            description: "Look up a thing",
            parameters: &parameters,
        }];
        let value = session_update_message("be brief", &tools);
        assert_eq!(value["type"], "session.update");
        assert_eq!(value["session"]["instructions"], "be brief");
        assert_eq!(value["session"]["tools"][0]["type"], "function");
        assert_eq!(value["session"]["tools"][0]["name"], "lookup");
        assert_eq!(value["session"]["tools"][0]["description"], "Look up a thing");
        assert_eq!(value["session"]["resumption"]["enabled"], true);
        assert_eq!(value["session"]["turn_detection"]["type"], "server_vad");
        assert_eq!(value["session"]["audio"]["input"]["format"]["rate"], 24_000);
        assert!(value["session"].get("voice").is_none());
    }

    #[test]
    fn seed_frames_are_items_and_never_response_create() {
        let turns = [
            SeedTurn {
                role: VoiceRole::User,
                text: "hi".into(),
            },
            SeedTurn {
                role: VoiceRole::Assistant,
                text: "hey".into(),
            },
        ];
        let frames = seed_frames(&turns);
        assert!(frames.iter().all(|frame| frame["type"] == "conversation.item.create"));
        assert_eq!(frames[0]["item"]["content"][0]["type"], "input_text");
        assert_eq!(frames[1]["item"]["content"][0]["type"], "text");
        let batch = serde_json::to_string(&frames).expect("json");
        assert!(!batch.contains("response.create"));

        let many: Vec<_> = (0..30)
            .map(|i| SeedTurn {
                role: VoiceRole::User,
                text: format!("turn {i}"),
            })
            .collect();
        assert_eq!(seed_frames(&many).len(), 24);

        let long = "y".repeat(3_000);
        let wide = vec![
            SeedTurn {
                role: VoiceRole::User,
                text: long.clone(),
            },
            SeedTurn {
                role: VoiceRole::Assistant,
                text: long.clone(),
            },
            SeedTurn {
                role: VoiceRole::User,
                text: long,
            },
        ];
        let trimmed = seed_frames(&wide);
        assert_eq!(trimmed.len(), 2);
        let chars: usize = trimmed
            .iter()
            .map(|frame| {
                frame["item"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .count()
            })
            .sum();
        assert!(chars <= 8_000);
    }

    #[test]
    fn function_call_accepts_a_string_or_an_object() {
        let as_string = json!({
            "type": "response.function_call_arguments.done",
            "name": "lookup",
            "call_id": "c1",
            "arguments": "{\"q\":1}"
        });
        match parse_server_event(&as_string) {
            ServerEvent::Function {
                name,
                call_id,
                arguments,
            } => {
                assert_eq!(name, "lookup");
                assert_eq!(call_id, "c1");
                assert_eq!(arguments["q"], 1);
            }
            _ => panic!("string arguments"),
        }
        let as_object = json!({
            "type": "response.function_call_arguments.done",
            "item": {"name": "lookup", "callId": "c2"},
            "arguments": {"q": "hi"}
        });
        match parse_server_event(&as_object) {
            ServerEvent::Function {
                name,
                call_id,
                arguments,
            } => {
                assert_eq!(name, "lookup");
                assert_eq!(call_id, "c2");
                assert_eq!(arguments["q"], "hi");
            }
            _ => panic!("object arguments"),
        }
    }
}
