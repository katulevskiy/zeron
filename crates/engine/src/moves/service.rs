//! The move service: one per engine, playing both ends of a move.
//!
//! As the **source** it runs a move of a chat it hosts (`source.rs`): it
//! asks the target, copies while the agent works, stops the agent between
//! steps, sends the last changes and hands the chat over. As the **target**
//! it takes chats over (`target.rs`): it decides where things land on this
//! device, accepts the transfers a move registered tickets for, and applies
//! them when the source commits.
//!
//! Progress is published on the chat's registry row (`ChatMove`), which every
//! device already syncs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use zeron_proto::{
    ChatMove, HarnessId, MoveCandidate, MovePhase, MoveWhen, StartMoveParams, StartMoveReply,
    capabilities,
};

use crate::EngineError;
use crate::agent_accounts::AgentAccounts;
use crate::doc_host::DocHost;
use crate::registry::HarnessRegistry;
use crate::repos::Repos;
use crate::sessions::SessionsEngine;
use crate::uploads::Uploads;
use crate::workspace_host::WorkspaceHost;

use super::lineage::LineageStore;
use super::protocol::{ProbeParams, ProbeReply};
use super::target::{CommitState, Incoming};

/// What a running move is told by the user (through any device).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Control {
    Run,
    Now,
    Cancel,
}

pub(super) struct Outgoing {
    pub control: watch::Sender<Control>,
}

/// Sync-transfer tickets a target registered for a move's rounds: the only
/// way a peer gets files written outside the inbox. Shared with the engine's
/// transfer network (`file_transfers.rs`), which consults it when a
/// ticketed offer arrives.
#[derive(Default)]
pub struct Tickets {
    grants: Mutex<HashMap<String, (String, zeron_transfer::SyncGrant)>>,
}

impl Tickets {
    pub fn grant(&self, peer: &str, ticket: &str) -> Option<zeron_transfer::SyncGrant> {
        let grants = self.lock();
        let (owner, grant) = grants.get(ticket)?;
        (owner == peer).then(|| grant.clone())
    }

    pub(super) fn insert(&self, ticket: String, peer: String, grant: zeron_transfer::SyncGrant) {
        self.lock().insert(ticket, (peer, grant));
    }

