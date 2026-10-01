//! Sending: offer the manifest, wait for the receiver's go-ahead (and its
//! list of already-verified blocks), then stream the missing blocks from
//! disk across parallel data lanes while whole-file digests are computed
//! alongside. A dropped tunnel is redialed with backoff; each new session
//! resumes from whatever the receiver reports. Nothing is ever buffered
//! beyond one block per lane.
//!
//! Sync sends ([`crate::sync`]) carry a ticket and content hashes in the
//! manifest: no digests are computed while sending, and every block read is
//! checked against the hash the manifest promised. A source that changed
//! since is skipped (tolerant) or fails the transfer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use zeron_proto::FileTransferState;

use crate::blocks::{BLOCK_SIZE, RangeSet, block_count, block_len};
use crate::hashcache::FileStat;
use crate::manifest::{Built, EntryKind, Manifest, hex};
use crate::service::TransportPolicy;
use crate::wire::{self, Block, Lane, Msg, PROTOCOL_VERSION};
use crate::{Transfers, Tunnel};

/// Data lanes at most, whatever the tunnel allows.
const MAX_LANES: usize = 8;
/// Whole-file digests computed at once.
const DIGEST_WORKERS: usize = 2;
/// Silence on the control lane that means the tunnel is dead (the receiver
/// reports progress twice a second and pings while waiting).
const CONTROL_SILENCE: Duration = Duration::from_secs(60);
/// Give up when a transfer makes no progress for this long.
const RESUME_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Per-file digest mismatches the sender will resend for.
const MAX_RESENDS: u32 = 3;

/// What a sync send adds to an outgoing transfer.
pub(crate) struct SyncOut {
    pub ticket: String,
    pub tolerant: bool,
    /// Parallel to the manifest: each file's version when it was hashed.
    pub stats: Vec<Option<FileStat>>,
}

pub(crate) struct Outgoing {
    id: String,
    to: String,
    to_name: String,
    manifest: Manifest,
    sources: Vec<PathBuf>,
    digest: String,
    destination: Option<String>,
    policy: TransportPolicy,
    skipped: u64,
    sync: Option<SyncOut>,
    cancel: CancellationToken,
    /// Whether the peer should be told when the cancel lands between sessions.
    notify_peer: std::sync::atomic::AtomicBool,
    /// Whole-file digests, kept across sessions.
    digests: Mutex<HashMap<u64, String>>,
    resends: Mutex<HashMap<u64, u32>>,
    /// Tolerant sync: files given up on (the receiver was sent `Skip`).
    dropped: Mutex<HashSet<u64>>,
}

enum Outcome {
    Completed,
    Declined(String),
    Cancelled,
    Failed(String),
}

/// A problem that ends the transfer rather than the session.
#[derive(Debug)]
struct Fatal(String);

impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fatal {}

impl Outgoing {
    pub(crate) fn new(
        id: String,
        to: String,
        to_name: String,
        destination: Option<String>,
        policy: TransportPolicy,
        built: Built,
        sync: Option<SyncOut>,
    ) -> Self {
        let digest = built.manifest.digest();
        Self {
            id,
            to,
            to_name,
            manifest: built.manifest,
            sources: built.sources,
            digest,
            destination,
            policy,
            skipped: built.skipped,
            sync,
            cancel: CancellationToken::new(),
            notify_peer: std::sync::atomic::AtomicBool::new(true),
            digests: Mutex::new(HashMap::new()),
            resends: Mutex::new(HashMap::new()),
            dropped: Mutex::new(HashSet::new()),
        }
    }

    fn tolerant(&self) -> bool {
        self.sync.as_ref().is_some_and(|s| s.tolerant)
    }

    pub(crate) fn cancel(&self, notify_peer: bool) {
        self.notify_peer
            .store(notify_peer, std::sync::atomic::Ordering::Relaxed);
        self.cancel.cancel();
    }
}

struct Work {
    queue: Mutex<VecDeque<(u32, u64)>>,
    ready: Notify,
}

impl Work {
    fn push(&self, items: impl IntoIterator<Item = (u32, u64)>) {
        self.queue.lock().unwrap().extend(items);
        self.ready.notify_waiters();
    }

