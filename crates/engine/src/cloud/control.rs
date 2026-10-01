//! The box side of a cloud box's lifecycle (docs/cloud.md §4): whether the
//! engine is busy, checkpoints on request and on a timer, and the control
//! server the Durable Object polls.
//!
//! - `GET /zeron/cloud/status` → `{busy, lastActivityAt, runs, moves,
//!   terminals, viewers}`.
//! - `POST /zeron/cloud/checkpoint?final=0|1` → `{seq, objects, bytes,
//!   changed}`. A final checkpoint first holds every hosted chat's commands
//!   and queue, as a move does, and keeps them held: the box is about to
//!   stop. If it isn't stopped within [`FINAL_HOLD_MAX`], the holds lift.
//!
//! Every request needs `x-zeron-control: $ZERON_CONTROL_TOKEN`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;

use super::checkpoint::{CheckpointStats, Checkpointer, RootSpec, workspace_roots};

/// Incremental checkpoints run this often (and write only when something
/// changed).
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(10 * 60);
/// A final checkpoint's holds lift if the box is still running after this.
pub const FINAL_HOLD_MAX: Duration = Duration::from_secs(10 * 60);
/// How often busy-ness is sampled into `lastActivityAt`.
const SAMPLE_EVERY: Duration = Duration::from_secs(5);
pub const CONTROL_HEADER: &str = "x-zeron-control";

/// What keeps a box awake right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Activity {
    /// Runs working or waiting for input.
    pub runs: usize,
    /// Moves into or out of this device that haven't finished.
    pub moves: usize,
    /// Open terminals.
    pub terminals: usize,
    /// RPC calls in flight and open streams (devices showing something
    /// hosted here).
    pub viewers: usize,
    /// Epoch ms of the last RPC call (0 = none).
    pub last_rpc_ms: i64,
}

impl Activity {
    pub fn busy(&self) -> bool {
        self.runs + self.moves + self.terminals + self.viewers > 0
    }
}

