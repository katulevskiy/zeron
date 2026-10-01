//! Moving a chat to a sleeping cloud box (docs/cloud.md §8), between real
//! engines on one local edge. The box starts out not running: its Worker
//! (a fake one here) is asked to wake it with a signed request, the "box" —
//! a second engine with the box's device id — comes up when that request
//! arrives, and the move carries on as an ordinary move once the box answers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;
use zeron_cloud::mock::{MockServer, Recorded, Reply};
use zeron_doc::SessionCommandPayload;
use zeron_engine::{Engine, EngineConfig, EngineRuntime, HarnessId};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_localedge::{LocalEdge, LocalEdgeConfig};
use zeron_proto::{
    AgentEvent, Chat, ChatConfig, CloudBox, CloudBoxState, DoneStatus, Model, MovePhase,
    ReasoningLevel, RunRequest, SandboxLevel, SteeringMode, capabilities,
};
use zeron_rpc::{RpcClient, methods};

const TOKEN: &str = "cloud-0123456789abcdef0123456789abcdef";
const BOX_ID: &str = "cloud-box-device-1";

/// An agent that answers at once; the move's scout finds nothing to add.
struct QuickAgent;

#[async_trait]
impl Harness for QuickAgent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Quick"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run_title(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter([
            Ok(AgentEvent::TextDelta {
                text: r#"{"include":[],"notes":[]}"#.into(),
            }),
            Ok(done()),
        ])
        .boxed())
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter([
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "quick".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "s-1".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: "Done.".into(),
            }),
            Ok(done()),
        ])
        .boxed())
    }
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: None,
    }
}

struct Device {
    runtime: EngineRuntime,
    rpc: RpcClient,
    id: String,
}

async fn device(root: &Path, name: &str, edge_url: &str, pinned_id: Option<&str>) -> Device {
    let dir = root.join(name);
    if let Some(id) = pinned_id {
        zeron_engine::pin_device_id(&dir, id).expect("pin the box's device id");
    }
    let config = EngineConfig {
        data_dir: dir,
        edge_url: String::new(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: None,
        dev_user_id: None,
        device_credential: None,
    }
    .with_local_edge(edge_url, TOKEN);
    let auth = Engine::build_auth(&config).await;
    let scope = Engine::initial_workspace_scope(&auth);
    let profile = Engine::resolve_profile(&config, &auth, scope)
        .unwrap()
        .expect("development profile");
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .expect("engine runtime assembles");
    runtime.core().registry.register(Arc::new(QuickAgent));
    let rpc = zeron_rpc::memory_client(runtime.core().rpc_service());
    let id = runtime.core().device_id.clone();
    Device { runtime, rpc, id }
}

async fn wait_for<T>(what: &str, timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn chat_on(device: &Device, chat_id: &str) -> Option<Chat> {
    device.runtime.core().workspace.chat(chat_id).ok().flatten()
}

fn git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_move_wakes_a_sleeping_box_and_carries_on_once_it_answers() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // One process stands in for two computers: they share this home, and
    // neither touches the real one.
    // SAFETY: set before any engine thread starts; this binary has one test.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("ZERON_WORKTREES_DIR", home.join(".zeron/worktrees"));
        std::env::set_var("GIT_CONFIG_GLOBAL", home.join(".gitconfig"));
    }
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(scenario(dir.path(), &home));
}

