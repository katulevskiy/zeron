//! Deleting a chat must retire active and parked provider streams before the
//! document is purged. A completed steering turn still owns a warm run.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{CancellationToken, Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_rpc::{RpcService, methods};

struct ProviderLifetime;

struct PersistentHarness {
    parked: bool,
    lifetime: Mutex<Option<Weak<ProviderLifetime>>>,
    cancel: Mutex<Option<CancellationToken>>,
}

#[async_trait]
impl Harness for PersistentHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }

    fn display_name(&self) -> &str {
        "Deletion fixture"
    }

    fn supports_steering(&self) -> bool {
        true
    }

    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }

    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }

    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(Vec::new())
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        // Auto-titling can invoke the same harness after the completed turn.
        if request.prompt != "deletion fixture" {
            return Ok(futures::stream::empty().boxed());
        }
        let lifetime = Arc::new(ProviderLifetime);
        *self.lifetime.lock().unwrap() = Some(Arc::downgrade(&lifetime));
        *self.cancel.lock().unwrap() = Some(controls.interrupt.clone());
        let mut events = VecDeque::from([
            AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "fixture".into(),
                tools: Vec::new(),
                cwd: request.cwd,
                session_id: "provider-session".into(),
                assistant_message_id: "assistant-fixture".into(),
            },
            AgentEvent::TextDelta {
                text: "provider reply".into(),
            },
        ]);
        if self.parked {
            events.push_back(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("provider-session".into()),
            });
        }
        Ok(futures::stream::unfold(
            (events, controls.interrupt, lifetime),
            |(mut events, cancel, lifetime)| async move {
                if let Some(event) = events.pop_front() {
                    return Some((Ok(event), (events, cancel, lifetime)));
                }
                cancel.cancelled().await;
                None
            },
        )
        .boxed())
    }
}

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("deleted chat releases its provider and document");
}

fn request(cwd: &std::path::Path) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: "deletion fixture".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

