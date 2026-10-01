//! Content-addressed checkpoints of a cloud box (docs/cloud.md §5).
//!
//! A box's disk disappears when it sleeps, so everything that has to
//! survive goes to the checkpoint store first: the engine data dir, the
//! agents' own state under the home folder, and the workspace of every chat
//! and project hosted here.
//!
//! **Objects.** Every object is a zstd frame named by the SHA-256 of its
//! *uncompressed* payload, so the name of anything can be computed without
//! compressing it, and a download is verified by decompressing and hashing.
//! Files under [`SMALL_FILE_MAX`] travel in packs: a tar of members named by
//! their content hash, up to [`OBJECT_MAX`] each. Larger files are cut into
//! [`OBJECT_MAX`] chunks, one object each. Only content the store doesn't
//! already have is uploaded: the local [`Index`] remembers which objects
//! exist (and the hashes of unchanged files), the previous manifest says
//! which pack holds which small file, and large chunks are probed with
//! `HEAD` before a `PUT`. A small change therefore uploads one small pack.
//!
//! **Manifests.** `manifests/{seq}.json` lists every root and entry. `HEAD`
//! is written last, so a checkpoint that fails halfway leaves the previous
//! one in force. A checkpoint identical to the previous one writes nothing.
//!
//! **Roots.**
//! - `data`: the engine data dir. SQLite databases are copied with
//!   `VACUUM INTO` (a consistent snapshot while the engine has them open);
//!   their `-wal`/`-shm`/`-journal` files, locks, temp files, logs, updates,
//!   managed adapters and worktrees (which are workspace roots) stay out.
//! - `harness`: the agents' state under the home folder ([`HARNESS_PATHS`]),
//!   logins included: the store is the user's own bucket, and a box that
//!   forgot its logins on every sleep would be useless.
//! - one root per workspace (`ws-<hash of path>`), everything included:
//!   `.git` and regenerable folders, so the box keeps its build caches. A
//!   linked worktree `requires` its main checkout, which holds its git
//!   metadata. A chat working in the home folder gets the `home` root, which
//!   leaves caches and toolchains out.
//!
//! **Restore.** At boot `data` and `harness` are restored in place before
//! the engine starts. Workspaces are restored lazily, before a chat's first
//! run ([`Checkpointer::restore_workspace`]), into a sibling folder that is
//! renamed into place. Until then a checkpoint carries their entries
//! forward unchanged.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{CheckpointStore, Head};

/// Largest object: a pack's raw tar, or one chunk of a large file.
pub const OBJECT_MAX: u64 = 64 * 1024 * 1024;
/// Files below this travel in packs; the rest are chunked.
pub const SMALL_FILE_MAX: u64 = 4 * 1024 * 1024;
const ZSTD_LEVEL: i32 = 3;
const MANIFEST_VERSION: u32 = 1;

pub const ROOT_DATA: &str = "data";
pub const ROOT_HARNESS: &str = "harness";
pub const ROOT_HOME: &str = "home";
/// The checkpointer's own state, inside the data dir and never checkpointed.
pub const STATE_DIR: &str = "cloud-checkpoints";

/// The agents' state, relative to the home folder. Cursor's Zeron-side
/// store lives in the data dir (`cursor-state`).
pub const HARNESS_PATHS: &[&str] = &[
    ".claude",
    ".claude.json",
    ".codex",
    ".config/opencode",
    ".local/share/opencode",
    ".local/state/opencode",
    ".pi",
    ".grok",
    ".gemini",
    ".hermes",
    ".cursor",
    ".config/devin",
    ".local/share/devin",
];

/// Regenerable or bulky parts of the harness paths.
const HARNESS_EXCLUDES: &[&str] = &[
    ".claude/local",
    ".claude/statsig",
    ".codex/log",
    ".codex/tmp",
    ".local/share/opencode/bin",
    ".local/share/opencode/log",
    ".cursor/extensions",
];

/// Top-level data-dir folders that are logs, downloads, regenerable, or
/// checkpointed as roots of their own.
const DATA_EXCLUDES: &[&str] = &[
    "logs",
    "updates",
    "adapters",
    "worktrees",
    "local-edge-server",
    STATE_DIR,
];

/// Caches and toolchains a chat working in the home folder doesn't carry.
const HOME_EXCLUDES: &[&str] = &[
    ".cache",
    ".npm",
    ".npm-global",
    ".pnpm-store",
    ".local/share/pnpm",
    ".bun",
    ".nvm",
    ".rustup",
    ".cargo",
    ".gradle",
    ".m2",
    "go",
    ".local/share/claude",
    ".local/bin",
    ".local/lib",
];

// ── manifest ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default)]
    pub version: u32,
    pub seq: u64,
    pub created_at: i64,
    pub roots: BTreeMap<String, RootManifest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootManifest {
    /// Where the root was on the box.
    pub path: String,
    /// Roots to restore before this one (a linked worktree's main checkout).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Symlink,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `/`-separated, relative to the root.
    pub rel: String,
    pub kind: EntryKind,
    /// Unix permission bits.
    pub mode: u32,
    /// File bytes (0 for links and folders).
    pub size: u64,
    pub mtime_ms: i64,
    /// A symlink's target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Content hashes: one per [`OBJECT_MAX`] chunk, or the whole content
    /// of a small file. Empty for empty files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<String>,
    /// The pack holding a small file (member name = `chunks[0]`). Absent
    /// when the content is an object of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
}

impl Manifest {
    /// Every object the manifest references.
    pub fn objects(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        for root in self.roots.values() {
            for entry in &root.entries {
                match &entry.pack {
                    Some(pack) => {
                        out.insert(pack.clone());
                    }
                    None => out.extend(entry.chunks.iter().cloned()),
                }
            }
        }
        out
    }
}

