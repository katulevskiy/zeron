//! Moving a live chat between two real engines on one local edge
//! (docs/session-move.md). A scripted agent stands in for a CLI on both
//! engines: it reads a file outside the project, works in a long step the
//! test controls, and records every prompt it receives.
//!
//! 1. The chat moves while its agent is mid-step: the move copies, waits
//!    for the step to end, stops the agent, and the desktop takes over —
//!    workspace (tracked, untracked, ignored `.env`; never `node_modules`),
//!    git history and branch, the outside file, the transcript seam, the
//!    note, and the continued turn.
//! 2. It moves back at once ("now"): it lands where it started and only the
//!    changed file travels.
//! 3. A cancelled move leaves the chat where it was, still running.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use zeron_doc::{MessagePart, SessionCommandPayload};
use zeron_engine::{Engine, EngineConfig, EngineRuntime, HarnessId};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_localedge::{LocalEdge, LocalEdgeConfig};
use zeron_proto::{
    AgentEvent, Chat, ChatConfig, DoneStatus, Model, MovePhase, ReasoningLevel, RunRequest,
    SandboxLevel, SteeringMode, ToolCall, capabilities,
};
use zeron_rpc::{RpcClient, methods};

const TOKEN: &str = "move-0123456789abcdef0123456789abcdef";

// ── the scripted agent ──────────────────────────────────────────────────

/// Shared by both engines' agents: prompts seen per device, and the gate
/// the long step waits on.
#[derive(Default)]
struct Script {
    prompts: Mutex<Vec<(String, String)>>,
    release: tokio::sync::Notify,
    in_step: Mutex<bool>,
}

struct ScriptedAgent {
    device: String,
    script: Arc<Script>,
    screenshot: PathBuf,
}

#[async_trait]
impl Harness for ScriptedAgent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Scripted"
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
    /// The move's scout: one piece of advice for the agent.
    async fn run_title(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter([
            Ok(AgentEvent::TextDelta {
                text: r#"{"include":[],"notes":["Restart the dev server with npm run dev."]}"#
                    .into(),
            }),
            Ok(done(DoneStatus::Completed)),
        ])
        .boxed())
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.script
            .prompts
            .lock()
            .unwrap()
            .push((self.device.clone(), request.prompt.clone()));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<AgentEvent, HarnessError>>();
        let script = self.script.clone();
        let screenshot = self.screenshot.clone();
        let interrupt = controls.interrupt.clone();
        let cwd = PathBuf::from(&request.cwd);
        // The user's own words come last (a move note or replayed history
        // may be prepended).
        let prompt = request.prompt.lines().last().unwrap_or_default().to_owned();
        tokio::spawn(async move {
            let send = |event| {
                let _ = tx.send(Ok(event));
            };
            send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "scripted".into(),
                tools: vec![],
                cwd: cwd.display().to_string(),
                session_id: format!("s-{}", uuid_like()),
                assistant_message_id: format!("a-{}", uuid_like()),
            });
            if prompt.contains("look at the screenshot") {
                send(AgentEvent::ToolCall {
                    id: "read-shot".into(),
                    call: ToolCall::ReadFile {
                        path: screenshot.display().to_string(),
                    },
                });
                send(AgentEvent::ToolResult {
                    id: "read-shot".into(),
                    is_error: false,
                    output: Some("a picture".into()),
                    diff: None,
                });
            }
            if prompt.contains("long step") {
                let step = format!("long-{}", uuid_like());
                send(AgentEvent::ToolCall {
                    id: step.clone(),
                    call: ToolCall::Exec {
                        command: "cargo build --release".into(),
                    },
                });
                *script.in_step.lock().unwrap() = true;
                tokio::select! {
                    _ = script.release.notified() => {}
                    _ = interrupt.cancelled() => {
                        *script.in_step.lock().unwrap() = false;
                        send(done(DoneStatus::Interrupted));
                        return;
                    }
                }
                *script.in_step.lock().unwrap() = false;
                std::fs::write(cwd.join("built.txt"), "built\n").unwrap();
                send(AgentEvent::ToolResult {
                    id: step,
                    is_error: false,
                    output: Some("Finished".into()),
                    diff: None,
                });
            }
            if prompt.contains("edit the readme") {
                std::fs::write(cwd.join("README.md"), "# proj\n\nedited on the desktop\n").unwrap();
            }
            // Give a stop request time to land between steps.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = interrupt.cancelled() => {
                    send(done(DoneStatus::Interrupted));
                    return;
                }
            }
            send(AgentEvent::TextDelta {
                text: "Done.".into(),
            });
            send(done(DoneStatus::Completed));
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: None,
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed).to_string()
}

