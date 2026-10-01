//! The workspace file set a move carries (plan: "The transfer plan").
//!
//! A moved chat must find its workspace on the target exactly as it left it,
//! including what git doesn't track: untracked files and git-ignored local
//! state such as `.env`. So the scope is *everything* under the workspace root
//! except things that are either not files at all (sockets, FIFOs, devices),
//! cheap to regenerate and expensive to copy (`node_modules`, `target`, …),
//! git's own metadata (`.git`, which travels as a bundle instead), secret
//! stores, or individually huge files that need an explicit confirmation.
//!
//! **Both sides walk.** The source walks its workspace and the target walks
//! its landing with the *same* [`ScopeRules`]; the two file sets line up, so
//! the sync can tell "unchanged", "changed" and "deleted" apart and never
//! touches a heavy folder the target regenerated on its own. Every exclusion
//! therefore depends only on what travels with the tree:
//!
//! - heavy folders are recognised by name, and a heavy-named folder holding
//!   *tracked* files is never pruned wholesale (only its untracked content
//!   is). Tracked-ness comes from the index, which the move lands on the
//!   target (`git::apply_index`) before the target walks;
//! - `ignored` is classified from the `.gitignore` files *inside* the tree.
//!   Global excludes and `.git/info/exclude` are device-local and would make
//!   the two sides disagree, so they are deliberately not consulted. The flag
//!   is informational: ignored files are included.
//!
//! A walk that hits [`ScopeRules::max_files`] or [`ScopeRules::max_total_bytes`]
//! stops early and reports `truncated`; its file list is partial and not
//! deterministic, so the caller must reduce the scope (or raise the guard
//! after the user confirms) rather than sync it.
//!
//! [`walk`] is blocking (it stats every file); async callers use
//! [`walk_async`], which runs it on the blocking pool. It walks in parallel
//! and stats each entry once: a warm 100k-file repository takes ~0.4–0.5 s
//! in a release build on an M-series Mac (~0.1 s `git ls-files` + index set,
//! ~0.25 s walk, ~0.1 s ignore classification).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use anyhow::Context;

use crate::repos::git_std_command;

/// Regenerable folders that are never worth copying. Single names match a
/// folder with that name at any depth; entries with a `/` match that path
/// suffix (`vendor/bundle` but not a lone `vendor`).
pub const DEFAULT_HEAVY_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".turbo",
    ".gradle",
    ".cache",
    "DerivedData",
    "Pods",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "coverage",
    ".parcel-cache",
    ".svelte-kit",
    "vendor/bundle",
];

/// A single file above this needs the scout's or the user's confirmation.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// The whole-tree size guard.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// A workspace with more files than this is "unreasonably large" (plan) and
/// gets reduced to the files the session actually used.
pub const DEFAULT_MAX_FILES: usize = 250_000;

/// What the walk leaves out. Both sides of a move must use equal rules
/// (except `secret_home`, which is each device's own home).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRules {
    /// See [`DEFAULT_HEAVY_DIRS`].
    pub heavy_dirs: Vec<String>,
    /// Files larger than this are excluded as [`ExcludeReason::TooLarge`].
    pub max_file_bytes: u64,
    /// Stop (and report `truncated`) once included bytes exceed this.
    pub max_total_bytes: u64,
    /// Stop (and report `truncated`) once included entries exceed this.
    pub max_files: usize,
    /// This device's home. Secret stores under it ([`is_secret_store`]) are
    /// never walked into, e.g. when the workspace *is* the home folder.
    pub secret_home: Option<PathBuf>,
}