    pub(super) fn remove_move(&self, move_id: &str) {
        let prefix = format!("{move_id}.");
        self.lock().retain(|ticket, _| !ticket.starts_with(&prefix));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, zeron_transfer::SyncGrant)>> {
        self.grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A round's ticket: `<move id>.r<round>`.
pub(super) fn ticket(move_id: &str, round: u32) -> String {
    format!("{move_id}.r{round}")
}

pub(super) struct Inner {
    pub device_id: String,
    pub device_name: String,
    pub sessions: SessionsEngine,
    pub doc_host: DocHost,
    pub workspace: WorkspaceHost,
    pub registry: Arc<HarnessRegistry>,
    pub repos: Repos,
    pub transfers: zeron_transfer::Transfers,
    pub accounts: AgentAccounts,
    pub uploads: Uploads,
    pub links: Arc<Mutex<Option<Arc<zeron_rpc::LinkCache>>>>,
    /// `{store}/moves`: lineage, staging for incoming moves, scratch for
    /// outgoing ones.
    pub dir: PathBuf,
    pub lineage: LineageStore,
    pub tickets: Arc<Tickets>,
    pub port_roots: zeron_harness::portable::PortRoots,
    /// Moves this engine runs as the source, by chat.
    pub outgoing: Mutex<HashMap<String, Outgoing>>,
    /// Moves this engine receives, by move id. Never held across awaits.
    pub incoming: Mutex<HashMap<String, Incoming>>,
    /// Where each commit this engine received stands (for aborts racing it).
    pub commits: Mutex<HashMap<String, CommitState>>,
}

#[derive(Clone)]
pub struct MoveService(pub(super) Arc<Inner>);

pub struct MoveServiceConfig {
    pub device_id: String,
    pub device_name: String,
    pub sessions: SessionsEngine,
    pub doc_host: DocHost,
    pub workspace: WorkspaceHost,
    pub registry: Arc<HarnessRegistry>,
    pub repos: Repos,
    pub transfers: zeron_transfer::Transfers,
    pub accounts: AgentAccounts,
    pub uploads: Uploads,
    pub links: Arc<Mutex<Option<Arc<zeron_rpc::LinkCache>>>>,
    pub dir: PathBuf,
    pub tickets: Arc<Tickets>,
}

impl MoveService {
    pub fn new(config: MoveServiceConfig) -> Self {
        let _ = std::fs::create_dir_all(&config.dir);
        // Staging and scratch from moves a restart cut short are useless:
        // their peer gave up (or will, and abort).
        for sub in ["in", "out"] {
            let _ = std::fs::remove_dir_all(config.dir.join(sub));
        }
        let lineage = LineageStore::open(&config.dir);
        Self(Arc::new(Inner {
            device_id: config.device_id,
            device_name: config.device_name,
            sessions: config.sessions,
            doc_host: config.doc_host,
            workspace: config.workspace,
            registry: config.registry,
            repos: config.repos,
            transfers: config.transfers,
            accounts: config.accounts,
            uploads: config.uploads,
            links: config.links,
            dir: config.dir,
            lineage,
            tickets: config.tickets,
            port_roots: zeron_harness::portable::PortRoots::from_env(),
            outgoing: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            commits: Mutex::new(HashMap::new()),
        }))
    }

    /// Boot: a move this engine was running when it stopped can't resume (its
    /// agent, transfers and target state are gone). Say so on the row.
    pub fn recover(&self) {
        let Ok(chats) = self.0.workspace.read_chats() else {
            return;
        };
        for chat in chats {
            let Some(state) = chat.move_state else { continue };
            if state.from_device_id != self.0.device_id || !state.is_live() {
                continue;
            }
            let mut state = state;
            state.phase = if chat.device_id == state.to_device_id {
                MovePhase::Done // the handover landed before the restart
            } else {
                MovePhase::Failed
            };
            if state.phase == MovePhase::Failed {
                state.error = Some("Zeron restarted during the move".into());
            }
            state.finished_at = Some(chrono::Utc::now().timestamp_millis());
            let _ = self.0.workspace.set_chat_move(&chat.id, Some(&state));
        }
    }

    /// `StartMove` on the chat's host.
    pub fn start(&self, params: StartMoveParams) -> Result<StartMoveReply, EngineError> {
        let inner = &self.0;
        let chat = inner
            .workspace
            .chat(&params.chat_id)?
            .ok_or_else(|| EngineError::Other("No such chat".into()))?;
        if chat.device_id != inner.device_id {
            return Err(EngineError::Other(
                "This chat runs on another device; ask that device to move it".into(),
            ));
        }
        if params.to_device_id == inner.device_id {
            return Err(EngineError::Other("The chat already runs on this device".into()));
        }
        if !chat.on_chat2() {
            return Err(EngineError::Other(
                "This chat hasn't finished syncing to your account yet; try again in a moment"
                    .into(),
            ));
        }
        if chat.cwd.as_deref().is_none_or(str::is_empty) {
            return Err(EngineError::Other("This chat has no folder to move".into()));
        }
        let device = inner
            .workspace
            .read_devices()?
            .into_iter()
            .find(|d| d.id == params.to_device_id)
            .ok_or_else(|| EngineError::Other("Unknown device".into()))?;
        if !device.supports(capabilities::SESSION_MOVE_V1) {
            return Err(EngineError::Other(format!(
                "{} runs a version of Zeron that can't take chats yet; update it first",
                device.name
            )));
        }
        let move_id = uuid::Uuid::new_v4().to_string();
        let (control, control_rx) = watch::channel(match params.when {
            MoveWhen::Now => Control::Now,
            MoveWhen::SafePoint => Control::Run,
        });
        {
            let mut outgoing = inner.outgoing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if outgoing.contains_key(&chat.id) {
                return Err(EngineError::Other("This chat is already moving".into()));
            }
            outgoing.insert(
                chat.id.clone(),
                Outgoing { control },
            );
        }
        let state = ChatMove {
            id: move_id.clone(),
            from_device_id: inner.device_id.clone(),
            to_device_id: device.id.clone(),
            to_device_name: device.name.clone(),
            phase: MovePhase::Preparing,
            when: params.when,
            detail: None,
            bytes_total: 0,
            bytes_done: 0,
            files_total: 0,
            started_at: chrono::Utc::now().timestamp_millis(),
            finished_at: None,
            error: None,
            notes: Vec::new(),
        };
        if let Err(error) = inner.workspace.set_chat_move(&chat.id, Some(&state)) {
            inner
                .outgoing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&chat.id);
            return Err(error);
        }
        let service = self.clone();
        tokio::spawn(async move {
            let chat_id = chat.id.clone();
            super::source::run(service.clone(), chat, state, control_rx).await;
            service
                .0
                .outgoing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&chat_id);
        });
        Ok(StartMoveReply { move_id })
    }

    /// `MoveNow`: stop waiting for a safe point.
    pub fn move_now(&self, chat_id: &str) -> Result<(), EngineError> {
        self.signal(chat_id, Control::Now)
    }

    /// `CancelMove`: allowed until the handover starts.
    pub fn cancel(&self, chat_id: &str) -> Result<(), EngineError> {
        self.signal(chat_id, Control::Cancel)
    }