// ── roots to checkpoint ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    Data,
    Harness,
    Workspace,
    /// A workspace that is the home folder itself.
    Home,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootSpec {
    pub name: String,
    pub path: PathBuf,
    pub kind: RootKind,
    pub requires: Vec<String>,
}

impl RootSpec {
    pub fn data(data_dir: &Path) -> Self {
        Self {
            name: ROOT_DATA.into(),
            path: data_dir.to_path_buf(),
            kind: RootKind::Data,
            requires: Vec::new(),
        }
    }

    pub fn harness(home: &Path) -> Self {
        Self {
            name: ROOT_HARNESS.into(),
            path: home.to_path_buf(),
            kind: RootKind::Harness,
            requires: Vec::new(),
        }
    }

    /// The paths this root covers (others' walks skip them).
    fn claims(&self) -> Vec<PathBuf> {
        match self.kind {
            RootKind::Harness => HARNESS_PATHS.iter().map(|p| self.path.join(p)).collect(),
            _ => vec![self.path.clone()],
        }
    }
}

/// The root name for a workspace at `path`.
pub fn workspace_root_name(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    format!("ws-{}", &hex(&digest)[..16])
}

/// Workspace roots for the folders chats and projects use here: each one's
/// git toplevel (else the folder), plus the main checkout of a linked
/// worktree, which holds its git metadata. The home folder becomes the
/// `home` root; `/` and folders above home are skipped. Blocking (git).
pub fn workspace_roots(folders: impl IntoIterator<Item = PathBuf>, home: &Path) -> Vec<RootSpec> {
    let mut out: BTreeMap<PathBuf, RootSpec> = BTreeMap::new();
    for folder in folders {
        if !folder.is_absolute() || !folder.is_dir() {
            continue;
        }
        let top = crate::moves::scope::workspace_root(&folder);
        if top.parent().is_none() || (home.starts_with(&top) && top != home) {
            tracing::warn!(path = %top.display(), "cloud checkpoint: not carrying a folder above home");
            continue;
        }
        let mut requires = Vec::new();
        if let Some(main) = main_checkout(&top).filter(|main| *main != top) {
            let spec = out
                .entry(main.clone())
                .or_insert_with(|| workspace_spec(main, home, Vec::new()));
            requires.push(spec.name.clone());
        }
        out.entry(top.clone())
            .or_insert_with(|| workspace_spec(top, home, requires));
    }
    out.into_values().collect()
}

fn workspace_spec(path: PathBuf, home: &Path, requires: Vec<String>) -> RootSpec {
    let home_root = path == home;
    RootSpec {
        name: if home_root {
            ROOT_HOME.into()
        } else {
            workspace_root_name(&path)
        },
        kind: if home_root {
            RootKind::Home
        } else {
            RootKind::Workspace
        },
        path,
        requires,
    }
}

/// For a linked worktree, the main checkout owning its git dir.
fn main_checkout(top: &Path) -> Option<PathBuf> {
    let output = crate::repos::git_std_command()
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(top)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if common == top.join(".git") || !common.is_absolute() {
        return None;
    }
    if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common) // a bare repository
    }
}

// ── the local index ─────────────────────────────────────────────────────────

/// What a stat says about a file's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    size: u64,
    mtime_ns: i64,
    ino: u64,
    dev: u64,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        let stat = zeron_transfer::hashcache::FileStat::of(meta);
        Self {
            size: stat.size,
            mtime_ns: stat.mtime_ns,
            ino: stat.ino,
            dev: stat.dev,
        }
    }

    /// Changed too recently to trust a cached hash (a second write in the
    /// same timestamp tick would go unnoticed).
    fn racy(&self) -> bool {
        let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
        now.saturating_sub(self.mtime_ns) < zeron_transfer::hashcache::RACY_WINDOW_NS
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedFile {
    stamp: Stamp,
    chunks: Vec<String>,
}

/// Objects known to be in the store, and the hashes of files as last seen.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    known: HashSet<String>,
    files: HashMap<String, CachedFile>,
}

impl Index {
    fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        write_atomic(path, &serde_json::to_vec(self)?)
    }

    fn remember(&mut self, path: &Path, meta: &std::fs::Metadata, chunks: &[String]) {
        let stamp = Stamp::of(meta);
        if !stamp.racy() {
            self.files.insert(
                path.to_string_lossy().into_owned(),
                CachedFile {
                    stamp,
                    chunks: chunks.to_vec(),
                },
            );
        }
    }

    fn cached(&self, path: &Path, meta: &std::fs::Metadata) -> Option<Vec<String>> {
        let cached = self.files.get(path.to_string_lossy().as_ref())?;
        (cached.stamp == Stamp::of(meta)).then(|| cached.chunks.clone())
    }
}

// ── the checkpointer ────────────────────────────────────────────────────────

/// Where `data` and `harness` restore to on this box.
#[derive(Debug, Clone)]
pub struct CloudPaths {
    pub data_dir: PathBuf,
    pub home: PathBuf,
}