    async fn pop(&self) -> (u32, u64) {
        loop {
            let ready = self.ready.notified();
            if let Some(item) = self.queue.lock().unwrap().pop_front() {
                return item;
            }
            ready.await;
        }
    }
}

enum Event {
    Digest(u64, String),
    /// Tolerant sync: give up on a file that changed.
    Skip(u64, String),
    Fatal(String),
    LaneFailed(anyhow::Error),
}

impl Transfers {
    pub(crate) async fn run_outgoing(&self, out: Arc<Outgoing>) {
        let mut failures: u32 = 0;
        let mut last_progress = Instant::now();
        let mut last_done = 0;
        let outcome = loop {
            let attempt = tokio::select! {
                _ = out.cancel.cancelled() => break Outcome::Cancelled,
                attempt = self.session(&out) => attempt,
            };
            match attempt {
                Ok(outcome) => break outcome,
                Err(error) => {
                    if let Some(fatal) = error.downcast_ref::<Fatal>() {
                        break Outcome::Failed(fatal.0.clone());
                    }
                    let done = self.row(&out.id).map(|r| r.done_bytes).unwrap_or_default();
                    if done > last_done {
                        last_done = done;
                        last_progress = Instant::now();
                        failures = 0;
                    }
                    if last_progress.elapsed() > RESUME_WINDOW {
                        break Outcome::Failed(format!("Could not reach {}: {error}", out.to_name));
                    }
                    failures += 1;
                    let delay =
                        Duration::from_secs(1 << failures.min(5)).min(Duration::from_secs(30));
                    tracing::info!(transfer = %out.id, %error, ?delay, "file transfer interrupted; resuming");
                    self.set_note(
                        &out.id,
                        FileTransferState::Reconnecting,
                        Some(format!("Reconnecting to {}…", out.to_name)),
                    );
                    tokio::select! {
                        _ = out.cancel.cancelled() => break Outcome::Cancelled,
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        };
        match outcome {
            Outcome::Completed => self.finish(&out.id, FileTransferState::Completed, None),
            Outcome::Declined(reason) => {
                self.finish(&out.id, FileTransferState::Declined, Some(reason))
            }
            Outcome::Cancelled => {
                self.finish(&out.id, FileTransferState::Cancelled, None);
                // A cancel that landed outside a session never reached the
                // receiver; tell it through the relay (idempotent).
                if out.notify_peer.load(std::sync::atomic::Ordering::Relaxed) {
                    self.notify_peer_cancel(&out.to, &out.id);
                }
            }
            Outcome::Failed(message) => {
                self.finish(&out.id, FileTransferState::Failed, Some(message))
            }
        }
    }

    async fn session(&self, out: &Arc<Outgoing>) -> anyhow::Result<Outcome> {
        let tunnel = self.0.network.connect(&out.to, out.policy).await?;
        let result = self.session_on(out, tunnel.as_ref()).await;
        tunnel.close().await;
        result
    }

    async fn session_on(
        &self,
        out: &Arc<Outgoing>,
        tunnel: &dyn Tunnel,
    ) -> anyhow::Result<Outcome> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let control = tunnel.open_lane().await?;
        let (mut read, mut write) = tokio::io::split(control);
        wire::write_msg(&mut write, &self.hello(out, &session_id, Lane::Control)).await?;
        wire::write_msg(
            &mut write,
            &Msg::Offer {
                entry_count: out.manifest.entries.len() as u64,
                file_count: out.manifest.file_count(),
                total_bytes: out.manifest.total_bytes(),
                digest: out.digest.clone(),
                destination: out.destination.clone(),
                skipped: out.skipped,
                ticket: out.sync.as_ref().map(|s| s.ticket.clone()),
            },
        )
        .await?;
        for chunk in wire::manifest_chunks(&out.manifest.entries) {
            wire::write_msg(
                &mut write,
                &Msg::Manifest {
                    entries: chunk.to_vec(),
                },
            )
            .await?;
        }
        wire::write_msg(&mut write, &Msg::ManifestEnd).await?;

        let (tx, mut rx) = mpsc::channel::<anyhow::Result<Msg>>(64);
        let reader = tokio::spawn(async move {
            loop {
                let msg =
                    match tokio::time::timeout(CONTROL_SILENCE, wire::read_msg(&mut read)).await {
                        Ok(msg) => msg,
                        Err(_) => Err(anyhow::anyhow!("the other device stopped responding")),
                    };
                let end = msg.is_err();
                if tx.send(msg).await.is_err() || end {
                    return;
                }
            }
        });
        let _reader = AbortOnDrop(reader);

        // Wait for the receiver's decision.
        let have = loop {
            let msg = tokio::select! {
                _ = out.cancel.cancelled() => {
                    let _ = wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await;
                    out.notify_peer.store(false, std::sync::atomic::Ordering::Relaxed);
                    return Ok(Outcome::Cancelled);
                }
                msg = rx.recv() => msg.ok_or_else(|| anyhow::anyhow!("the other device closed the transfer"))??,
            };
            match msg {
                // A sync receiver pings while it compares its basis; there
                // is no one to accept.
                Msg::Pending if out.sync.is_some() => {}
                Msg::Pending => {
                    self.set_state(&out.id, FileTransferState::AwaitingAcceptance, None)
                }
                Msg::Accept { have, sync } => {
                    if out.sync.is_some() && !sync {
                        // An older receiver took the ticket for an inbox
                        // delivery: call it off before anything lands.
                        let _ = wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await;
                        out.notify_peer
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        return Ok(Outcome::Failed(format!(
                            "{} can't receive session moves yet; update Zeron there",
                            out.to_name
                        )));
                    }
                    break have;
                }
                Msg::Decline { reason } => return Ok(Outcome::Declined(reason)),
                Msg::Cancel { .. } => {
                    out.notify_peer
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    return Ok(Outcome::Cancelled);
                }
                Msg::Complete => return Ok(Outcome::Completed),
                Msg::Error { message } => return Ok(Outcome::Failed(message)),
                _ => {}
            }
        };

        // What's left to send.
        let mut finished: Vec<bool> = vec![false; out.manifest.entries.len()];
        let mut verified: HashMap<u64, RangeSet> = HashMap::new();
        for file_have in have {
            let Some(entry) = out
                .manifest
                .entries
                .get(file_have.file as usize)
                .filter(|e| e.kind == EntryKind::File)
            else {
                continue;
            };
            if file_have.done {
                finished[file_have.file as usize] = true;
                if file_have.skipped {
                    out.dropped.lock().unwrap().insert(file_have.file);
                }
            } else if file_have.blocks.within(block_count(entry.size)) {
                verified.insert(file_have.file, file_have.blocks);
            }
        }
        let work = Arc::new(Work {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
        });
        let mut pending_files = Vec::new();
        for (index, entry) in out.manifest.entries.iter().enumerate() {
            if entry.kind != EntryKind::File || finished[index] {
                continue;
            }
            if out.dropped.lock().unwrap().contains(&(index as u64)) {
                // Skipped earlier, but the receiver never heard: say it again.
                wire::write_msg(
                    &mut write,
                    &Msg::Skip {
                        file: index as u64,
                        reason: format!("{} changed while sending", entry.path),
                    },
                )
                .await?;
                continue;
            }
            if entry.sha256.is_none() {
                // Sync entries carry their digest in the manifest.
                pending_files.push(index as u64);
            }
            let have = verified.remove(&(index as u64)).unwrap_or_default();
            work.push(
                have.missing(block_count(entry.size))
                    .into_iter()
                    .map(|block| (index as u32, block)),
            );
        }
        self.set_state(
            &out.id,
            FileTransferState::Transferring,
            Some(tunnel.transport()),
        );

        let stop = CancellationToken::new();
        let _stop_guard = stop.clone().drop_guard();
        let (events, mut event_rx) = mpsc::channel::<Event>(64);
        let digest_slots = Arc::new(Semaphore::new(DIGEST_WORKERS));
        for &file in &pending_files {
            self.spawn_digest(out, file, &digest_slots, &events, &stop);
        }
        let queued = work.queue.lock().unwrap().len();
        let lanes = tunnel.max_lanes().min(MAX_LANES).min(queued.max(1));
        let mut lane_tasks = tokio::task::JoinSet::new();
        if queued > 0 {
            for _ in 0..lanes {
                let lane = tunnel.open_lane().await?;
                let hello = self.hello(out, &session_id, Lane::Data);
                let (out, work, events, stop) =
                    (out.clone(), work.clone(), events.clone(), stop.clone());
                lane_tasks.spawn(async move {
                    tokio::select! {
                        _ = stop.cancelled() => {}
                        result = data_lane(lane, hello, &out, &work, &events) => {
                            if let Err(error) = result {
                                let event = match error.downcast::<Fatal>() {
                                    Ok(fatal) => Event::Fatal(fatal.0),
                                    Err(error) => Event::LaneFailed(error),
                                };
                                let _ = events.send(event).await;
                            }
                        }
                    }
                });
            }
        }

        loop {
            tokio::select! {
                _ = out.cancel.cancelled() => {
                    let _ = wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await;
                    out.notify_peer.store(false, std::sync::atomic::Ordering::Relaxed);
                    return Ok(Outcome::Cancelled);
                }
                Some(event) = event_rx.recv() => match event {
                    Event::Digest(file, sha256) => {
                        wire::write_msg(&mut write, &Msg::Digest { file, sha256 }).await?;
                    }
                    Event::Skip(file, reason) => {
                        if out.dropped.lock().unwrap().insert(file) {
                            tracing::debug!(transfer = %out.id, %reason, "sync: skipping a changed file");
                            wire::write_msg(&mut write, &Msg::Skip { file, reason }).await?;
                        }
                    }
                    Event::Fatal(message) => {
                        let _ = wire::write_msg(&mut write, &Msg::Error { message: message.clone() }).await;
                        return Ok(Outcome::Failed(message));
                    }
                    Event::LaneFailed(error) => return Err(error),
                },
                msg = rx.recv() => match msg.ok_or_else(|| anyhow::anyhow!("the other device closed the transfer"))?? {
                    Msg::Progress { done_bytes } => self.progress(&out.id, done_bytes),
                    Msg::Resend { file, blocks } => {
                        let Some(entry) = out.manifest.entries.get(file as usize).filter(|e| e.kind == EntryKind::File) else {
                            continue;
                        };
                        let count = block_count(entry.size);
                        match blocks {
                            Some(blocks) => work.push(blocks.into_iter().filter(|b| *b < count).map(|b| (file as u32, b))),
                            // Sync: the receiver assembled exactly what was
                            // read, so a whole-file mismatch means the source
                            // no longer has the content the manifest promised.
                            None if out.sync.is_some() => {
                                let reason = format!("{} changed while sending", entry.path);
                                if !out.tolerant() {
                                    let _ = wire::write_msg(&mut write, &Msg::Error { message: reason.clone() }).await;
                                    return Ok(Outcome::Failed(reason));
                                }
                                out.dropped.lock().unwrap().insert(file);
                                wire::write_msg(&mut write, &Msg::Skip { file, reason }).await?;
                                continue;
                            }
                            None => {
                                let resends = {
                                    let mut resends = out.resends.lock().unwrap();
                                    let n = resends.entry(file).or_default();
                                    *n += 1;
                                    *n
                                };
                                if resends > MAX_RESENDS {
                                    let message = format!("{} kept failing verification", entry.path);
                                    let _ = wire::write_msg(&mut write, &Msg::Error { message: message.clone() }).await;
                                    return Ok(Outcome::Failed(message));
                                }
                                // The source may have changed: hash it again.
                                out.digests.lock().unwrap().remove(&file);
                                self.spawn_digest(out, file, &digest_slots, &events, &stop);
                                work.push((0..count).map(|b| (file as u32, b)));
                            }
                        }
                        if lane_tasks.is_empty() {
                            // Everything was sent before; lanes are needed again.
                            let lane = tunnel.open_lane().await?;
                            let hello = self.hello(out, &session_id, Lane::Data);
                            let (out, work, events, stop) = (out.clone(), work.clone(), events.clone(), stop.clone());
                            lane_tasks.spawn(async move {
                                tokio::select! {
                                    _ = stop.cancelled() => {}
                                    result = data_lane(lane, hello, &out, &work, &events) => {
                                        if let Err(error) = result {
                                            let _ = events.send(Event::LaneFailed(error)).await;
                                        }
                                    }
                                }
                            });
                        }
                    }
                    Msg::Complete => {
                        self.progress(&out.id, out.manifest.total_bytes());
                        return Ok(Outcome::Completed);
                    }
                    Msg::Cancel { .. } => {
                        out.notify_peer.store(false, std::sync::atomic::Ordering::Relaxed);
                        return Ok(Outcome::Cancelled);
                    }
                    Msg::Error { message } => return Ok(Outcome::Failed(message)),
                    Msg::Decline { reason } => return Ok(Outcome::Declined(reason)),
                    _ => {}
                },
            }
        }
    }

    fn hello(&self, out: &Outgoing, session_id: &str, lane: Lane) -> Msg {
        Msg::Hello {
            v: PROTOCOL_VERSION,
            transfer_id: out.id.clone(),
            session_id: session_id.to_owned(),
            lane,
            sender_name: self.0.config.device_name.clone(),
        }
    }

    /// Hash `file` (unless already known this transfer) and report it.
    fn spawn_digest(
        &self,
        out: &Arc<Outgoing>,
        file: u64,
        slots: &Arc<Semaphore>,
        events: &mpsc::Sender<Event>,
        stop: &CancellationToken,
    ) {
        let (out, slots, events, stop) = (out.clone(), slots.clone(), events.clone(), stop.clone());
        tokio::spawn(async move {
            let known = out.digests.lock().unwrap().get(&file).cloned();
            let event = match known {
                Some(digest) => Event::Digest(file, digest),
                None => {
                    let slot = tokio::select! {
                        _ = stop.cancelled() => return,
                        slot = slots.acquire_owned() => slot,
                    };
                    let Ok(_slot) = slot else { return };
                    let source = out.sources[file as usize].clone();
                    let entry = out.manifest.entries[file as usize].clone();
                    let hashing = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                        let file = std::fs::File::open(&source)?;
                        let mut hasher = Sha256::new();
                        let mut reader = std::io::BufReader::with_capacity(1024 * 1024, file);
                        let read = std::io::copy(&mut reader, &mut hasher)?;
                        anyhow::ensure!(read == entry.size, "{} changed while sending", entry.path);
                        Ok(hex(&hasher.finalize()))
                    });
                    let result = tokio::select! {
                        _ = stop.cancelled() => return,
                        result = hashing => result,
                    };
                    match result.map_err(anyhow::Error::from).and_then(|r| r) {
                        Ok(digest) => {
                            out.digests.lock().unwrap().insert(file, digest.clone());
                            Event::Digest(file, digest)
                        }
                        Err(error) => Event::Fatal(format!(
                            "Could not read {}: {error}",
                            out.manifest.entries[file as usize].path
                        )),
                    }
                }
            };
            let _ = events.send(event).await;
        });
    }
}

