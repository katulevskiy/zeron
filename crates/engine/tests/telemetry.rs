//! Product telemetry through the real engine: a scripted turn becomes one
//! content-free `turn.started`/`turn.finished` pair plus a usage-ledger row,
//! RPC traffic lands in the hourly rollup, and the telemetry RPC surface
//! answers over the in-memory transport. Own test binary: the telemetry
//! handle is process-global.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::mock::MockHarness;
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, SessionStatus, ToolCall,
};
use zeron_rpc::methods;
use zeron_telemetry::{RunMode, Telemetry, TelemetryOptions, Tuning};

const CHAT: &str = "chat-telemetry";
const SECRET_PROMPT: &str = "refactor /home/alice/secret-project/payroll.rs";

fn script() -> Vec<AgentEvent> {
    vec![
        AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: "/home/alice/secret-project".into(),
            session_id: "hs-1".into(),
            assistant_message_id: "a-1".into(),
        },
        AgentEvent::TextDelta {
            text: "payroll secrets".into(),
        },
        AgentEvent::ToolCall {
            id: "t-1".into(),
            call: ToolCall::Exec {
                command: "cat ~/.ssh/id_rsa".into(),
            },
        },
        AgentEvent::ToolResult {
            id: "t-1".into(),
            is_error: true,
            output: Some("denied".into()),
            diff: None,
        },
        AgentEvent::ContextUsage {
            tokens: Some(40_000),
            window: Some(200_000),
        },
        AgentEvent::usage(1_200, 300)
            .with_cache(Some(50_000), Some(2_000))
            .with_cost_usd(Some(0.12)),
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("hs-1".into()),
        },
    ]
}

fn request() -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: SECRET_PROMPT.into(),
        harness: None,
        model: Some("mock-1".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: "/home/alice/secret-project".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn install_telemetry(dir: &std::path::Path) -> Telemetry {
    let telemetry = Telemetry::start_with(
        TelemetryOptions {
            data_dir: dir.to_path_buf(),
            run_mode: RunMode::Headless,
            workspace_scope: "local".into(),
            device_id: "dev-1".into(),
            app_version: "0.0.0-test".into(),
        },
        Tuning {
            endpoint: None,
            api_key: "phc_test".into(),
            batch_size: 100,
            flush_every: Duration::from_secs(3600),
            rollup_every: Duration::from_secs(3600),
            debug_log: false,
        },
        None,
    );
    assert!(zeron_telemetry::install_global(&telemetry));
    telemetry
}

fn recent_named(telemetry: &Telemetry, name: &str) -> Vec<Value> {
    telemetry
        .status()
        .recent
        .into_iter()
        .filter(|e| e["event"] == name)
        .collect()
}

#[tokio::test]
async fn turn_rpc_and_usage_flow_through_telemetry() {
    let state = tempfile::tempdir().unwrap();
    let telemetry = install_telemetry(state.path());

    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness { script: script() }));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");

    // A turn, queued the way a viewer does.
    let mut watch = core.sessions.watch_sessions();
    let handle = core.doc_host.open(CHAT).unwrap();
    handle
        .doc()
        .queue_command(&SessionCommandEntry {
            id: "cmd-1".into(),
            payload: SessionCommandPayload::Run {
                request: request(),
                message_id: "m-1".into(),
            },
            issued_by: "viewer".into(),
            issued_at: chrono::Utc::now().timestamp_millis(),
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while recent_named(&telemetry, "turn.finished").is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "turn never finished: {:?}",
            watch.borrow().first().map(|s| s.status)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(matches!(
        watch.borrow().first().map(|s| s.status),
        Some(SessionStatus::Idle | SessionStatus::Working) | None
    ));

    // RPC traffic → rollup; telemetry methods answer locally.
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(methods::LIST_HARNESSES, Value::Null)
        .await
        .unwrap();
    client
        .call(methods::LIST_HARNESSES, Value::Null)
        .await
        .unwrap();
    let accepted = client
        .call(
            methods::TRACK_EVENTS,
            json!({"surface": "desktop", "events": [
                {"name": "panel.toggled", "props": {"panel": "changes", "open": true}},
                {"name": "Bad Name", "props": {}},
            ]}),
        )
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], 1);
    telemetry.flush(Duration::from_secs(5)).await;

    let started = recent_named(&telemetry, "turn.started");
    assert_eq!(started.len(), 1, "one turn started");
    let finished = recent_named(&telemetry, "turn.finished");
    assert_eq!(finished.len(), 1, "one turn finished");
    let p = &finished[0]["properties"];
    assert_eq!(p["harness"], "mock");
    assert_eq!(p["model"], "mock-1");
    assert_eq!(p["outcome"], "completed");
    assert_eq!(p["sandbox"], "workspace-write");
    assert_eq!(p["tools"]["exec"], 1);
    assert_eq!(p["tool_errors"], 1);
    assert_eq!(p["input_tokens"], 1_200);
    assert_eq!(p["output_tokens"], 300);
    assert_eq!(p["cache_read_tokens"], 50_000);
    assert_eq!(p["cache_write_tokens"], 2_000);
    assert_eq!(p["cost_usd"], 0.12);
    assert_eq!(p["context_pct"], 20.0);
    assert_eq!(p["usage_complete"], true);
    assert_eq!(p["workspace_scope"], "local");

    let ui = recent_named(&telemetry, "ui.panel.toggled");
    assert_eq!(ui.len(), 1);
    assert_eq!(ui[0]["properties"]["surface"], "desktop");

    let rollups = recent_named(&telemetry, "rollup");
    let rpc = rollups
        .iter()
        .find(|e| e["properties"]["metric"] == "rpc")
        .expect("rpc rollup");
    assert_eq!(rpc["properties"]["calls"]["ListHarnesses"], 2);

    // Nothing the user or agent wrote is anywhere in what would be sent.
    let everything = serde_json::to_string(&telemetry.status().recent).unwrap();
    for secret in ["secret-project", "payroll", "alice", "id_rsa", "denied"] {
        assert!(
            !everything.contains(secret),
            "{secret} leaked: {everything}"
        );
    }

    // The local ledger counted the turn.
    let stats = client
        .call(methods::USAGE_STATS, Value::Null)
        .await
        .unwrap();
    let row = &stats["zeron"][0];
    assert_eq!(row["source"], "mock");
    assert_eq!(row["model"], "mock-1");
    assert_eq!(row["requests"], 1);
    assert_eq!(row["cacheReadTokens"], 50_000);

    // Opting out over RPC stops collection immediately.
    let status = client
        .call(methods::SET_TELEMETRY_SETTINGS, json!({"enabled": false}))
        .await
        .unwrap();
    assert_eq!(status["collecting"], false);
    client
        .call(methods::LIST_HARNESSES, Value::Null)
        .await
        .unwrap();
    telemetry.flush(Duration::from_secs(5)).await;
    assert!(telemetry.status().recent.is_empty());

    core.shutdown().await;
}
