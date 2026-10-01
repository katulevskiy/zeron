//! Receiving: validate the offer, decide (auto-accept or ask), lay the
//! items out under a fresh name, then verify and write blocks as data lanes
//! deliver them. Verified blocks are persisted (after an fsync of the data
//! they describe) so a dropped tunnel — or a restart of this engine —
//! resumes where it stopped. Each file lands as a hidden `*.part`, is
//! checked against the sender's whole-file SHA-256, and only then renamed.
//!
//! A sync offer ([`crate::sync`]) carries a ticket instead: the engine's
//! grant names the staging folder and the basis folders. Before accepting,
//! the receiver compares every file with its basis — unchanged files are
//! reported done, matching blocks are copied into the `.part` — and when
//! the transfer completes it writes the result record at the staging root.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::{FileTransfer, FileTransferDirection, FileTransferState, FileTransferTransport};

use crate::blocks::{BLOCK_SIZE, RangeSet, block_count, block_len};
use crate::manifest::{self, Entry, EntryKind, MAX_ENTRIES, Manifest, hex};
use crate::service::{INCOMING_RETENTION_MS, write_atomic};
use crate::sync::{self, Basis, SyncGrant};
use crate::wire::{self, FileHave, Frame, Msg};
use crate::{BoxIo, Transfers, now_ms};

/// Whole-file digest mismatches tolerated per file before giving up.
const MAX_RESENDS: u32 = 3;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);
const PENDING_PING: Duration = Duration::from_secs(10);
/// Free space kept beyond the transfer's remaining bytes.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// What survives a restart: written once when the transfer is accepted.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    id: String,
    peer: String,
    peer_name: String,
    digest: String,
    dest_root: PathBuf,
    /// Sender's top-level name → the name used here.
    tops: HashMap<String, String>,
    created_at: i64,
    entries: Vec<Entry>,
    /// Sync mode: the ticket the transfer was granted under. A resumed
    /// session needs no new grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ticket: Option<String>,
    /// Sync mode: root id → basis folders, as granted.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    basis: HashMap<String, Vec<PathBuf>>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Progress {
    files: Vec<FileHave>,
    /// Sync: unchanged file → the basis folder (its position) holding it.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    basis_index: HashMap<u64, u32>,
}

#[derive(Default)]
struct FileState {
    blocks: RangeSet,
    digest: Option<String>,
    done: bool,
    finalizing: bool,
    resends: u32,
    /// Written since the last durable flush.
    touched: bool,
    /// Final name (top-level files may be renamed around a conflict).
    landed: Option<PathBuf>,
    /// Sync, with `done`: the basis already had it…
    unchanged: bool,
    /// …in this folder of the root's basis list.
    basis_index: Option<u32>,
    /// Sync, with `done`: the sender skipped it.
    skipped: bool,
    skip_reason: Option<String>,
}

#[derive(Default)]
struct RecvState {
    files: HashMap<u64, FileState>,
    done_bytes: u64,
    completing: bool,
}

enum Decision {
    Decided(bool),
    Cancelled,
    Lost(anyhow::Error),
}

pub(crate) struct Live {
    session_id: String,
    stop: CancellationToken,
    control: mpsc::Sender<Msg>,
}

pub(crate) struct Incoming {
    id: String,
    peer: String,
    digest: String,
    manifest: Manifest,
    dest_root: PathBuf,
    tops: HashMap<String, String>,
    /// Sync mode (see [`crate::sync`]).
    ticket: Option<String>,
    state: Mutex<RecvState>,
    session: Mutex<Option<Live>>,
    /// This device's user cancelled.
    cancel: CancellationToken,
}

impl Incoming {
    fn final_path(&self, entry: &Entry) -> PathBuf {
        let (top, rest) = match entry.path.split_once('/') {
            Some((top, rest)) => (top, Some(rest)),
            None => (entry.path.as_str(), None),
        };
        let top = self.tops.get(top).map(String::as_str).unwrap_or(top);
        let root = self.dest_root.join(top);
        match rest {
            Some(rest) => manifest::join(&root, rest),
            None => root,
        }
    }

    fn part_path(&self, entry: &Entry) -> PathBuf {
        let target = self.final_path(entry);
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tag: String = self
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        target.with_file_name(format!(".{name}.zeron-{tag}.part"))
    }

    fn have(&self) -> Vec<FileHave> {
        let mut have = have_of(&self.state.lock().unwrap());
        have.sort_by_key(|h| h.file);
        have
    }

    fn file_entries(&self) -> impl Iterator<Item = (u64, &Entry)> {
        self.manifest
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == EntryKind::File)
            .map(|(i, e)| (i as u64, e))
    }

    fn is_sync(&self) -> bool {
        self.ticket.is_some()
    }

    fn all_done(&self) -> bool {
        let state = self.state.lock().unwrap();
        self.file_entries()
            .all(|(i, _)| state.files.get(&i).is_some_and(|f| f.done))
    }

    async fn send_control(&self, msg: Msg) {
        let control = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|l| l.control.clone());
        if let Some(control) = control {
            let _ = control.send(msg).await;
        }
    }
}