/// One data lane: pull blocks off the shared queue, read them from disk at
/// their offset, and write them with their SHA-256. The next block is read
/// and hashed while the current one is being written (one block of
/// read-ahead per lane). A failed lane ends its session; the next session
/// starts from the receiver's verified blocks, so nothing is lost.
async fn data_lane(
    mut lane: crate::BoxIo,
    hello: Msg,
    out: &Arc<Outgoing>,
    work: &Arc<Work>,
    events: &mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    wire::write_msg(&mut lane, &hello).await?;
    let (blocks, mut next) = mpsc::channel::<Result<Read, Fatal>>(1);
    let reader = {
        let (out, work) = (out.clone(), work.clone());
        tokio::spawn(async move {
            let mut open: Option<(u32, Arc<std::fs::File>)> = None;
            loop {
                let (file, block) = work.pop().await;
                if out.dropped.lock().unwrap().contains(&(file as u64)) {
                    continue;
                }
                let read = match read_block(&out, &mut open, file, block).await {
                    Ok(block) => Ok(Read::Block(block)),
                    Err(ReadError::Changed(reason)) if out.tolerant() => {
                        open = None;
                        Ok(Read::Skip(file as u64, reason))
                    }
                    Err(ReadError::Changed(reason) | ReadError::Fatal(reason)) => {
                        Err(Fatal(reason))
                    }
                };
                let failed = read.is_err();
                if blocks.send(read).await.is_err() || failed {
                    return;
                }
            }
        })
    };
    let _reader = AbortOnDrop(reader);
    while let Some(read) = next.recv().await {
        match read? {
            Read::Block(block) => wire::write_block(&mut lane, &block).await?,
            Read::Skip(file, reason) => {
                let _ = events.send(Event::Skip(file, reason)).await;
            }
        }
        if next.is_empty() && work.queue.lock().unwrap().is_empty() {
            lane.flush().await?;
        }
    }
    anyhow::bail!("data lane reader stopped")
}

