//! The target side of a move: this engine takes a chat over.
//!
//! It decides where everything lands on its own disk (`MovePrepare`),
//! builds the workspace landing once commits have arrived (`MoveStage`),
//! accepts the ticketed sync rounds into private staging folders, and on
//! `MoveCommit` applies them: git refs and index, the working tree (with
//! deletions), extras, uploads, the agent's session and login, then writes
//! the transcript seam and the chat's new placement — the moment it becomes
//! the host.
//!
//! Nothing outside staging is touched before the commit, except a landing
//! this move created itself (a fresh worktree, clone or folder), which an
//! abort removes again. A file that already exists here with other content
//! is backed up before it is replaced, unless it is exactly what this
//! device had when the chat last left it (or the landing is brand new).
//!
//! Long work (cloning, git, applying) never holds the move table's lock, and
//! a commit records where it is: an abort that arrives while it applies
//! stops it before the handover, and one that arrives after learns the move
//! happened.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine as _;
use zeron_doc::{MessagePart, MessageRole, MessageStatus, MoveSeam, SessionCommandPayload};

use crate::EngineError;

use super::lineage::{ChatLineage, extra_origin};
use super::note;
use super::protocol::*;
use super::service::{MoveService, ticket};

/// Highest round number a move may use (bundle, copy, final + slack).
const MAX_ROUNDS: u32 = 4;
/// A prepared move whose source never committed or aborted is dropped (and
/// what it created removed) after this long.
const ABANDONED_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// How the workspace lands here.
#[derive(Debug, Clone)]
enum WsPlan {
    /// The chat's work already lives here (it was here before).
    Returning { path: PathBuf, git: bool },
    /// A new worktree of a checkout of the same repository.
    Worktree { repo: PathBuf },
    /// A clone made for this move.
    Clone { path: PathBuf },
    /// A new folder (or a repository rebuilt from the bundle).
    Fresh { path: PathBuf, git: bool },
}

#[derive(Debug, Clone)]
struct ExtraLanding {
    desc: ExtraDesc,
    /// The folder the item lands in.
    parent: PathBuf,
}

#[derive(Clone)]
pub(super) struct Incoming {
    move_id: String,
    chat_id: String,
    from_device_id: String,
    from_device_name: String,
    staging: PathBuf,
    workspace: WorkspaceDesc,
    plan: Option<WsPlan>,
    /// The workspace root here once staged.
    landing: Option<PathBuf>,
    /// The branch the landing is on after the stage (possibly renamed).
    branch: Option<String>,
    /// HEAD of the landing after the stage.
    staged_head: Option<String>,
    /// A folder this move created (worktree, clone, new folder): removed if
    /// the move doesn't happen, and freely synced (deletions included).
    created: Option<PathBuf>,
    extras: Vec<ExtraLanding>,
    notes: Vec<String>,
    since: Instant,
}

impl Incoming {
    fn round_dir(&self, round: u32) -> PathBuf {
        self.staging.join(format!("r{round}"))
    }

    fn returning(&self) -> bool {
        matches!(self.plan, Some(WsPlan::Returning { .. }))
    }

    fn created_this_move(&self) -> bool {
        self.created.is_some() && self.created == self.landing
    }

    fn root_landing(&self, root: &str, uploads: &Path) -> Option<PathBuf> {
        if root == ROOT_WORKSPACE {
            return self.landing.clone();
        }
        if root == ROOT_UPLOADS {
            return Some(uploads.to_path_buf());
        }
        self.extras
            .iter()
            .find(|e| e.desc.root == root)
            .map(|e| e.parent.clone())
    }

    /// Where round `round` compares files of `root` against, in order: the
    /// staging of earlier rounds (newest first), then the landing itself.
    /// The transfer reports matches by index into exactly this list.
    fn basis_dirs(&self, round: u32, root: &str, uploads: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = (0..round)
            .rev()
            .map(|r| self.round_dir(r).join(root))
            .collect();
        dirs.extend(self.root_landing(root, uploads));
        dirs
    }

    fn roots(&self) -> Vec<String> {
        let mut roots = vec![ROOT_WORKSPACE.to_owned(), ROOT_UPLOADS.to_owned()];
        roots.extend(self.extras.iter().map(|e| e.desc.root.clone()));
        roots
    }
}

/// Where a commit is, for an abort that races it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommitState {
    Applying { abort_requested: bool },
    Committed,
    Failed,
}

impl MoveService {
    fn incoming_table(&self) -> std::sync::MutexGuard<'_, HashMap<String, Incoming>> {
        self.0
            .incoming
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn commit_table(&self) -> std::sync::MutexGuard<'_, HashMap<String, CommitState>> {
        self.0
            .commits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// (Re)register the tickets of every round with up-to-date basis folders.
    fn register_tickets(&self, incoming: &Incoming) {
        let uploads = self.0.uploads.dir().to_path_buf();
        for round in 0..=MAX_ROUNDS {
            let basis: HashMap<String, Vec<PathBuf>> = incoming
                .roots()
                .into_iter()
                .map(|root| {
                    let dirs = incoming.basis_dirs(round, &root, &uploads);
                    (root, dirs)
                })
                .collect();
            self.0.tickets.insert(
                ticket(&incoming.move_id, round),
                incoming.from_device_id.clone(),
                zeron_transfer::SyncGrant {
                    dest_root: incoming.round_dir(round),
                    basis,
                },
            );
        }
    }