async fn scenario(root: &Path, home: &Path) {
    let project: PathBuf = home.join("dev/proj");
    std::fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-q"]);
    std::fs::write(project.join("README.md"), "# proj\n").unwrap();
    std::fs::write(project.join("notes.txt"), "untracked\n").unwrap();
    git(&project, &["add", "README.md"]);
    git(&project, &["commit", "-qm", "first"]);

    let edge = LocalEdge::start(LocalEdgeConfig::loopback(root.join("edge"), 0, TOKEN))
        .await
        .unwrap();
    let laptop = device(root, "laptop", &edge.url(), None).await;

    // The box's Worker: it checks the signature, says the box is waking,
    // and (the test, standing in for Cloudflare) starts the box.
    let wake_key = zeron_cloud::sign::new_wake_key();
    let wakes: Arc<Mutex<Vec<Recorded>>> = Arc::default();
    let (wake_tx, mut wake_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let worker = {
        let (wake_key, wakes) = (wake_key.clone(), wakes.clone());
        MockServer::start(move |request: &Recorded| {
            let path = request.path_only().to_string();
            let signed = zeron_cloud::sign::verify(
                &wake_key,
                &request.method,
                &path,
                request.header(zeron_cloud::sign::TIMESTAMP_HEADER).unwrap_or_default(),
                request.header(zeron_cloud::sign::SIGNATURE_HEADER).unwrap_or_default(),
                &request.body,
                chrono::Utc::now().timestamp_millis(),
            );
            if request.method != "POST" || path != format!("/v1/boxes/{BOX_ID}/wake") || signed.is_err()
            {
                return Reply::json(401, json!({ "error": "bad request" }));
            }
            wakes.lock().unwrap().push(request.clone());
            let _ = wake_tx.send(());
            Reply::json(200, json!({ "state": "waking" }))
        })
        .await
    };
    laptop
        .runtime
        .core()
        .workspace
        .upsert_cloud_box(&CloudBox {
            id: BOX_ID.into(),
            provider: "cloudflare".into(),
            name: "Cloud".into(),
            account_id: "acc-1".into(),
            worker_url: worker.url.clone(),
            wake_key,
            instance_type: "standard-2".into(),
            idle_minutes: 15,
            created_at: 1,
            state: CloudBoxState::Asleep,
            state_at: 1,
            error: None,
        })
        .unwrap();

    // A chat on the laptop.
    let (space_id, chat_id) = ("space-proj", "chat-cloud-1");
    laptop
        .rpc
        .call(
            methods::MUTATE,
            json!({ "op": "createSpace", "spaceId": space_id, "deviceId": laptop.id,
                "path": project, "name": "proj", "gitDetected": true }),
        )
        .await
        .unwrap();
    laptop
        .rpc
        .call(
            methods::MUTATE,
            json!({ "op": "createChat", "chatId": chat_id, "spaceId": space_id,
                "config": ChatConfig { harness: HarnessId::Mock, model: None, reasoning: None,
                    model_options: serde_json::Map::new(), sandbox: SandboxLevel::WorkspaceWrite } }),
        )
        .await
        .unwrap();
    let request = RunRequest {
        mcp: None,
        prompt: "hello".into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: project.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
    };
    laptop
        .rpc
        .call(
            methods::QUEUE_COMMAND,
            json!({ "chatId": chat_id, "command": SessionCommandPayload::Run {
                request, message_id: "m-1".into() } }),
        )
        .await
        .unwrap();
    let sessions = laptop.runtime.core().sessions.clone();
    wait_for("the first turn", Duration::from_secs(60), || {
        (!sessions.turn_in_flight(chat_id)
            && chat_on(&laptop, chat_id).is_some_and(|c| c.last_message_at.is_some()))
        .then_some(())
    })
    .await;

    // The picker offers the sleeping box, and says so.
    let candidates = laptop
        .rpc
        .call(methods::MOVE_CANDIDATES, json!({ "chatId": chat_id }))
        .await
        .unwrap();
    let box_row = candidates
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["deviceId"] == BOX_ID)
        .expect("the box is a candidate");
    assert_eq!(box_row["asleep"], true, "{candidates}");
    assert_eq!(box_row["note"], "Asleep — wakes when you move");
    assert_eq!(box_row["supported"], true);

    // Start the move: it wakes the box first.
    let box_root = root.to_path_buf();
    let edge_url = edge.url();
    let starter = tokio::spawn(async move {
        wake_rx.recv().await.expect("the Worker is asked to wake the box");
        device(&box_root, "box", &edge_url, Some(BOX_ID)).await
    });
    laptop
        .rpc
        .call(
            methods::START_MOVE,
            json!({ "chatId": chat_id, "toDeviceId": BOX_ID }),
        )
        .await
        .expect("move starts");
    let waking = wait_for("the move to say it's waking the box", Duration::from_secs(30), || {
        chat_on(&laptop, chat_id)
            .and_then(|c| c.move_state)
            .filter(|m| m.detail.as_deref() == Some("Waking the cloud box") || m.phase.is_terminal())
    })
    .await;
    assert_eq!(waking.detail.as_deref(), Some("Waking the cloud box"), "{waking:#?}");
    assert!(!waking.phase.is_terminal());

    let box_device = starter.await.unwrap();
    wait_for("the box's presence", Duration::from_secs(60), || {
        let workspace = laptop.runtime.core().workspace.clone();
        (workspace
            .read_devices()
            .unwrap_or_default()
            .iter()
            .any(|d| d.id == BOX_ID && d.supports(capabilities::SESSION_MOVE_V1))
            && workspace.peer_liveness(BOX_ID) == zeron_rpc::PeerLiveness::Live)
            .then_some(())
    })
    .await;
    let moved = wait_for("the box to host the chat", Duration::from_secs(180), || {
        let chat = chat_on(&laptop, chat_id)?;
        let state = chat.move_state.clone()?;
        assert!(
            !matches!(state.phase, MovePhase::Failed | MovePhase::Cancelled),
            "move ended: {state:#?}"
        );
        (state.phase == MovePhase::Done).then_some(chat)
    })
    .await;
    assert_eq!(moved.device_id, BOX_ID, "the box hosts the chat now");
    assert_eq!(box_device.id, BOX_ID);

    // The workspace arrived.
    let landing = PathBuf::from(moved.cwd.clone().unwrap());
    assert_eq!(std::fs::read_to_string(landing.join("README.md")).unwrap(), "# proj\n");
    assert_eq!(std::fs::read_to_string(landing.join("notes.txt")).unwrap(), "untracked\n");

    // Exactly one wake, signed, and the record knows the box is running.
    let seen = wakes.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "one wake request: {seen:#?}");
    wait_for("the record to say running", Duration::from_secs(10), || {
        laptop
            .runtime
            .core()
            .workspace
            .read_cloud_boxes()
            .into_iter()
            .find(|b| b.id == BOX_ID)
            .filter(|b| b.state == CloudBoxState::Running)
            .map(|_| ())
    })
    .await;
}
