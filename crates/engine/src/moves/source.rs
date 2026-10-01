//! The source side of a move: the engine hosting the chat drives it.
//!
//! ```text
//! Preparing ─▶ Copying ─▶ Waiting ─▶ Finishing ─▶ Handover ─▶ Done
//!   ask the     workspace   for a      agent        target
//!   target,     + extras    safe       stopped:     applies and
//!   plan        while the   point      last delta,  takes the
//!               agent works            session      chat over
//! ```
//!
//! Until the target commits, every failure (or a cancel) puts the chat back
//! exactly as it was here: the hold is lifted and a run this move stopped
//! continues. After the commit the target owns the chat; the source only
//! records where the work went.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use tokio::sync::watch;
use zeron_doc::{MessagePart, SessionCommandPayload};
use zeron_proto::{Chat, ChatMove, FileTransferState, HarnessId, MovePhase};

use crate::EngineError;

use super::harvest;
use super::lineage::extra_origin;
use super::note;
use super::protocol::*;
use super::scout;
use super::service::{Control, MoveService, harness_name, ticket};

/// How long the target may take to apply a move and take the chat over.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Preparing can include cloning the repository on the target.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Waiting for a stopped run to end before giving up on the move.
const STOP_TIMEOUT: Duration = Duration::from_secs(20);
/// Progress writes to the chat row at most this often.
const PUBLISH_EVERY: Duration = Duration::from_millis(1000);

enum Ending {
    Done(Vec<String>),
    Cancelled,
    Failed(String),
}

/// What the move did to the chat here, so a failure can put it back.
#[derive(Default)]
struct Stopped {
    held: bool,
    /// A turn was running (or waiting on a question) and this move ended it.
    was_active: bool,
    pending_question: Option<String>,
    /// `MoveCommit` went out: only from here on can the target take over.
    commit_sent: bool,
}