impl CloudPaths {
    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join(STATE_DIR)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointStats {
    /// The checkpoint now in force.
    pub seq: u64,
    /// Objects uploaded.
    pub objects: u64,
    /// Compressed bytes uploaded.
    pub bytes: u64,
    /// False when nothing changed since the previous checkpoint (nothing
    /// was written).
    pub changed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootRestore {
    /// `None` on a first boot (an empty store).
    pub seq: Option<u64>,
    pub files: u64,
    /// Workspace roots waiting for [`Checkpointer::restore_workspace`].
    pub pending: Vec<PathBuf>,
}

struct State {
    index: Index,
    last: Option<Manifest>,
    /// Workspace roots of `last` not restored yet, by name.
    pending: BTreeMap<String, RootManifest>,
}

#[derive(Clone)]
pub struct Checkpointer {
    inner: Arc<Inner>,
}

struct Inner {
    store: CheckpointStore,
    paths: CloudPaths,
    /// Serialises checkpoints and restores.
    state: tokio::sync::Mutex<State>,
}

impl Checkpointer {
    pub fn new(store: CheckpointStore, paths: CloudPaths) -> Self {
        let index = Index::load(&paths.state_dir().join("index.json"));
        Self {
            inner: Arc::new(Inner {
                store,
                paths,
                state: tokio::sync::Mutex::new(State {
                    index,
                    last: None,
                    pending: BTreeMap::new(),
                }),
            }),
        }
    }

    pub fn paths(&self) -> &CloudPaths {
        &self.inner.paths
    }

    pub fn store(&self) -> &CheckpointStore {
        &self.inner.store
    }

    /// Restore `data` and `harness` from the store's `HEAD` (in place,
    /// before the engine opens anything) and queue the workspaces for lazy
    /// restore. A first boot finds no `HEAD` and restores nothing.
    pub async fn restore_boot(&self) -> anyhow::Result<BootRestore> {
        let mut state = self.inner.state.lock().await;
        let Some(head) = self.inner.store.get_head().await? else {
            tracing::info!("cloud checkpoint: empty store, first boot");
            return Ok(BootRestore::default());
        };
        let manifest = self.fetch_manifest(head.seq).await?;
        let mut report = BootRestore {
            seq: Some(head.seq),
            ..Default::default()
        };
        state.index.known.extend(manifest.objects());
        // `home` (a chat working in the home folder) restores in place
        // like `harness`: the folder exists, and every cwd is under it.
        for (name, dest) in [
            (ROOT_DATA, self.inner.paths.data_dir.clone()),
            (ROOT_HARNESS, self.inner.paths.home.clone()),
            (ROOT_HOME, self.inner.paths.home.clone()),
        ] {
            if let Some(root) = manifest.roots.get(name) {
                std::fs::create_dir_all(&dest)?;
                report.files += restore_root(
                    &self.inner.store,
                    root,
                    &dest,
                    &mut state.index,
                )
                .await
                .with_context(|| format!("restoring {name}"))?;
            }
        }
        state.pending = manifest
            .roots
            .iter()
            .filter(|(name, _)| ![ROOT_DATA, ROOT_HARNESS, ROOT_HOME].contains(&name.as_str()))
            .map(|(name, root)| (name.clone(), root.clone()))
            .collect();
        report.pending = state.pending.values().map(|r| PathBuf::from(&r.path)).collect();
        state.last = Some(manifest);
        self.save_index(&state.index);
        tracing::info!(seq = head.seq, files = report.files, workspaces = report.pending.len(),
            "cloud checkpoint: boot restore done");
        Ok(report)
    }

    /// Restore the workspace holding `path` (and what it requires) if it is
    /// still waiting. A workspace folder that already exists with content
    /// is left as it is: whatever put it there (a move) is newer. Returns
    /// the files restored.
    pub async fn restore_workspace(&self, path: &Path) -> anyhow::Result<u64> {
        let mut state = self.inner.state.lock().await;
        if state.pending.is_empty() {
            return Ok(0);
        }
        // Roots are recorded as git reports them (symlinks resolved); a
        // chat's cwd may not be.
        let resolved = resolve_existing_prefix(path);
        let mut wanted: Vec<String> = state
            .pending
            .iter()
            .filter(|(_, root)| path.starts_with(&root.path) || resolved.starts_with(&root.path))
            .map(|(name, _)| name.clone())
            .collect();
        let mut files = 0;
        // Bounded: a `requires` cycle can't spin forever.
        let mut budget = state.pending.len() * 2 + 2;
        while let Some(name) = wanted.pop() {
            budget = budget.saturating_sub(1);
            anyhow::ensure!(budget > 0, "workspace roots require each other in a cycle");
            let Some(root) = state.pending.get(&name).cloned() else {
                continue;
            };
            let missing: Vec<String> = root
                .requires
                .iter()
                .filter(|r| state.pending.contains_key(*r))
                .cloned()
                .collect();
            if !missing.is_empty() {
                wanted.push(name);
                wanted.extend(missing);
                continue;
            }
            state.pending.remove(&name);
            files += restore_workspace_root(&self.inner.store, &root, &mut state.index)
                .await
                .with_context(|| format!("restoring workspace {}", root.path))?;
        }
        if files > 0 {
            self.save_index(&state.index);
        }
        Ok(files)
    }

    /// Workspaces not restored yet.
    pub async fn pending_workspaces(&self) -> Vec<PathBuf> {
        let state = self.inner.state.lock().await;
        state.pending.values().map(|r| PathBuf::from(&r.path)).collect()
    }

    /// Workspace roots of the last checkpoint that a new one must keep:
    /// those still on disk (or still waiting to be restored).
    pub async fn previous_workspaces(&self) -> Vec<PathBuf> {
        let state = self.inner.state.lock().await;
        let Some(last) = &state.last else {
            return Vec::new();
        };
        last.roots
            .iter()
            .filter(|(name, _)| *name != ROOT_DATA && *name != ROOT_HARNESS)
            .map(|(_, root)| PathBuf::from(&root.path))
            .collect()
    }

    /// Checkpoint `roots`. Workspaces still waiting for their lazy restore
    /// are carried over from the previous manifest instead of walked.
    pub async fn checkpoint(&self, roots: Vec<RootSpec>) -> anyhow::Result<CheckpointStats> {
        let mut state = self.inner.state.lock().await;
        let result = self.checkpoint_locked(&mut state, roots).await;
        self.save_index(&state.index);
        let tmp = self.inner.paths.state_dir().join("tmp");
        let _ = std::fs::remove_dir_all(&tmp);
        result
    }

    async fn checkpoint_locked(
        &self,
        state: &mut State,
        roots: Vec<RootSpec>,
    ) -> anyhow::Result<CheckpointStats> {
        let store = &self.inner.store;
        let mut stats = CheckpointStats::default();

        // Walk and hash (blocking) every root that isn't waiting.
        let mut carried: BTreeMap<String, RootManifest> = BTreeMap::new();
        let mut walk: Vec<RootSpec> = Vec::new();
        for root in roots {
            match state.pending.get(&root.name) {
                Some(waiting) => {
                    carried.insert(root.name.clone(), waiting.clone());
                }
                None => walk.push(root),
            }
        }
        for (name, waiting) in &state.pending {
            carried.entry(name.clone()).or_insert_with(|| waiting.clone());
        }
        let claims: Vec<(String, PathBuf)> = walk
            .iter()
            .flat_map(|r| r.claims().into_iter().map(|p| (r.name.clone(), p)))
            .chain(
                carried
                    .iter()
                    .map(|(name, r)| (name.clone(), PathBuf::from(&r.path))),
            )
            .collect();
        let tmp = self.inner.paths.state_dir().join("tmp");
        let index = std::mem::take(&mut state.index);
        let (scanned, index) = tokio::task::spawn_blocking(move || {
            let mut index = index;
            let scanned = scan_roots(walk, &claims, &mut index, &tmp);
            (scanned, index)
        })
        .await
        .context("checkpoint scan panicked")?;
        state.index = index;
        let mut scanned = scanned?;

        // Which pack holds which small file, as of the last checkpoint.
        let mut pack_of: HashMap<String, String> = HashMap::new();
        if let Some(last) = &state.last {
            for root in last.roots.values() {
                for entry in &root.entries {
                    if let (Some(pack), Some(sha)) = (&entry.pack, entry.chunks.first()) {
                        pack_of.insert(sha.clone(), pack.clone());
                    }
                }
            }
        }

        // Large files: upload unknown chunks. Small files: reuse a pack or
        // an object that has the content, else queue them for a new pack.
        let mut to_pack: Vec<(usize, usize)> = Vec::new();
        for (ri, root) in scanned.iter_mut().enumerate() {
            for (ei, item) in root.entries.iter_mut().enumerate() {
                let entry = &mut item.entry;
                if entry.kind != EntryKind::File || entry.size == 0 {
                    continue;
                }
                if entry.size < SMALL_FILE_MAX && entry.chunks.len() == 1 {
                    let sha = &entry.chunks[0];
                    if let Some(pack) = pack_of.get(sha)
                        && state.index.known.contains(pack)
                    {
                        entry.pack = Some(pack.clone());
                    } else if !state.index.known.contains(sha) {
                        to_pack.push((ri, ei));
                    }
                    continue;
                }
                if entry.chunks.iter().all(|c| state.index.known.contains(c)) {
                    continue;
                }
                let source = item.source.clone().context("file without a source")?;
                match upload_large(store, &source, &mut state.index.known, &mut stats).await? {
                    Some((chunks, size)) => {
                        entry.chunks = chunks;
                        entry.size = size;
                    }
                    None => item.gone = true,
                }
            }
        }

        // New packs. Contents are re-read and re-hashed here, so a file that
        // changed since the walk is packed (and listed) as it is now.
        let mut packed: HashMap<String, String> = HashMap::new();
        let mut next = 0;
        while next < to_pack.len() {
            let batch: Vec<(usize, PathBuf)> = to_pack[next..]
                .iter()
                .enumerate()
                .map(|(i, (ri, ei))| {
                    let source = scanned[*ri].entries[*ei].source.clone().unwrap_or_default();
                    (next + i, source)
                })
                .collect();
            let seen: HashSet<String> = packed.keys().cloned().collect();
            let built = tokio::task::spawn_blocking(move || build_pack(batch, &seen))
                .await
                .context("pack build panicked")??;
            next = built.next;
            if let Some((pack_sha, tar)) = built.pack {
                upload_object(store, &pack_sha, tar, &mut state.index.known, &mut stats).await?;
                for member in &built.members {
                    packed.insert(member.clone(), pack_sha.clone());
                }
            }
            for (slot, read) in built.read {
                let (ri, ei) = to_pack[slot];
                let item = &mut scanned[ri].entries[ei];
                let Some((sha, size)) = read else {
                    item.gone = true;
                    continue;
                };
                let entry = &mut item.entry;
                entry.size = size;
                if size == 0 {
                    entry.chunks.clear();
                    continue;
                }
                entry.pack = packed.get(&sha).cloned();
                entry.chunks = vec![sha];
            }
        }

        let mut roots: BTreeMap<String, RootManifest> = carried;
        for root in scanned {
            roots.insert(
                root.spec.name.clone(),
                RootManifest {
                    path: root.spec.path.to_string_lossy().into_owned(),
                    requires: root.spec.requires.clone(),
                    // Files deleted while the checkpoint ran are left out.
                    entries: root
                        .entries
                        .into_iter()
                        .filter(|e| !e.gone)
                        .map(|e| e.entry)
                        .collect(),
                },
            );
        }
        if let Some(last) = &state.last
            && last.roots == roots
        {
            stats.seq = last.seq;
            return Ok(stats);
        }
        let previous = match &state.last {
            Some(last) => last.seq,
            None => store.get_head().await?.map_or(0, |h| h.seq),
        };
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            seq: previous + 1,
            created_at: crate::now_ms(),
            roots,
        };
        store
            .put_manifest(manifest.seq, serde_json::to_vec(&manifest)?)
            .await?;
        store.put_head(Head { seq: manifest.seq }).await?;
        stats.seq = manifest.seq;
        stats.changed = true;
        tracing::info!(seq = stats.seq, objects = stats.objects, bytes = stats.bytes,
            "cloud checkpoint written");
        state.last = Some(manifest);
        Ok(stats)
    }

    async fn fetch_manifest(&self, seq: u64) -> anyhow::Result<Manifest> {
        let bytes = self.inner.store.get_manifest(seq).await?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).with_context(|| format!("manifest {seq} is invalid"))?;
        ensure!(manifest.seq == seq, "manifest {seq} names itself {}", manifest.seq);
        ensure!(
            manifest.version <= MANIFEST_VERSION,
            "manifest {seq} is version {}; this zeron reads up to {MANIFEST_VERSION}",
            manifest.version
        );
        Ok(manifest)
    }