impl Transfers {
    /// Restore accepted, unfinished transfers so their senders can resume.
    pub(crate) fn load_incoming(&self) {
        let dir = self.0.config.state_dir.join("incoming");
        let Ok(read) = std::fs::read_dir(&dir) else {
            return;
        };
        for child in read.filter_map(Result::ok) {
            let path = child.path();
            let meta: Option<Meta> = std::fs::read(path.join("meta.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let Some(meta) = meta else {
                let _ = std::fs::remove_dir_all(&path);
                continue;
            };
            let progress: Progress = std::fs::read(path.join("progress.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            let row = self.row(&meta.id);
            let stale = now_ms() - meta.created_at > INCOMING_RETENTION_MS;
            if stale || row.as_ref().is_some_and(|r| r.state.is_terminal()) {
                let incoming = incoming_from(meta, progress);
                remove_parts(&incoming);
                let _ = std::fs::remove_dir_all(&path);
                if stale {
                    self.finish(
                        &incoming.id,
                        FileTransferState::Failed,
                        Some("The sending device never resumed this transfer".into()),
                    );
                }
                continue;
            }
            let peer_name = meta.peer_name.clone();
            let incoming = Arc::new(incoming_from(meta, progress));
            if row.is_none() {
                self.insert(incoming_row(
                    &incoming,
                    &peer_name,
                    FileTransferState::Reconnecting,
                ));
            }
            self.0
                .incoming
                .lock()
                .unwrap()
                .insert(incoming.id.clone(), incoming);
        }
    }

    pub(crate) fn cancel_incoming(&self, incoming: &Arc<Incoming>, notify_peer: bool) {
        let live = incoming.session.lock().unwrap().is_some();
        incoming.cancel.cancel();
        if !live {
            // No session to carry the news: clean up here.
            remove_parts(incoming);
            let _ = std::fs::remove_dir_all(self.incoming_dir(&incoming.id));
            self.finish(&incoming.id, FileTransferState::Cancelled, None);
            if notify_peer {
                self.notify_peer_cancel(&incoming.peer, &incoming.id);
            }
        }
    }

    pub(crate) async fn serve_control(
        &self,
        peer: String,
        peer_name: String,
        transport: FileTransferTransport,
        id: String,
        session_id: String,
        io: BoxIo,
    ) -> anyhow::Result<()> {
        let (mut read, mut write) = tokio::io::split(io);
        let Msg::Offer {
            entry_count,
            file_count,
            total_bytes,
            digest,
            destination,
            skipped,
            ticket,
        } = wire::read_msg(&mut read).await?
        else {
            anyhow::bail!("expected a transfer offer");
        };
        anyhow::ensure!(
            entry_count as usize <= MAX_ENTRIES,
            "the transfer lists too many entries"
        );
        let mut entries = Vec::with_capacity(entry_count.min(65536) as usize);
        loop {
            match wire::read_msg(&mut read).await? {
                Msg::Manifest { entries: chunk } => {
                    anyhow::ensure!(
                        entries.len() + chunk.len() <= entry_count as usize,
                        "the manifest is longer than announced"
                    );
                    entries.extend(chunk);
                }
                Msg::ManifestEnd => break,
                _ => anyhow::bail!("unexpected message in the manifest"),
            }
        }
        let manifest = Manifest { entries };
        let invalid = |message: String| Msg::Error { message };
        let valid = if ticket.is_some() {
            manifest.validate_sync()
        } else {
            manifest.validate()
        };
        if let Err(error) = valid {
            wire::write_msg(&mut write, &invalid(format!("Refused: {error}"))).await?;
            return Err(error);
        }
        if manifest.digest() != digest
            || manifest.file_count() != file_count
            || manifest.total_bytes() != total_bytes
            || manifest.entries.len() as u64 != entry_count
        {
            wire::write_msg(&mut write, &invalid("The manifest is inconsistent".into())).await?;
            anyhow::bail!("inconsistent manifest");
        }

        // Everything the sender says from here on arrives through a reader
        // task, so waiting (for the user, for data) never loses a frame.
        let (tx, mut rx) = mpsc::channel::<anyhow::Result<Msg>>(64);
        tokio::spawn(async move {
            loop {
                let msg = wire::read_msg(&mut read).await;
                let end = msg.is_err();
                if tx.send(msg).await.is_err() || end {
                    return;
                }
            }
        });

        let existing = self.0.incoming.lock().unwrap().get(&id).cloned();
        let incoming = match existing {
            Some(incoming) => {
                if incoming.peer != peer || incoming.digest != digest || incoming.ticket != ticket {
                    wire::write_msg(&mut write, &invalid("Conflicting transfer id".into())).await?;
                    anyhow::bail!("conflicting transfer id {id}");
                }
                if incoming.cancel.is_cancelled() {
                    wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await?;
                    return Ok(());
                }
                incoming
            }
            None => {
                if let Some(row) = self.row(&id)
                    && row.state.is_terminal()
                {
                    let answer = match row.state {
                        FileTransferState::Completed => Msg::Complete,
                        FileTransferState::Declined => Msg::Decline {
                            reason: "Declined".into(),
                        },
                        FileTransferState::Cancelled => Msg::Cancel { reason: None },
                        _ => invalid(row.error.unwrap_or_else(|| "The transfer failed".into())),
                    };
                    wire::write_msg(&mut write, &answer).await?;
                    return Ok(());
                }
                if let Some(ticket) = ticket {
                    let offer = SyncOffer {
                        peer,
                        peer_name,
                        id,
                        digest,
                        ticket,
                        skipped,
                    };
                    return self
                        .accept_sync(offer, manifest, session_id, transport, write, rx)
                        .await;
                }
                let dest_root = match self.resolve_destination(&peer_name, destination.as_deref()) {
                    Ok(root) => root,
                    Err(error) => {
                        wire::write_msg(&mut write, &invalid(error.to_string())).await?;
                        return Err(error);
                    }
                };
                if let Some(message) = space_problem(&dest_root, total_bytes) {
                    wire::write_msg(&mut write, &invalid(message.clone())).await?;
                    anyhow::bail!(message);
                }
                let mut row = new_row(&id, &peer, &peer_name, &manifest, skipped);
                row.destination = Some(dest_root.to_string_lossy().into_owned());
                if self.require_confirmation() {
                    row.state = FileTransferState::AwaitingAcceptance;
                    let (decision, decided) = watch::channel(None);
                    self.0.pending.lock().unwrap().insert(id.clone(), decision);
                    self.insert(row);
                    let accepted = match self.await_decision(&mut write, &mut rx, decided).await {
                        Decision::Decided(accepted) => accepted,
                        Decision::Cancelled => {
                            self.finish(&id, FileTransferState::Cancelled, None);
                            return Ok(());
                        }
                        Decision::Lost(error) => {
                            // The sender reconnects and asks again.
                            self.0.pending.lock().unwrap().remove(&id);
                            self.set_note(
                                &id,
                                FileTransferState::Reconnecting,
                                Some("Waiting for the sender to reconnect".into()),
                            );
                            return Err(error);
                        }
                    };
                    self.0.pending.lock().unwrap().remove(&id);
                    if !accepted {
                        self.finish(&id, FileTransferState::Declined, None);
                        wire::write_msg(
                            &mut write,
                            &Msg::Decline {
                                reason: "The other device declined the files".into(),
                            },
                        )
                        .await?;
                        return Ok(());
                    }
                } else {
                    row.state = FileTransferState::Connecting;
                    self.insert(row);
                }
                let incoming = match self
                    .lay_out(&id, &peer, &peer_name, &digest, dest_root, manifest)
                    .await
                {
                    Ok(incoming) => incoming,
                    Err(error) => {
                        let message = format!("Could not save the files: {error}");
                        self.finish(&id, FileTransferState::Failed, Some(message.clone()));
                        wire::write_msg(&mut write, &invalid(message)).await?;
                        return Err(error);
                    }
                };
                let paths: Vec<(String, PathBuf)> = incoming
                    .manifest
                    .entries
                    .iter()
                    .filter(|e| e.is_top_level())
                    .map(|e| (e.path.clone(), incoming.final_path(e)))
                    .collect();
                self.update_row(&id, |row| {
                    for (item, (_, path)) in row.items.iter_mut().zip(&paths) {
                        item.path = Some(path.to_string_lossy().into_owned());
                    }
                });
                self.0
                    .incoming
                    .lock()
                    .unwrap()
                    .insert(id.clone(), incoming.clone());
                incoming
            }
        };
        self.run_receive_session(incoming, session_id, transport, write, rx)
            .await
    }

    /// Tell the sender to wait (every [`PENDING_PING`]) until this device's
    /// user decides, the sender cancels, or the lane drops. A lane that
    /// fails while writing is drained first: a `Cancel` the sender sent
    /// just before closing must not be mistaken for a lost connection.
    async fn await_decision<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        write: &mut W,
        rx: &mut mpsc::Receiver<anyhow::Result<Msg>>,
        mut decided: watch::Receiver<Option<bool>>,
    ) -> Decision {
        let mut ping = tokio::time::interval(PENDING_PING);
        let lost = loop {
            tokio::select! {
                _ = decided.changed() => {
                    if let Some(accepted) = *decided.borrow() {
                        return Decision::Decided(accepted);
                    }
                }
                _ = ping.tick() => {
                    if let Err(error) = wire::write_msg(write, &Msg::Pending).await {
                        break error;
                    }
                }
                msg = rx.recv() => match msg {
                    Some(Ok(Msg::Cancel { .. })) => return Decision::Cancelled,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Decision::Lost(error),
                    None => return Decision::Lost(anyhow::anyhow!("the sender went away")),
                },
            }
        };
        loop {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                Ok(Some(Ok(Msg::Cancel { .. }))) => return Decision::Cancelled,
                Ok(Some(Ok(_))) => {}
                _ => return Decision::Lost(lost),
            }
        }
    }

    async fn lay_out(
        &self,
        id: &str,
        peer: &str,
        peer_name: &str,
        digest: &str,
        dest_root: PathBuf,
        manifest: Manifest,
    ) -> anyhow::Result<Arc<Incoming>> {
        let meta_dir = self.incoming_dir(id);
        let (id, peer, peer_name, digest) = (
            id.to_owned(),
            peer.to_owned(),
            peer_name.to_owned(),
            digest.to_owned(),
        );
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dest_root)?;
            let mut tops = HashMap::new();
            for entry in manifest.entries.iter().filter(|e| e.is_top_level()) {
                let mut name = entry.path.clone();
                let mut n = 2;
                while std::fs::symlink_metadata(dest_root.join(&name)).is_ok() {
                    name = manifest::numbered(&entry.path, n);
                    n += 1;
                }
                if entry.kind == EntryKind::Dir {
                    // Creating it reserves the name.
                    std::fs::create_dir(dest_root.join(&name))?;
                }
                tops.insert(entry.path.clone(), name);
            }
            let meta = Meta {
                id,
                peer,
                peer_name,
                digest,
                dest_root,
                tops,
                created_at: now_ms(),
                entries: manifest.entries,
                ticket: None,
                basis: HashMap::new(),
            };
            let bytes = serde_json::to_vec(&meta)?;
            let incoming = incoming_from(meta, Progress::default());
            for entry in incoming
                .manifest
                .entries
                .iter()
                .filter(|e| e.kind == EntryKind::Dir)
            {
                ensure_dir(&incoming.final_path(entry))?;
            }
            write_atomic(&meta_dir.join("meta.json"), &bytes)?;
            anyhow::Ok(Arc::new(incoming))
        })
        .await?
    }

    /// A ticketed offer: ask the engine for its grant, compare the manifest
    /// with the basis (telling the sender to wait meanwhile), then receive
    /// into the granted staging folder. Nothing asks this device's user —
    /// the move itself was the user's decision.
    async fn accept_sync<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        offer: SyncOffer,
        manifest: Manifest,
        session_id: String,
        transport: FileTransferTransport,
        mut write: W,
        rx: mpsc::Receiver<anyhow::Result<Msg>>,
    ) -> anyhow::Result<()> {
        let refuse = |message: String| Msg::Error { message };
        // A sender that redials while the basis is still being compared
        // gets a closed lane and retries.
        if !self.0.preparing.lock().unwrap().insert(offer.id.clone()) {
            anyhow::bail!("transfer {} is still being prepared", offer.id);
        }
        let _preparing = Preparing(self, offer.id.clone());
        let grant = sync::valid_ticket(&offer.ticket)
            .then(|| self.0.network.sync_grant(&offer.peer, &offer.ticket))
            .flatten()
            .filter(|grant| grant.dest_root.is_absolute());
        let Some(grant) = grant else {
            let message = "Refused: this device is not expecting a transfer with that ticket";
            wire::write_msg(&mut write, &refuse(message.into())).await?;
            anyhow::bail!(message);
        };
        let mut row = new_row(
            &offer.id,
            &offer.peer,
            &offer.peer_name,
            &manifest,
            offer.skipped,
        );
        row.destination = Some(grant.dest_root.to_string_lossy().into_owned());
        self.insert(row);

        let id = offer.id.clone();
        let prepare = self.lay_out_sync(offer, grant, manifest);
        tokio::pin!(prepare);
        let mut ping = tokio::time::interval(PENDING_PING);
        let mut lane_up = true;
        let prepared = loop {
            tokio::select! {
                prepared = &mut prepare => break prepared,
                _ = ping.tick(), if lane_up => {
                    // A lost lane doesn't stop the work: the sender resumes.
                    lane_up = wire::write_msg(&mut write, &Msg::Pending).await.is_ok();
                }
            }
        };
        let incoming = match prepared {
            Ok(incoming) => incoming,
            Err(error) => {
                let message = format!("Could not save the files: {error}");
                self.finish(&id, FileTransferState::Failed, Some(message.clone()));
                let _ = wire::write_msg(&mut write, &refuse(message)).await;
                return Err(error);
            }
        };
        self.0
            .incoming
            .lock()
            .unwrap()
            .insert(id.clone(), incoming.clone());
        drop(_preparing);
        self.run_receive_session(incoming, session_id, transport, write, rx)
            .await
    }

    /// Compare the manifest with the grant's basis, seed `.part` files from
    /// matching blocks, and persist the result before accepting.
    async fn lay_out_sync(
        &self,
        offer: SyncOffer,
        grant: SyncGrant,
        manifest: Manifest,
    ) -> anyhow::Result<Arc<Incoming>> {
        let meta_dir = self.incoming_dir(&offer.id);
        let cache = self.hash_cache();
        tokio::task::spawn_blocking(move || {
            let SyncGrant { dest_root, basis } = grant;
            // The engine's own choice, but a root id is still one plain name.
            let basis: HashMap<String, Vec<PathBuf>> = basis
                .into_iter()
                .filter(|(root, dirs)| {
                    manifest::validate_name(root).is_ok() && dirs.iter().all(|d| d.is_absolute())
                })
                .collect();
            std::fs::create_dir_all(&dest_root)?;
            let total = manifest.total_bytes();
            let meta = Meta {
                id: offer.id,
                peer: offer.peer,
                peer_name: offer.peer_name,
                digest: offer.digest,
                dest_root: dest_root.clone(),
                tops: HashMap::new(),
                created_at: now_ms(),
                entries: manifest.entries,
                ticket: Some(offer.ticket),
                basis: basis.clone(),
            };
            let meta_bytes = serde_json::to_vec(&meta)?;
            let incoming = incoming_from(meta, Progress::default());

            let files: Vec<(u64, &Entry)> = incoming
                .file_entries()
                .filter(|(_, e)| e.sha256.is_some())
                .collect();
            let found = sync::parallel(&files, |(_, entry)| {
                sync::examine_basis(entry, &basis, &cache)
            });
            if let Err(error) = cache.lock().unwrap().save() {
                tracing::warn!(%error, "could not save the file hash cache");
            }
            let mut seeds = Vec::new();
            let mut unchanged_bytes = 0;
            {
                let mut state = incoming.state.lock().unwrap();
                for (&(index, entry), basis) in files.iter().zip(found) {
                    match basis {
                        Basis::Unchanged { index: folder } => {
                            state.files.insert(
                                index,
                                FileState {
                                    blocks: RangeSet::full(block_count(entry.size)),
                                    done: true,
                                    unchanged: true,
                                    basis_index: Some(folder),
                                    ..Default::default()
                                },
                            );
                            state.done_bytes += entry.size;
                            unchanged_bytes += entry.size;
                        }
                        Basis::Seed { blocks, .. } => seeds.push((index, blocks)),
                        Basis::None => {}
                    }
                }
            }
            if let Some(message) = space_problem(&dest_root, total - unchanged_bytes) {
                anyhow::bail!(message);
            }
            for entry in incoming
                .manifest
                .entries
                .iter()
                .filter(|e| e.kind == EntryKind::Dir)
            {
                ensure_dir(&incoming.final_path(entry))?;
            }
            let seeded = sync::parallel(&seeds, |(index, blocks)| {
                let entry = &incoming.manifest.entries[*index as usize];
                sync::seed_part(&incoming.part_path(entry), entry, &basis, blocks.as_deref())
            });
            {
                let mut state = incoming.state.lock().unwrap();
                for ((index, ..), blocks) in seeds.iter().zip(seeded) {
                    let blocks = blocks?;
                    if blocks.is_empty() {
                        continue;
                    }
                    let size = incoming.manifest.entries[*index as usize].size;
                    state.done_bytes += blocks.bytes(size);
                    state.files.entry(*index).or_default().blocks = blocks;
                }
            }
            // Seeded blocks are already durable; record them, then accept.
            let progress = progress_of(&incoming.state.lock().unwrap());
            write_atomic(
                &meta_dir.join("progress.json"),
                &serde_json::to_vec(&progress)?,
            )?;
            write_atomic(&meta_dir.join("meta.json"), &meta_bytes)?;
            anyhow::Ok(Arc::new(incoming))
        })
        .await?
    }

    async fn run_receive_session<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        incoming: Arc<Incoming>,
        session_id: String,
        transport: FileTransferTransport,
        mut write: W,
        mut rx: mpsc::Receiver<anyhow::Result<Msg>>,
    ) -> anyhow::Result<()> {
        let stop = CancellationToken::new();
        let (control, mut outbox) = mpsc::channel::<Msg>(256);
        if let Some(old) = incoming.session.lock().unwrap().replace(Live {
            session_id: session_id.clone(),
            stop: stop.clone(),
            control,
        }) {
            old.stop.cancel();
        }
        let id = incoming.id.clone();
        let result = async {
            wire::write_msg(
                &mut write,
                &Msg::Accept {
                    have: incoming.have(),
                    sync: incoming.is_sync(),
                },
            )
            .await?;
            self.set_state(&id, FileTransferState::Transferring, Some(transport));
            self.progress(&id, incoming.state.lock().unwrap().done_bytes);
            if incoming.all_done() {
                self.spawn_complete(incoming.clone());
            } else if incoming.is_sync() {
                // Fully seeded (or restored) files need no data, only checking.
                for (file, _) in incoming.file_entries() {
                    self.maybe_finalize(&incoming, file);
                }
            }
            let mut progress = tokio::time::interval(PROGRESS_INTERVAL);
            let mut flush = tokio::time::interval(FLUSH_INTERVAL);
            loop {
                tokio::select! {
                    _ = stop.cancelled() => return Ok(()),
                    _ = incoming.cancel.cancelled() => {
                        let _ = wire::write_msg(&mut write, &Msg::Cancel { reason: None }).await;
                        self.abandon(&incoming, FileTransferState::Cancelled, None).await;
                        return Ok(());
                    }
                    msg = rx.recv() => match msg {
                        Some(Ok(Msg::Digest { file, sha256 })) => self.on_digest(&incoming, file, sha256),
                        Some(Ok(Msg::Skip { file, reason })) => self.on_skip(&incoming, file, reason).await,
                        Some(Ok(Msg::Cancel { .. })) => {
                            self.abandon(&incoming, FileTransferState::Cancelled, None).await;
                            return Ok(());
                        }
                        Some(Ok(Msg::Error { message })) => {
                            self.abandon(&incoming, FileTransferState::Failed, Some(message)).await;
                            return Ok(());
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => return Err(error),
                        None => anyhow::bail!("the sender went away"),
                    },
                    Some(msg) = outbox.recv() => {
                        let end = matches!(msg, Msg::Complete | Msg::Error { .. });
                        wire::write_msg(&mut write, &msg).await?;
                        if end {
                            let _ = write.shutdown().await;
                            return Ok(());
                        }
                    }
                    _ = progress.tick() => {
                        let done_bytes = incoming.state.lock().unwrap().done_bytes;
                        self.progress(&id, done_bytes);
                        wire::write_msg(&mut write, &Msg::Progress { done_bytes }).await?;
                    }
                    _ = flush.tick() => self.flush(&incoming).await,
                }
            }
        }
        .await;
        {
            let mut session = incoming.session.lock().unwrap();
            if session.as_ref().is_some_and(|l| l.session_id == session_id) {
                session.take();
            }
        }
        if let Err(error) = &result
            && !stop.is_cancelled()
        {
            self.flush(&incoming).await;
            self.progress(&id, incoming.state.lock().unwrap().done_bytes);
            self.set_note(
                &id,
                FileTransferState::Reconnecting,
                Some(format!(
                    "Connection lost ({error}); waiting for the sender to resume"
                )),
            );
        }
        result
    }

    pub(crate) async fn serve_data(
        &self,
        peer: String,
        id: String,
        session_id: String,
        mut io: BoxIo,
    ) -> anyhow::Result<()> {
        let incoming = self
            .0
            .incoming
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown transfer {id}"))?;
        anyhow::ensure!(incoming.peer == peer, "transfer belongs to another device");
        let stop = {
            let session = incoming.session.lock().unwrap();
            let live = session
                .as_ref()
                .filter(|l| l.session_id == session_id)
                .ok_or_else(|| anyhow::anyhow!("stale transfer session"))?;
            live.stop.clone()
        };
        let mut files: HashMap<u32, Arc<std::fs::File>> = HashMap::new();
        let result = async {
            loop {
                let frame = tokio::select! {
                    _ = stop.cancelled() => return Ok(()),
                    _ = incoming.cancel.cancelled() => return Ok(()),
                    frame = wire::read_frame(&mut io) => frame?,
                };
                match frame {
                    None => return Ok(()),
                    Some(Frame::Msg(_)) => anyhow::bail!("unexpected message on a data lane"),
                    Some(Frame::Block(block)) => {
                        self.on_block(&incoming, &mut files, block).await?
                    }
                }
            }
        }
        .await;
        // Blocks that drained after the control lane dropped are recorded
        // too: a restart before the sender resumes keeps them.
        if !incoming.cancel.is_cancelled() {
            self.flush(&incoming).await;
        }
        result
    }

    async fn on_block(
        &self,
        incoming: &Arc<Incoming>,
        files: &mut HashMap<u32, Arc<std::fs::File>>,
        block: wire::Block,
    ) -> anyhow::Result<()> {
        let entry = incoming
            .manifest
            .entries
            .get(block.file as usize)
            .filter(|e| e.kind == EntryKind::File)
            .ok_or_else(|| anyhow::anyhow!("block for an unknown file"))?;
        anyhow::ensure!(block.offset.is_multiple_of(BLOCK_SIZE), "misaligned block");
        let index = block.offset / BLOCK_SIZE;
        anyhow::ensure!(
            index < block_count(entry.size)
                && block.data.len() as u64 == block_len(entry.size, index),
            "block outside {}",
            entry.path
        );
        let file = block.file as u64;
        {
            let state = incoming.state.lock().unwrap();
            if state
                .files
                .get(&file)
                .is_some_and(|f| f.done || f.blocks.contains(index))
            {
                return Ok(());
            }
        }
        let handle = match files.get(&block.file) {
            Some(handle) => handle.clone(),
            None => {
                if files.len() >= 16 {
                    files.clear();
                }
                let path = incoming.part_path(entry);
                let handle =
                    Arc::new(tokio::task::spawn_blocking(move || open_part(&path)).await??);
                files.insert(block.file, handle.clone());
                handle
            }
        };
        let length = block.data.len() as u64;
        let verified = tokio::task::spawn_blocking(move || -> anyhow::Result<bool> {
            if !block.verify() {
                return Ok(false);
            }
            write_at(&handle, &block.data, block.offset)?;
            Ok(true)
        })
        .await??;
        if !verified {
            incoming
                .send_control(Msg::Resend {
                    file,
                    blocks: Some(vec![index]),
                })
                .await;
            return Ok(());
        }
        let (done_bytes, skipped) = {
            let mut state = incoming.state.lock().unwrap();
            let entry_state = state.files.entry(file).or_default();
            let skipped = entry_state.skipped;
            if !entry_state.done && entry_state.blocks.insert(index) {
                entry_state.touched = true;
                state.done_bytes += length;
            }
            (state.done_bytes, skipped)
        };
        if skipped {
            // The sender gave up on this file while the block was in flight.
            files.remove(&(file as u32));
            let part = incoming.part_path(entry);
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(part)).await;
            return Ok(());
        }
        // Blocks still draining after the control lane dropped count too.
        self.progress(&incoming.id, done_bytes);
        self.maybe_finalize(incoming, file);
        Ok(())
    }

    fn on_digest(&self, incoming: &Arc<Incoming>, file: u64, sha256: String) {
        // Sync entries carry their digest in the manifest.
        let valid = incoming
            .manifest
            .entries
            .get(file as usize)
            .is_some_and(|e| e.kind == EntryKind::File && e.sha256.is_none())
            && sha256.len() == 64;
        if !valid {
            return;
        }
        incoming
            .state
            .lock()
            .unwrap()
            .files
            .entry(file)
            .or_default()
            .digest = Some(sha256);
        self.maybe_finalize(incoming, file);
    }

    /// Tolerant sync: the sender gave up on `file`; complete without it.
    async fn on_skip(&self, incoming: &Arc<Incoming>, file: u64, reason: String) {
        let Some(entry) = incoming
            .manifest
            .entries
            .get(file as usize)
            .filter(|e| e.kind == EntryKind::File && incoming.is_sync())
        else {
            return;
        };
        {
            let mut state = incoming.state.lock().unwrap();
            let f = state.files.entry(file).or_default();
            // A file already verifying landed with the promised content.
            if f.done || f.finalizing {
                return;
            }
            let had = f.blocks.bytes(entry.size);
            f.done = true;
            f.skipped = true;
            f.skip_reason = Some(reason.chars().take(500).collect());
            f.blocks = RangeSet::full(block_count(entry.size));
            f.touched = false;
            state.done_bytes += entry.size - had;
        }
        let part = incoming.part_path(entry);
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(part)).await;
        self.update_row(&incoming.id, |row| row.skipped += 1);
        if incoming.all_done() {
            self.spawn_complete(incoming.clone());
        }
    }

    fn maybe_finalize(&self, incoming: &Arc<Incoming>, file: u64) {
        let entry = &incoming.manifest.entries[file as usize];
        let digest = {
            let mut state = incoming.state.lock().unwrap();
            let f = state.files.entry(file).or_default();
            if f.done || f.finalizing || f.blocks.len() < block_count(entry.size) {
                return;
            }
            let Some(digest) = f.digest.clone().or_else(|| entry.sha256.clone()) else {
                return;
            };
            f.finalizing = true;
            digest
        };
        let entry = entry.clone();
        let transfers = self.clone();
        let incoming = incoming.clone();
        tokio::spawn(async move {
            let part = incoming.part_path(&entry);
            let target = incoming.final_path(&entry);
            // Sync layouts are exact: nothing is renamed around a conflict.
            let top_level = entry.is_top_level() && !incoming.is_sync();
            let (size, mode, mtime_ms) = (entry.size, entry.mode, entry.mtime_ms);
            let result = tokio::task::spawn_blocking(move || {
                finalize_file(&part, &target, size, mode, mtime_ms, &digest, top_level)
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            match result {
                Ok(Some(landed)) => {
                    if top_level && landed != incoming.final_path(&entry) {
                        let shown = landed.to_string_lossy().into_owned();
                        let name = entry.path.clone();
                        transfers.update_row(&incoming.id, |row| {
                            if let Some(item) = row.items.iter_mut().find(|i| i.name == name) {
                                item.path = Some(shown);
                            }
                        });
                    }
                    {
                        let mut state = incoming.state.lock().unwrap();
                        let f = state.files.entry(file).or_default();
                        f.done = true;
                        f.finalizing = false;
                        f.landed = Some(landed);
                    }
                    if incoming.all_done() {
                        transfers.spawn_complete(incoming);
                    }
                }
                Ok(None) => {
                    // Whole-file digest mismatch: fetch the file again.
                    let resends = {
                        let mut state = incoming.state.lock().unwrap();
                        let f = state.files.entry(file).or_default();
                        let verified = f.blocks.bytes(entry.size);
                        f.blocks = RangeSet::default();
                        f.finalizing = false;
                        f.digest = None;
                        f.resends += 1;
                        let resends = f.resends;
                        state.done_bytes = state.done_bytes.saturating_sub(verified);
                        resends
                    };
                    if resends > MAX_RESENDS {
                        let message = format!("{} kept failing verification", entry.path);
                        incoming
                            .send_control(Msg::Error {
                                message: message.clone(),
                            })
                            .await;
                        transfers
                            .abandon(&incoming, FileTransferState::Failed, Some(message))
                            .await;
                    } else {
                        incoming
                            .send_control(Msg::Resend { file, blocks: None })
                            .await;
                    }
                }
                Err(error) => {
                    let message = format!("Could not save {}: {error}", entry.path);
                    incoming
                        .send_control(Msg::Error {
                            message: message.clone(),
                        })
                        .await;
                    transfers
                        .abandon(&incoming, FileTransferState::Failed, Some(message))
                        .await;
                }
            }
        });
    }

    fn spawn_complete(&self, incoming: Arc<Incoming>) {
        {
            let mut state = incoming.state.lock().unwrap();
            if state.completing {
                return;
            }
            state.completing = true;
        }
        let transfers = self.clone();
        tokio::spawn(async move {
            let links = incoming.clone();
            let result = tokio::task::spawn_blocking(move || {
                if let Err(error) = finish_tree(&links) {
                    tracing::warn!(%error, "file transfer: could not finish folders");
                }
                if links.is_sync() {
                    remove_skipped_parts(&links);
                    write_sync_result(&links)?;
                }
                anyhow::Ok(())
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
            if let Err(error) = result {
                let message = format!("Could not save the files: {error}");
                incoming
                    .send_control(Msg::Error {
                        message: message.clone(),
                    })
                    .await;
                transfers
                    .abandon(&incoming, FileTransferState::Failed, Some(message))
                    .await;
                return;
            }
            let _ = std::fs::remove_dir_all(transfers.incoming_dir(&incoming.id));
            transfers.finish(&incoming.id, FileTransferState::Completed, None);
            incoming.send_control(Msg::Complete).await;
        });
    }

    /// End an incoming transfer here: drop partial data and resume state.
    async fn abandon(
        &self,
        incoming: &Arc<Incoming>,
        state: FileTransferState,
        error: Option<String>,
    ) {
        let cleanup = incoming.clone();
        let _ = tokio::task::spawn_blocking(move || remove_parts(&cleanup)).await;
        let _ = std::fs::remove_dir_all(self.incoming_dir(&incoming.id));
        self.finish(&incoming.id, state, error);
    }

    /// Persist verified blocks — only after the bytes they describe are
    /// on disk, so a crash never records a block that isn't there.
    async fn flush(&self, incoming: &Arc<Incoming>) {
        let (touched, progress) = {
            let mut state = incoming.state.lock().unwrap();
            let touched: Vec<u64> = state
                .files
                .iter_mut()
                .filter_map(|(i, f)| (std::mem::take(&mut f.touched) && !f.done).then_some(*i))
                .collect();
            (touched, progress_of(&state))
        };
        if incoming.cancel.is_cancelled() {
            return;
        }
        let parts: Vec<PathBuf> = touched
            .iter()
            .map(|i| incoming.part_path(&incoming.manifest.entries[*i as usize]))
            .collect();
        let path = self.incoming_dir(&incoming.id).join("progress.json");
        let _ = tokio::task::spawn_blocking(move || {
            for part in parts {
                if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&part) {
                    let _ = file.sync_data();
                }
            }
            if path.parent().is_some_and(Path::exists) {
                let _ = serde_json::to_vec(&progress)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| write_atomic(&path, &bytes));
            }
        })
        .await;
    }
}

/// Resume state: every file with verified blocks or done.
fn have_of(state: &RecvState) -> Vec<FileHave> {
    state
        .files
        .iter()
        .filter(|(_, f)| f.done || !f.blocks.is_empty())
        .map(|(file, f)| FileHave {
            file: *file,
            blocks: f.blocks.clone(),
            done: f.done,
            unchanged: f.unchanged,
            skipped: f.skipped,
        })
        .collect()
}

fn progress_of(state: &RecvState) -> Progress {
    Progress {
        files: have_of(state),
        basis_index: state
            .files
            .iter()
            .filter_map(|(file, f)| Some((*file, f.basis_index?)))
            .collect(),
    }
}

/// A ticketed offer, before it was granted.
struct SyncOffer {
    peer: String,
    peer_name: String,
    id: String,
    digest: String,
    ticket: String,
    skipped: u64,
}

/// Marks a sync transfer as being prepared until dropped.
struct Preparing<'a>(&'a Transfers, String);