pub(super) async fn run(
    service: MoveService,
    chat: Chat,
    mut state: ChatMove,
    control: watch::Receiver<Control>,
) {
    let chat_id = chat.id.clone();
    let scratch = service.0.dir.join("out").join(&state.id);
    let mut stopped = Stopped::default();
    let ending = match drive(&service, &chat, &mut state, control, &scratch, &mut stopped).await {
        Ok(notes) => Ending::Done(notes),
        Err(MoveError::Cancelled) => Ending::Cancelled,
        Err(MoveError::Failed(message)) => Ending::Failed(message),
    };
    let inner = &service.0;
    // A failure late in the handover may still have been a success on the
    // target: only the target can say, and the chat must never run on both.
    let ending = match ending {
        Ending::Done(notes) => Ending::Done(notes),
        other => {
            let reason = match &other {
                Ending::Failed(message) => Some(message.clone()),
                _ => None,
            };
            let settled = if stopped.commit_sent {
                settle_with_target(&service, &chat_id, &state, reason).await
            } else {
                // No commit, no takeover: tell the target to clean up and
                // move on.
                let _ = tokio::time::timeout(
                    Duration::from_secs(10),
                    service.call::<AbortReply>(
                        &state.to_device_id,
                        zeron_rpc::methods::MOVE_ABORT,
                        &AbortParams {
                            move_id: state.id.clone(),
                            reason,
                        },
                    ),
                )
                .await;
                Settled::Aborted
            };
            match settled {
                Settled::Committed => Ending::Done(vec![
                    "The other device confirmed the move after a hiccup".into(),
                ]),
                Settled::Aborted => other,
            }
        }
    };
    match &ending {
        Ending::Done(notes) => {
            inner.sessions.forget_harness_session(&chat_id);
            state.phase = MovePhase::Done;
            state.detail = None;
            state.notes = notes.clone();
            if stopped.held {
                // Keep the chat's commands parked until this device's
                // registry also says the target hosts it: until then a
                // queued message would run here too.
                stopped.held = false;
                let service = service.clone();
                let chat_id = chat_id.clone();
                tokio::spawn(async move {
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(10 * 60);
                    while service.0.workspace.is_host(&chat_id)
                        && tokio::time::Instant::now() < deadline
                    {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    service.0.doc_host.release_move_hold(&chat_id);
                });
            }
            record_departure_later(&service, &chat_id, &state).await;
        }
        Ending::Cancelled | Ending::Failed(_) => {
            if stopped.was_active {
                resume_here(&service, &chat_id);
            }
            state.phase = if matches!(ending, Ending::Cancelled) {
                MovePhase::Cancelled
            } else {
                MovePhase::Failed
            };
            state.detail = None;
            if let Ending::Failed(message) = &ending {
                tracing::warn!(chat = %chat_id, move_id = %state.id, error = %message, "move failed");
                state.error = Some(message.clone());
            }
        }
    }
    if stopped.held {
        inner.doc_host.release_move_hold(&chat_id);
    }
    state.finished_at = Some(chrono::Utc::now().timestamp_millis());
    service.publish(&chat_id, &state);
    let _ = tokio::fs::remove_dir_all(&scratch).await;
}

enum MoveError {
    Cancelled,
    Failed(String),
}

impl From<EngineError> for MoveError {
    fn from(error: EngineError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<anyhow::Error> for MoveError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

fn check(control: &watch::Receiver<Control>) -> Result<(), MoveError> {
    if *control.borrow() == Control::Cancel {
        return Err(MoveError::Cancelled);
    }
    Ok(())
}

/// The workspace as planned on this device.
struct Workspace {
    chat_id: String,
    root: Option<PathBuf>,
    desc: WorkspaceDesc,
    repo: Option<super::git::RepoState>,
}

async fn drive(
    service: &MoveService,
    chat: &Chat,
    state: &mut ChatMove,
    mut control: watch::Receiver<Control>,
    scratch: &Path,
    stopped: &mut Stopped,
) -> Result<Vec<String>, MoveError> {
    let inner = &service.0;
    let to = state.to_device_id.clone();
    let harness = inner.doc_host.harness_for(&chat.id);
    let cwd = PathBuf::from(chat.cwd.clone().unwrap_or_default());
    let home = crate::repos::session_home_dir().ok();
    tokio::fs::create_dir_all(scratch).await.map_err(anyhow::Error::from)?;

    // ── Preparing ──────────────────────────────────────────────────────
    state.detail = Some(format!("Asking {}", state.to_device_name));
    service.publish(&chat.id, state);
    let workspace = plan_workspace(&chat.id, &cwd, home.as_deref()).await?;
    let events = inner.sessions.journal_events(&chat.id).unwrap_or_default();
    let harvested = {
        let root = workspace.root.clone();
        let home = home.clone();
        tokio::task::spawn_blocking(move || {
            harvest::candidates(&events, root.as_deref(), home.as_deref(), |path| {
                home.as_deref()
                    .is_some_and(|home| super::scope::is_secret_store(path, home))
            })
        })
        .await
        .map_err(anyhow::Error::from)?
    };
    let mut extras: Vec<(ExtraDesc, PathBuf)> = Vec::new();
    for candidate in &harvested {
        push_extra(service, &chat.id, &mut extras, &candidate.path, candidate.is_dir, home.as_deref());
    }
    let prepare = PrepareParams {
        move_id: state.id.clone(),
        chat_id: chat.id.clone(),
        from_device_id: inner.device_id.clone(),
        from_device_name: inner.device_name.clone(),
        chat_title: chat.title.clone(),
        harness,
        workspace: workspace.desc.clone(),
        extras: extras.iter().map(|(d, _)| d.clone()).collect(),
    };
    let prepared: PrepareReply = with_cancel(
        &mut control,
        tokio::time::timeout(
            PREPARE_TIMEOUT,
            service.call(&to, zeron_rpc::methods::MOVE_PREPARE, &prepare),
        ),
    )
    .await?
    .map_err(|_| MoveError::Failed(format!("{} took too long to get ready", state.to_device_name)))??;
    if !prepared.harness_installed {
        return Err(MoveError::Failed(format!(
            "{} isn't installed on {}",
            harness_name(harness),
            state.to_device_name
        )));
    }

    // The scout reads the conversation while the workspace copies.
    let processes = workspace
        .root
        .as_deref()
        .map(note::processes_in)
        .unwrap_or_default();
    let scout = {
        let registry = inner.registry.clone();
        let transcript = inner
            .doc_host
            .open(&chat.id)
            .ok()
            .and_then(|handle| handle.doc().read_entries().ok())
            .unwrap_or_default();
        let root = workspace.root.clone();
        let carried: Vec<PathBuf> = extras.iter().map(|(_, p)| p.clone()).collect();
        let processes = processes.clone();
        tokio::spawn(async move {
            scout::ask(
                &registry,
                scout::ScoutInput {
                    harness,
                    workspace_root: root.as_deref(),
                    carried: &carried,
                    processes: &processes,
                    transcript: &transcript,
                },
            )
            .await
        })
    };

    // ── Round 0: git objects the target lacks; then it builds its landing.
    let mut round = 0u32;
    let mut bundle_round = None;
    let head0 = workspace.repo.as_ref().and_then(|r| r.head.clone());
    if let (Some(root), Some(head)) = (workspace.root.as_deref(), head0.as_deref())
        && workspace.desc.kind == WorkspaceKind::Git
    {
        state.detail = Some("Sending commits".into());
        service.publish(&chat.id, state);
        let bundle = scratch.join("head.bundle");
        let outcome =
            super::git::create_bundle(root, head, &prepared.repo_tips, &bundle).await?;
        if matches!(outcome, super::git::BundleOutcome::Written { .. }) {
            let files = vec![(format!("{ROOT_GIT}/head.bundle"), bundle)];
            // The target needs these commits whatever the user presses.
            let (_keep, mut steady) = watch::channel(Control::Run);
            send_round(service, chat, state, &mut steady, round, files, false).await?;
            bundle_round = Some(round);
        }
        round += 1;
    }
    let staged: StageReply = with_cancel(
        &mut control,
        service.call(
            &to,
            zeron_rpc::methods::MOVE_STAGE,
            &StageParams {
                move_id: state.id.clone(),
                bundle_round,
                head: head0.clone(),
                branch: workspace.repo.as_ref().and_then(|r| r.branch.clone()),
            },
        ),
    )
    .await??;
    let mut notes: Vec<String> = staged.notes.clone();

    // ── Copying: everything, while the agent keeps working ─────────────
    let mut rounds: Vec<u32> = Vec::new();
    if *control.borrow() != Control::Now {
        state.phase = MovePhase::Copying;
        state.detail = Some("Copying the workspace".into());
        service.publish(&chat.id, state);
        let files = round_files(service, &workspace, &extras).await?;
        match send_round(service, chat, state, &mut control, round, files, true).await {
            Ok(()) => rounds.push(round),
            // "Move now" mid-copy: drop this round; the final one carries all.
            Err(MoveError::Failed(message)) if message == MOVE_NOW => {}
            Err(error) => return Err(error),
        }
        round += 1;
    }

    // Late scout advice still rides the final round.
    let advice = match tokio::time::timeout(Duration::from_secs(5), scout).await {
        Ok(Ok(Some(advice))) => advice,
        _ => scout::ScoutAdvice::default(),
    };
    for path in &advice.include {
        let path = match path.strip_prefix("~") {
            Ok(rest) => match home.as_deref() {
                Some(home) => home.join(rest),
                None => continue,
            },
            Err(_) => path.clone(),
        };
        if !path.is_absolute()
            || workspace
                .root
                .as_deref()
                .is_some_and(|root| path.starts_with(root) || root.starts_with(&path))
            || home.as_deref().is_some_and(|home| {
                path == home || super::scope::is_secret_store(&path, home)
            })
            || extras.iter().any(|(_, p)| path.starts_with(p))
        {
            continue;
        }
        let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
            continue;
        };
        if meta.file_type().is_symlink() || (meta.is_file() && meta.len() > harvest::MAX_EXTRA_BYTES)
        {
            continue;
        }
        if meta.is_dir() {
            let dir = path.clone();
            let size = tokio::task::spawn_blocking(move || {
                harvest::folder_bytes(&dir, harvest::MAX_EXTRA_BYTES)
            })
            .await
            .ok()
            .flatten();
            if size.is_none() {
                continue; // too big (or unreadable) to carry unasked
            }
        }
        push_extra(service, &chat.id, &mut extras, &path, meta.is_dir(), home.as_deref());
    }
    let late_extras: Vec<ExtraDesc> = extras
        .iter()
        .map(|(d, _)| d.clone())
        .filter(|d| !prepare.extras.iter().any(|p| p.root == d.root))
        .collect();
    if !late_extras.is_empty() {
        let added = tokio::time::timeout(
            Duration::from_secs(60),
            service.call::<PrepareReply>(
                &to,
                zeron_rpc::methods::MOVE_PREPARE,
                &PrepareParams {
                    extras: late_extras.clone(),
                    ..prepare.clone()
                },
            ),
        )
        .await;
        if !matches!(added, Ok(Ok(_))) {
            // The target never heard of them: they can't travel.
            extras.retain(|(d, _)| !late_extras.iter().any(|l| l.root == d.root));
        }
    }

    // ── Waiting for a safe point ───────────────────────────────────────
    check(&control)?;
    wait_for_safe_point(service, chat, state, &mut control).await?;

    // ── Stop the agent here ────────────────────────────────────────────
    inner.doc_host.hold_for_move(&chat.id);
    stopped.held = true;
    stopped.was_active = inner.sessions.turn_in_flight(&chat.id);
    stopped.pending_question = pending_question(service, &chat.id);
    if inner.sessions.watch_activity(&chat.id).is_some() {
        let _ = inner.sessions.interrupt(&chat.id).await;
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while inner.sessions.watch_activity(&chat.id).is_some() {
            if tokio::time::Instant::now() >= deadline {
                return Err(MoveError::Failed("The agent didn't stop".into()));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    inner.doc_host.settle_commands(&chat.id).await?;
    state.phase = MovePhase::Finishing;
    state.detail = Some("Copying the last changes".into());
    service.publish(&chat.id, state);

    // ── Finishing: final git state, session, last delta ────────────────
    let mut git_final = None;
    let mut extra_files: Vec<(String, PathBuf)> = Vec::new();
    if let Some(root) = workspace.root.as_deref()
        && workspace.desc.kind == WorkspaceKind::Git
    {
        let repo = super::git::capture(root).await?;
        let head = repo.as_ref().and_then(|r| r.head.clone());
        let branch = repo.as_ref().and_then(|r| r.branch.clone());
        let mut final_bundle = None;
        if let Some(head) = head.as_deref()
            && Some(head) != head0.as_deref()
        {
            let mut tips = prepared.repo_tips.clone();
            tips.extend(head0.clone());
            let bundle = scratch.join("final.bundle");
            if matches!(
                super::git::create_bundle(root, head, &tips, &bundle).await?,
                super::git::BundleOutcome::Written { .. }
            ) {
                extra_files.push((format!("{ROOT_GIT}/final.bundle"), bundle));
                final_bundle = Some(round);
            }
        }
        let patch = super::git::staged_patch(root).await.unwrap_or_default();
        git_final = Some(GitFinal {
            head,
            branch,
            bundle_round: final_bundle,
            staged_patch_b64: base64::engine::general_purpose::STANDARD.encode(patch),
        });
    }
    let session = export_session(service, chat, harness, scratch).await;
    if let Some(export) = &session {
        for file in &export.files {
            extra_files.push((format!("{ROOT_SESSION}/{}", file.rel), file.path.clone()));
        }
    }
    let mut login_included = false;
    if prepared.harness_signed_in == Some(false)
        && let Ok(Some(handoff)) = inner.accounts.export_login(harness).await
    {
        let path = scratch.join("login.json");
        if write_private(&path, &serde_json::to_vec(&handoff).map_err(anyhow::Error::from)?).is_ok() {
            extra_files.push((format!("{ROOT_LOGIN}/handoff.json"), path));
            login_included = true;
        }
    }
    let mut files = round_files(service, &workspace, &extras).await?;
    let workspace_rels: Vec<String> = files
        .iter()
        .filter_map(|(rel, _)| rel.strip_prefix("ws/").map(str::to_owned))
        .collect();
    files.extend(extra_files);
    // The agent is stopped: "move now" means nothing any more, and a cancel
    // is honoured right after this round.
    let (_keep, mut steady) = watch::channel(Control::Run);
    send_round(service, chat, state, &mut steady, round, files, true).await?;
    rounds.push(round);
    let (files_sent, bytes_sent) = sent_totals(service, &state.id);

    // ── Handover ───────────────────────────────────────────────────────
    check(&control)?;
    state.phase = MovePhase::Handover;
    state.detail = Some(format!("{} is taking over", state.to_device_name));
    service.publish(&chat.id, state);
    let mut note_lines: Vec<String> = Vec::new();
    note_lines.extend(processes.iter().map(|p| format!("Left running on {}: {p}", inner.device_name)));
    note_lines.extend(advice.notes.iter().cloned());
    let commit = CommitParams {
        move_id: state.id.clone(),
        chat_id: chat.id.clone(),
        rounds,
        git: git_final.clone(),
        session,
        processed_commands: inner.doc_host.processed_commands_for(&chat.id)?,
        was_active: stopped.was_active,
        pending_question: stopped.pending_question.clone(),
        note_lines,
        started_at: state.started_at,
        files_sent,
        bytes_sent,
        login_included,
        workspace_rels,
    };
    // Everything this device wrote (the stopped turn's last words) must be in
    // the room before the target writes after it.
    inner
        .doc_host
        .await_synced(&chat.id, Duration::from_secs(60))
        .await?;
    stopped.commit_sent = true;
    let committed = tokio::time::timeout(
        COMMIT_TIMEOUT,
        service.call::<CommitReply>(&to, zeron_rpc::methods::MOVE_COMMIT, &commit),
    )
    .await;
    // What left, for a later return (recorded once the move is settled).
    stash_departure(&state.id, &workspace, git_final.as_ref());
    match committed {
        Ok(Ok(reply)) => {
            notes.extend(reply.notes);
            Ok(notes)
        }
        // Unknown outcome: `run` asks the target before anything resumes.
        Ok(Err(error)) => Err(MoveError::Failed(error.to_string())),
        Err(_) => Err(MoveError::Failed(format!(
            "{} didn't confirm taking the chat",
            state.to_device_name
        ))),
    }
}

/// Where the chat's workspace is and what kind it is.
async fn plan_workspace(
    chat_id: &str,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<Workspace, MoveError> {
    let root = super::scope::workspace_root(cwd);
    let broad = home.is_some_and(|home| root == home) || root.parent().is_none();
    let repo = if broad {
        None
    } else {
        super::git::capture(&root).await?
    };
    let kind = if broad {
        WorkspaceKind::None
    } else if repo.is_some() {
        WorkspaceKind::Git
    } else {
        WorkspaceKind::Folder
    };
    let desc = WorkspaceDesc {
        kind,
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".into()),
        home_rel: home.and_then(|home| home_rel(&root, home)),
        cwd_rel: cwd
            .strip_prefix(&root)
            .map(slash_path)
            .unwrap_or_default(),
        source_root: root.display().to_string(),
        repo: repo.as_ref().map(|r| RepoDesc {
            root_commits: r.root_commits.clone(),
            remotes: r
                .remotes
                .iter()
                .map(|remote| (remote.name.clone(), remote.url.clone()))
                .collect(),
            head: r.head.clone(),
            branch: r.branch.clone(),
        }),
    };
    Ok(Workspace {
        chat_id: chat_id.to_owned(),
        root: (!broad).then_some(root),
        desc,
        repo,
    })
}

fn push_extra(
    service: &MoveService,
    chat_id: &str,
    extras: &mut Vec<(ExtraDesc, PathBuf)>,
    path: &Path,
    is_dir: bool,
    home: Option<&Path>,
) {
    if extras.iter().any(|(_, p)| p == path) {
        return;
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    let origin = service.0.lineage.get(chat_id).and_then(|lineage| {
        lineage
            .extras
            .iter()
            .find(|(_, local)| Path::new(local.as_str()) == path)
            .map(|(origin, _)| origin.clone())
    });
    let desc = ExtraDesc {
        root: extra_root(extras.len()),
        name,
        source_path: path.display().to_string(),
        parent_home_rel: home.and_then(|home| path.parent().and_then(|parent| home_rel(parent, home))),
        is_dir,
        origin: origin.or_else(|| Some(extra_origin(&service.0.device_id, &path.display().to_string()))),
    };
    extras.push((desc, path.to_path_buf()));
}

/// Every file a round carries: the workspace tree, extras and uploads the
/// transcript shows.
async fn round_files(
    service: &MoveService,
    workspace: &Workspace,
    extras: &[(ExtraDesc, PathBuf)],
) -> Result<Vec<(String, PathBuf)>, MoveError> {
    let root = workspace.root.clone();
    let extras: Vec<(ExtraDesc, PathBuf)> = extras.to_vec();
    let uploads_dir = service.0.uploads.dir().to_path_buf();
    let images = transcript_uploads(service, &workspace.chat_id, &uploads_dir);
    let files = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(String, PathBuf)>> {
        let mut files = Vec::new();
        let rules = super::scope::ScopeRules::default();
        if let Some(root) = root {
            let scope = super::scope::walk(&root, &rules)?;
            anyhow::ensure!(
                !scope.truncated,
                "{} is too large to move ({} or more files, {} of them not build output)",
                root.display(),
                scope.files.len(),
                size_label(scope.total_bytes)
            );
            for file in scope.files {
                if matches!(file.kind, super::scope::ScopeKind::File | super::scope::ScopeKind::Symlink) {
                    files.push((format!("{ROOT_WORKSPACE}/{}", file.rel), file.abs));
                }
            }
        }
        for (desc, path) in &extras {
            if std::fs::symlink_metadata(path).is_err() {
                continue; // gone since it was planned: nothing to carry
            }
            if desc.is_dir {
                let scope = match super::scope::walk(path, &rules) {
                    Ok(scope) if !scope.truncated => scope,
                    _ => continue,
                };
                for file in scope.files {
                    if matches!(file.kind, super::scope::ScopeKind::File | super::scope::ScopeKind::Symlink) {
                        files.push((format!("{}/{}/{}", desc.root, desc.name, file.rel), file.abs));
                    }
                }
            } else {
                files.push((format!("{}/{}", desc.root, desc.name), path.clone()));
            }
        }
        for image in images {
            if let Some(name) = image.file_name() {
                files.push((
                    format!("{ROOT_UPLOADS}/{}", name.to_string_lossy()),
                    image.clone(),
                ));
            }
        }
        Ok(files)
    })
    .await
    .map_err(anyhow::Error::from)??;
    Ok(files)
}

/// Image attachments the transcript shows that live in this device's uploads
/// folder: they move so old messages still render on the new host.
fn transcript_uploads(service: &MoveService, chat_id: &str, uploads_dir: &Path) -> Vec<PathBuf> {
    let Ok(handle) = service.0.doc_host.open(chat_id) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = handle
        .doc()
        .read_entries()
        .unwrap_or_default()
        .iter()
        .flat_map(|entry| entry.parts.iter())
        .filter_map(|part| match part {
            MessagePart::Image { path, .. } => Some(PathBuf::from(path)),
            _ => None,
        })
        // An image from an earlier move still names the device it was first
        // uploaded on; here it lives under the same name in our uploads.
        .filter_map(|path| {
            let local = uploads_dir.join(path.file_name()?);
            local.is_file().then_some(local)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// One sync round to the target; waits for it to finish, publishing
/// progress. `tolerant` = files changing while sent are skipped (the agent
/// may still be working).
async fn send_round(
    service: &MoveService,
    chat: &Chat,
    state: &mut ChatMove,
    control: &mut watch::Receiver<Control>,
    round: u32,
    files: Vec<(String, PathBuf)>,
    tolerant: bool,
) -> Result<(), MoveError> {
    let inner = &service.0;
    let reply = inner
        .transfers
        .send_sync(zeron_transfer::SyncSend {
            to: state.to_device_id.clone(),
            ticket: ticket(&state.id, round),
            files: files
                .into_iter()
                .map(|(rel, source)| zeron_transfer::SyncSource { rel, source })
                .collect(),
            tolerant,
            policy: zeron_transfer::TransportPolicy::Auto,
        })
        .await?;
    let transfer_id = reply.transfer_id;
    remember_transfer(&state.id, &transfer_id);
    let mut rows = inner.transfers.watch();
    let mut last_publish = tokio::time::Instant::now() - PUBLISH_EVERY;
    loop {
        let row = rows
            .borrow_and_update()
            .iter()
            .find(|r| r.id == transfer_id)
            .cloned();
        if let Some(row) = row {
            if last_publish.elapsed() >= PUBLISH_EVERY || row.state.is_terminal() {
                state.bytes_total = row.total_bytes;
                state.bytes_done = row.done_bytes;
                state.files_total = row.file_count;
                service.publish(&chat.id, state);
                last_publish = tokio::time::Instant::now();
            }
            match row.state {
                FileTransferState::Completed => return Ok(()),
                FileTransferState::Failed | FileTransferState::Declined | FileTransferState::Cancelled => {
                    return Err(MoveError::Failed(
                        row.error
                            .unwrap_or_else(|| "The copy to the other device failed".into()),
                    ));
                }
                _ => {}
            }
        }
        tokio::select! {
            changed = rows.changed() => {
                if changed.is_err() {
                    return Err(MoveError::Failed("File transfer stopped".into()));
                }
            }
            changed = control.changed() => {
                if changed.is_err() {
                    continue;
                }
                let signal = *control.borrow();
                if signal == Control::Cancel || signal == Control::Now {
                    let _ = inner.transfers.cancel(&transfer_id, true);
                    return Err(if signal == Control::Cancel {
                        MoveError::Cancelled
                    } else {
                        MoveError::Failed(MOVE_NOW.into())
                    });
                }
            }
        }
    }
}

/// Internal marker: the user chose "Move now" during the copy.
const MOVE_NOW: &str = "\u{0}move-now";

fn remember_transfer(move_id: &str, transfer_id: &str) {
    TRANSFERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(move_id.to_owned())
        .or_default()
        .push(transfer_id.to_owned());
}

static TRANSFERS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>> =
    std::sync::LazyLock::new(Default::default);

/// Files and bytes that actually travelled (unchanged files excluded).
fn sent_totals(service: &MoveService, move_id: &str) -> (u64, u64) {
    let ids = TRANSFERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(move_id)
        .unwrap_or_default();
    let rows = service.0.transfers.list();
    rows.iter()
        .filter(|r| ids.contains(&r.id) && r.state == FileTransferState::Completed)
        .fold((0, 0), |(files, bytes), r| {
            (files + r.file_count, bytes + r.total_bytes)
        })
}

/// Wait until the run is between steps (or the user says "now").
async fn wait_for_safe_point(
    service: &MoveService,
    chat: &Chat,
    state: &mut ChatMove,
    control: &mut watch::Receiver<Control>,
) -> Result<(), MoveError> {
    let inner = &service.0;
    let Some(mut activity) = inner.sessions.watch_activity(&chat.id) else {
        return Ok(()); // no live run
    };
    let waiting_since = tokio::time::Instant::now();
    let mut announced = false;
    loop {
        check(control)?;
        if *control.borrow() == Control::Now || activity.borrow().at_safe_point() {
            return Ok(());
        }
        if !announced {
            state.phase = MovePhase::Waiting;
            announced = true;
        }
        let running = running_tool(service, &chat.id);
        let elapsed = waiting_since.elapsed().as_secs();
        state.detail = Some(match running {
            Some(tool) if elapsed >= 60 => format!("Waiting for {tool} ({} min)", elapsed / 60),
            Some(tool) => format!("Waiting for {tool}"),
            None => "Waiting for the agent's current step".into(),
        });
        service.publish(&chat.id, state);
        tokio::select! {
            changed = activity.changed() => {
                if changed.is_err() {
                    return Ok(()); // the run ended
                }
            }
            _ = control.changed() => {}
            _ = tokio::time::sleep(Duration::from_secs(15)) => {}
        }
    }
}

/// The tool the agent is running right now, as a short label.
fn running_tool(service: &MoveService, chat_id: &str) -> Option<String> {
    let handle = service.0.doc_host.open(chat_id).ok()?;
    let entries = handle.doc().read_entries().ok()?;
    let entry = entries.last()?;
    entry.parts.iter().rev().find_map(|part| match part {
        MessagePart::Tool {
            call,
            resolved: false,
            ..
        } => Some(tool_label(call)),
        _ => None,
    })
}

fn tool_label(call: &zeron_proto::ToolCall) -> String {
    use zeron_proto::ToolCall;
    let clip = |text: &str| -> String {
        let line = text.lines().next().unwrap_or_default();
        if line.chars().count() > 48 {
            format!("{}…", line.chars().take(47).collect::<String>())
        } else {
            line.to_owned()
        }
    };
    match call {
        ToolCall::Exec { command } => clip(command),
        ToolCall::ReadFile { path } => format!("reading {}", clip(path)),
        ToolCall::WriteFile { path, .. } | ToolCall::EditFile { path, .. } => {
            format!("editing {}", clip(path))
        }
        ToolCall::Unknown { name, .. } if name.starts_with("Agent") => "a subagent".into(),
        ToolCall::Unknown { name, .. } => clip(name),
        ToolCall::Mcp { tool, .. } => clip(tool),
        _ => "the current tool".into(),
    }
}

/// The question the agent is waiting on, if any (asked again after the move).
fn pending_question(service: &MoveService, chat_id: &str) -> Option<String> {
    let handle = service.0.doc_host.open(chat_id).ok()?;
    let entries = handle.doc().read_entries().ok()?;
    entries.iter().rev().take(3).find_map(|entry| {
        entry.parts.iter().find_map(|part| match part {
            MessagePart::Input {
                questions,
                resolved: false,
                ..
            } => questions.first().map(|q| q.question.clone()),
            _ => None,
        })
    })
}

/// The harness's own session files, frozen into `scratch` (the agent is
/// stopped, but a CLI may still flush on exit).
async fn export_session(
    service: &MoveService,
    chat: &Chat,
    harness: HarnessId,
    scratch: &Path,
) -> Option<zeron_harness::portable::SessionExport> {
    if !zeron_harness::portable::portable(harness) {
        return None;
    }
    let (session_id, session_cwd) = service.0.workspace.chat_harness_session(&chat.id)?;
    if session_id.is_empty() {
        return None;
    }
    let cwd = session_cwd.or_else(|| chat.cwd.clone())?;
    let roots = service.0.port_roots.clone();
    let dest = scratch.join("session");
    tokio::task::spawn_blocking(move || {
        let export = zeron_harness::portable::export_session(harness, &roots, &session_id, &cwd)
            .map_err(|error| tracing::warn!(%error, "could not export the agent's session"))
            .ok()??;
        zeron_harness::portable::snapshot_export(&export, &dest)
            .map_err(|error| tracing::warn!(%error, "could not snapshot the agent's session"))
            .ok()
    })
    .await
    .ok()?
}

/// After a cancelled or failed move that had stopped a running turn: carry
/// on here, telling the agent what happened.
fn resume_here(service: &MoveService, chat_id: &str) {
    let inner = &service.0;
    if let Ok(handle) = inner.doc_host.open(chat_id) {
        let _ = handle.doc().set_move_note(Some(&zeron_doc::MoveNote {
            move_id: String::new(),
            text: "[Zeron] You were paused to move this session to another computer, but the move \
                   was cancelled. Nothing changed; continue where you left off."
                .into(),
            replay_history: false,
        }));
    }
    let Some(request) = inner
        .doc_host
        .request_from_chat_row(chat_id, "Continue where you left off.")
    else {
        return;
    };
    inner.doc_host.release_move_hold(chat_id);
    if let Err(error) = inner.doc_host.queue_command(
        chat_id,
        SessionCommandPayload::Run {
            request,
            message_id: uuid::Uuid::new_v4().to_string(),
        },
    ) {
        tracing::warn!(chat = %chat_id, %error, "could not resume a chat after a cancelled move");
    }
}

/// What left with a move whose outcome is still being settled: recorded
/// as the chat's departure once the target confirms.
struct PendingDeparture {
    root: Option<PathBuf>,
    head: Option<String>,
}

static DEPARTURES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, PendingDeparture>>,
> = std::sync::LazyLock::new(Default::default);

fn stash_departure(move_id: &str, workspace: &Workspace, git: Option<&GitFinal>) {
    DEPARTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            move_id.to_owned(),
            PendingDeparture {
                root: workspace.root.clone(),
                head: git.and_then(|g| g.head.clone()),
            },
        );
}

/// What the workspace looked like when the chat left (for a later return):
/// content hashes of every file, unless the listing was partial — then no
/// file counts as known, and a return deletes nothing here.
async fn record_departure_later(service: &MoveService, chat_id: &str, state: &ChatMove) {
    let Some(pending) = DEPARTURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&state.id)
    else {
        return;
    };
    let mut files = std::collections::HashMap::new();
    if let Some(root) = pending.root.clone() {
        let rules = super::scope::ScopeRules::default();
        let scope = tokio::task::spawn_blocking(move || super::scope::walk(&root, &rules))
            .await
            .ok()
            .and_then(Result::ok)
            .filter(|scope| !scope.truncated);
        if let Some(scope) = scope {
            let (rels, paths): (Vec<String>, Vec<PathBuf>) = scope
                .files
                .into_iter()
                .filter(|f| matches!(f.kind, super::scope::ScopeKind::File))
                .map(|f| (f.rel, f.abs))
                .unzip();
            if let Ok(hashes) = service.0.transfers.hash_files(paths).await {
                for (rel, hash) in rels.into_iter().zip(hashes) {
                    if let Some(hash) = hash {
                        files.insert(rel, hash);
                    }
                }
            }
        }
    }
    let root = pending.root.as_ref().map(|r| r.display().to_string());
    service.0.lineage.update(chat_id, |lineage| {
        if root.is_some() {
            lineage.workspace = root.clone();
        }
        lineage.departure = Some(super::lineage::Departure {
            move_id: state.id.clone(),
            to_device_id: state.to_device_id.clone(),
            at: chrono::Utc::now().timestamp_millis(),
            head: pending.head.clone(),
            files,
        });
    });
}

enum Settled {
    Committed,
    Aborted,
}

/// After a failure or cancel: make sure the target won't take the chat over
/// (or learn that it already did). Keeps asking while the target is mid-
/// commit or unreachable — the chat stays parked rather than risk running on
/// both devices — and gives up only after a long silence.
async fn settle_with_target(
    service: &MoveService,
    chat_id: &str,
    state: &ChatMove,
    reason: Option<String>,
) -> Settled {
    const GIVE_UP: Duration = Duration::from_secs(30 * 60);
    let deadline = tokio::time::Instant::now() + GIVE_UP;
    let params = AbortParams {
        move_id: state.id.clone(),
        reason,
    };
    let mut announced = false;
    loop {
        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            service.call::<AbortReply>(&state.to_device_id, zeron_rpc::methods::MOVE_ABORT, &params),
        )
        .await;
        match reply {
            Ok(Ok(reply)) => match reply.outcome {
                AbortOutcome::Committed => return Settled::Committed,
                AbortOutcome::Aborted => {
                    // It may have committed just before forgetting: the row
                    // is the last word.
                    let landed = service
                        .0
                        .workspace
                        .chat(chat_id)
                        .ok()
                        .flatten()
                        .is_some_and(|row| row.device_id == state.to_device_id);
                    return if landed { Settled::Committed } else { Settled::Aborted };
                }
                AbortOutcome::Applying => {}
            },
            _ => {
                if service
                    .0
                    .workspace
                    .chat(chat_id)
                    .ok()
                    .flatten()
                    .is_some_and(|row| row.device_id == state.to_device_id)
                {
                    return Settled::Committed;
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(chat = %chat_id, move_id = %state.id,
                "move: target unreachable for 30 minutes; keeping the chat here");
            return Settled::Aborted;
        }
        if !announced {
            announced = true;
            let mut waiting = state.clone();
            waiting.detail = Some(format!("Checking with {}", state.to_device_name));
            service.publish(chat_id, &waiting);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Await `future` unless the user cancels first.
async fn with_cancel<T>(
    control: &mut watch::Receiver<Control>,
    future: impl std::future::Future<Output = T>,
) -> Result<T, MoveError> {
    check(control)?;
    tokio::pin!(future);
    loop {
        tokio::select! {
            out = &mut future => return Ok(out),
            changed = control.changed() => {
                if changed.is_ok() {
                    check(control)?;
                }
            }
        }
    }
}

fn size_label(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{} MB", b >> 20),
        b => format!("{} KB", b >> 10),
    }
}

fn home_rel(path: &Path, home: &Path) -> Option<String> {
    path.strip_prefix(home).ok().map(slash_path)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(path)?.write_all(bytes)
}