impl Default for ScopeRules {
    fn default() -> Self {
        Self {
            heavy_dirs: DEFAULT_HEAVY_DIRS.iter().map(|s| s.to_string()).collect(),
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_files: DEFAULT_MAX_FILES,
            secret_home: Some(crate::repos::home_dir()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    File,
    /// Recorded as a link (never followed); the sync reads its target.
    Symlink,
    /// Only *empty* directories are listed; every other directory is implied
    /// by the paths beneath it.
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeFile {
    /// `/`-separated, relative to the walk root.
    pub rel: String,
    pub abs: PathBuf,
    pub kind: ScopeKind,
    /// Bytes for files; the link's own size for symlinks; 0 for directories.
    pub size: u64,
    /// Modification time, ms since the Unix epoch (0 if unavailable).
    pub mtime_ms: i64,
    /// Unix permission bits (`0o7777` mask); synthesised on Windows.
    pub mode: u32,
    /// Matched by a `.gitignore` inside the tree (included regardless).
    pub ignored: bool,
    /// In the git index.
    pub tracked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExcludeReason {
    /// A regenerable heavy folder (or untracked content inside one).
    Heavy,
    /// Over [`ScopeRules::max_file_bytes`].
    TooLarge,
    /// Socket, FIFO, or device node.
    Special,
    /// Could not be listed or stat'ed.
    Unreadable,
    /// A credential store ([`is_secret_store`]).
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excluded {
    pub rel: String,
    pub reason: ExcludeReason,
    /// Known for files; `None` for pruned folders (not walked, so unknown).
    pub bytes_estimate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub root: PathBuf,
    /// Sorted by `rel`.
    pub files: Vec<ScopeFile>,
    /// Sorted by `rel`.
    pub excluded: Vec<Excluded>,
    /// Sum of included regular-file sizes.
    pub total_bytes: u64,
    /// The walk stopped at a guard; `files` is partial.
    pub truncated: bool,
}

/// The workspace root for a chat whose cwd is `cwd`: the git toplevel that
/// contains it (for a linked worktree, that worktree's own toplevel, not the
/// main checkout), else `cwd` itself. Blocking (spawns `git`).
pub fn workspace_root(cwd: &Path) -> PathBuf {
    let output = git_std_command()
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if top.is_empty() {
                cwd.to_path_buf()
            } else {
                PathBuf::from(top)
            }
        }
        _ => cwd.to_path_buf(),
    }
}

/// [`walk`] on the blocking pool.
pub async fn walk_async(root: PathBuf, rules: ScopeRules) -> anyhow::Result<Scope> {
    tokio::task::spawn_blocking(move || walk(&root, &rules))
        .await
        .context("workspace walk panicked")?
}

/// Walk `root` and classify every entry (see the module docs). Blocking.
pub fn walk(root: &Path, rules: &ScopeRules) -> anyhow::Result<Scope> {
    let meta = std::fs::metadata(root)
        .with_context(|| format!("workspace {} is not readable", root.display()))?;
    anyhow::ensure!(
        meta.is_dir(),
        "workspace {} is not a folder",
        root.display()
    );
    if let Some(home) = &rules.secret_home {
        anyhow::ensure!(
            !is_secret_store(root, home),
            "workspace {} is a credential store",
            root.display()
        );
    }

    let tracked = tracked_files(root);
    let mut tracked_dirs: HashSet<&str> = HashSet::new();
    for path in &tracked {
        let mut rest = path.as_str();
        while let Some((parent, _)) = rest.rsplit_once('/') {
            if !tracked_dirs.insert(parent) {
                break;
            }
            rest = parent;
        }
    }
    let heavy = HeavyMatcher::new(&rules.heavy_dirs);
    let secrets = rules
        .secret_home
        .as_deref()
        .map(|home| SecretPaths::under(root, home));

    let sink = Mutex::new(Collected::default());
    let count = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let truncated = AtomicBool::new(false);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(12))
        .unwrap_or(4);

    ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .threads(threads)
        .build_parallel()
        .run(|| {
            let mut local = Flush {
                items: Collected::default(),
                sink: &sink,
            };
            let ctx = VisitCtx {
                root,
                rules,
                tracked: &tracked,
                tracked_dirs: &tracked_dirs,
                heavy: &heavy,
                secrets: secrets.as_ref(),
                count: &count,
                bytes: &bytes,
                truncated: &truncated,
            };
            Box::new(move |entry| ctx.visit(entry, &mut local.items))
        });

    let mut collected = sink
        .into_inner()
        .map_err(|_| anyhow::anyhow!("workspace walk poisoned"))?;
    let truncated = truncated.load(Ordering::Relaxed);

    // Empty folders: listed directories nothing else names as a parent.
    let mut parents: HashSet<&str> = HashSet::new();
    for rel in collected
        .files
        .iter()
        .map(|f| f.rel.as_str())
        .chain(collected.excluded.iter().map(|e| e.rel.as_str()))
        .chain(collected.dirs.iter().map(String::as_str))
    {
        if let Some((parent, _)) = rel.rsplit_once('/') {
            parents.insert(parent);
        }
    }
    let excluded_rels: HashSet<&str> = collected.excluded.iter().map(|e| e.rel.as_str()).collect();
    let empty_dirs: Vec<String> = collected
        .dirs
        .iter()
        .filter(|d| !parents.contains(d.as_str()) && !excluded_rels.contains(d.as_str()))
        .cloned()
        .collect();
    drop(parents);
    drop(excluded_rels);
    for rel in empty_dirs {
        let abs = root.join(&rel);
        let meta = std::fs::symlink_metadata(&abs).ok();
        collected.files.push(ScopeFile {
            mtime_ms: meta.as_ref().map(mtime_ms).unwrap_or(0),
            mode: meta.as_ref().map(mode_bits).unwrap_or(0o755),
            tracked: false,
            ignored: false,
            size: 0,
            kind: ScopeKind::Dir,
            abs,
            rel,
        });
    }

    let mut files = collected.files;
    files.sort_unstable_by(|a, b| a.rel.cmp(&b.rel));
    let mut ignores = IgnoreStack::new(root, &files);
    for file in &mut files {
        file.ignored = ignores.is_ignored(&file.rel, file.kind == ScopeKind::Dir);
    }
    let mut excluded = collected.excluded;
    excluded.sort_unstable_by(|a, b| a.rel.cmp(&b.rel));
    let total_bytes = files
        .iter()
        .filter(|f| f.kind == ScopeKind::File)
        .map(|f| f.size)
        .sum();
    Ok(Scope {
        root: root.to_path_buf(),
        files,
        excluded,
        total_bytes,
        truncated,
    })
}

#[derive(Default)]
struct Collected {
    files: Vec<ScopeFile>,
    excluded: Vec<Excluded>,
    /// Every directory walked into (for empty-folder detection).
    dirs: Vec<String>,
}

/// Per-walker-thread buffer, merged into the shared sink when the thread's
/// visitor is dropped (one lock per thread instead of one per entry).
struct Flush<'a> {
    items: Collected,
    sink: &'a Mutex<Collected>,
}

impl Drop for Flush<'_> {
    fn drop(&mut self) {
        if let Ok(mut all) = self.sink.lock() {
            all.files.append(&mut self.items.files);
            all.excluded.append(&mut self.items.excluded);
            all.dirs.append(&mut self.items.dirs);
        }
    }
}

struct VisitCtx<'a> {
    root: &'a Path,
    rules: &'a ScopeRules,
    tracked: &'a HashSet<String>,
    tracked_dirs: &'a HashSet<&'a str>,
    heavy: &'a HeavyMatcher,
    secrets: Option<&'a SecretPaths>,
    count: &'a AtomicUsize,
    bytes: &'a AtomicU64,
    truncated: &'a AtomicBool,
}

