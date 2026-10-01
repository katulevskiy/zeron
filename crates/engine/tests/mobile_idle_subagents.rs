//! A mobile follow-up to a desktop-started, parked parent must reuse its runtime.

#[path = "../../client/tests/support/mock_edge.rs"]
mod mock_edge;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use mock_edge::MockEdge;
use zeron_client::events::NullListener;
use zeron_client::{Client, ClientConfig, Credentials, SendOutcome, SendRequest};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionCommandStatus,
    SubagentStatus,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall,
};

const CHAT: &str = "mobile-idle-subagents";
const SPAWN: &str = "spawn-background";

struct BackgroundHarness {
    starts: Arc<AtomicUsize>,
    interrupted: Arc<AtomicBool>,
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("background-session".into()),
    }
}

#[async_trait]
impl Harness for BackgroundHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Background subagent probe"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _request: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let interrupted = self.interrupted.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for event in [
                AgentEvent::ToolCall {
                    id: SPAWN.into(),
                    call: ToolCall::Unknown {
                        name: "Agent: background probe".into(),
                        input: None,
                    },
                },
                AgentEvent::Subagent {
                    parent_tool_use_id: SPAWN.into(),
                    event: Box::new(AgentEvent::TextDelta {
                        text: "child still working".into(),
                    }),
                },
                AgentEvent::TextDelta {
                    text: "parent done".into(),
                },
                done(),
            ] {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            loop {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => {
                        interrupted.store(true, Ordering::SeqCst);
                        return;
                    }
                    message = controls.steering.recv() => {
                        let Some(message) = message else { return };
                        for event in [
                            AgentEvent::Steered {
                                assistant_message_id: None,
                                next_assistant_message_id: None,
                            },
                            AgentEvent::Subagent {
                                parent_tool_use_id: SPAWN.into(),
                                event: Box::new(AgentEvent::TextDelta {
                                    text: "; child survived follow-up".into(),
                                }),
                            },
                            AgentEvent::TextDelta { text: message.prompt },
                            done(),
                        ] {
                            if tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

async fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_send_to_idle_parent_preserves_running_subagents() {
    let edge = MockEdge::start().await;
    let host_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let interrupted = Arc::new(AtomicBool::new(false));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(BackgroundHarness {
        starts: starts.clone(),
        interrupted: interrupted.clone(),
    }));
    let core = EngineCore::assemble_with_identity(
        host_dir.path(),
        Arc::new(registry),
        HarnessId::Mock,
        None,
        "org-1",
        "user-1",
    )
    .unwrap();
    core.workspace.connect_registry_url(&edge.registry.url());
    core.workspace
        .create_chat(
            CHAT,
            None,
            Some(&core.device_id),
            Some(ChatConfig {
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            Some("/tmp".into()),
        )
        .unwrap();
    let handle = core.doc_host.open(CHAT).unwrap();
    // Match the desktop composer, including its approval policy.
    let initial = RunRequest {
        mcp: None,
        prompt: "start background work".into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: vec![],
        resume: None,
        worktree: None,
    };
    core.sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            initial,
            Some("desktop-message".into()),
        )
        .await
        .unwrap();
    wait_for("idle parent with a running child", || {
        core.sessions
            .session_status(CHAT)
            .is_some_and(|s| s.status == SessionStatus::Idle && s.running_subagents == 1)
    })
    .await;
    edge.inject(
        CHAT,
        &core.device_id,
        handle.doc().export_snapshot().unwrap(),
    );

    let mut config = ClientConfig::new(edge.edge_url(), phone_dir.path());
    config.device_id = "android-viewer".into();
    config.platform = "android".into();
    let phone = Client::new(
        config,
        Credentials::Dev {
            user_id: "user-1".into(),
            org_id: "org-1".into(),
        },
        Arc::new(NullListener),
    )
    .unwrap();
    wait_for("remote chat row", || {
        phone
            .workspace()
            .session(CHAT)
            .is_some_and(|s| s.running_subagents == 1)
    })
    .await;
    let session = phone.open_session(CHAT).unwrap();
    session.set_view_attached(true);
    wait_for("remote transcript", || session.snapshot().hydrated).await;
    assert!(!session.composer().live.turn_running);
    let SendOutcome::Started { message_id } = session
        .send(SendRequest::text("follow up from Android"))
        .unwrap()
    else {
        panic!("an idle parent accepts a new turn");
    };
    // Deliver the viewer's real command rows to the host doc, as its chat
    // room would. The production command executor handles the Run.
    wait_for("follow-up applied on the host", || {
        for row in edge.rows(CHAT) {
            handle.doc().doc().import(&row.bytes).unwrap();
        }
        handle.doc().read_commands().unwrap().iter().any(|c| {
            c.status == SessionCommandStatus::Applied
                && matches!(&c.payload, SessionCommandPayload::Run { message_id: id, .. }
                    if id == &message_id)
        })
    })
    .await;

    assert!(
        !interrupted.load(Ordering::SeqCst),
        "mobile send cancelled the runtime"
    );
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "mobile send respawned the harness"
    );
    let status = core.sessions.session_status(CHAT).unwrap();
    assert_eq!(status.running_subagents, 1);
    wait_for("parent reply to the follow-up", || {
        handle.doc().read_entries().unwrap().iter().any(|e| {
            e.role == MessageRole::Assistant
                && e.status == Some(MessageStatus::Complete)
                && e.parts.iter().any(|p| {
                    matches!(p,
                    MessagePart::Text { text, .. } if text == "follow up from Android")
                })
        })
    })
    .await;
    let entries = handle.doc().read_entries().unwrap();
    assert_eq!(entries.iter().filter(|e| e.id == message_id).count(), 1);
    assert!(entries.iter().flat_map(|e| &e.parts).any(|p| matches!(p,
        MessagePart::Tool { id, subagent_status: Some(SubagentStatus::Running), .. }
            if id == SPAWN)));
    let child = core
        .doc_host
        .open(&format!("{CHAT}--sub--{SPAWN}"))
        .unwrap();
    wait_for("child output after the follow-up", || {
        child.doc().read_entries().unwrap().iter().flat_map(|e| &e.parts).any(|p|
            matches!(p, MessagePart::Text { text, .. } if text.contains("child survived follow-up")))
    })
    .await;
    phone.shutdown();
    core.shutdown().await;
}