    /// Moves whose source went quiet for hours: forget them and remove what
    /// they created here.
    async fn sweep_abandoned(&self) {
        let stale: Vec<Incoming> = {
            let mut table = self.incoming_table();
            let ids: Vec<String> = table
                .iter()
                .filter(|(_, m)| m.since.elapsed() > ABANDONED_AFTER)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| table.remove(id)).collect()
        };
        for incoming in stale {
            tracing::info!(move_id = %incoming.move_id, "move abandoned by its source; cleaning up");
            self.forget(&incoming).await;
        }
    }

    /// `MovePrepare`.
    pub async fn prepare(&self, params: PrepareParams) -> Result<PrepareReply, EngineError> {
        self.sweep_abandoned().await;
        let inner = &self.0;
        let probe = self
            .probe(ProbeParams {
                harness: params.harness,
            })
            .await;
        let home = crate::repos::session_home_dir().ok();
        let lineage = inner.lineage.get(&params.chat_id).unwrap_or_default();

        // A second call adds extras the scout found late.
        let known = self.incoming_table().get(&params.move_id).cloned();
        if let Some(mut incoming) = known {
            for desc in params.extras {
                if incoming.extras.iter().any(|e| e.desc.root == desc.root) || !safe_item(&desc) {
                    continue;
                }
                let parent = self.extra_parent(&desc, &lineage, home.as_deref(), &params.chat_title);
                incoming.extras.push(ExtraLanding { desc, parent });
            }
            self.register_tickets(&incoming);
            let reply = PrepareReply {
                harness_installed: probe.harness_installed,
                harness_signed_in: probe.harness_signed_in,
                repo_tips: Vec::new(),
                returning: incoming.returning(),
                landing: incoming.landing.as_ref().map(|p| p.display().to_string()),
            };
            if let Some(slot) = self.incoming_table().get_mut(&params.move_id) {
                slot.extras = incoming.extras;
            }
            return Ok(reply);
        }
        if !probe.harness_installed {
            return Ok(PrepareReply {
                harness_installed: false,
                harness_signed_in: None,
                repo_tips: Vec::new(),
                returning: false,
                landing: None,
            });
        }
        let home = home.ok_or_else(|| EngineError::Other("This device has no home folder".into()))?;
        let (plan, tips, mut notes, created) = self.plan_workspace(&params, &lineage, &home).await?;
        let extras = params
            .extras
            .iter()
            .filter(|desc| safe_item(desc))
            .cloned()
            .map(|desc| {
                let parent = self.extra_parent(&desc, &lineage, Some(&home), &params.chat_title);
                ExtraLanding { desc, parent }
            })
            .collect();
        let staging = inner.dir.join("in").join(&params.move_id);
        tokio::fs::create_dir_all(&staging)
            .await
            .map_err(|e| EngineError::Other(e.to_string()))?;
        let landing = match &plan {
            Some(WsPlan::Returning { path, .. }) => Some(path.clone()),
            _ => None,
        };
        if let Some(path) = &landing {
            notes.push(format!("Landed back in {}", path.display()));
        }
        let incoming = Incoming {
            move_id: params.move_id.clone(),
            chat_id: params.chat_id.clone(),
            from_device_id: params.from_device_id.clone(),
            from_device_name: params.from_device_name.clone(),
            staging,
            workspace: params.workspace.clone(),
            plan,
            landing: landing.clone(),
            branch: None,
            staged_head: None,
            created,
            extras,
            notes,
            since: Instant::now(),
        };
        self.register_tickets(&incoming);
        let returning = incoming.returning();
        self.incoming_table().insert(params.move_id.clone(), incoming);
        Ok(PrepareReply {
            harness_installed: true,
            harness_signed_in: probe.harness_signed_in,
            repo_tips: tips,
            returning,
            landing: landing.map(|p| p.display().to_string()),
        })
    }

    /// Decide the workspace landing (cloning first if that's the way).
    /// Returns the plan, the commits the target already has, notes, and a
    /// folder created on the way (a clone).
    async fn plan_workspace(
        &self,
        params: &PrepareParams,
        lineage: &ChatLineage,
        home: &Path,
    ) -> Result<(Option<WsPlan>, Vec<String>, Vec<String>, Option<PathBuf>), EngineError> {
        let desc = &params.workspace;
        let git = desc.kind == WorkspaceKind::Git;
        if desc.kind == WorkspaceKind::None {
            return Ok((None, Vec::new(), Vec::new(), None));
        }
        // The chat was here before: its work is where it left it.
        if let Some(path) = lineage.workspace.as_deref().map(PathBuf::from)
            && path.is_dir()
        {
            let tips = if git {
                super::git::tips(&path).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            return Ok((Some(WsPlan::Returning { path, git }), tips, Vec::new(), None));
        }
        let mut notes = Vec::new();
        if let Some(repo) = desc.repo.as_ref() {
            let candidates = self.known_checkouts();
            let want = super::git::RepoState {
                root: PathBuf::new(),
                head: repo.head.clone(),
                branch: repo.branch.clone(),
                upstream: None,
                remotes: repo
                    .remotes
                    .iter()
                    .map(|(name, url)| super::git::Remote {
                        name: name.clone(),
                        url: url.clone(),
                        normalized: super::git::normalize_remote(url),
                    })
                    .collect(),
                root_commits: repo.root_commits.clone(),
                is_linked_worktree: false,
                common_dir: PathBuf::new(),
            };
            if let Some(found) = super::git::find_repo(&candidates, &want).await.map_err(other)? {
                return Ok((
                    Some(WsPlan::Worktree { repo: found.path }),
                    found.tips,
                    notes,
                    None,
                ));
            }
            let dest = free_landing(home, desc);
            if let Some((_, url)) = repo.remotes.first() {
                match super::git::clone_from_remote(url, &dest).await {
                    Ok(()) => {
                        let tips = super::git::tips(&dest).await.unwrap_or_default();
                        return Ok((
                            Some(WsPlan::Clone { path: dest.clone() }),
                            tips,
                            notes,
                            Some(dest),
                        ));
                    }
                    Err(error) => {
                        tracing::info!(%error, "move: clone failed; rebuilding from the bundle");
                        notes.push(
                            "Couldn't clone the repository here; rebuilt it from the other device's history"
                                .into(),
                        );
                        let _ = tokio::fs::remove_dir_all(&dest).await;
                    }
                }
            }
            return Ok((Some(WsPlan::Fresh { path: dest, git: true }), Vec::new(), notes, None));
        }
        Ok((
            Some(WsPlan::Fresh {
                path: free_landing(home, desc),
                git: false,
            }),
            Vec::new(),
            notes,
            None,
        ))
    }

    /// Existing checkouts on this device a repository match can come from.
    fn known_checkouts(&self) -> Vec<PathBuf> {
        let me = &self.0.device_id;
        let mut paths: Vec<PathBuf> = self
            .0
            .workspace
            .read_spaces()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| &s.device_id == me)
            .map(|s| PathBuf::from(s.path))
            .filter(|p| p.is_dir())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    /// The folder an extra lands in. Peers only ever suggest; this device
    /// keeps every extra inside its home, out of secret stores and away from
    /// places that run things (shell startup files, autostart folders).
    fn extra_parent(
        &self,
        desc: &ExtraDesc,
        lineage: &ChatLineage,
        home: Option<&Path>,
        chat_title: &Option<String>,
    ) -> PathBuf {
        let fallback = || {
            let title = chat_title
                .as_deref()
                .map(safe_name)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Moved chat".into());
            home.unwrap_or_else(|| Path::new("/"))
                .join("Zeron Transfers")
                .join("Moves")
                .join(title)
        };
        let Some(home) = home else {
            return fallback();
        };
        let acceptable = |parent: &Path| -> bool {
            let item = parent.join(&desc.name);
            plain_path(&item)
                && item.starts_with(home)
                && item != home
                && !super::scope::is_secret_store(&item, home)
                && !runs_things(&item, home)
        };
        // Back where it came from.
        if let Some(origin) = desc.origin.as_deref() {
            if let Some((device, path)) = origin.split_once(':')
                && device == self.0.device_id
                && let Some(parent) = Path::new(path).parent()
                && acceptable(parent)
            {
                return parent.to_path_buf();
            }
            if let Some(local) = lineage.extras.get(origin)
                && let Some(parent) = Path::new(local).parent()
                && acceptable(parent)
            {
                return parent.to_path_buf();
            }
        }
        if let Some(rel) = desc.parent_home_rel.as_deref() {
            let parent = join_rel(home, rel);
            if acceptable(&parent) && !parent.join(&desc.name).exists() {
                return parent;
            }
        }
        fallback()
    }

    /// `MoveStage`: commits have landed (or there were none to send): build
    /// the workspace landing so the copy compares against real content.
    pub async fn stage(&self, params: StageParams) -> Result<StageReply, EngineError> {
        let mut incoming = self
            .incoming_table()
            .get(&params.move_id)
            .cloned()
            .ok_or_else(|| EngineError::Other("This move is no longer known here".into()))?;
        let bundle = params
            .bundle_round
            .map(|round| incoming.round_dir(round).join(ROOT_GIT).join("head.bundle"));
        let device = incoming.from_device_name.clone();
        let mut notes = Vec::new();
        match incoming.plan.clone() {
            None => {}
            Some(WsPlan::Returning { path, git }) => {
                if git && let Some(bundle) = &bundle {
                    super::git::fetch_bundle(&path, bundle, &incoming.move_id)
                        .await
                        .map_err(other)?;
                }
                incoming.landing = Some(path);
            }
            Some(WsPlan::Worktree { repo }) => {
                if let Some(bundle) = &bundle {
                    super::git::fetch_bundle(&repo, bundle, &incoming.move_id)
                        .await
                        .map_err(other)?;
                }
                let head = params
                    .head
                    .clone()
                    .ok_or_else(|| EngineError::Other("The repository has no commits".into()))?;
                let dest = self.worktree_dest(&repo, &incoming.workspace, params.branch.as_deref());
                // Recorded before it exists: a failure halfway still cleans up.
                incoming.created = Some(dest.clone());
                self.save_incoming(&incoming);
                let landing = super::git::land_new_worktree(
                    &repo,
                    &dest,
                    &head,
                    params.branch.as_deref(),
                    &device,
                )
                .await
                .map_err(other)?;
                notes.extend(landing.notes);
                incoming.branch = landing.branch;
                incoming.staged_head = Some(head);
                incoming.created = Some(landing.path.clone());
                incoming.landing = Some(landing.path);
            }
            Some(WsPlan::Clone { path }) => {
                if let Some(bundle) = &bundle {
                    super::git::fetch_bundle(&path, bundle, &incoming.move_id)
                        .await
                        .map_err(other)?;
                }
                if let Some(head) = params.head.as_deref() {
                    let landing = super::git::update_existing_checkout(
                        &path,
                        head,
                        params.branch.as_deref(),
                        None,
                        &device,
                    )
                    .await
                    .map_err(other)?;
                    notes.extend(landing.notes);
                    incoming.branch = landing.branch;
                    incoming.staged_head = Some(head.to_owned());
                    reset_hard(&path).await?;
                }
                incoming.landing = Some(path);
            }
            Some(WsPlan::Fresh { path, git }) => {
                incoming.created = Some(path.clone());
                self.save_incoming(&incoming);
                if git {
                    match (&bundle, params.head.as_deref()) {
                        (Some(bundle), Some(head)) => {
                            super::git::init_from_bundle(bundle, &path)
                                .await
                                .map_err(other)?;
                            let landing = super::git::update_existing_checkout(
                                &path,
                                head,
                                params.branch.as_deref(),
                                None,
                                &device,
                            )
                            .await
                            .map_err(other)?;
                            notes.extend(landing.notes);
                            incoming.branch = landing.branch;
                            incoming.staged_head = Some(head.to_owned());
                            reset_hard(&path).await?;
                        }
                        _ => git_init(&path).await?,
                    }
                } else {
                    tokio::fs::create_dir_all(&path)
                        .await
                        .map_err(|e| EngineError::Other(e.to_string()))?;
                }
                incoming.landing = Some(path);
            }
        }
        incoming.notes.extend(notes.clone());
        let landing = incoming
            .landing
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.register_tickets(&incoming);
        self.save_incoming(&incoming);
        Ok(StageReply { landing, notes })
    }

    /// Write back a changed move record (unless it was aborted meanwhile).
    fn save_incoming(&self, incoming: &Incoming) {
        if let Some(slot) = self.incoming_table().get_mut(&incoming.move_id) {
            *slot = incoming.clone();
        }
    }

    /// `~/.zeron/worktrees/<repo>/<branch or workspace name>`, made unique.
    fn worktree_dest(&self, repo: &Path, workspace: &WorkspaceDesc, branch: Option<&str>) -> PathBuf {
        let repo_name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| workspace.name.clone());
        let leaf = safe_name(branch.unwrap_or(&workspace.name));
        let base = self.0.repos.worktrees_root().join(safe_name(&repo_name));
        unique_path(&base.join(if leaf.is_empty() { "moved".into() } else { leaf }))
    }

    /// `MoveCommit`: apply everything and take the chat over.
    pub async fn commit(&self, params: CommitParams) -> Result<CommitReply, EngineError> {
        let incoming = self
            .incoming_table()
            .get(&params.move_id)
            .cloned()
            .ok_or_else(|| EngineError::Other("This move is no longer known here".into()))?;
        self.commit_table().insert(
            params.move_id.clone(),
            CommitState::Applying {
                abort_requested: false,
            },
        );
        let result = self.apply(&incoming, &params).await;
        self.commit_table().insert(
            params.move_id.clone(),
            if result.is_ok() {
                CommitState::Committed
            } else {
                CommitState::Failed
            },
        );
        self.incoming_table().remove(&params.move_id);
        match &result {
            Ok(_) => {
                self.0.tickets.remove_move(&incoming.move_id);
                if let Some(landing) = incoming.landing.as_deref() {
                    super::git::cleanup_move_refs(landing, &incoming.move_id).await;
                }
                let _ = tokio::fs::remove_dir_all(&incoming.staging).await;
            }
            Err(_) => self.forget(&incoming).await,
        }
        result
    }

    /// Everything that can fail and changes nothing comes first; then the
    /// landing is changed; the placement flip is last, and nothing after it
    /// can fail the commit.
    async fn apply(&self, incoming: &Incoming, params: &CommitParams) -> Result<CommitReply, EngineError> {
        let inner = &self.0;
        let uploads = inner.uploads.dir().to_path_buf();

        // ── checks (nothing changes yet) ───────────────────────────────
        let last_round = *params
            .rounds
            .last()
            .ok_or_else(|| EngineError::Other("The move carried no files".into()))?;
        let mut results = Vec::new();
        for round in &params.rounds {
            results.push((
                *round,
                zeron_transfer::Transfers::sync_result(&incoming.round_dir(*round)).map_err(|e| {
                    EngineError::Other(format!("The copied files are incomplete: {e}"))
                })?,
            ));
        }
        let handle = inner.doc_host.open(&incoming.chat_id)?;
        inner
            .doc_host
            .await_synced(&incoming.chat_id, Duration::from_secs(90))
            .await?;
        self.check_not_aborted(&incoming.move_id)?;
        let lineage = inner.lineage.get(&incoming.chat_id).unwrap_or_default();
        let returning = incoming.returning();
        let created = incoming.created_this_move();
        let mut notes = incoming.notes.clone();
        let backups = inner.dir.join("backups").join(&incoming.move_id);
        let departure: HashMap<String, String> = if returning {
            lineage
                .departure
                .as_ref()
                .map(|d| d.files.clone())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let staged = collect_staged(incoming, last_round, &results, &uploads, &mut notes);

        // ── the landing ────────────────────────────────────────────────
        inner
            .doc_host
            .import_processed_commands(&params.processed_commands)?;
        // Git refs first (they never touch the working tree), then files,
        // then the index.
        let mut branch = incoming.branch.clone();
        if let (Some(landing), Some(git)) = (incoming.landing.as_deref(), params.git.as_ref())
            && let Some(head) = git.head.as_deref()
        {
            if let Some(round) = git.bundle_round {
                let bundle = incoming.round_dir(round).join(ROOT_GIT).join("final.bundle");
                super::git::fetch_bundle(landing, &bundle, &incoming.move_id)
                    .await
                    .map_err(other)?;
            }
            // A landing made for this move keeps the branch it got at the
            // stage (perhaps renamed); a returning one goes back to the
            // source's branch.
            let (want_branch, departure_head) = if created || !returning {
                (
                    incoming.branch.clone().or_else(|| git.branch.clone()),
                    incoming.staged_head.clone(),
                )
            } else {
                (
                    git.branch.clone(),
                    lineage.departure.as_ref().and_then(|d| d.head.clone()),
                )
            };
            let landed = super::git::update_existing_checkout(
                landing,
                head,
                want_branch.as_deref(),
                departure_head.as_deref(),
                &incoming.from_device_name,
            )
            .await
            .map_err(other)?;
            notes.extend(landed.notes);
            branch = landed.branch;
        }

        for item in &staged.files {
            let Some(base) = incoming.root_landing(&item.root, &uploads) else {
                continue;
            };
            let target = join_rel(&base, &item.rest);
            if item.root == ROOT_UPLOADS && target.exists() {
                continue; // same upload id = same bytes
            }
            let known_here = if item.root == ROOT_WORKSPACE {
                departure.get(&item.rest).cloned()
            } else {
                None
            };
            let free = created && item.root == ROOT_WORKSPACE;
            match place(self, item, &target, known_here.as_deref(), free, &backups).await {
                Ok(Some(note)) => notes.push(note),
                Ok(None) => {}
                Err(error) => notes.push(format!("Couldn't place {}: {error}", target.display())),
            }
        }

        // Deletions: workspace files the source no longer has. Only in a
        // landing made for this move, or where the file is exactly what this
        // device had when the chat left; never from a partial listing.
        if let Some(landing) = incoming.landing.clone()
            && (created || returning)
        {
            let keep: HashSet<String> = params
                .workspace_rels
                .iter()
                .cloned()
                .chain(staged.workspace_rels.iter().cloned())
                .collect();
            let listing = tokio::task::spawn_blocking({
                let landing = landing.clone();
                move || super::scope::walk(&landing, &super::scope::ScopeRules::default())
            })
            .await
            .map_err(other)?
            .map_err(other)?;
            if listing.truncated || params.workspace_rels.is_empty() {
                notes.push(
                    "Files deleted on the other device were left in place here (the folder was too large to compare)"
                        .into(),
                );
            } else {
                let gone: Vec<(String, PathBuf)> = listing
                    .files
                    .into_iter()
                    .filter(|f| {
                        matches!(f.kind, super::scope::ScopeKind::File | super::scope::ScopeKind::Symlink)
                            && !keep.contains(&f.rel)
                    })
                    .map(|f| (f.rel, f.abs))
                    .collect();
                let hashes = inner
                    .transfers
                    .hash_files(gone.iter().map(|(_, p)| p.clone()).collect())
                    .await
                    .unwrap_or_default();
                for (index, (rel, path)) in gone.into_iter().enumerate() {
                    let unchanged_here = departure.get(&rel).is_some_and(|known| {
                        hashes.get(index).and_then(|h| h.as_deref()) == Some(known.as_str())
                    });
                    if created || unchanged_here {
                        let _ = tokio::fs::remove_file(&path).await;
                    } else {
                        notes.push(format!(
                            "Kept {rel}: it's gone on {} but was changed or added here",
                            incoming.from_device_name
                        ));
                    }
                }
            }
        }
        if let (Some(landing), Some(git)) = (incoming.landing.as_deref(), params.git.as_ref()) {
            let patch = base64::engine::general_purpose::STANDARD
                .decode(&git.staged_patch_b64)
                .unwrap_or_default();
            let head = git.head.clone().unwrap_or_default();
            if git.head.is_some() || !patch.is_empty() {
                notes.extend(
                    super::git::apply_index(landing, &head, &patch)
                        .await
                        .map_err(other)?,
                );
            }
        }

        // The agent's own session, else a transcript replay.
        let cwd = match incoming.landing.as_deref() {
            Some(landing) if !incoming.workspace.cwd_rel.is_empty() => {
                join_rel(landing, &incoming.workspace.cwd_rel)
            }
            Some(landing) => landing.to_path_buf(),
            None => crate::repos::session_home_dir().map_err(|e| EngineError::Other(e.to_string()))?,
        };
        let _ = tokio::fs::create_dir_all(&cwd).await;
        let cwd_text = cwd.display().to_string();
        let mut replay_history = true;
        let mut session_id = String::new();
        if let Some(export) = params.session.clone() {
            let staged_dir = incoming.round_dir(last_round).join(ROOT_SESSION);
            let roots = inner.port_roots.clone();
            let new_cwd = cwd_text.clone();
            let imported = tokio::task::spawn_blocking(move || {
                zeron_harness::portable::import_session(&export, &staged_dir, &roots, &new_cwd)
            })
            .await
            .map_err(other)?;
            match imported {
                Ok(imported) => {
                    session_id = imported.session_id;
                    replay_history = false;
                }
                Err(error) => {
                    tracing::warn!(%error, "move: agent session import failed; replaying history");
                    notes.push(
                        "The agent's own session couldn't be carried over; it continues from the transcript"
                            .into(),
                    );
                }
            }
        }
        if params.login_included {
            let path = incoming.round_dir(last_round).join(ROOT_LOGIN).join("handoff.json");
            match tokio::fs::read(&path)
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            {
                Some(handoff) => match inner.accounts.import_login(&handoff).await {
                    Ok(outcome) => tracing::info!(?outcome, "move: agent login handed over"),
                    Err(error) => notes.push(format!("The agent login couldn't be set up here: {error}")),
                },
                None => notes.push("The agent login didn't arrive; sign in here to continue".into()),
            }
            let _ = tokio::fs::remove_file(&path).await;
        }
        let space_id = match incoming.landing.as_deref() {
            Some(landing) => Some(self.space_for(landing, &incoming.workspace)?),
            None => None,
        };

        // ── the transcript and the handover ────────────────────────────
        // What actually travelled (unchanged files never did), as the seam
        // shows it: the user's files, not the agent's bookkeeping.
        let (mut files_sent, mut bytes_sent) = (0u64, 0u64);
        let mut counted: HashSet<&str> = HashSet::new();
        for (_, result) in results.iter().rev() {
            for file in &result.files {
                if !matches!(file.outcome, zeron_transfer::SyncOutcome::Landed)
                    || !counted.insert(file.rel.as_str())
                {
                    continue;
                }
                bytes_sent += file.size;
                let root = file.rel.split('/').next().unwrap_or_default();
                if !matches!(root, ROOT_SESSION | ROOT_LOGIN | ROOT_GIT) {
                    files_sent += 1;
                }
            }
        }
        let mut path_map: Vec<(String, String)> = Vec::new();
        if let Some(landing) = incoming.landing.as_deref() {
            path_map.push((
                incoming.workspace.source_root.clone(),
                landing.display().to_string(),
            ));
        }
        for extra in &incoming.extras {
            path_map.push((
                extra.desc.source_path.clone(),
                extra.parent.join(&extra.desc.name).display().to_string(),
            ));
        }
        let text = note::compose(&note::NoteInput {
            from_device: &incoming.from_device_name,
            to_device: &inner.device_name,
            path_map: &path_map,
            processes: &[],
            pending_question: params.pending_question.as_deref(),
            was_active: params.was_active,
            lines: &params.note_lines,
        });
        // The last point where an abort still wins.
        self.check_not_aborted(&incoming.move_id)?;
        inner.sessions.forget_harness_session(&incoming.chat_id);
        let now = chrono::Utc::now().timestamp_millis();
        let seam_id = format!("moved:{}", incoming.move_id);
        handle.doc().push_message(&zeron_doc::SessionMessageEntry {
            duration_ms: None,
            id: seam_id.clone(),
            role: MessageRole::System,
            parts: vec![MessagePart::Moved {
                id: seam_id,
                seam: MoveSeam {
                    from_device_id: incoming.from_device_id.clone(),
                    from_device_name: incoming.from_device_name.clone(),
                    to_device_id: inner.device_id.clone(),
                    to_device_name: inner.device_name.clone(),
                    files_sent,
                    bytes_sent,
                    duration_ms: (now - params.started_at).max(0) as u64,
                    cwd: Some(cwd_text.clone()),
                },
            }],
            created_at: now,
            device_id: inner.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        })?;
        handle.doc().set_move_note(Some(&zeron_doc::MoveNote {
            move_id: incoming.move_id.clone(),
            text,
            replay_history,
        }))?;
        inner.workspace.set_chat_placement(
            &incoming.chat_id,
            &zeron_doc::ChatPlacement {
                device_id: &inner.device_id,
                space_id: space_id.as_deref(),
                cwd: Some(&cwd_text),
                branch: branch.as_deref(),
                harness_session: Some((&session_id, &cwd_text)),
            },
        )?;

        // ── this device hosts the chat now: nothing below can fail it ──
        let landing_text = incoming.landing.as_ref().map(|p| p.display().to_string());
        let created_ever = created || (returning && lineage.created_by_move);
        let origins: Vec<(String, String)> = incoming
            .extras
            .iter()
            .map(|extra| {
                (
                    extra.desc.origin.clone().unwrap_or_else(|| {
                        extra_origin(&incoming.from_device_id, &extra.desc.source_path)
                    }),
                    extra.parent.join(&extra.desc.name).display().to_string(),
                )
            })
            .collect();
        inner.lineage.update(&incoming.chat_id, |lineage| {
            if landing_text.is_some() {
                lineage.workspace = landing_text.clone();
                lineage.created_by_move = created_ever;
            }
            for (origin, local) in origins {
                lineage.extras.insert(origin, local);
            }
            lineage.departure = None;
        });
        if let Err(error) = inner.doc_host.drain_now(&incoming.chat_id) {
            tracing::warn!(chat = %incoming.chat_id, %error, "moved chat: drain failed");
        }
        if params.was_active
            && let Some(request) = inner
                .doc_host
                .request_from_chat_row(&incoming.chat_id, "Continue where you left off.")
            && let Err(error) = inner.doc_host.queue_command(
                &incoming.chat_id,
                SessionCommandPayload::Run {
                    request,
                    message_id: uuid::Uuid::new_v4().to_string(),
                },
            )
        {
            tracing::warn!(chat = %incoming.chat_id, %error, "moved chat: couldn't continue the turn");
            notes.push("Send a message to continue: the agent couldn't be restarted automatically".into());
        }
        Ok(CommitReply {
            cwd: cwd_text,
            notes,
        })
    }

    fn check_not_aborted(&self, move_id: &str) -> Result<(), EngineError> {
        match self.commit_table().get(move_id) {
            Some(CommitState::Applying {
                abort_requested: true,
            }) => Err(EngineError::Other("The move was cancelled".into())),
            _ => Ok(()),
        }
    }

    /// The space a landed workspace belongs to on this device (made if new).
    fn space_for(&self, landing: &Path, workspace: &WorkspaceDesc) -> Result<String, EngineError> {
        let me = &self.0.device_id;
        let path = landing.display().to_string();
        if let Some(space) = self
            .0
            .workspace
            .read_spaces()?
            .into_iter()
            .find(|s| &s.device_id == me && s.path == path)
        {
            return Ok(space.id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.0.workspace.create_space(
            &id,
            me,
            &path,
            Some(workspace.name.clone()),
            workspace.kind == WorkspaceKind::Git,
        )?;
        // `create_space` dedupes by (device, path): read back the winner.
        Ok(self
            .0
            .workspace
            .read_spaces()?
            .into_iter()
            .find(|s| &s.device_id == me && s.path == path)
            .map(|s| s.id)
            .unwrap_or(id))
    }

    /// `MoveAbort`: forget the move and remove what it created here — or,
    /// when its commit is already running or done, say so.
    pub async fn abort(&self, params: AbortParams) -> Result<AbortReply, EngineError> {
        let state = self.commit_table().get(&params.move_id).copied();
        match state {
            Some(CommitState::Committed) => {
                return Ok(AbortReply {
                    outcome: AbortOutcome::Committed,
                });
            }
            Some(CommitState::Applying { .. }) => {
                self.commit_table().insert(
                    params.move_id.clone(),
                    CommitState::Applying {
                        abort_requested: true,
                    },
                );
                return Ok(AbortReply {
                    outcome: AbortOutcome::Applying,
                });
            }
            Some(CommitState::Failed) | None => {}
        }
        let incoming = self.incoming_table().remove(&params.move_id);
        if let Some(incoming) = incoming {
            self.forget(&incoming).await;
        }
        self.0.tickets.remove_move(&params.move_id);
        Ok(AbortReply {
            outcome: AbortOutcome::Aborted,
        })
    }

    /// Drop a move that won't happen: tickets, staging, refs, and a landing
    /// it created (never one that existed before).
    async fn forget(&self, incoming: &Incoming) {
        self.0.tickets.remove_move(&incoming.move_id);
        if let Some(landing) = incoming.landing.as_deref() {
            super::git::cleanup_move_refs(landing, &incoming.move_id).await;
        }
        let _ = tokio::fs::remove_dir_all(&incoming.staging).await;
        let Some(created) = incoming.created.as_deref() else {
            return;
        };
        if incoming.returning() {
            return;
        }
        if let Some(WsPlan::Worktree { repo }) = &incoming.plan {
            super::git::cleanup_move_refs(repo, &incoming.move_id).await;
            let _ = tokio::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["worktree", "remove", "--force"])
                .arg(created)
                .stdin(std::process::Stdio::null())
                .output()
                .await;
        }
        let _ = tokio::fs::remove_dir_all(created).await;
    }
}

#[derive(Default)]
struct Staged {
    files: Vec<StagedFile>,
    /// Workspace files the transfer accounts for (landed, unchanged or
    /// skipped): never deleted here.
    workspace_rels: HashSet<String>,
}

struct StagedFile {
    root: String,
    rest: String,
    source: PathBuf,
    sha256: String,
    symlink: Option<String>,
}

/// Which staged copy each file of the final round comes from.
fn collect_staged(
    incoming: &Incoming,
    last_round: u32,
    results: &[(u32, zeron_transfer::SyncResult)],
    uploads: &Path,
    notes: &mut Vec<String>,
) -> Staged {
    let mut staged = Staged::default();
    let Some((_, result)) = results.iter().find(|(r, _)| *r == last_round) else {
        return staged;
    };
    for file in &result.files {
        let Some((root, rest)) = file.rel.split_once('/') else {
            continue;
        };
        if root == ROOT_WORKSPACE {
            staged.workspace_rels.insert(rest.to_owned());
        }
        match file.outcome {
            zeron_transfer::SyncOutcome::Landed => push(
                &mut staged,
                &file.rel,
                incoming.round_dir(last_round).join(&file.rel),
                file.sha256.clone(),
                None,
            ),
            zeron_transfer::SyncOutcome::Unchanged => {
                let dirs = incoming.basis_dirs(last_round, root, uploads);
                let landing = incoming.root_landing(root, uploads);
                match file.basis_index.and_then(|i| dirs.get(i)) {
                    // Already right where it lands.
                    Some(dir) if Some(dir) == landing.as_ref() => {}
                    Some(dir) => push(
                        &mut staged,
                        &file.rel,
                        join_rel(dir, rest),
                        file.sha256.clone(),
                        None,
                    ),
                    None => {}
                }
            }
            zeron_transfer::SyncOutcome::Skipped => {
                // Changing while it was sent: the newest earlier copy, if
                // any, is better than none.
                let earlier = results
                    .iter()
                    .rev()
                    .filter(|(r, _)| *r != last_round)
                    .find_map(|(r, result)| {
                        result
                            .files
                            .iter()
                            .find(|f| {
                                f.rel == file.rel
                                    && matches!(f.outcome, zeron_transfer::SyncOutcome::Landed)
                            })
                            .map(|f| (incoming.round_dir(*r).join(&f.rel), f.sha256.clone()))
                    });
                if let Some((source, sha256)) = earlier {
                    push(&mut staged, &file.rel, source, sha256, None);
                    notes.push(format!(
                        "{rest} was still changing on {}; it arrived as it was a moment earlier",
                        incoming.from_device_name
                    ));
                } else {
                    notes.push(format!(
                        "{rest} was still changing on {} and didn't come along",
                        incoming.from_device_name
                    ));
                }
            }
        }
    }
    for link in &result.symlinks {
        if let Some(("ws", rest)) = link.rel.split_once('/') {
            staged.workspace_rels.insert(rest.to_owned());
        }
        push(&mut staged, &link.rel, PathBuf::new(), String::new(), Some(link.target.clone()));
    }
    staged
}

fn push(staged: &mut Staged, rel: &str, source: PathBuf, sha256: String, symlink: Option<String>) {
    let Some((root, rest)) = rel.split_once('/') else {
        return;
    };
    if matches!(root, ROOT_SESSION | ROOT_LOGIN | ROOT_GIT) {
        return; // consumed separately
    }
    staged.files.push(StagedFile {
        root: root.to_owned(),
        rest: rest.to_owned(),
        source,
        sha256,
        symlink,
    });
}

fn other(error: impl std::fmt::Display) -> EngineError {
    EngineError::Other(error.to_string())
}

/// Put one staged file (or symlink) at `target`. An existing file with other
/// content is copied to `backups` first — unless the landing is new
/// (`free`) or it is exactly what this device had when the chat left
/// (`known_here`). `Ok(Some(note))` reports a backup.
async fn place(
    service: &MoveService,
    item: &StagedFile,
    target: &Path,
    known_here: Option<&str>,
    free: bool,
    backups: &Path,
) -> anyhow::Result<Option<String>> {
    let mut note = None;
    if let Ok(meta) = tokio::fs::symlink_metadata(target).await {
        if meta.is_file() {
            let current = service
                .0
                .transfers
                .hash_files(vec![target.to_path_buf()])
                .await
                .ok()
                .and_then(|mut h| h.pop().flatten());
            if item.symlink.is_none() && current.as_deref() == Some(item.sha256.as_str()) {
                return Ok(None); // already the right content
            }
            let expected = known_here.is_some() && current.as_deref() == known_here;
            if !free && !expected {
                let backup = join_rel(backups, &format!("{}/{}", item.root, item.rest));
                if let Some(parent) = backup.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::copy(target, &backup).await?;
                note = Some(format!(
                    "{} had other changes on this device; the local copy is saved in {}",
                    target.display(),
                    backup.display()
                ));
            }
        }
        if meta.is_dir() {
            tokio::fs::remove_dir_all(target).await?;
        } else {
            tokio::fs::remove_file(target).await?;
        }
    }
    if let Some(parent) = target.parent() {
        ensure_dirs(parent).await?;
    }
    match item.symlink.as_deref() {
        Some(link) => {
            #[cfg(unix)]
            tokio::fs::symlink(link, target).await?;
            #[cfg(not(unix))]
            let _ = link;
        }
        None => {
            if tokio::fs::rename(&item.source, target).await.is_err() {
                // Another volume (or a basis copy kept for later): copy, then
                // swap in atomically.
                let tmp = target.with_extension("zeron-move-tmp");
                tokio::fs::copy(&item.source, &tmp).await?;
                tokio::fs::rename(&tmp, target).await?;
            }
        }
    }
    Ok(note)
}

/// `create_dir_all`, replacing a file that sits where a folder must be.
async fn ensure_dirs(dir: &Path) -> anyhow::Result<()> {
    match tokio::fs::create_dir_all(dir).await {
        Ok(()) => Ok(()),
        Err(_) => {
            let mut probe = dir.to_path_buf();
            loop {
                if let Ok(meta) = tokio::fs::symlink_metadata(&probe).await {
                    if !meta.is_dir() {
                        tokio::fs::remove_file(&probe).await?;
                    }
                    break;
                }
                if !probe.pop() {
                    break;
                }
            }
            tokio::fs::create_dir_all(dir).await?;
            Ok(())
        }
    }
}

async fn reset_hard(repo: &Path) -> Result<(), EngineError> {
    run_git(repo, &["reset", "--hard", "--quiet", "HEAD"]).await
}

async fn git_init(path: &Path) -> Result<(), EngineError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?;
    run_git(path, &["init", "--quiet"]).await
}

