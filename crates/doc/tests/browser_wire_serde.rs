//! Pin the serialized command/response contract, not the Rust source text.
use serde_json::json;
use zeron_doc::{
    MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry, TranscriptFrame,
    TranscriptUpdate,
};

#[test]
fn command_tags_and_camel_case_fields_remain_compatible() {
    for (payload, value) in [
        (
            SessionCommandPayload::Interrupt {},
            json!({"kind": "interrupt"}),
        ),
        (
            SessionCommandPayload::Steer {
                prompt: "next".into(),
                message_id: None,
            },
            json!({"kind": "steer", "prompt": "next", "messageId": null}),
        ),
        (
            SessionCommandPayload::RespondInput {
                request_id: "input-1".into(),
                answers: vec![],
            },
            json!({"kind": "respondInput", "requestId": "input-1", "answers": []}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&payload).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<SessionCommandPayload>(value).unwrap(),
            payload
        );
    }
    let value = json!({"kind": "run", "messageId": "user-1", "request": {
        "prompt": "hello", "cwd": "/tmp", "sandbox": "workspace-write",
        "model": null, "reasoning": null, "resume": null,
        "modelOptions": {}, "autoApprove": false
    }});
    let command: SessionCommandPayload = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(command).unwrap(), value);
}

#[test]
fn transcript_is_flat_untagged_reset_or_delta_not_a_fork_wrapper() {
    let entry = SessionMessageEntry {
        id: "assistant-1".into(),
        role: MessageRole::Assistant,
        parts: vec![MessagePart::Text {
            id: "text-1".into(),
            text: "hello".into(),
        }],
        created_at: 1000,
        device_id: "host".into(),
        status: None,
        continuation_of: None,
        duration_ms: None,
    };
    let update = TranscriptUpdate {
        frame: TranscriptFrame::Reset { reset: vec![entry] },
        context_usage: None,
        replay_baseline: None,
    };
    assert_eq!(
        serde_json::to_value(update).unwrap(),
        json!({
            "reset": [{"id": "assistant-1", "role": "assistant",
                "parts": [{"kind": "text", "id": "text-1", "text": "hello"}],
                "createdAt": 1000, "deviceId": "host"}],
            "contextUsage": null
        })
    );
    let old_delta = json!({"count": 0});
    let update: TranscriptUpdate = serde_json::from_value(old_delta).unwrap();
    assert_eq!(
        serde_json::to_value(update).unwrap(),
        json!({
            "upsert": [], "append": [], "remove": [], "count": 0,
            "contextUsage": null
        })
    );
    assert!(serde_json::from_value::<TranscriptFrame>(json!({"kind": "reset"})).is_err());
}