    fn signal(&self, chat_id: &str, control: Control) -> Result<(), EngineError> {
        let outgoing = self
            .0
            .outgoing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let move_ = outgoing
            .get(chat_id)
            .ok_or_else(|| EngineError::Other("This chat isn't moving".into()))?;
        move_.control.send_if_modified(|current| {
            // A cancel always wins; "now" never undoes it.
            if *current == Control::Cancel || *current == control {
                return false;
            }
            *current = control;
            true
        });
        Ok(())
    }

    /// `MoveCandidates`: every other device, with whether it can take this
    /// chat (asked in parallel; a device that doesn't answer is offline).
    pub async fn candidates(&self, chat_id: &str) -> Result<Vec<MoveCandidate>, EngineError> {
        let inner = &self.0;
        let harness = inner
            .workspace
            .chat_config(chat_id)
            .map(|c| c.harness)
            .unwrap_or(HarnessId::ClaudeCode);
        let devices = inner.workspace.read_devices()?;
        let probes = devices
            .into_iter()
            .filter(|d| d.id != inner.device_id)
            .map(|device| {
                let service = self.clone();
                async move {
                    let supported = device.supports(capabilities::SESSION_MOVE_V1);
                    let mut candidate = MoveCandidate {
                        device_id: device.id.clone(),
                        device_name: device.name.clone(),
                        online: false,
                        supported,
                        harness_installed: false,
                        harness_signed_in: None,
                        problem: None,
                        asleep: false,
                        note: None,
                    };
                    if !supported {
                        candidate.problem = Some("Needs a newer Zeron".into());
                        return candidate;
                    }
                    let probe = tokio::time::timeout(
                        std::time::Duration::from_secs(4),
                        service.call::<ProbeReply>(
                            &device.id,
                            zeron_rpc::methods::MOVE_PROBE,
                            &ProbeParams { harness },
                        ),
                    )
                    .await;
                    match probe {
                        Ok(Ok(reply)) => {
                            candidate.online = true;
                            candidate.harness_installed = reply.harness_installed;
                            candidate.harness_signed_in = reply.harness_signed_in;
                            if !reply.harness_installed {
                                candidate.problem = Some(format!(
                                    "{} isn't installed there",
                                    harness_name(harness)
                                ));
                            }
                        }
                        _ => candidate.problem = Some("Offline".into()),
                    }
                    candidate
                }
            });
        let mut out = futures::future::join_all(probes).await;
        out.sort_by(|a, b| {
            (b.online && b.problem.is_none())
                .cmp(&(a.online && a.problem.is_none()))
                .then_with(|| a.device_name.cmp(&b.device_name))
        });
        Ok(out)
    }

    /// `MoveProbe` (target side): can this device run `harness`?
    pub async fn probe(&self, params: ProbeParams) -> ProbeReply {
        let inner = &self.0;
        let harness_installed = inner
            .registry
            .descriptors()
            .into_iter()
            .any(|d| d.id == params.harness && d.installed);
        let harness_signed_in = inner.accounts.signed_in(params.harness).await.ok().flatten();
        ProbeReply {
            harness_installed,
            harness_signed_in,
            free_bytes: free_bytes(&inner.dir),
        }
    }

    // ── engine ⇄ engine ────────────────────────────────────────────────────

    pub(super) async fn call<T: serde::de::DeserializeOwned>(
        &self,
        device_id: &str,
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<T, EngineError> {
        let links = self
            .0
            .links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| EngineError::Other("Not connected to your other devices".into()))?;
        let client = links
            .client(device_id)
            .await
            .map_err(|e| EngineError::Other(e.to_string()))?;
        let value = client
            .call(method, serde_json::to_value(params).map_err(|e| EngineError::Other(e.to_string()))?)
            .await
            .map_err(|e| match e {
                zeron_rpc::RpcError::Failed(message) => EngineError::Other(message),
                other => EngineError::Other(other.to_string()),
            })?;
        serde_json::from_value(value).map_err(|e| EngineError::Other(format!("{method}: {e}")))
    }

    /// Publish a move's state on the chat row.
    pub(super) fn publish(&self, chat_id: &str, state: &ChatMove) {
        if let Err(error) = self.0.workspace.set_chat_move(chat_id, Some(state)) {
            tracing::warn!(chat = %chat_id, %error, "could not publish move progress");
        }
    }

    pub fn tickets(&self) -> Arc<Tickets> {
        self.0.tickets.clone()
    }
}

pub(super) fn harness_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        HarnessId::Cursor => "Cursor",
        HarnessId::Devin => "Devin",
        HarnessId::Grok => "Grok",
        HarnessId::Hermes => "Hermes",
        HarnessId::Pi => "Pi",
        HarnessId::Opencode => "OpenCode",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Mock => "Mock",
    }
}

/// Free space on the volume holding `path` (Unix; `None` elsewhere).
pub(super) fn free_bytes(path: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let mut probe = path.to_path_buf();
        while !probe.exists() {
            probe = probe.parent()?.to_path_buf();
        }
        let path = std::ffi::CString::new(probe.to_string_lossy().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