    fn save_index(&self, index: &Index) {
        let path = self.inner.paths.state_dir().join("index.json");
        if let Err(error) = index.save(&path) {
            tracing::warn!(%error, "cloud checkpoint: index not saved");
        }
    }
}

// ── scanning ────────────────────────────────────────────────────────────────

struct ScannedRoot {
    spec: RootSpec,
    entries: Vec<ScanEntry>,
}

struct ScanEntry {
    entry: Entry,
    /// Where the content is read from (a consistent copy for SQLite).
    source: Option<PathBuf>,
    /// Deleted before its content was read.
    gone: bool,
}

impl ScanEntry {
    fn new(entry: Entry, source: Option<PathBuf>) -> Self {
        Self {
            entry,
            source,
            gone: false,
        }
    }
}

fn scan_roots(
    roots: Vec<RootSpec>,
    claims: &[(String, PathBuf)],
    index: &mut Index,
    tmp: &Path,
) -> anyhow::Result<Vec<ScannedRoot>> {
    let _ = std::fs::remove_dir_all(tmp);
    let mut out = Vec::new();
    for spec in roots {
        let claimed: HashSet<PathBuf> = claims
            .iter()
            .filter(|(name, _)| *name != spec.name)
            .map(|(_, path)| path.clone())
            .collect();
        let mut walker = Walker {
            spec: &spec,
            claimed: &claimed,
            index,
            tmp,
            out: Vec::new(),
        };
        walker.walk()?;
        let mut entries = walker.out;
        entries.sort_by(|a, b| a.entry.rel.cmp(&b.entry.rel));
        out.push(ScannedRoot { spec, entries });
    }
    Ok(out)
}