/// The engine as a box sees it.
pub trait BoxEngine: Send + Sync + 'static {
    fn activity(&self) -> Activity;
    /// Folders of chats and projects hosted here.
    fn workspace_folders(&self) -> Vec<PathBuf>;
    /// Hold every hosted chat's commands and queue; returns the chats held.
    fn hold_all(&self) -> Vec<String>;
    fn release(&self, chats: &[String]);
    /// Write open docs to the store (blocking).
    fn flush(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudStatus {
    pub busy: bool,
    pub last_activity_at: i64,
    pub runs: usize,
    pub moves: usize,
    pub terminals: usize,
    pub viewers: usize,
}

#[derive(Clone)]
pub struct CloudBox {
    inner: Arc<BoxInner>,
}

struct BoxInner {
    engine: Arc<dyn BoxEngine>,
    checkpointer: Checkpointer,
    last_activity_ms: AtomicI64,
    /// Chats a final checkpoint holds, and the timer that lifts the holds.
    held: Mutex<Option<(Vec<String>, tokio::task::JoinHandle<()>)>>,
}

impl CloudBox {
    pub fn new(engine: Arc<dyn BoxEngine>, checkpointer: Checkpointer) -> Self {
        Self {
            inner: Arc::new(BoxInner {
                engine,
                checkpointer,
                last_activity_ms: AtomicI64::new(crate::now_ms()),
                held: Mutex::new(None),
            }),
        }
    }

    pub fn checkpointer(&self) -> &Checkpointer {
        &self.inner.checkpointer
    }

    pub fn engine(&self) -> &Arc<dyn BoxEngine> {
        &self.inner.engine
    }

    pub fn status(&self) -> CloudStatus {
        let activity = self.inner.engine.activity();
        let busy = activity.busy();
        if busy {
            self.touch();
        }
        CloudStatus {
            busy,
            last_activity_at: self
                .inner
                .last_activity_ms
                .load(Ordering::Acquire)
                .max(activity.last_rpc_ms),
            runs: activity.runs,
            moves: activity.moves,
            terminals: activity.terminals,
            viewers: activity.viewers,
        }
    }

    fn touch(&self) {
        self.inner
            .last_activity_ms
            .fetch_max(crate::now_ms(), Ordering::AcqRel);
    }

    /// The roots a checkpoint covers now: `data`, `harness`, and the
    /// workspaces of hosted chats and projects plus those of the last
    /// checkpoint still on disk (a workspace is never dropped while its
    /// folder exists).
    pub async fn roots(&self) -> Vec<RootSpec> {
        let paths = self.inner.checkpointer.paths().clone();
        let mut folders = self.inner.engine.workspace_folders();
        folders.extend(self.inner.checkpointer.previous_workspaces().await);
        let home = paths.home.clone();
        let workspaces = tokio::task::spawn_blocking(move || workspace_roots(folders, &home))
            .await
            .unwrap_or_default();
        let mut roots = vec![RootSpec::data(&paths.data_dir), RootSpec::harness(&paths.home)];
        roots.extend(workspaces);
        roots
    }

    /// Checkpoint now. `final_` holds every chat first and leaves it held.
    pub async fn checkpoint(&self, final_: bool) -> anyhow::Result<CheckpointStats> {
        if final_ {
            self.hold_all();
        }
        let engine = self.inner.engine.clone();
        let _ = tokio::task::spawn_blocking(move || engine.flush()).await;
        let roots = self.roots().await;
        self.inner.checkpointer.checkpoint(roots).await
    }

    /// Hold every hosted chat until the box stops (or [`FINAL_HOLD_MAX`]).
    pub fn hold_all(&self) {
        let mut chats = self.inner.engine.hold_all();
        let this = self.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(FINAL_HOLD_MAX).await;
            tracing::warn!("cloud box: still running after a final checkpoint; resuming chats");
            this.release_holds();
        });
        let mut held = self
            .inner
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Chats an earlier final checkpoint holds stay on the list.
        if let Some((previous, previous_timer)) = held.take() {
            previous_timer.abort();
            chats.extend(previous);
        }
        *held = Some((chats, timer));
    }

    pub fn release_holds(&self) {
        let held = self
            .inner
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some((chats, timer)) = held {
            timer.abort();
            self.inner.engine.release(&chats);
        }
    }

    /// Sample busy-ness into `lastActivityAt` and checkpoint every `every`.
    pub fn spawn_background(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let mut sample = tokio::time::interval(SAMPLE_EVERY);
            let mut checkpoint = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = sample.tick() => {
                        if this.inner.engine.activity().busy() {
                            this.touch();
                        }
                    }
                    _ = checkpoint.tick() => {
                        if let Err(error) = this.checkpoint(false).await {
                            tracing::warn!(error = %format!("{error:#}"), "cloud box: periodic checkpoint failed");
                        }
                    }
                }
            }
        })
    }

    /// Serve the control API on `addr`. Returns the bound address.
    pub async fn serve(
        &self,
        addr: SocketAddr,
        token: String,
    ) -> anyhow::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
        anyhow::ensure!(!token.trim().is_empty(), "the control token is empty");
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        tracing::info!(%bound, "cloud control server listening");
        let token: Arc<str> = token.into();
        let this = self.clone();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        tracing::warn!(%error, "cloud control: accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let this = this.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request| {
                        let this = this.clone();
                        let token = token.clone();
                        async move { Ok::<_, std::convert::Infallible>(this.route(request, &token).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok((bound, task))
    }

    async fn route(&self, request: Request<Incoming>, token: &str) -> Response<Full<Bytes>> {
        use subtle::ConstantTimeEq;
        let presented = request
            .headers()
            .get(CONTROL_HEADER)
            .map(|v| v.as_bytes())
            .unwrap_or_default();
        if !bool::from(presented.ct_eq(token.as_bytes())) {
            return json(StatusCode::UNAUTHORIZED, &serde_json::json!({"error": "unauthorized"}));
        }
        match (request.method(), request.uri().path()) {
            (&Method::GET, "/zeron/cloud/status") => json(StatusCode::OK, &self.status()),
            (&Method::POST, "/zeron/cloud/checkpoint") => {
                let final_ = request
                    .uri()
                    .query()
                    .unwrap_or("")
                    .split('&')
                    .any(|pair| pair == "final=1" || pair == "final=true");
                match self.checkpoint(final_).await {
                    Ok(stats) => json(StatusCode::OK, &stats),
                    Err(error) => {
                        let message = format!("{error:#}");
                        tracing::warn!(error = %message, final_, "cloud box: checkpoint failed");
                        json(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &serde_json::json!({"error": message}),
                        )
                    }
                }
            }
            (_, "/zeron/cloud/status" | "/zeron/cloud/checkpoint") => json(
                StatusCode::METHOD_NOT_ALLOWED,
                &serde_json::json!({"error": "method_not_allowed"}),
            ),
            _ => json(StatusCode::NOT_FOUND, &serde_json::json!({"error": "not_found"})),
        }
    }
}

fn json<T: Serialize>(status: StatusCode, value: &T) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(
        serde_json::to_vec(value).unwrap_or_default(),
    )));
    *response.status_mut() = status;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}
