//! JSON contract tests: TS derives must not change the existing serde wire.
use serde_json::json;
use zeron_proto::{ChatConfig, EngineInfo, HarnessId, RunRequest, SandboxLevel, WorkspaceScope};

#[test]
fn identity_omits_missing_optional_fields_but_accepts_old_engines() {
    let old = json!({"deviceId": "host", "workspaceScope": "local"});
    let info: EngineInfo = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(info.workspace_scope, WorkspaceScope::Local);
    assert!(info.cursor_sdk_version.is_none());
    assert!(info.capabilities.is_empty());
    assert_eq!(serde_json::to_value(info).unwrap(), old);
}

#[test]
fn run_request_names_defaults_nulls_and_omissions_remain_compatible() {
    let input = json!({
        "prompt": "hello", "cwd": "/tmp", "sandbox": "workspace-write",
        "model": null, "reasoning": null, "resume": null
    });
    let request: RunRequest = serde_json::from_value(input).unwrap();
    assert!(request.harness.is_none());
    assert!(request.attachments.is_empty());
    assert!(request.worktree.is_none());
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({
            "prompt": "hello", "cwd": "/tmp", "sandbox": "workspace-write",
            "model": null, "reasoning": null, "resume": null,
            "modelOptions": {}, "autoApprove": false
        })
    );
    let mut request = request;
    request.harness = Some(HarnessId::Mock);
    request.attachments = vec!["/tmp/image.png".into()];
    request.model_options.insert("choice".into(), json!("fast"));
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["harness"], "mock");
    assert_eq!(value["attachments"], json!(["/tmp/image.png"]));
    assert_eq!(value["modelOptions"], json!({"choice": "fast"}));
    assert!(value.get("worktree").is_none());
}

#[test]
fn chat_configuration_uses_actual_upstream_enum_spelling() {
    let config = ChatConfig {
        harness: HarnessId::ClaudeCode,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
    };
    assert_eq!(
        serde_json::to_value(config).unwrap(),
        json!({
            "harness": "claude-code", "model": null, "reasoning": null,
            "modelOptions": {}, "sandbox": "workspace-write"
        })
    );
}

#[test]
fn optional_tool_fields_are_omitted_even_without_serde_default() {
    for value in [
        json!({"kind": "writeFile", "path": "/tmp/a"}),
        json!({"kind": "editFile", "path": "/tmp/a"}),
        json!({"kind": "applyPatch"}),
        json!({"kind": "search", "pattern": "needle"}),
        json!({"kind": "webFetch", "url": "https://example.com"}),
        json!({"kind": "mcp", "server": "s", "tool": "t"}),
        json!({"kind": "unknown", "name": "custom"}),
    ] {
        let call: zeron_proto::ToolCall = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(call).unwrap(), value);
    }
}
