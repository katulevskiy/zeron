//! Cloud boxes: the engine side (docs/cloud.md §4–§5).
//!
//! A box is an ordinary engine in a container that sleeps when idle. What
//! it adds is here:
//! - [`checkpoint`]: content-addressed checkpoints of its state to the
//!   checkpoint store, and the restore at boot (lazy for workspaces);
//! - [`control`]: busy detection, periodic and final checkpoints, and the
//!   control server the box's Durable Object polls;
//! - [`activity`]: RPC calls and streams, the "connected viewer" signal;
//! - [`CloudBoot`]: `zeron cloud-boot`, which ties them to a headless engine.

pub mod activity;
#[cfg(unix)]
pub mod checkpoint;
#[cfg(unix)]
pub mod control;
#[cfg(unix)]
pub mod store;

#[cfg(unix)]
pub use boot::{BootedBox, CloudBoot, CloudEnv, CoreBox, DRAIN_WAIT};

#[cfg(unix)]
mod boot {
    use std::future::Future;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::Context;
    use futures::FutureExt;
    use zeron_proto::SessionStatus;

    use super::activity::RpcActivity;
    use super::checkpoint::{CheckpointStats, Checkpointer, CloudPaths};
    use super::control::{Activity, BoxEngine, CHECKPOINT_EVERY, CloudBox};
    use super::store::CheckpointStore;
    use crate::{
        DocHost, Engine, EngineConfig, EngineCore, RunningEngine, SessionsEngine, Terminals,
        WorkspaceHost,
    };

    /// How long a stopping box waits for live runs to finish before it
    /// interrupts them (Cloudflare allows 15 minutes after SIGTERM; the
    /// rest is for the final checkpoint).
    pub const DRAIN_WAIT: Duration = Duration::from_secs(10 * 60);

    /// The environment the Durable Object starts a box with (§3).
    #[derive(Debug, Clone)]
    pub struct CloudEnv {
        pub device_id: Option<String>,
        /// `ZERON_CHECKPOINT_URL`.
        pub checkpoint_url: String,
        /// `ZERON_CLOUD_CONTROL_PORT`; no control server without it.
        pub control_port: Option<u16>,
        /// `ZERON_CONTROL_TOKEN`.
        pub control_token: Option<String>,
    }

    impl CloudEnv {
        pub fn from_env() -> anyhow::Result<Self> {
            let var = |name: &str| {
                std::env::var(name)
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            let checkpoint_url =
                var("ZERON_CHECKPOINT_URL").context("ZERON_CHECKPOINT_URL is not set")?;
            let control_port = var("ZERON_CLOUD_CONTROL_PORT")
                .map(|p| p.parse::<u16>())
                .transpose()
                .context("ZERON_CLOUD_CONTROL_PORT is not a port")?;
            let control_token = var("ZERON_CONTROL_TOKEN");
            anyhow::ensure!(
                control_port.is_none() || control_token.is_some(),
                "ZERON_CLOUD_CONTROL_PORT is set but ZERON_CONTROL_TOKEN is not"
            );
            Ok(Self {
                device_id: var("ZERON_DEVICE_ID"),
                checkpoint_url,
                control_port,
                control_token,
            })
        }
    }

    /// `zeron cloud-boot`: restore `data` and `harness`, run the headless
    /// engine with the control server and checkpoints attached, and on
    /// shutdown drain, stop the engine and write a final checkpoint.
    pub struct CloudBoot {
        pub config: EngineConfig,
        pub env: CloudEnv,
        pub paths: CloudPaths,
        pub drain_wait: Duration,
        pub checkpoint_every: Duration,
    }

    impl CloudBoot {
        pub fn new(config: EngineConfig, env: CloudEnv) -> Self {
            let paths = CloudPaths {
                data_dir: config.data_dir.clone(),
                home: crate::repos::home_dir(),
            };
            Self {
                config,
                env,
                paths,
                drain_wait: DRAIN_WAIT,
                checkpoint_every: CHECKPOINT_EVERY,
            }
        }

        /// Run until `shutdown` resolves (SIGTERM) or the engine stops by
        /// itself, then [`BootedBox::stop`].
        pub async fn run(self, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
            let mut booted = self.start().await?;
            tokio::pin!(shutdown);
            tokio::select! {
                _ = &mut shutdown => tracing::info!("cloud box: stopping"),
                _ = booted.running.stop_requested() => {}
            }
            booted.stop().await.map(|_| ())
        }