impl VisitCtx<'_> {
    fn visit(
        &self,
        entry: Result<ignore::DirEntry, ignore::Error>,
        out: &mut Collected,
    ) -> ignore::WalkState {
        use ignore::WalkState::{Continue, Quit, Skip};
        if self.truncated.load(Ordering::Relaxed) {
            return Quit;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                if let Some(rel) = error_path(&err).and_then(|p| self.rel(p)) {
                    out.excluded.push(Excluded {
                        rel,
                        reason: ExcludeReason::Unreadable,
                        bytes_estimate: None,
                    });
                }
                return Continue;
            }
        };
        if entry.depth() == 0 {
            return Continue;
        }
        if entry.file_name() == ".git" {
            // The repository (or a nested one) travels as git, not as files.
            return Skip;
        }
        let path = entry.path();
        let Some(rel) = self.rel(path) else {
            return Continue;
        };
        let Some(file_type) = entry.file_type() else {
            return Continue;
        };
        let secret = self.secrets.is_some_and(|s| s.contains(path));
        if file_type.is_dir() {
            if secret {
                out.excluded
                    .push(excluded(rel, ExcludeReason::Secret, None));
                return Skip;
            }
            let heavy = self.heavy.matches(&rel) || self.heavy.inside(&rel);
            if heavy && !self.tracked_dirs.contains(rel.as_str()) {
                out.excluded.push(excluded(rel, ExcludeReason::Heavy, None));
                return Skip;
            }
            out.dirs.push(rel);
            return Continue;
        }
        let tracked = self.tracked.contains(&rel);
        if secret {
            out.excluded
                .push(excluded(rel, ExcludeReason::Secret, None));
            return Continue;
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => {
                out.excluded
                    .push(excluded(rel, ExcludeReason::Unreadable, None));
                return Continue;
            }
        };
        if !tracked && self.heavy.inside(&rel) {
            out.excluded
                .push(excluded(rel, ExcludeReason::Heavy, Some(meta.len())));
            return Continue;
        }
        let kind = if file_type.is_symlink() {
            ScopeKind::Symlink
        } else if file_type.is_file() {
            ScopeKind::File
        } else {
            out.excluded
                .push(excluded(rel, ExcludeReason::Special, None));
            return Continue;
        };
        let size = meta.len();
        if kind == ScopeKind::File && size > self.rules.max_file_bytes {
            out.excluded
                .push(excluded(rel, ExcludeReason::TooLarge, Some(size)));
            return Continue;
        }
        let total = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        let total_bytes = if kind == ScopeKind::File {
            self.bytes.fetch_add(size, Ordering::Relaxed) + size
        } else {
            self.bytes.load(Ordering::Relaxed)
        };
        if total > self.rules.max_files || total_bytes > self.rules.max_total_bytes {
            self.truncated.store(true, Ordering::Relaxed);
            return Quit;
        }
        out.files.push(ScopeFile {
            abs: path.to_path_buf(),
            kind,
            size,
            mtime_ms: mtime_ms(&meta),
            mode: mode_bits(&meta),
            ignored: false,
            tracked,
            rel,
        });
        Continue
    }

    fn rel(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.root).ok()?;
        let mut out = String::new();
        for component in rel.components() {
            if let Component::Normal(part) = component {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&part.to_string_lossy());
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

fn excluded(rel: String, reason: ExcludeReason, bytes_estimate: Option<u64>) -> Excluded {
    Excluded {
        rel,
        reason,
        bytes_estimate,
    }
}

fn error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Partial(errs) => errs.iter().find_map(error_path),
        ignore::Error::Loop { child, .. } => Some(child),
        _ => None,
    }
}