struct Walker<'a> {
    spec: &'a RootSpec,
    claimed: &'a HashSet<PathBuf>,
    index: &'a mut Index,
    tmp: &'a Path,
    out: Vec<ScanEntry>,
}

impl Walker<'_> {
    fn walk(&mut self) -> anyhow::Result<()> {
        let root = self.spec.path.clone();
        match self.spec.kind {
            RootKind::Harness => {
                for rel in HARNESS_PATHS {
                    let abs = root.join(rel);
                    if let Ok(meta) = std::fs::symlink_metadata(&abs) {
                        self.ensure_parents(rel)?;
                        self.visit(rel.to_string(), &abs, &meta);
                    }
                }
            }
            _ => {
                let meta = match std::fs::metadata(&root) {
                    Ok(meta) => meta,
                    // A project folder deleted on the box is simply no longer
                    // there to keep; failing here would stall every
                    // checkpoint after it.
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && matches!(self.spec.kind, RootKind::Workspace) =>
                    {
                        tracing::warn!(root = %root.display(), "checkpoint: workspace folder is gone; skipping it");
                        return Ok(());
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("{} is not readable", root.display()));
                    }
                };
                ensure!(meta.is_dir(), "{} is not a folder", root.display());
                self.children("", &root);
            }
        }
        Ok(())
    }

    /// The folders above a harness path (`.config` for `.config/opencode`),
    /// so a restore recreates them with their modes.
    fn ensure_parents(&mut self, rel: &str) -> anyhow::Result<()> {
        let mut prefix = String::new();
        let parts: Vec<&str> = rel.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if self.out.iter().any(|e| e.entry.rel == prefix) {
                continue;
            }
            let abs = self.spec.path.join(&prefix);
            let meta = std::fs::symlink_metadata(&abs)?;
            self.out
                .push(ScanEntry::new(dir_entry(prefix.clone(), &meta), None));
        }
        Ok(())
    }

    fn children(&mut self, rel: &str, abs: &Path) {
        let entries = match std::fs::read_dir(abs) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(path = %abs.display(), %error, "cloud checkpoint: folder not readable");
                return;
            }
        };
        let mut children: Vec<_> = entries.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name();
            let Some(name) = name.to_str() else {
                tracing::warn!(path = %child.path().display(), "cloud checkpoint: skipping a non-UTF-8 name");
                continue;
            };
            let child_rel = if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            };
            let path = child.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) => self.visit(child_rel, &path, &meta),
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "cloud checkpoint: not readable")
                }
            }
        }
    }

    fn visit(&mut self, rel: String, abs: &Path, meta: &std::fs::Metadata) {
        if self.claimed.contains(abs) || self.excluded(&rel, meta.is_dir()) {
            return;
        }
        let file_type = meta.file_type();
        if file_type.is_dir() {
            self.out
                .push(ScanEntry::new(dir_entry(rel.clone(), meta), None));
            self.children(&rel, abs);
        } else if file_type.is_symlink() {
            let Ok(target) = std::fs::read_link(abs) else {
                return;
            };
            let mut entry = base_entry(rel, EntryKind::Symlink, meta);
            entry.target = Some(target.to_string_lossy().into_owned());
            self.out.push(ScanEntry::new(entry, None));
        } else if file_type.is_file() {
            match self.file(rel, abs, meta) {
                Ok(entry) => self.out.push(entry),
                // Deleted (or unreadable) mid-walk.
                Err(error) => {
                    tracing::debug!(path = %abs.display(), %error, "cloud checkpoint: file skipped")
                }
            }
        }
        // Sockets, FIFOs and devices never travel.
    }

    fn file(&mut self, rel: String, abs: &Path, meta: &std::fs::Metadata) -> anyhow::Result<ScanEntry> {
        let mut entry = base_entry(rel, EntryKind::File, meta);
        let consistent_sqlite = matches!(self.spec.kind, RootKind::Data | RootKind::Harness);
        if consistent_sqlite && is_sqlite(abs) {
            let copy = self.tmp.join(format!("{}.sqlite", self.out.len()));
            std::fs::create_dir_all(self.tmp)?;
            match sqlite_snapshot(abs, &copy) {
                Ok(()) => {
                    entry.size = std::fs::metadata(&copy)?.len();
                    entry.chunks = hash_chunks(&copy, entry.size)?;
                    return Ok(ScanEntry::new(entry, Some(copy)));
                }
                Err(error) => {
                    tracing::warn!(path = %abs.display(), %error,
                        "cloud checkpoint: VACUUM INTO failed; copying the file as it is");
                }
            }
        }
        entry.size = meta.len();
        entry.chunks = match self.index.cached(abs, meta) {
            Some(chunks) => chunks,
            None => {
                let chunks = hash_chunks(abs, entry.size)?;
                self.index.remember(abs, meta, &chunks);
                chunks
            }
        };
        Ok(ScanEntry::new(entry, Some(abs.to_path_buf())))
    }

    fn excluded(&self, rel: &str, is_dir: bool) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        match self.spec.kind {
            RootKind::Data => {
                let top = rel.split('/').next().unwrap_or(rel);
                DATA_EXCLUDES.contains(&top)
                    || (is_dir && (rel.ends_with("moves/in") || rel.ends_with("moves/out")))
                    || (!is_dir && state_file_excluded(name))
            }
            RootKind::Harness => {
                HARNESS_EXCLUDES
                    .iter()
                    .any(|ex| rel == *ex || rel.starts_with(&format!("{ex}/")))
                    || (is_dir && (name == "node_modules" || name == ".cache"))
                    || (!is_dir && state_file_excluded(name))
            }
            RootKind::Workspace | RootKind::Home => {
                // A lock left inside `.git` would wedge the restored repo.
                if !is_dir && name.ends_with(".lock") && in_git_dir(rel) {
                    return true;
                }
                self.spec.kind == RootKind::Home
                    && (HOME_EXCLUDES.contains(&rel)
                        || (is_dir
                            && crate::moves::scope::DEFAULT_HEAVY_DIRS.contains(&name)))
            }
        }
    }
}