async fn exercise_deletion(parked: bool, delete_space: bool) {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(PersistentHarness {
        parked,
        lifetime: Mutex::new(None),
        cancel: Mutex::new(None),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let core = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    core.workspace
        .create_space(
            "space",
            &core.device_id,
            &dir.path().to_string_lossy(),
            None,
            false,
        )
        .unwrap();
    core.workspace
        .create_chat("chat", Some("space"), None, None, None)
        .unwrap();
    let handle = core.doc_host.open("chat").unwrap();
    let document = Arc::downgrade(&handle);
    drop(handle);
    core.sessions
        .dispatch("chat", HarnessId::Mock, request(dir.path()), None)
        .await
        .unwrap();
    wait_for(|| {
        harness.lifetime.lock().unwrap().is_some()
            && core.sessions.session_status("chat").is_some_and(|s| {
                s.status
                    == if parked {
                        SessionStatus::Idle
                    } else {
                        SessionStatus::Working
                    }
            })
    })
    .await;
    assert!(core.sessions.live_run_steerable("chat"));
    let lifetime = harness.lifetime.lock().unwrap().clone().unwrap();
    let cancel = harness.cancel.lock().unwrap().clone().unwrap();
    assert!(lifetime.upgrade().is_some());

    core.rpc_service()
        .handle(
            methods::MUTATE,
            if delete_space {
                serde_json::json!({ "op": "deleteSpace", "spaceId": "space" })
            } else {
                serde_json::json!({ "op": "deleteChat", "chatId": "chat" })
            },
        )
        .await
        .unwrap();
    assert!(core.workspace.chat("chat").unwrap().is_none());
    wait_for(|| {
        cancel.is_cancelled()
            && lifetime.upgrade().is_none()
            && !core.sessions.live_run_steerable("chat")
            && document.upgrade().is_none()
    })
    .await;
    assert!(core.workspace.chat("chat").unwrap().is_none());
    assert!(core.workspace.read_sessions().unwrap().is_empty());
    let delayed = core
        .sessions
        .dispatch("chat", HarnessId::Mock, request(dir.path()), None)
        .await
        .unwrap_err();
    assert!(delayed.to_string().contains("chat was deleted"));
    assert!(
        core.sessions
            .steer("chat", "late steer", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("chat was deleted")
    );
    assert!(document.upgrade().is_none());
    core.shutdown().await;
}

#[tokio::test]
async fn deleting_chat_stops_active_provider_and_releases_document() {
    exercise_deletion(false, false).await;
}

#[tokio::test]
async fn deleting_chat_stops_parked_provider_and_releases_document() {
    exercise_deletion(true, false).await;
}

#[tokio::test]
async fn deleting_space_still_stops_parked_provider_and_releases_document() {
    exercise_deletion(true, true).await;
}

async fn exercise_remote_provider_deletion(parked: bool) {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let source = EngineCore::assemble(
        source_dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let harness = Arc::new(PersistentHarness {
        parked,
        lifetime: Mutex::new(None),
        cancel: Mutex::new(None),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let target = EngineCore::assemble(target_dir.path(), registry, HarnessId::Mock, None).unwrap();
    let server = zeron_sync::registry::mock_server::MockRegistryServer::start().await;
    source.workspace.connect_registry_url(&server.url());
    target.workspace.connect_registry_url(&server.url());
    source
        .workspace
        .create_chat("remote-live", None, Some(&target.device_id), None, None)
        .unwrap();
    wait_for(|| target.workspace.chat("remote-live").unwrap().is_some()).await;
    let handle = target.doc_host.open("remote-live").unwrap();
    let document = Arc::downgrade(&handle);
    drop(handle);
    target
        .sessions
        .dispatch(
            "remote-live",
            HarnessId::Mock,
            request(target_dir.path()),
            None,
        )
        .await
        .unwrap();
    wait_for(|| {
        harness.lifetime.lock().unwrap().is_some()
            && target
                .sessions
                .session_status("remote-live")
                .is_some_and(|s| {
                    s.status
                        == if parked {
                            SessionStatus::Idle
                        } else {
                            SessionStatus::Working
                        }
                })
    })
    .await;
    let lifetime = harness.lifetime.lock().unwrap().clone().unwrap();
    let cancel = harness.cancel.lock().unwrap().clone().unwrap();
    // No target DeleteChat RPC: only the remote registry tombstone arrives.
    source.workspace.delete_chat("remote-live").unwrap();
    wait_for(|| {
        target.workspace.chat("remote-live").unwrap().is_none()
            && cancel.is_cancelled()
            && lifetime.upgrade().is_none()
            && document.upgrade().is_none()
            && !target.sessions.live_run_steerable("remote-live")
    })
    .await;
    assert!(target.workspace.read_sessions().unwrap().is_empty());
    target.shutdown().await;
    source.shutdown().await;
}

#[tokio::test]
async fn remote_deletion_stops_active_provider_and_releases_document() {
    exercise_remote_provider_deletion(false).await;
}

#[tokio::test]
async fn remote_deletion_stops_parked_provider_and_releases_document() {
    exercise_remote_provider_deletion(true).await;
}

#[tokio::test]
async fn queued_deletion_does_not_purge_an_explicitly_restored_provider() {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(PersistentHarness {
        parked: false,
        lifetime: Mutex::new(None),
        cancel: Mutex::new(None),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let core = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    core.workspace
        .create_chat("restored", None, Some(&core.device_id), None, None)
        .unwrap();
    core.sessions
        .dispatch("restored", HarnessId::Mock, request(dir.path()), None)
        .await
        .unwrap();
    wait_for(|| harness.lifetime.lock().unwrap().is_some()).await;
    let original_provider = harness.lifetime.lock().unwrap().clone().unwrap();
    let handle = core.doc_host.open("restored").unwrap();
    let original = Arc::downgrade(&handle);
    drop(handle);
    // The current-thread executor cannot run queued teardown between these
    // synchronous writes, exercising a delete/restore coalesced by WatchChats.
    core.rpc_service()
        .handle(
            methods::MUTATE,
            serde_json::json!({ "op": "deleteChat", "chatId": "restored" }),
        )
        .await
        .unwrap();
    core.workspace
        .create_chat("restored", None, Some(&core.device_id), None, None)
        .unwrap();
    let mut replacement = request(dir.path());
    replacement.model = Some("replacement-model".into());
    core.sessions
        .dispatch("restored", HarnessId::Mock, replacement, None)
        .await
        .unwrap();
    wait_for(|| {
        harness
            .lifetime
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|weak| !Weak::ptr_eq(weak, &original_provider) && weak.upgrade().is_some())
    })
    .await;
    let cancel = harness.cancel.lock().unwrap().clone().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!cancel.is_cancelled());
    assert!(core.sessions.live_run_steerable("restored"));
    assert!(original.upgrade().is_some());
    assert!(core.workspace.chat("restored").unwrap().is_some());
    core.shutdown().await;
}

#[tokio::test]
async fn remote_tombstone_rejects_delayed_dispatch_but_explicit_restore_allows_it() {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let source = EngineCore::assemble(
        source_dir.path(),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let harness = Arc::new(PersistentHarness {
        parked: false,
        lifetime: Mutex::new(None),
        cancel: Mutex::new(None),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let target = EngineCore::assemble(target_dir.path(), registry, HarnessId::Mock, None).unwrap();
    let server = zeron_sync::registry::mock_server::MockRegistryServer::start().await;
    source.workspace.connect_registry_url(&server.url());
    target.workspace.connect_registry_url(&server.url());
    source
        .workspace
        .create_chat("remote-chat", None, Some(&target.device_id), None, None)
        .unwrap();
    wait_for(|| target.workspace.chat("remote-chat").unwrap().is_some()).await;
    source.workspace.delete_chat("remote-chat").unwrap();
    wait_for(|| {
        server
            .row("chats", "remote-chat")
            .is_some_and(|row| row.deleted)
            && target.workspace.chat("remote-chat").unwrap().is_none()
    })
    .await;
    assert!(
        target
            .sessions
            .dispatch(
                "remote-chat",
                HarnessId::Mock,
                request(target_dir.path()),
                None
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("chat was deleted")
    );
    assert!(harness.lifetime.lock().unwrap().is_none());
    assert!(target.doc_host.sync_statuses().is_empty());

    source
        .workspace
        .create_chat("remote-chat", None, Some(&target.device_id), None, None)
        .unwrap();
    wait_for(|| target.workspace.chat("remote-chat").unwrap().is_some()).await;
    target
        .sessions
        .dispatch(
            "remote-chat",
            HarnessId::Mock,
            request(target_dir.path()),
            None,
        )
        .await
        .unwrap();
    wait_for(|| harness.lifetime.lock().unwrap().is_some()).await;
    assert!(target.sessions.live_run_steerable("remote-chat"));
    target.sessions.interrupt("remote-chat").await.unwrap();

    // Unknown chats remain legal while their create row is still in flight.
    target
        .sessions
        .dispatch(
            "not-yet-created",
            HarnessId::Mock,
            request(target_dir.path()),
            None,
        )
        .await
        .unwrap();
    assert!(target.sessions.live_run_steerable("not-yet-created"));
    target.shutdown().await;
    source.shutdown().await;
}