impl Drop for Preparing<'_> {
    fn drop(&mut self) {
        self.0.0.preparing.lock().unwrap().remove(&self.1);
    }
}

/// The record the receiving move engine applies staging from.
fn write_sync_result(incoming: &Incoming) -> anyhow::Result<()> {
    let result = {
        let state = incoming.state.lock().unwrap();
        let mut result = sync::SyncResult {
            transfer_id: incoming.id.clone(),
            ticket: incoming.ticket.clone().unwrap_or_default(),
            files: Vec::new(),
            dirs: Vec::new(),
            symlinks: Vec::new(),
        };
        for (index, entry) in incoming.manifest.entries.iter().enumerate() {
            match entry.kind {
                EntryKind::Dir => result.dirs.push(sync::SyncDir {
                    rel: entry.path.clone(),
                    mode: entry.mode,
                }),
                EntryKind::Symlink => result.symlinks.push(sync::SyncSymlink {
                    rel: entry.path.clone(),
                    target: entry.target.clone().unwrap_or_default(),
                }),
                EntryKind::File => {
                    let f = state.files.get(&(index as u64));
                    let outcome = match f {
                        Some(f) if f.skipped => sync::SyncOutcome::Skipped,
                        Some(f) if f.unchanged => sync::SyncOutcome::Unchanged,
                        Some(f) if f.done => sync::SyncOutcome::Landed,
                        _ => anyhow::bail!("{} never arrived", entry.path),
                    };
                    result.files.push(sync::SyncFile {
                        rel: entry.path.clone(),
                        outcome,
                        sha256: entry
                            .sha256
                            .clone()
                            .or_else(|| f.and_then(|f| f.digest.clone()))
                            .unwrap_or_default(),
                        size: entry.size,
                        mode: entry.mode,
                        mtime_ms: entry.mtime_ms,
                        reason: f.and_then(|f| f.skip_reason.clone()),
                        basis_index: f.and_then(|f| f.basis_index).map(|i| i as usize),
                    });
                }
            }
        }
        result
    };
    let path = incoming.dest_root.join(sync::SYNC_RESULT_FILE);
    let tmp = incoming
        .dest_root
        .join(format!(".zeron-sync.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    std::io::Write::write_all(&mut file, &serde_json::to_vec(&result)?)?;
    file.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn incoming_from(meta: Meta, progress: Progress) -> Incoming {
    let mut state = RecvState::default();
    let manifest = Manifest {
        entries: meta.entries,
    };
    for have in progress.files {
        let Some(entry) = manifest
            .entries
            .get(have.file as usize)
            .filter(|e| e.kind == EntryKind::File)
        else {
            continue;
        };
        let count = block_count(entry.size);
        if !have.blocks.within(count) && !have.blocks.is_empty() {
            continue;
        }
        state.done_bytes += if have.done {
            entry.size
        } else {
            have.blocks.bytes(entry.size)
        };
        state.files.insert(
            have.file,
            FileState {
                blocks: if have.done {
                    RangeSet::full(count)
                } else {
                    have.blocks
                },
                done: have.done,
                unchanged: have.done && have.unchanged,
                basis_index: progress.basis_index.get(&have.file).copied(),
                skipped: have.done && have.skipped,
                ..Default::default()
            },
        );
    }
    Incoming {
        id: meta.id,
        peer: meta.peer,
        digest: meta.digest,
        manifest,
        dest_root: meta.dest_root,
        tops: meta.tops,
        ticket: meta.ticket,
        state: Mutex::new(state),
        session: Mutex::new(None),
        cancel: CancellationToken::new(),
    }
}

fn new_row(
    id: &str,
    peer: &str,
    peer_name: &str,
    manifest: &Manifest,
    skipped: u64,
) -> FileTransfer {
    let now = now_ms();
    FileTransfer {
        id: id.to_owned(),
        direction: FileTransferDirection::Incoming,
        peer_device_id: peer.to_owned(),
        peer_device_name: peer_name.to_owned(),
        state: FileTransferState::Connecting,
        transport: None,
        items: manifest.items(),
        file_count: manifest.file_count(),
        total_bytes: manifest.total_bytes(),
        done_bytes: 0,
        bytes_per_sec: 0,
        destination: None,
        skipped,
        error: None,
        created_at: now,
        updated_at: now,
        finished_at: None,
    }
}

fn incoming_row(incoming: &Incoming, peer_name: &str, state: FileTransferState) -> FileTransfer {
    let mut row = new_row(
        &incoming.id,
        &incoming.peer,
        peer_name,
        &incoming.manifest,
        0,
    );
    row.state = state;
    row.destination = Some(incoming.dest_root.to_string_lossy().into_owned());
    row.done_bytes = incoming.state.lock().unwrap().done_bytes;
    for (item, entry) in row.items.iter_mut().zip(
        incoming
            .manifest
            .entries
            .iter()
            .filter(|e| e.is_top_level()),
    ) {
        item.path = Some(incoming.final_path(entry).to_string_lossy().into_owned());
    }
    row
}

/// A manifest directory: create it, or accept an existing real directory
/// (a resumed transfer) — never a symlink or file in its place.
fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path)?;
            anyhow::ensure!(
                meta.is_dir(),
                "{} exists and is not a folder",
                path.display()
            );
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn open_part(path: &Path) -> anyhow::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    Ok(options.open(path)?)
}

pub(crate) fn write_at(file: &std::fs::File, data: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(data, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut written = 0;
        while written < data.len() {
            written += file.seek_write(&data[written..], offset + written as u64)?;
        }
        Ok(())
    }
}

/// Check the assembled part against the sender's digest and move it into
/// place. `Ok(None)` = mismatch (resend); `Ok(Some(path))` = where it landed.
fn finalize_file(
    part: &Path,
    target: &Path,
    size: u64,
    mode: u32,
    mtime_ms: Option<i64>,
    expected: &str,
    top_level: bool,
) -> anyhow::Result<Option<PathBuf>> {
    let file = open_part(part)?;
    if size == 0 {
        file.set_len(0)?;
    }
    anyhow::ensure!(
        file.metadata()?.len() >= size,
        "{} is shorter than expected",
        part.display()
    );
    file.set_len(size)?;
    let mut hasher = Sha256::new();
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, &file);
    std::io::copy(&mut reader, &mut hasher)?;
    if hex(&hasher.finalize()) != expected {
        return Ok(None);
    }
    file.sync_all()?;
    #[cfg(unix)]
    if mode != 0 {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    if let Some(time) = mtime_ms
        .and_then(|ms| u64::try_from(ms).ok())
        .and_then(|ms| std::time::UNIX_EPOCH.checked_add(Duration::from_millis(ms)))
        && let Err(error) = file.set_modified(time)
    {
        tracing::debug!(%error, part = %part.display(), "could not restore a modification time");
    }
    let mut landed = target.to_path_buf();
    if top_level {
        // Something appeared under the reserved name meanwhile: keep both.
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut n = 2;
        while std::fs::symlink_metadata(&landed).is_ok() {
            landed = target.with_file_name(manifest::numbered(&name, n));
            n += 1;
        }
    }
    std::fs::rename(part, &landed)?;
    Ok(Some(landed))
}

/// Last step: symlinks (as symlinks) and folder permissions, deepest first
/// so a read-only folder never blocks its children.
fn finish_tree(incoming: &Incoming) -> anyhow::Result<()> {
    for entry in incoming
        .manifest
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::Symlink)
    {
        let path = incoming.final_path(entry);
        if std::fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(entry.target.as_deref().unwrap_or_default(), &path)?;
        #[cfg(not(unix))]
        tracing::debug!(path = %path.display(), "symlinks are not recreated on this platform");
    }
    #[cfg(unix)]
    for entry in incoming
        .manifest
        .entries
        .iter()
        .rev()
        .filter(|e| e.kind == EntryKind::Dir && e.mode != 0)
    {
        use std::os::unix::fs::PermissionsExt;
        // Keep folders writable by their owner: received files must stay
        // removable from the inbox.
        let mode = (entry.mode & 0o777) | 0o700;
        let _ = std::fs::set_permissions(
            incoming.final_path(entry),
            std::fs::Permissions::from_mode(mode),
        );
    }
    Ok(())
}