fn in_git_dir(rel: &str) -> bool {
    rel.split('/').any(|part| part == ".git")
}

/// Locks, SQLite side files (the snapshot already holds their content) and
/// temp files.
fn state_file_excluded(name: &str) -> bool {
    name.ends_with(".lock")
        || name.ends_with("-wal")
        || name.ends_with("-shm")
        || name.ends_with("-journal")
        || name.ends_with(".tmp")
        || name.starts_with(".tmp")
        || name.contains(".tmp-")
}

fn base_entry(rel: String, kind: EntryKind, meta: &std::fs::Metadata) -> Entry {
    Entry {
        rel,
        kind,
        mode: mode_bits(meta),
        size: 0,
        mtime_ms: mtime_ms(meta),
        target: None,
        chunks: Vec::new(),
        pack: None,
    }
}

fn dir_entry(rel: String, meta: &std::fs::Metadata) -> Entry {
    base_entry(rel, EntryKind::Dir, meta)
}

fn is_sqlite(path: &Path) -> bool {
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"SQLite format 3\0"
}

/// A transactionally consistent copy of a database another connection may
/// be writing (WAL included).
fn sqlite_snapshot(source: &Path, dest: &Path) -> anyhow::Result<()> {
    use rusqlite::OpenFlags;
    let _ = std::fs::remove_file(dest);
    let open = |flags| rusqlite::Connection::open_with_flags(source, flags);
    let conn = open(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .or_else(|_| open(OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])?;
    Ok(())
}

/// The content hashes of a file: whole for a small file, else per
/// [`OBJECT_MAX`] chunk.
fn hash_chunks(path: &Path, size: u64) -> std::io::Result<Vec<String>> {
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut file = std::fs::File::open(path)?;
    let per = if size < SMALL_FILE_MAX { u64::MAX } else { OBJECT_MAX };
    let mut chunks = Vec::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let mut hasher = Sha256::new();
        let mut taken = 0u64;
        while taken < per {
            let want = buffer.len().min((per - taken) as usize);
            let n = file.read(&mut buffer[..want])?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            taken += n as u64;
        }
        if taken == 0 {
            break;
        }
        chunks.push(hex(&hasher.finalize()));
        if taken < per {
            break;
        }
    }
    Ok(chunks)
}

