//! Real EngineCore served exclusively through the supported outbound host relay.
//! Arguments: edge-url data-dir owner-id organization-id label.
//! stdin controls: disconnect, reconnect, shutdown. No local RPC/IPC listener.
use std::{future::Future, path::PathBuf, sync::Arc};

use futures::stream::BoxStream;
use tokio::io::{AsyncBufReadExt, BufReader};
use zeron_engine::{
    Auth, AuthConfig, EdgeConfig, EngineCore, EngineProfile, HarnessId, HarnessRegistry,
};
use zeron_harness::mock::MockHarness;
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{AgentEvent, DoneStatus, Model, ReasoningLevel, RunRequest, SteeringMode};
use zeron_rpc::{LinkCache, LinkCacheConfig, RpcService};

/**
 * Fixture-only provider: it deliberately owns the ordinary ClaudeCode id so
 * the real registry reports it installed and enabled. Its event stream remains
 * scripted; it never claims to be a locally installed Claude CLI.
 */
struct ScriptedClaudeHarness {
    inner: MockHarness,
}

#[async_trait::async_trait]
impl Harness for ScriptedClaudeHarness {
    fn id(&self) -> HarnessId {
        HarnessId::ClaudeCode
    }

    fn display_name(&self) -> &str {
        "Claude Code (scripted fixture)"
    }

    fn supports_steering(&self) -> bool {
        self.inner.supports_steering()
    }

    fn steering_mode(&self) -> SteeringMode {
        self.inner.steering_mode()
    }

    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        self.inner.reasoning_levels()
    }

    fn installed(&self) -> bool {
        true
    }

    fn deterministic_turn_end(&self) -> bool {
        true
    }

    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.inner.models().await
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.inner.run(request, controls).await
    }
}

fn scripted_events(label: &str, harness: HarnessId, cwd: &std::path::Path) -> Vec<AgentEvent> {
    vec![
        AgentEvent::SessionStarted {
            harness,
            model: "mock-1".into(),
            tools: vec![],
            cwd: cwd.to_string_lossy().into_owned(),
            session_id: format!("{label}-session"),
            assistant_message_id: format!("{label}-assistant"),
        },
        AgentEvent::TextDelta {
            text: format!("Reply from {label}. "),
        },
        AgentEvent::TextDelta {
            text: "Deterministic fixture response.".into(),
        },
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some(format!("{label}-session")),
        },
    ]
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() == 5,
        "usage: browser_relay edge-url data-dir owner-id organization-id label"
    );
    let [edge_url, data_dir, owner, org, label] = <[String; 5]>::try_from(args).unwrap();
    let url = reqwest::Url::parse(&edge_url)?;
    anyhow::ensure!(
        url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        "fixture edge must be loopback HTTP"
    );
    for id in [&owner, &org, &label] {
        anyhow::ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "invalid fixture identity"
        );
    }
    let data_dir = PathBuf::from(data_dir);
    let project_root = data_dir.join("fixture-project");
    std::fs::create_dir_all(&project_root)?;
    if !project_root.join(".git").exists() {
        anyhow::ensure!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&project_root)
                .status()?
                .success(),
            "fixture git init failed"
        );
        std::fs::write(
            project_root.join("fixture.txt"),
            format!("Project on {label}\n"),
        )?;
    }
    let registry = Arc::new(HarnessRegistry::new());
    let scripted_claude = std::env::var("ZERON_BROWSER_RELAY_SCRIPTED_CLAUDE")
        .map(|value| value != "0")
        .unwrap_or(true);
    // Keep Mock registered for explicit wire-contract and no-agent tests. The
    // native registry intentionally excludes it from automatic enablement.
    registry.register(Arc::new(MockHarness {
        script: scripted_events(&label, HarnessId::Mock, &project_root),
    }));
    if scripted_claude {
        registry.register(Arc::new(ScriptedClaudeHarness {
            inner: MockHarness {
                script: scripted_events(&label, HarnessId::ClaudeCode, &project_root),
            },
        }));
    }
    let default_harness = if scripted_claude {
        HarnessId::ClaudeCode
    } else {
        HarnessId::Mock
    };
    let mut auth_config = AuthConfig::new(&edge_url, &data_dir);
    auth_config.dev_user_id = format!("{owner}@{org}");
    let auth = Auth::new(auth_config);
    let edge = EdgeConfig::new(&edge_url, Arc::new(auth.clone()));
    let core = EngineCore::assemble_with_profile(
        EngineProfile::development(&data_dir, &org, &owner),
        registry,
        default_harness,
        Some(edge),
    )?;
    core.set_auth(auth.clone());
    let mut link_config = LinkCacheConfig::new(&edge_url, Arc::new(auth));
    let workspace = core.workspace.clone();
    link_config.liveness = Some(Arc::new(move |device: &str| {
        workspace.peer_liveness(device)
    }));
    let links = LinkCache::new(link_config);
    let presence_links = links.clone();
    core.workspace
        .set_peer_alive_hook(Arc::new(move |device: &str| {
            presence_links.reset_cooldown(device)
        }));
    core.set_links(links);

    let space_id = format!("{label}-space");
    let chat_id = format!("{label}-chat");
    // Seed through the real dispatcher. Persistent state is not recreated on restart.
    let service = core.rpc_service();
    service
        .handle(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({"op": "renameDevice", "deviceId": core.device_id, "name": label}),
        )
        .await?;
    if !core
        .workspace
        .read_chats()?
        .iter()
        .any(|chat| chat.id == chat_id)
    {
        for params in [
            serde_json::json!({"op": "createSpace", "spaceId": space_id, "deviceId": core.device_id, "path": project_root}),
            serde_json::json!({"op": "createChat", "chatId": chat_id, "deviceId": core.device_id, "spaceId": space_id}),
            serde_json::json!({"op": "renameChat", "chatId": chat_id, "title": format!("Chat on {label}")}),
            serde_json::json!({"op": "setChatCwd", "chatId": chat_id, "cwd": project_root}),
        ] {
            service.handle(zeron_rpc::methods::MUTATE, params).await?;
        }
    }
    let mut relay = Some(core.start_host_relay(&edge_url));
    let shutdown = shutdown_signal()?;
    tokio::pin!(shutdown);
    println!(
        "BROWSER RELAY {}",
        serde_json::json!({
            "deviceId": core.device_id, "ownerId": owner, "organizationId": org,
            "label": label, "projectRoot": project_root, "chatId": chat_id, "spaceId": space_id,
        })
    );
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = tokio::select! {
            _ = &mut shutdown => break,
            line = input.next_line() => line?,
        };
        match line.as_deref() {
            Some("disconnect") => {
                relay.take();
            }
            Some("reconnect") => {
                if relay.is_none() {
                    relay = Some(core.start_host_relay(&edge_url));
                }
            }
            Some("shutdown") | None => break,
            Some(other) => anyhow::bail!("unknown fixture command: {other}"),
        }
        println!("BROWSER CONTROL {}", line.unwrap());
    }
    drop(relay);
    core.shutdown().await;
    Ok(())
}

#[cfg(unix)]
fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        tokio::select! { _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
    })
}

#[cfg(not(unix))]
fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}