/// Paths in the index under `root`, relative to it (`--recurse-submodules`
/// lists initialised submodules' files too). Empty outside a repository.
fn tracked_files(root: &Path) -> HashSet<String> {
    let run = |recurse: bool| {
        let mut cmd = git_std_command();
        cmd.args(["ls-files", "-z", "--cached"]);
        if recurse {
            cmd.arg("--recurse-submodules");
        }
        cmd.current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
    };
    let Some(output) = run(true).or_else(|| run(false)) else {
        return HashSet::new();
    };
    output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

struct HeavyMatcher {
    names: HashSet<String>,
    suffixes: Vec<String>,
}

impl HeavyMatcher {
    fn new(patterns: &[String]) -> Self {
        let mut names = HashSet::new();
        let mut suffixes = Vec::new();
        for pattern in patterns {
            let pattern = pattern.trim_matches('/');
            if pattern.is_empty() {
                continue;
            }
            if pattern.contains('/') {
                suffixes.push(pattern.to_string());
            } else {
                names.insert(pattern.to_string());
            }
        }
        Self { names, suffixes }
    }

    /// Is the folder at `rel` itself heavy-named?
    fn matches(&self, rel: &str) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        self.names.contains(name)
            || self.suffixes.iter().any(|suffix| {
                rel == suffix
                    || (rel.len() > suffix.len()
                        && rel.ends_with(suffix.as_str())
                        && rel.as_bytes()[rel.len() - suffix.len() - 1] == b'/')
            })
    }

    /// Is `rel` strictly inside a heavy-named folder? (Only reachable when
    /// that folder holds tracked files; otherwise it was pruned.)
    fn inside(&self, rel: &str) -> bool {
        rel.match_indices('/')
            .any(|(end, _)| self.matches(&rel[..end]))
    }
}

/// `.gitignore` semantics over the walked tree: a path is ignored when a
/// parent folder is, else the deepest `.gitignore` with a matching rule
/// decides (a `!negation` re-includes).
struct IgnoreStack {
    root: PathBuf,
    matchers: HashMap<String, ignore::gitignore::Gitignore>,
    dirs: HashMap<String, bool>,
}