// ── engines ─────────────────────────────────────────────────────────────

struct Device {
    runtime: EngineRuntime,
    rpc: RpcClient,
    id: String,
}

async fn device(root: &Path, name: &str, edge_url: &str, script: &Arc<Script>, shot: &Path) -> Device {
    let dir = root.join(name);
    let config = EngineConfig {
        data_dir: dir.clone(),
        edge_url: String::new(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: None,
        dev_user_id: None,
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
    runtime.core().registry.register(Arc::new(ScriptedAgent {
        device: name.to_owned(),
        script: script.clone(),
        screenshot: shot.to_path_buf(),
    }));
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn chat_on(device: &Device, chat_id: &str) -> Option<Chat> {
    device.runtime.core().workspace.chat(chat_id).ok().flatten()
}

fn transcript(device: &Device, chat_id: &str) -> Vec<zeron_doc::SessionMessageEntry> {
    device
        .runtime
        .core()
        .doc_host
        .open(chat_id)
        .and_then(|h| Ok(h.doc().read_entries()?))
        .unwrap_or_default()
}

async fn send(device: &Device, chat_id: &str, cwd: &Path, prompt: &str) {
    let request = RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
    };
    let command = SessionCommandPayload::Run {
        request,
        message_id: format!("m-{}", uuid_like()),
    };
    device
        .rpc
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({ "chatId": chat_id, "command": command }),
        )
        .await
        .expect("queue a run");
}

async fn idle(device: &Device, chat_id: &str) {
    let sessions = device.runtime.core().sessions.clone();
    let chat = chat_id.to_owned();
    wait_for("the turn to end", Duration::from_secs(60), || {
        (!sessions.turn_in_flight(&chat)).then_some(())
    })
    .await;
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

// ── the scenario ────────────────────────────────────────────────────────

#[test]
fn a_live_chat_moves_to_another_device_and_back() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // One process stands in for two computers: both share this home (so
    // the desktop can't land on the laptop's own folder and picks its own
    // places), and neither touches the real one.
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
    // The laptop's project: a repo with history, an unpushed commit, an
    // untracked file, an ignored .env and an ignored node_modules.
    let project = home.join("dev/proj");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join("node_modules/left-pad")).unwrap();
    git(&project, &["init", "-q"]);
    std::fs::write(project.join("README.md"), "# proj\n").unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join(".gitignore"), ".env\nnode_modules/\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-qm", "first"]);
    git(&project, &["checkout", "-qb", "feature"]);
    std::fs::write(project.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-qm", "second"]);
    std::fs::write(project.join("notes.txt"), "untracked\n").unwrap();
    std::fs::write(project.join(".env"), "SECRET=1\n").unwrap();
    std::fs::write(project.join("node_modules/left-pad/index.js"), "x\n").unwrap();
    let head = git(&project, &["rev-parse", "HEAD"]);
    // A screenshot elsewhere on disk the session used.
    let shot = home.join("Desktop/shot.png");
    std::fs::create_dir_all(shot.parent().unwrap()).unwrap();
    std::fs::write(&shot, b"\x89PNG fake").unwrap();

    let script = Arc::new(Script::default());
    let edge = LocalEdge::start(LocalEdgeConfig::loopback(root.join("edge"), 0, TOKEN))
        .await
        .unwrap();
    let laptop = device(root, "laptop", &edge.url(), &script, &shot).await;
    let desktop = device(root, "desktop", &edge.url(), &script, &shot).await;
    for (a, b) in [(&laptop, &desktop), (&desktop, &laptop)] {
        let workspace = a.runtime.core().workspace.clone();
        let other = b.id.clone();
        wait_for("peer presence", Duration::from_secs(60), || {
            (workspace
                .read_devices()
                .unwrap_or_default()
                .iter()
                .any(|d| d.id == other && d.supports(capabilities::SESSION_MOVE_V1))
                && workspace.peer_liveness(&other) == zeron_rpc::PeerLiveness::Live)
                .then_some(())
        })
        .await;
    }

    // A chat on the laptop, in the project.
    let space_id = "space-proj";
    let chat_id = "chat-move-1";
    laptop
        .rpc
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "createSpace", "spaceId": space_id, "deviceId": laptop.id,
                "path": project, "name": "proj", "gitDetected": true }),
        )
        .await
        .unwrap();
    laptop
        .rpc
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": chat_id, "spaceId": space_id,
                "config": ChatConfig { harness: HarnessId::Mock, model: None, reasoning: None,
                    model_options: serde_json::Map::new(), sandbox: SandboxLevel::WorkspaceWrite } }),
        )
        .await
        .unwrap();
    laptop
        .rpc
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "renameChat", "chatId": chat_id, "title": "Ship it" }),
        )
        .await
        .ok();
    send(&laptop, chat_id, &project, "please look at the screenshot").await;
    idle(&laptop, chat_id).await;
    wait_for("the chat on the desktop's registry", Duration::from_secs(30), || {
        chat_on(&desktop, chat_id).filter(|c| c.on_chat2()).map(|_| ())
    })
    .await;

    // ── 1. move mid-step, at the next safe point ───────────────────────
    send(&laptop, chat_id, &project, "now do the long step").await;
    wait_for("the long step", Duration::from_secs(30), || {
        (*script.in_step.lock().unwrap()).then_some(())
    })
    .await;
    let started = laptop
        .rpc
        .call(
            methods::START_MOVE,
            serde_json::json!({ "chatId": chat_id, "toDeviceId": desktop.id }),
        )
        .await
        .expect("move starts");
    assert!(started["moveId"].is_string());
    let waiting = wait_for("the move to wait for the step", Duration::from_secs(120), || {
        chat_on(&laptop, chat_id)
            .and_then(|c| c.move_state)
            .filter(|m| m.phase == MovePhase::Waiting || m.phase.is_terminal())
    })
    .await;
    assert_eq!(waiting.phase, MovePhase::Waiting, "{waiting:#?}");
    assert!(
        waiting.detail.as_deref().unwrap_or_default().contains("cargo build"),
        "{waiting:#?}"
    );
    assert_eq!(chat_on(&laptop, chat_id).unwrap().device_id, laptop.id, "still on the laptop");
    script.release.notify_waiters();
    let moved = wait_for("the desktop to host the chat", Duration::from_secs(180), || {
        let chat = chat_on(&laptop, chat_id)?;
        let state = chat.move_state.clone()?;
        assert!(
            !matches!(state.phase, MovePhase::Failed | MovePhase::Cancelled),
            "move ended: {state:#?}"
        );
        (state.phase == MovePhase::Done).then_some(chat)
    })
    .await;
    assert_eq!(moved.device_id, desktop.id);
    let landing = PathBuf::from(moved.cwd.clone().unwrap());
    assert_ne!(landing, project, "both 'devices' share a disk: the desktop picks its own folder");
    assert!(landing.starts_with(home), "{}", landing.display());
    // The workspace: tracked, committed, untracked, ignored; never node_modules.
    for (file, body) in [
        ("README.md", "# proj\n"),
        ("src/lib.rs", "pub fn f() {}\n"),
        ("notes.txt", "untracked\n"),
        (".env", "SECRET=1\n"),
        ("built.txt", "built\n"),
    ] {
        assert_eq!(
            std::fs::read_to_string(landing.join(file)).unwrap_or_default(),
            body,
            "{file} on the desktop"
        );
    }
    assert!(!landing.join("node_modules").exists());
    assert_eq!(git(&landing, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&landing, &["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
    // The screenshot came along (its usual place is taken on this shared disk).
    let moves_dir = home.join("Zeron Transfers/Moves/Ship it");
    assert_eq!(std::fs::read(moves_dir.join("shot.png")).unwrap(), b"\x89PNG fake");
    // The transcript shows the seam; the desktop continued the turn with the note.
    let entries = transcript(&desktop, chat_id);
    assert!(
        entries
            .iter()
            .flat_map(|e| e.parts.iter())
            .any(|p| matches!(p, MessagePart::Moved { seam, .. } if seam.to_device_name.len() > 0)),
        "a Moved seam; desktop: {:?}; laptop: {:?}",
        entries.iter().map(|e| (e.role, e.parts.iter().map(|p| p.id().to_owned()).collect::<Vec<_>>())).collect::<Vec<_>>(),
        transcript(&laptop, chat_id).iter().map(|e| (e.role, e.parts.len())).collect::<Vec<_>>()
    );
    let continued = wait_for("the desktop's agent to continue", Duration::from_secs(60), || {
        script
            .prompts
            .lock()
            .unwrap()
            .iter()
            .find(|(device, prompt)| device == "desktop" && prompt.contains("just moved"))
            .cloned()
    })
    .await;
    let note = continued.1;
    assert!(note.contains("Continue where you left off."), "{note}");
    assert!(note.contains(&landing.display().to_string()), "paths in the note: {note}");
    assert!(note.contains("Restart the dev server"), "the scout's advice: {note}");
    assert!(note.contains("<conversation>"), "history replayed for an unportable agent: {note}");
    idle(&desktop, chat_id).await;

    // ── 2. back to the laptop, at once: only what changed travels ──────
    send(&desktop, chat_id, &landing, "edit the readme").await;
    idle(&desktop, chat_id).await;
    desktop
        .rpc
        .call(
            methods::START_MOVE,
            serde_json::json!({ "chatId": chat_id, "toDeviceId": laptop.id, "when": "now" }),
        )
        .await
        .expect("move back starts");
    let back = wait_for("the laptop to host the chat again", Duration::from_secs(180), || {
        let chat = chat_on(&desktop, chat_id)?;
        let state = chat.move_state.clone()?;
        if state.from_device_id != desktop.id {
            return None;
        }
        assert!(
            !matches!(state.phase, MovePhase::Failed | MovePhase::Cancelled),
            "move back ended: {state:#?}"
        );
        (state.phase == MovePhase::Done).then_some(chat)
    })
    .await;
    assert_eq!(back.device_id, laptop.id);
    assert_eq!(PathBuf::from(back.cwd.unwrap()), project, "home again, where it started");
    assert_eq!(
        std::fs::read_to_string(project.join("README.md")).unwrap(),
        "# proj\n\nedited on the desktop\n"
    );
    let seams: Vec<zeron_doc::MoveSeam> = transcript(&laptop, chat_id)
        .iter()
        .flat_map(|e| e.parts.iter())
        .filter_map(|p| match p {
            MessagePart::Moved { seam, .. } => Some(seam.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(seams.len(), 2, "{seams:#?}");
    assert!(
        seams[1].files_sent <= 3,
        "only the edited file (and the agent's state) travels back: {:#?}",
        seams[1]
    );

    // ── 3. a cancelled move changes nothing ────────────────────────────
    send(&laptop, chat_id, &project, "another long step").await;
    wait_for("the long step", Duration::from_secs(30), || {
        (*script.in_step.lock().unwrap()).then_some(())
    })
    .await;
    laptop
        .rpc
        .call(
            methods::START_MOVE,
            serde_json::json!({ "chatId": chat_id, "toDeviceId": desktop.id }),
        )
        .await
        .unwrap();
    wait_for("the move to wait", Duration::from_secs(120), || {
        chat_on(&laptop, chat_id)
            .and_then(|c| c.move_state)
            .filter(|m| m.phase == MovePhase::Waiting)
    })
    .await;
    laptop
        .rpc
        .call(methods::CANCEL_MOVE, serde_json::json!({ "chatId": chat_id }))
        .await
        .unwrap();
    wait_for("the cancel", Duration::from_secs(60), || {
        chat_on(&laptop, chat_id)
            .and_then(|c| c.move_state)
            .filter(|m| m.phase == MovePhase::Cancelled)
    })
    .await;
    assert_eq!(chat_on(&laptop, chat_id).unwrap().device_id, laptop.id);
    assert!(*script.in_step.lock().unwrap(), "the agent kept working");
    script.release.notify_waiters();
    idle(&laptop, chat_id).await;

    // ── 4. to the desktop again: its earlier landing is reused, and a file
    //       the user made there meanwhile is kept, not deleted ──────────
    std::fs::write(landing.join("desktop-notes.md"), "mine\n").unwrap();
    std::fs::write(project.join("README.md"), "# proj\n\nback on the laptop\n").unwrap();
    laptop
        .rpc
        .call(
            methods::START_MOVE,
            serde_json::json!({ "chatId": chat_id, "toDeviceId": desktop.id, "when": "now" }),
        )
        .await
        .expect("second move starts");
    let again = wait_for("the desktop to host the chat again", Duration::from_secs(180), || {
        let chat = chat_on(&laptop, chat_id)?;
        let state = chat.move_state.clone()?;
        if state.from_device_id != laptop.id || state.to_device_id != desktop.id {
            return None;
        }
        assert!(
            !matches!(state.phase, MovePhase::Failed | MovePhase::Cancelled) || state.started_at < 0,
            "second move ended: {state:#?}"
        );
        (state.phase == MovePhase::Done && chat.device_id == desktop.id).then_some(chat)
    })
    .await;
    assert_eq!(PathBuf::from(again.cwd.unwrap()), landing, "the earlier landing is reused");
    assert_eq!(
        std::fs::read_to_string(landing.join("desktop-notes.md")).unwrap(),
        "mine\n",
        "a file the user added on the desktop survives"
    );
    assert_eq!(
        std::fs::read_to_string(landing.join("README.md")).unwrap(),
        "# proj\n\nback on the laptop\n"
    );

    laptop.runtime.shutdown().await;
    desktop.runtime.shutdown().await;
    edge.shutdown().await;
}