/// Remove unfinished `*.part` files and the folders left empty by them.
fn remove_parts(incoming: &Incoming) {
    // Landed files have no part left; unchanged ones never had one.
    let settled: HashSet<u64> = {
        let state = incoming.state.lock().unwrap();
        state
            .files
            .iter()
            .filter(|(_, f)| f.done && !f.skipped)
            .map(|(i, _)| *i)
            .collect()
    };
    for (index, entry) in incoming.file_entries() {
        if !settled.contains(&index) {
            let _ = std::fs::remove_file(incoming.part_path(entry));
        }
    }
    for entry in incoming
        .manifest
        .entries
        .iter()
        .rev()
        .filter(|e| e.kind == EntryKind::Dir)
    {
        let _ = std::fs::remove_dir(incoming.final_path(entry));
    }
}

/// Parts of skipped files that a block still in flight may have recreated.
fn remove_skipped_parts(incoming: &Incoming) {
    let skipped: Vec<u64> = {
        let state = incoming.state.lock().unwrap();
        state
            .files
            .iter()
            .filter(|(_, f)| f.skipped)
            .map(|(i, _)| *i)
            .collect()
    };
    for index in skipped {
        let _ =
            std::fs::remove_file(incoming.part_path(&incoming.manifest.entries[index as usize]));
    }
}

/// `Some(message)` when the destination volume can't take `bytes` more.
fn space_problem(dest: &Path, bytes: u64) -> Option<String> {
    #[cfg(unix)]
    {
        let mut probe = dest.to_path_buf();
        while !probe.exists() {
            probe = probe.parent()?.to_path_buf();
        }
        let path = std::ffi::CString::new(probe.to_string_lossy().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        let available = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
        if available < bytes.saturating_add(SPACE_MARGIN) {
            return Some(format!(
                "Not enough space on the receiving device: needs {} MB, {} MB free",
                bytes / 1_000_000 + 1,
                available / 1_000_000
            ));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = (dest, bytes);
        None
    }
}