impl IgnoreStack {
    fn new(root: &Path, files: &[ScopeFile]) -> Self {
        let mut matchers = HashMap::new();
        for file in files {
            let (dir, name) = match file.rel.rsplit_once('/') {
                Some((dir, name)) => (dir, name),
                None => ("", file.rel.as_str()),
            };
            if name != ".gitignore" || file.kind != ScopeKind::File {
                continue;
            }
            let base = if dir.is_empty() {
                root.to_path_buf()
            } else {
                root.join(dir)
            };
            let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
            let _ = builder.add(&file.abs);
            if let Ok(matcher) = builder.build()
                && !matcher.is_empty()
            {
                matchers.insert(dir.to_string(), matcher);
            }
        }
        Self {
            root: root.to_path_buf(),
            matchers,
            dirs: HashMap::new(),
        }
    }

    fn is_ignored(&mut self, rel: &str, is_dir: bool) -> bool {
        if self.matchers.is_empty() {
            return false;
        }
        if let Some((parent, _)) = rel.rsplit_once('/')
            && self.dir_ignored(parent)
        {
            return true;
        }
        self.matches(rel, is_dir)
    }

    fn dir_ignored(&mut self, rel: &str) -> bool {
        if let Some(known) = self.dirs.get(rel) {
            return *known;
        }
        let ignored = self.is_ignored(rel, true);
        self.dirs.insert(rel.to_string(), ignored);
        ignored
    }

    fn matches(&self, rel: &str, is_dir: bool) -> bool {
        let abs = self.root.join(rel);
        let mut dir = rel;
        loop {
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
            if let Some(matcher) = self.matchers.get(dir) {
                let found = matcher.matched(&abs, is_dir);
                if found.is_ignore() {
                    return true;
                }
                if found.is_whitelist() {
                    return false;
                }
            }
            if dir.is_empty() {
                return false;
            }
        }
    }
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn mode_bits(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_bits(meta: &std::fs::Metadata) -> u32 {
    match (meta.is_dir(), meta.permissions().readonly()) {
        (true, _) => 0o755,
        (false, true) => 0o444,
        (false, false) => 0o644,
    }
}

// ── secret stores ───────────────────────────────────────────────────────────

/// Home-relative credential stores: folders (everything inside counts) and
/// single files. Includes the coding harnesses' own login files.
const HOME_SECRET_STORES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".kube",
    ".password-store",
    ".local/share/keyrings",
    "Library/Keychains",
    ".config/gh/hosts.yml",
    ".config/hub",
    ".config/op",
    ".config/git/credentials",
    ".docker/config.json",
    ".netrc",
    "_netrc",
    ".npmrc",
    ".yarnrc.yml",
    ".pypirc",
    ".pgpass",
    ".my.cnf",
    ".s3cfg",
    ".boto",
    ".vault-token",
    ".git-credentials",
    ".gem/credentials",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".terraform.d/credentials.tfrc.json",
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".cursor/sdk/auth.json",
    ".grok/auth.json",
    ".pi/agent/auth.json",
    ".hermes/auth.json",
    ".gemini/oauth_creds.json",
    ".local/share/devin/credentials.toml",
    ".local/share/opencode/auth.json",
    "AppData/Roaming/gcloud",
    "AppData/Roaming/Microsoft/Credentials",
    "AppData/Local/Microsoft/Credentials",
    "AppData/Roaming/Microsoft/Protect",
];

/// Absolute system locations holding credentials.
const SYSTEM_SECRET_STORES: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/etc/ssh",
    "/private/etc/ssh",
    "/private/etc/sudoers",
    "/Library/Keychains",
    "/var/root",
];

/// File names that are private keys wherever they live.
const SECRET_FILE_NAMES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ecdsa_sk",
    "id_ed25519_sk",
];

/// Is `path` a credential store, or inside one? A move never carries these
/// (`~/.ssh`, `~/.aws`, keychains, `~/.netrc`, harness login files, …), even
/// when an agent touched them. Checked lexically and, when the path exists,
/// again after resolving symlinks, so a link into `~/.ssh` is caught too.
pub fn is_secret_store(path: &Path, home: &Path) -> bool {
    if secret_store_lexical(path, home) {
        return true;
    }
    match (std::fs::canonicalize(path), std::fs::canonicalize(home)) {
        (Ok(real), Ok(real_home)) => secret_store_lexical(&real, &real_home),
        (Ok(real), Err(_)) => secret_store_lexical(&real, home),
        _ => false,
    }
}

/// The secret stores a walk of `root` could run into, resolved up front so
/// the per-entry check is a set lookup plus a file-name test.
struct SecretPaths {
    paths: HashSet<PathBuf>,
}

