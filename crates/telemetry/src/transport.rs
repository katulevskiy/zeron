//! The delivery worker: one tokio task that owns the buffer, batches events,
//! gzips them and POSTs to PostHog's `/batch/` endpoint.
//!
//! Delivery is at-least-once and idempotent: every event carries the uuid it
//! was minted with, so a retried batch that PostHog already stored dedupes
//! server-side. A failed batch backs off exponentially and is spooled to disk
//! (bounded) so offline sessions aren't lost; the spool drains on the next
//! successful send, including on the next launch.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::Inner;

/// One stamped event, ready to serialize.
pub(crate) struct Envelope {
    pub name: std::borrow::Cow<'static, str>,
    pub props: Map<String, Value>,
    pub distinct_id: Arc<str>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub uuid: uuid::Uuid,
}

pub(crate) enum Command {
    Event(Box<Envelope>),
    Flush(oneshot::Sender<()>),
    /// Telemetry was switched off: drop everything buffered and spooled.
    Purge,
}

pub(crate) struct WorkerConfig {
    pub endpoint: Option<String>,
    pub api_key: String,
    pub spool: Option<PathBuf>,
    pub debug_log: Option<PathBuf>,
    pub batch_size: usize,
    pub flush_every: Duration,
    pub rollup_every: Duration,
}

const SPOOL_MAX_BYTES: u64 = 4 * 1024 * 1024;
const RECENT_CAP: usize = 200;
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn run(inner: Arc<Inner>, mut rx: mpsc::Receiver<Command>, cfg: WorkerConfig) {
    let client = reqwest::Client::builder()
        .timeout(SEND_TIMEOUT)
        .user_agent(concat!("zeron-telemetry/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok();
    let mut worker = Worker {
        inner,
        client,
        cfg,
        buffer: Vec::new(),
        backoff: None,
        failures: 0,
    };
    let mut flush_tick = tokio::time::interval(worker.cfg.flush_every);
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut rollup_tick = tokio::time::interval(worker.cfg.rollup_every);
    rollup_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Both intervals fire immediately; swallow that so startup isn't a flush.
    flush_tick.tick().await;
    rollup_tick.tick().await;
    // A spool left by an offline previous session goes out first.
    worker.drain_spool().await;

    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Command::Event(env)) => {
                    worker.push(*env);
                    if worker.buffer.len() >= worker.cfg.batch_size {
                        worker.flush().await;
                    }
                }
                Some(Command::Flush(ack)) => {
                    worker.take_rollups();
                    worker.flush().await;
                    let _ = ack.send(());
                }
                Some(Command::Purge) => {
                    // Channel order: anything captured before the opt-out
                    // was pushed above; clear it all here, after the fact.
                    worker.discard();
                }
                None => {
                    worker.take_rollups();
                    worker.flush().await;
                    break;
                }
            },
            _ = flush_tick.tick() => worker.flush().await,
            _ = rollup_tick.tick() => {
                worker.take_rollups();
                worker.flush().await;
            }
        }
    }
}

struct Worker {
    inner: Arc<Inner>,
    client: Option<reqwest::Client>,
    cfg: WorkerConfig,
    buffer: Vec<Value>,
    backoff: Option<Instant>,
    failures: u32,
}

impl Worker {
    /// The single consent gate for anything leaving the process or landing
    /// on disk. Checked at every send, not just at capture: a spool written
    /// before an opt-out (or before `DO_NOT_TRACK` was exported) must never
    /// be replayed after it.
    fn consented(&self) -> bool {
        self.inner.collecting.load(Ordering::Relaxed)
    }

    fn discard(&mut self) {
        self.buffer.clear();
        if let Ok(mut recent) = self.inner.recent.lock() {
            recent.clear();
        }
        if let Some(spool) = &self.cfg.spool {
            let _ = std::fs::remove_file(spool);
        }
    }

    fn push(&mut self, env: Envelope) {
        if !self.consented() {
            return;
        }
        let event = self.serialize(env);
        self.remember(&event);
        if let Some(path) = &self.cfg.debug_log {
            append_line(path, &event, u64::MAX);
        }
        if self.cfg.endpoint.is_some() {
            self.buffer.push(event);
        }
    }

    fn serialize(&self, env: Envelope) -> Value {
        let mut props = self.inner.common.clone();
        props.extend(env.props);
        props.insert("$process_person_profile".into(), Value::Bool(false));
        json!({
            "event": env.name,
            "distinct_id": &*env.distinct_id,
            "properties": props,
            "timestamp": env.timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "uuid": env.uuid.to_string(),
        })
    }

    fn remember(&self, event: &Value) {
        if let Ok(mut recent) = self.inner.recent.lock() {
            if recent.len() == RECENT_CAP {
                recent.pop_front();
            }
            recent.push_back(event.clone());
        }
    }