async fn run_git(repo: &Path, args: &[&str]) -> Result<(), EngineError> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(EngineError::Other(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Where a workspace that is new here lands: the same place under the home
/// folder when it's free, else `~/Zeron/Moved/<name>` (made unique).
fn free_landing(home: &Path, desc: &WorkspaceDesc) -> PathBuf {
    if let Some(rel) = desc.home_rel.as_deref().filter(|r| !r.is_empty()) {
        let same = join_rel(home, rel);
        let free = same != home
            && !super::scope::is_secret_store(&same, home)
            && match std::fs::read_dir(&same) {
                Ok(mut entries) => entries.next().is_none(),
                Err(_) => !same.exists(),
            };
        if free {
            return same;
        }
    }
    unique_path(&home.join("Zeron").join("Moved").join(safe_name(&desc.name)))
}

fn unique_path(base: &Path) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (2..)
        .map(|n| base.with_file_name(format!("{name}-{n}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| base.to_path_buf())
}

/// A `/`-separated relative path under `base` (never escaping it).
fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        out.push(part);
    }
    out
}

/// An absolute path with no `.`/`..` components (lexical checks hold).
fn plain_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
}

/// Places where a dropped file runs something: shell startup files in the
/// home folder itself and autostart folders.
fn runs_things(path: &Path, home: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(home) else {
        return false;
    };
    let mut parts = rel.components();
    let first = parts
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    let top_level_dotfile = rel.components().count() == 1 && first.starts_with('.');
    let autostart = rel.starts_with("Library/LaunchAgents")
        || rel.starts_with("Library/LaunchDaemons")
        || rel.starts_with(".config/autostart")
        || rel.starts_with(".config/systemd")
        || rel.starts_with(".local/share/applications");
    top_level_dotfile || autostart
}

/// An extra's name is one plain path component and its root a short
/// `x<n>` id: nothing a peer sends can address another folder.
fn safe_item(desc: &ExtraDesc) -> bool {
    let name_ok = !desc.name.is_empty()
        && desc.name != "."
        && desc.name != ".."
        && !desc.name.contains(['/', '\\', '\0']);
    let root_ok = desc.root.len() >= 2
        && desc.root.len() <= 8
        && desc.root.starts_with('x')
        && desc.root[1..].chars().all(|c| c.is_ascii_digit());
    name_ok && root_ok
}

fn safe_name(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect();
    cleaned.trim().trim_matches('.').chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_joins_never_escape() {
        let base = Path::new("/home/bob/ws");
        assert_eq!(join_rel(base, "a/../../b/./c"), PathBuf::from("/home/bob/ws/a/b/c"));
        assert_eq!(join_rel(base, ""), base);
    }

    #[test]
    fn a_free_home_relative_landing_is_preferred() {
        let home = tempfile::tempdir().unwrap();
        let desc = WorkspaceDesc {
            kind: WorkspaceKind::Folder,
            name: "proj".into(),
            home_rel: Some("dev/proj".into()),
            cwd_rel: String::new(),
            source_root: "/Users/alice/dev/proj".into(),
            repo: None,
        };
        assert_eq!(free_landing(home.path(), &desc), home.path().join("dev/proj"));
        std::fs::create_dir_all(home.path().join("dev/proj")).unwrap();
        assert_eq!(free_landing(home.path(), &desc), home.path().join("dev/proj"), "empty folders are free");
        std::fs::write(home.path().join("dev/proj/x"), b"x").unwrap();
        assert_eq!(free_landing(home.path(), &desc), home.path().join("Zeron/Moved/proj"));
        std::fs::create_dir_all(home.path().join("Zeron/Moved/proj")).unwrap();
        assert_eq!(free_landing(home.path(), &desc), home.path().join("Zeron/Moved/proj-2"));
    }

    #[test]
    fn peers_cannot_name_folders_for_extras() {
        let desc = |root: &str, name: &str| ExtraDesc {
            root: root.into(),
            name: name.into(),
            source_path: "/x".into(),
            parent_home_rel: None,
            is_dir: false,
            origin: None,
        };
        assert!(safe_item(&desc("x0", "shot.png")));
        assert!(!safe_item(&desc("x0", "../etc")));
        assert!(!safe_item(&desc("x0", "a/b")));
        assert!(!safe_item(&desc("ws", "a")));
        assert!(!safe_item(&desc("x", "a")));
        assert!(!safe_item(&desc("x0/..", "a")));
        let home = Path::new("/home/bob");
        assert!(runs_things(Path::new("/home/bob/.bashrc"), home));
        assert!(runs_things(Path::new("/home/bob/Library/LaunchAgents/x.plist"), home));
        assert!(!runs_things(Path::new("/home/bob/Desktop/shot.png"), home));
        assert!(!runs_things(Path::new("/home/bob/.config/app/settings.toml"), home));
        assert!(!plain_path(Path::new("/home/bob/../../usr/bin/x")));
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("feat/login: v2"), "feat-login- v2");
        assert_eq!(safe_name(".."), "");
    }
}