        /// Restore `data` and `harness`, start the engine, and attach the
        /// lazy workspace restore, the control server and the periodic
        /// checkpoints.
        pub async fn start(self) -> anyhow::Result<BootedBox> {
            let store = CheckpointStore::new(&self.env.checkpoint_url);
            let checkpointer = Checkpointer::new(store, self.paths.clone());
            let restored = checkpointer
                .restore_boot()
                .await
                .context("restoring the box from its checkpoint")?;
            tracing::info!(device_id = ?self.env.device_id, seq = ?restored.seq,
                workspaces = restored.pending.len(), "cloud box booting");

            let running = Engine::new(self.config).start().await?;
            let core = running.runtime().core();
            let restorer = checkpointer.clone();
            core.sessions.set_cwd_preparer(Arc::new(move |cwd: PathBuf| {
                let restorer = restorer.clone();
                async move {
                    if let Err(error) = restorer.restore_workspace(&cwd).await {
                        tracing::error!(cwd = %cwd.display(), error = %format!("{error:#}"),
                            "cloud box: workspace restore failed");
                    }
                }
                .boxed()
            }));
            let cloud = CloudBox::new(Arc::new(CoreBox::new(core)), checkpointer);
            let background = cloud.spawn_background(self.checkpoint_every);
            let control = match (self.env.control_port, self.env.control_token.clone()) {
                (Some(port), Some(token)) => Some(
                    cloud
                        .serve(std::net::SocketAddr::from(([0, 0, 0, 0], port)), token)
                        .await?,
                ),
                _ => None,
            };
            Ok(BootedBox {
                running,
                cloud,
                background,
                control,
                drain_wait: self.drain_wait,
            })
        }
    }

    /// A box whose engine is running (see [`CloudBoot::start`]).
    pub struct BootedBox {
        running: RunningEngine,
        cloud: CloudBox,
        background: tokio::task::JoinHandle<()>,
        control: Option<(std::net::SocketAddr, tokio::task::JoinHandle<()>)>,
        drain_wait: Duration,
    }

    impl BootedBox {
        pub fn runtime(&self) -> &crate::EngineRuntime {
            self.running.runtime()
        }

        pub fn cloud(&self) -> &CloudBox {
            &self.cloud
        }

        /// Where the control server listens, if it runs.
        pub fn control_addr(&self) -> Option<std::net::SocketAddr> {
            self.control.as_ref().map(|(addr, _)| *addr)
        }

        /// Stop: hold every chat, give live runs up to the drain window to
        /// finish, stop the engine gracefully, then write the final
        /// checkpoint.
        pub async fn stop(self) -> anyhow::Result<CheckpointStats> {
            self.background.abort();
            // The workspace list comes from the registry, which closes with
            // the engine.
            let roots = self.cloud.roots().await;
            self.cloud.hold_all();
            let deadline = tokio::time::Instant::now() + self.drain_wait;
            while self.cloud.engine().activity().runs > 0 && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            self.running.shutdown().await;
            let result = self.cloud.checkpointer().checkpoint(roots).await;
            if let Some((_, control)) = self.control {
                control.abort();
            }
            let stats = result.context("final checkpoint")?;
            tracing::info!(seq = stats.seq, objects = stats.objects, bytes = stats.bytes,
                "cloud box: final checkpoint written");
            Ok(stats)
        }
    }

    /// [`BoxEngine`] over an assembled engine.
    pub struct CoreBox {
        device_id: String,
        sessions: SessionsEngine,
        terminals: Terminals,
        workspace: WorkspaceHost,
        doc_host: DocHost,
        rpc: Arc<RpcActivity>,
    }

    impl CoreBox {
        pub fn new(core: &EngineCore) -> Self {
            Self {
                device_id: core.device_id.clone(),
                sessions: core.sessions.clone(),
                terminals: core.terminals.clone(),
                workspace: core.workspace.clone(),
                doc_host: core.doc_host.clone(),
                rpc: core.rpc_activity.clone(),
            }
        }

        fn hosted_chats(&self) -> Vec<zeron_proto::Chat> {
            self.workspace
                .read_chats()
                .unwrap_or_default()
                .into_iter()
                .filter(|chat| chat.device_id == self.device_id)
                .collect()
        }
    }

    impl BoxEngine for CoreBox {
        fn activity(&self) -> Activity {
            let runs = self
                .sessions
                .watch_sessions()
                .borrow()
                .iter()
                .filter(|s| matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput))
                .count();
            let moves = self
                .workspace
                .read_chats()
                .unwrap_or_default()
                .iter()
                .filter_map(|chat| chat.move_state.as_ref())
                .filter(|m| {
                    m.is_live()
                        && (m.from_device_id == self.device_id || m.to_device_id == self.device_id)
                })
                .count();
            Activity {
                runs,
                moves,
                terminals: self.terminals.open_count(),
                viewers: self.rpc.live(),
                last_rpc_ms: self.rpc.last_call_ms(),
            }
        }

        fn workspace_folders(&self) -> Vec<PathBuf> {
            let chats = self
                .hosted_chats()
                .into_iter()
                .filter_map(|chat| chat.cwd)
                .filter_map(|cwd| crate::repos::expand_home(&cwd).ok());
            let spaces = self
                .workspace
                .read_spaces()
                .unwrap_or_default()
                .into_iter()
                .filter(|space| space.device_id == self.device_id)
                .map(|space| space.path);
            chats.chain(spaces).map(PathBuf::from).collect()
        }

        fn hold_all(&self) -> Vec<String> {
            self.hosted_chats()
                .into_iter()
                // A chat a move already holds stays the move's to release.
                .filter(|chat| !self.doc_host.move_held(&chat.id))
                .map(|chat| {
                    self.doc_host.hold_for_move(&chat.id);
                    chat.id
                })
                .collect()
        }

        fn release(&self, chats: &[String]) {
            for chat in chats {
                self.doc_host.release_move_hold(chat);
            }
        }

        fn flush(&self) {
            self.doc_host.flush_all();
        }
    }
}