// ── uploading ───────────────────────────────────────────────────────────────

async fn upload_object(
    store: &CheckpointStore,
    sha: &str,
    payload: Vec<u8>,
    known: &mut HashSet<String>,
    stats: &mut CheckpointStats,
) -> anyhow::Result<()> {
    if known.contains(sha) || store.has_object(sha).await? {
        known.insert(sha.to_string());
        return Ok(());
    }
    let compressed = tokio::task::spawn_blocking(move || zstd::bulk::compress(&payload, ZSTD_LEVEL))
        .await
        .context("compression panicked")??;
    stats.bytes += compressed.len() as u64;
    stats.objects += 1;
    store.put_object(sha, compressed).await?;
    known.insert(sha.to_string());
    Ok(())
}

/// Read a large file chunk by chunk, uploading the chunks the store lacks.
/// Returns the chunk list and size as read; `None` if the file is gone.
async fn upload_large(
    store: &CheckpointStore,
    source: &Path,
    known: &mut HashSet<String>,
    stats: &mut CheckpointStats,
) -> anyhow::Result<Option<(Vec<String>, u64)>> {
    let mut chunks = Vec::new();
    let mut offset = 0u64;
    loop {
        let path = source.to_path_buf();
        let read = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut file = std::fs::File::open(&path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut out = Vec::new();
            file.take(OBJECT_MAX).read_to_end(&mut out)?;
            Ok(out)
        })
        .await
        .context("chunk read panicked")?;
        let chunk = match read {
            Ok(chunk) => chunk,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("reading {}", source.display())),
        };
        if chunk.is_empty() {
            break;
        }
        let sha = hex(&Sha256::digest(&chunk));
        let len = chunk.len() as u64;
        upload_object(store, &sha, chunk, known, stats).await?;
        offset += len;
        chunks.push(sha);
        if len < OBJECT_MAX {
            break;
        }
    }
    Ok(Some((chunks, offset)))
}

struct BuiltPack {
    /// `(sha of the tar, the tar)`; `None` when every file read was
    /// already packed (or empty).
    pack: Option<(String, Vec<u8>)>,
    members: Vec<String>,
    /// `(slot, (content sha, size))` for every file read; `None` for a
    /// file deleted since the walk.
    read: Vec<(usize, Option<(String, u64)>)>,
    /// The first slot not read.
    next: usize,
}

/// Read files into one pack until it would pass [`OBJECT_MAX`]. Content
/// already in `seen` (or earlier in this pack) is not added twice.
fn build_pack(items: Vec<(usize, PathBuf)>, seen: &HashSet<String>) -> anyhow::Result<BuiltPack> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut raw = 0u64;
    let mut members: Vec<String> = Vec::new();
    let mut in_pack: HashSet<String> = HashSet::new();
    let mut read = Vec::new();
    let mut next = items.last().map_or(0, |(slot, _)| slot + 1);
    for (slot, path) in items {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                read.push((slot, None));
                continue;
            }
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        if !members.is_empty() && raw + bytes.len() as u64 + 1024 > OBJECT_MAX {
            next = slot;
            break;
        }
        let sha = hex(&Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        if size > 0 && !seen.contains(&sha) && in_pack.insert(sha.clone()) {
            let mut header = tar::Header::new_ustar();
            header.set_size(size);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut header, &sha, bytes.as_slice())?;
            raw += size + 512;
            members.push(sha.clone());
        }
        read.push((slot, Some((sha, size))));
    }
    let tar = builder.into_inner()?;
    let pack = (!members.is_empty()).then(|| (hex(&Sha256::digest(&tar)), tar));
    Ok(BuiltPack {
        pack,
        members,
        read,
        next,
    })
}

// ── restoring ───────────────────────────────────────────────────────────────

/// Download, decompress and verify one object.
async fn fetch_object(store: &CheckpointStore, sha: &str) -> anyhow::Result<Vec<u8>> {
    let compressed = store.get_object(sha).await?;
    let sha_owned = sha.to_string();
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
        let payload = zstd::stream::decode_all(compressed.as_slice())
            .with_context(|| format!("object {sha_owned} is not a zstd frame"))?;
        let actual = hex(&Sha256::digest(&payload));
        ensure!(actual == sha_owned, "object {sha_owned} hashes to {actual}");
        Ok(payload)
    })
    .await
    .context("decompression panicked")?
}