impl SecretPaths {
    fn under(root: &Path, home: &Path) -> Self {
        let mut homes = vec![home.to_path_buf()];
        if let Ok(real) = std::fs::canonicalize(home)
            && real != home
        {
            homes.push(real);
        }
        let mut paths = HashSet::new();
        for home in &homes {
            for store in HOME_SECRET_STORES {
                let path = home.join(store);
                if path.starts_with(root) {
                    paths.insert(path);
                }
            }
        }
        for store in SYSTEM_SECRET_STORES {
            let path = PathBuf::from(store);
            if path.starts_with(root) {
                paths.insert(path);
            }
        }
        Self { paths }
    }

    fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
            || path
                .file_name()
                .map(|name| name.to_string_lossy())
                .is_some_and(|name| secret_file_name(&name))
    }
}

fn secret_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_FILE_NAMES.iter().any(|s| eq_name(name, s))
        || lower.ends_with(".keychain")
        || lower.ends_with(".keychain-db")
}

fn secret_store_lexical(path: &Path, home: &Path) -> bool {
    let path = normalize_components(path);
    if path.last().is_some_and(|name| secret_file_name(name)) {
        return true;
    }
    let home = normalize_components(home);
    if !home.is_empty() && starts_with(&path, &home) {
        let rel = &path[home.len()..];
        if HOME_SECRET_STORES
            .iter()
            .any(|store| starts_with(rel, &split_store(store)))
        {
            return true;
        }
    }
    SYSTEM_SECRET_STORES
        .iter()
        .any(|store| starts_with(&path, &normalize_components(Path::new(store))))
}

fn split_store(store: &str) -> Vec<String> {
    store.split('/').map(str::to_string).collect()
}

/// Lexical normal form: `.` dropped, `..` popped, prefix/root kept as one
/// leading element so different drives never compare equal.
fn normalize_components(path: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                out.push(prefix.as_os_str().to_string_lossy().to_ascii_lowercase())
            }
            Component::RootDir => out.push("/".into()),
            Component::CurDir => {}
            Component::ParentDir => {
                if out
                    .last()
                    .is_some_and(|last| last != "/" && !last.ends_with(':'))
                {
                    out.pop();
                }
            }
            Component::Normal(part) => out.push(part.to_string_lossy().into_owned()),
        }
    }
    out
}

fn starts_with(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| eq_name(a, b))
}

/// macOS and Windows file systems are case-insensitive by default.
fn eq_name(a: &str, b: &str) -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heavy_matcher_names_and_suffixes() {
        let heavy = HeavyMatcher::new(&["node_modules".into(), "vendor/bundle".into()]);
        assert!(heavy.matches("node_modules"));
        assert!(heavy.matches("a/b/node_modules"));
        assert!(!heavy.matches("node_modules_x"));
        assert!(heavy.matches("vendor/bundle"));
        assert!(heavy.matches("app/vendor/bundle"));
        assert!(!heavy.matches("vendor"));
        assert!(!heavy.matches("xvendor/bundle"));
        assert!(heavy.inside("node_modules/x/y.js"));
        assert!(heavy.inside("app/vendor/bundle/gems/a"));
        assert!(!heavy.inside("src/node_modules"));
    }

    #[test]
    fn secret_store_paths() {
        let home = Path::new("/home/u");
        for secret in [
            "/home/u/.ssh",
            "/home/u/.ssh/config",
            "/home/u/.aws/credentials",
            "/home/u/.config/gcloud/application_default_credentials.json",
            "/home/u/.netrc",
            "/home/u/.docker/config.json",
            "/home/u/.claude/.credentials.json",
            "/home/u/.codex/auth.json",
            "/home/u/proj/../.ssh/id_rsa",
            "/home/u/Library/Keychains/login.keychain-db",
            "/tmp/id_ed25519",
            "/etc/ssh/ssh_host_rsa_key",
        ] {
            assert!(is_secret_store(Path::new(secret), home), "{secret}");
        }
        for fine in [
            "/home/u",
            "/home/u/.config",
            "/home/u/.claude/projects/x.jsonl",
            "/home/u/.codex/sessions",
            "/home/u/.docker/contexts",
            "/home/u/dev/app/.env",
            "/home/u/Desktop/shot.png",
            "/home/u/.sshx",
            "/other/.ssh",
        ] {
            assert!(!is_secret_store(Path::new(fine), home), "{fine}");
        }
    }
}