    fn take_rollups(&mut self) {
        for event in self.inner.rollup.drain() {
            let env = self.inner.envelope(event);
            self.push(env);
        }
    }

    async fn flush(&mut self) {
        if !self.consented() {
            self.discard();
            return;
        }
        if self.buffer.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.buffer);
        if self.backoff.is_some_and(|until| Instant::now() < until) {
            self.spool(&batch);
            return;
        }
        match self.send(&batch).await {
            Ok(()) => {
                self.on_success(batch.len());
                self.drain_spool().await;
            }
            Err(SendError::Retryable(err)) => {
                self.on_failure(err);
                self.spool(&batch);
            }
            Err(SendError::Rejected(err)) => {
                // 4xx: the payload itself is bad; retrying can't help.
                self.inner
                    .stats
                    .dropped
                    .fetch_add(batch.len() as u64, Ordering::Relaxed);
                self.set_error(err);
            }
        }
    }

    async fn send(&self, batch: &[Value]) -> Result<(), SendError> {
        let (Some(client), Some(endpoint)) = (&self.client, &self.cfg.endpoint) else {
            return Ok(());
        };
        let body = json!({ "api_key": self.cfg.api_key, "batch": batch });
        let raw = serde_json::to_vec(&body).map_err(|e| SendError::Rejected(e.to_string()))?;
        let gz = gzip(&raw).map_err(|e| SendError::Rejected(e.to_string()))?;
        let url = format!("{}/batch/", endpoint.trim_end_matches('/'));
        let response = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::CONTENT_ENCODING, "gzip")
            .body(gz)
            .send()
            .await
            .map_err(|e| SendError::Retryable(format!("network: {}", without_url(&e))))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else if status.as_u16() == 429 || status.is_server_error() {
            Err(SendError::Retryable(format!("http {status}")))
        } else {
            Err(SendError::Rejected(format!("http {status}")))
        }
    }

    fn on_success(&mut self, n: usize) {
        self.failures = 0;
        self.backoff = None;
        self.inner.stats.sent.fetch_add(n as u64, Ordering::Relaxed);
        if let Ok(mut last) = self.inner.stats.last_error.lock() {
            *last = None;
        }
    }

    fn on_failure(&mut self, err: String) {
        self.failures = self.failures.saturating_add(1);
        let secs = 2u64.saturating_pow(self.failures.min(10));
        self.backoff = Some(Instant::now() + Duration::from_secs(secs).min(MAX_BACKOFF));
        tracing::debug!(%err, failures = self.failures, "telemetry: send failed; spooling");
        self.set_error(err);
    }

    fn set_error(&self, err: String) {
        if let Ok(mut last) = self.inner.stats.last_error.lock() {
            *last = Some(err);
        }
    }

    fn spool(&self, batch: &[Value]) {
        let Some(path) = &self.cfg.spool else {
            self.inner
                .stats
                .dropped
                .fetch_add(batch.len() as u64, Ordering::Relaxed);
            return;
        };
        if !append_line(path, &Value::Array(batch.to_vec()), SPOOL_MAX_BYTES) {
            self.inner
                .stats
                .dropped
                .fetch_add(batch.len() as u64, Ordering::Relaxed);
        }
    }

    async fn drain_spool(&mut self) {
        let Some(path) = self.cfg.spool.clone() else {
            return;
        };
        if !self.consented() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            return;
        };
        let batches: VecDeque<Value> = std::io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect();
        let _ = std::fs::remove_file(&path);
        let mut batches = batches;
        while let Some(batch) = batches.pop_front() {
            if !self.consented() {
                return;
            }
            let Value::Array(events) = batch else {
                continue;
            };
            match self.send(&events).await {
                Ok(()) => self.on_success(events.len()),
                Err(SendError::Retryable(err)) => {
                    self.on_failure(err);
                    self.spool(&events);
                    for rest in batches {
                        if let Value::Array(events) = rest {
                            self.spool(&events);
                        }
                    }
                    return;
                }
                Err(SendError::Rejected(err)) => self.set_error(err),
            }
        }
    }
}

enum SendError {
    Retryable(String),
    Rejected(String),
}

/// reqwest's Display includes the URL; keep logs/status to the error kind.
fn without_url(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_request() {
        "request"
    } else {
        "other"
    }
}

fn gzip(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(
        Vec::with_capacity(raw.len() / 4),
        flate2::Compression::fast(),
    );
    enc.write_all(raw)?;
    enc.finish()
}

/// Appends one JSON line unless that would push the file past `cap` bytes.
fn append_line(path: &PathBuf, value: &Value, cap: u64) -> bool {
    let Ok(line) = serde_json::to_vec(value) else {
        return false;
    };
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size.saturating_add(line.len() as u64 + 1) > cap {
        return false;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return false;
    };
    file.write_all(&line).is_ok() && file.write_all(b"\n").is_ok()
}