/// Restore a workspace into a sibling folder, then rename it into place.
async fn restore_workspace_root(
    store: &CheckpointStore,
    root: &RootManifest,
    index: &mut Index,
) -> anyhow::Result<u64> {
    let dest = PathBuf::from(&root.path);
    if let Ok(mut existing) = std::fs::read_dir(&dest) {
        if existing.next().is_some() {
            tracing::info!(path = %dest.display(),
                "cloud checkpoint: workspace already present; keeping it");
            return Ok(0);
        }
        std::fs::remove_dir(&dest)?;
    }
    let parent = dest.parent().context("workspace has no parent folder")?;
    std::fs::create_dir_all(parent)?;
    let name = dest
        .file_name()
        .context("workspace has no name")?
        .to_string_lossy()
        .into_owned();
    let staging = parent.join(format!(".{name}.zeron-restore"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    // Hash-cache entries are recorded against the final paths.
    let mut staged = Index::default();
    let files = restore_root(store, root, &staging, &mut staged).await?;
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("moving the restored workspace to {}", dest.display()))?;
    let staging_prefix = staging.to_string_lossy().into_owned();
    for (path, cached) in staged.files {
        if let Some(rest) = path.strip_prefix(&staging_prefix) {
            let path = format!("{}{rest}", dest.to_string_lossy());
            // The rename changed no stamps (same inode, same mtime).
            index.files.insert(path, cached);
        }
    }
    tracing::info!(path = %dest.display(), files, "cloud checkpoint: workspace restored");
    Ok(files)
}

/// Restore one root's entries under `dest`, merging with what's there.
async fn restore_root(
    store: &CheckpointStore,
    root: &RootManifest,
    dest: &Path,
    index: &mut Index,
) -> anyhow::Result<u64> {
    for entry in &root.entries {
        check_rel(&entry.rel)?;
    }
    let mut files = 0;
    // Folders first.
    for entry in root.entries.iter().filter(|e| e.kind == EntryKind::Dir) {
        let path = dest.join(&entry.rel);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.is_dir()) {
            remove_any(&path)?;
        }
        std::fs::create_dir_all(&path)?;
    }
    // Small files, one pack download at a time.
    let mut by_pack: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    let mut own: Vec<&Entry> = Vec::new();
    for entry in root.entries.iter().filter(|e| e.kind == EntryKind::File) {
        match &entry.pack {
            Some(pack) => by_pack.entry(pack.as_str()).or_default().push(entry),
            None => own.push(entry),
        }
    }
    for (pack, entries) in by_pack {
        let tar = fetch_object(store, pack).await?;
        let mut members = unpack(&tar).with_context(|| format!("pack {pack}"))?;
        for entry in entries {
            let sha = entry.chunks.first().context("packed entry without a hash")?;
            let bytes = members
                .get(sha.as_str())
                .with_context(|| format!("pack {pack} lacks {sha}"))?;
            write_file(dest, entry, &[bytes.as_slice()], index)?;
            files += 1;
        }
        members.clear();
    }
    // Large files and content stored as objects of its own.
    for entry in own {
        let mut parts = Vec::with_capacity(entry.chunks.len());
        for sha in &entry.chunks {
            parts.push(fetch_object(store, sha).await?);
        }
        let slices: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        write_file(dest, entry, &slices, index)?;
        files += 1;
    }
    for entry in root.entries.iter().filter(|e| e.kind == EntryKind::Symlink) {
        write_symlink(dest, entry)?;
    }
    // Folder modes and times last (writing inside a folder changes its
    // mtime), deepest first.
    let mut dirs: Vec<&Entry> = root
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::Dir)
        .collect();
    dirs.sort_by_key(|e| std::cmp::Reverse(e.rel.matches('/').count()));
    for entry in dirs {
        let path = dest.join(&entry.rel);
        set_mode(&path, entry.mode)?;
        filetime::set_file_mtime(&path, file_time(entry.mtime_ms))?;
    }
    Ok(files)
}

fn unpack(tar: &[u8]) -> anyhow::Result<HashMap<String, Vec<u8>>> {
    let mut out = HashMap::new();
    let mut archive = tar::Archive::new(tar);
    for member in archive.entries()? {
        let mut member = member?;
        let name = member.path()?.to_string_lossy().into_owned();
        let mut bytes = Vec::with_capacity(member.size() as usize);
        member.read_to_end(&mut bytes)?;
        let actual = hex(&Sha256::digest(&bytes));
        ensure!(actual == name, "member {name} hashes to {actual}");
        out.insert(name, bytes);
    }
    Ok(out)
}

fn write_file(
    dest: &Path,
    entry: &Entry,
    parts: &[&[u8]],
    index: &mut Index,
) -> anyhow::Result<()> {
    let path = dest.join(&entry.rel);
    let total: u64 = parts.iter().map(|p| p.len() as u64).sum();
    ensure!(
        total == entry.size,
        "{}: {} bytes restored, {} expected",
        entry.rel,
        total,
        entry.size
    );
    let parent = path.parent().context("file without a parent")?;
    std::fs::create_dir_all(parent)?;
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
        remove_any(&path)?;
    }
    let temp = parent.join(format!(
        ".{}.zeron-restore-{}",
        path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    {
        let mut file = std::fs::File::create(&temp)?;
        for part in parts {
            file.write_all(part)?;
        }
        file.sync_all()?;
    }
    set_mode(&temp, entry.mode)?;
    std::fs::rename(&temp, &path)?;
    filetime::set_file_mtime(&path, file_time(entry.mtime_ms))?;
    if let Ok(meta) = std::fs::metadata(&path) {
        index.remember(&path, &meta, &entry.chunks);
    }
    Ok(())
}

fn write_symlink(dest: &Path, entry: &Entry) -> anyhow::Result<()> {
    let path = dest.join(&entry.rel);
    let target = entry.target.as_deref().context("symlink without a target")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::symlink_metadata(&path).is_ok() {
        remove_any(&path)?;
    }
    std::os::unix::fs::symlink(target, &path)?;
    filetime::set_symlink_file_times(
        &path,
        file_time(entry.mtime_ms),
        file_time(entry.mtime_ms),
    )?;
    Ok(())
}

/// `path` with its longest existing ancestor canonicalized (the rest may
/// not exist yet).
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(current) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Reject entries that would land outside their root.
fn check_rel(rel: &str) -> anyhow::Result<()> {
    let path = Path::new(rel);
    if rel.is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("manifest entry {rel:?} escapes its root");
    }
    Ok(())
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))
}

fn mode_bits(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o7777
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn file_time(ms: i64) -> filetime::FileTime {
    filetime::FileTime::from_unix_time(ms.div_euclid(1000), (ms.rem_euclid(1000) * 1_000_000) as u32)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().context("no parent folder")?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