enum Read {
    Block(Block),
    Skip(u64, String),
}

enum ReadError {
    Fatal(String),
    /// Sync: the source no longer matches what the manifest promised.
    Changed(String),
}

async fn read_block(
    out: &Outgoing,
    open: &mut Option<(u32, Arc<std::fs::File>)>,
    file: u32,
    block: u64,
) -> Result<Block, ReadError> {
    let entry = &out.manifest.entries[file as usize];
    let promised = out
        .sync
        .as_ref()
        .and_then(|s| s.stats.get(file as usize).copied().flatten());
    let handle = match open {
        Some((index, handle)) if *index == file => handle.clone(),
        _ => {
            let source = out.sources[file as usize].clone();
            let path = entry.path.clone();
            let opened =
                tokio::task::spawn_blocking(move || -> Result<std::fs::File, ReadError> {
                    let Some(promised) = promised else {
                        return std::fs::File::open(source)
                            .map_err(|e| ReadError::Fatal(format!("Could not read {path}: {e}")));
                    };
                    // Sync: the very version that was hashed, never a link.
                    let changed = |what: String| ReadError::Changed(format!("{path} {what}"));
                    let handle = crate::sync::open_nofollow(&source)
                        .map_err(|e| changed(format!("could not be read ({e})")))?;
                    let meta = handle
                        .metadata()
                        .map_err(|e| changed(format!("could not be read ({e})")))?;
                    if !meta.is_file() || !FileStat::of(&meta).same_content(&promised) {
                        return Err(changed("changed while sending".into()));
                    }
                    Ok(handle)
                })
                .await
                .map_err(|e| ReadError::Fatal(e.to_string()))??;
            let handle = Arc::new(opened);
            *open = Some((file, handle.clone()));
            handle
        }
    };
    let offset = block * BLOCK_SIZE;
    let length = block_len(entry.size, block) as usize;
    let path = entry.path.clone();
    let expected = entry
        .blocks
        .as_ref()
        .and_then(|b| b.get(block as usize))
        .and_then(|h| crate::sync::unhex32(h));
    let sync = out.sync.is_some();
    tokio::task::spawn_blocking(move || -> Result<Block, ReadError> {
        let mut data = vec![0u8; length];
        if let Err(error) = read_at(&handle, &mut data, offset) {
            let message = format!("{path} changed while sending ({error})");
            return Err(if sync {
                ReadError::Changed(message)
            } else {
                ReadError::Fatal(message)
            });
        }
        let block = Block::new(file, offset, data);
        if expected.is_some_and(|hash| hash != block.sha256) {
            return Err(ReadError::Changed(format!("{path} changed while sending")));
        }
        Ok(block)
    })
    .await
    .map_err(|e| ReadError::Fatal(e.to_string()))?
}

pub(crate) fn read_at(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buffer, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut filled = 0;
        while filled < buffer.len() {
            let n = file.seek_read(&mut buffer[filled..], offset + filled as u64)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            filled += n;
        }
        Ok(())
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
