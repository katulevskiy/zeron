//! Working-tree snapshots for the "Latest turn" scope, kept out of the
//! checkout's object database.
//!
//! A snapshot is `git add -A` into a throwaway index followed by `git
//! write-tree`, and `git add` writes a blob for every file it hashes. Those
//! blobs used to land in the checkout's own `.git/objects`, where nothing ever
//! referenced them. A checkout with an unignored build directory collected
//! ~20 GB of loose objects that way, and git's auto-maintenance (run after
//! every commit, merge and fetch) then tried to delta-compress them: one
//! multi-gigabyte `pack-objects` per agent commit, 23 at once on the machine
//! that reported it.
//!
//! Two rules keep that from happening again:
//!
//! - **Zeron's objects stay in Zeron's directory.** Each snapshot writes into
//!   its own object directory under the data dir (`GIT_OBJECT_DIRECTORY`) and
//!   reads the checkout's objects as an alternate. The checkout's database is
//!   never written to, and the directory is deleted with the snapshot.
//! - **The work is bounded.** Untracked files past [`MAX_UNTRACKED_FILE_BYTES`]
//!   and untracked directories past the untracked budget are left out, and the
//!   whole snapshot runs under [`SNAPSHOT_TIMEOUT`]. A left-out path simply
//!   never shows in the turn diff; tracked files are always snapshotted.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{MAX_PATH_ARGUMENT_BYTES, capture_git, path_argument, path_batches};
use crate::EngineError;
use crate::repos::Repos;

/// Snapshot stores live in `{data_dir}/turn-snapshots/{run}/{snapshot}`.
const STORE_DIR: &str = "turn-snapshots";
/// Largest untracked file a snapshot carries. The Changes pane cannot render
/// more than [`super::MAX_DIFF_SOURCE_BYTES`] of a file anyway; the headroom
/// keeps ordinary binary assets (screenshots, fonts) in the file list.
const MAX_UNTRACKED_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Budget for untracked content, applied to each untracked directory on its
/// own and then to everything that is left. Source-sized trees fit with room
/// to spare; build output, dependency trees and datasets do not.
const MAX_UNTRACKED_FILES: usize = 10_000;
const MAX_UNTRACKED_BYTES: u64 = 256 * 1024 * 1024;
/// Cap on one untracked listing from git.
const MAX_UNTRACKED_LISTING_BYTES: usize = 8 * 1024 * 1024;
/// Backstop for a whole snapshot (a wedged mount, a hung clean filter). The
/// budgets above keep a healthy checkout far below it.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(windows)]
const ALTERNATES_SEPARATOR: char = ';';
#[cfg(not(windows))]
const ALTERNATES_SEPARATOR: char = ':';

/// A working tree written as a git tree, together with the private object
/// store that holds it. Cloning shares the store; the last clone to drop
/// deletes it.
#[derive(Debug, Clone)]
pub struct TreeSnapshot {
    tree: String,
    store: Arc<ObjectStore>,
}

impl TreeSnapshot {
    /// `git write-tree` sha of the tracked + untracked (unignored) tree.
    pub fn tree(&self) -> &str {
        &self.tree
    }

    /// Point a git command at this snapshot's objects. Without it the tree
    /// does not exist as far as the checkout is concerned.
    pub(super) fn apply_to(&self, cmd: &mut tokio::process::Command) {
        self.store.apply_to(cmd);
    }
}

/// One snapshot's object directory.
#[derive(Debug)]
struct ObjectStore {
    dir: PathBuf,
    /// `GIT_ALTERNATE_OBJECT_DIRECTORIES`: the checkout's objects, behind the
    /// base snapshot's when there is one.
    alternates: OsString,
    /// A layered snapshot reads through its base's directory, so the base
    /// has to outlive it.
    _base: Option<Arc<ObjectStore>>,
}

impl ObjectStore {
    async fn create(
        repos: &Repos,
        root: &Path,
        base: Option<&Arc<ObjectStore>>,
    ) -> Result<Self, EngineError> {
        let alternates = match base {
            Some(base) => {
                let mut value = alternates_value(&[&base.objects_dir()]);
                value.push(ALTERNATES_SEPARATOR.to_string());
                value.push(&base.alternates);
                value
            }
            None => {
                // `--git-path` resolves linked worktrees to the shared database.
                let listed = capture_git(
                    root,
                    &[
                        "rev-parse",
                        "--path-format=absolute",
                        "--git-path",
                        "objects",
                    ],
                    16 * 1024,
                )
                .await?;
                let mut path = listed.stdout.as_slice();
                while let Some(rest) = path
                    .strip_suffix(b"\n")
                    .or_else(|| path.strip_suffix(b"\r"))
                {
                    path = rest;
                }
                if path.is_empty() {
                    return Err(EngineError::Other("git object directory unknown".into()));
                }
                let mut value = alternates_value(&[Path::new(&path_argument(path))]);
                // A checkout that already borrows objects this way keeps doing so.
                if let Some(inherited) = std::env::var_os("GIT_ALTERNATE_OBJECT_DIRECTORIES")
                    .filter(|inherited| !inherited.is_empty())
                {
                    value.push(ALTERNATES_SEPARATOR.to_string());
                    value.push(inherited);
                }
                value
            }
        };
        let dir = store_root(repos.data_dir()).join(uuid::Uuid::new_v4().to_string());
        let objects = dir.join("objects");
        tokio::fs::create_dir_all(&objects)
            .await
            .map_err(|error| EngineError::Other(format!("turn snapshot store: {error}")))?;
        Ok(Self {
            dir,
            alternates,
            _base: base.cloned(),
        })
    }

    fn objects_dir(&self) -> PathBuf {
        self.dir.join("objects")
    }

    fn apply_to(&self, cmd: &mut tokio::process::Command) {
        cmd.env("GIT_OBJECT_DIRECTORY", self.objects_dir());
        cmd.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &self.alternates);
    }
}

impl Drop for ObjectStore {
    fn drop(&mut self) {
        let dir = std::mem::take(&mut self.dir);
        // A store can hold thousands of loose objects: delete it off the
        // runtime's worker threads.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(move || {
                    let _ = std::fs::remove_dir_all(dir);
                });
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }
}

/// Encode directories for `GIT_ALTERNATE_OBJECT_DIRECTORIES`. Git splits the
/// value on the platform's path-list separator, and reads an entry that
/// starts with `"` as a C-quoted path, which is the only way to name a
/// directory containing that separator.
fn alternates_value(dirs: &[&Path]) -> OsString {
    let mut value = OsString::new();
    for (index, dir) in dirs.iter().enumerate() {
        if index > 0 {
            value.push(ALTERNATES_SEPARATOR.to_string());
        }
        match dir.to_str() {
            Some(text) if text.contains(ALTERNATES_SEPARATOR) || text.starts_with('"') => {
                value.push("\"");
                value.push(text.replace('\\', "\\\\").replace('"', "\\\""));
                value.push("\"");
            }
            _ => value.push(dir.as_os_str()),
        }
    }
    value
}

/// This process's stores. An engine holds its data dir exclusively
/// ([`crate::InstanceLock`]), so any sibling directory was left by a run that
/// is gone — see [`sweep_stale_stores`].
fn store_root(data_dir: &Path) -> PathBuf {
    data_dir.join(STORE_DIR).join(run_id())
}

fn run_id() -> &'static str {
    static RUN_ID: OnceLock<String> = OnceLock::new();
    RUN_ID.get_or_init(|| {
        format!(
            "{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros()
        )
    })
}

/// Delete the stores of earlier engine runs. Snapshots are in-memory state, so
/// nothing on disk from a previous run is reachable; a run that was killed, or
/// exited without unwinding, leaves its stores behind for this to collect.
/// Requires a tokio runtime.
pub(super) fn sweep_stale_stores(data_dir: &Path) {
    let parent = data_dir.join(STORE_DIR);
    tokio::task::spawn_blocking(move || remove_other_runs(&parent, OsStr::new(run_id())));
}

fn remove_other_runs(parent: &Path, current: &OsStr) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() == current {
            continue;
        }
        let path = entry.path();
        if std::fs::remove_dir_all(&path).is_err() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Write the checkout's current tracked + untracked (unignored) tree into a
/// private object store: `git add -A` under a throwaway `GIT_INDEX_FILE`, then
/// `git write-tree`. Neither the real index nor the checkout's object database
/// is touched. Costs one hash pass over the tracked files and the untracked
/// files within budget (no stat cache in a fresh index).
pub async fn snapshot_tree(repos: &Repos, root: &Path) -> Result<TreeSnapshot, EngineError> {
    snapshot_tree_onto(repos, root, None).await
}

/// [`snapshot_tree`] layered on `base`: objects the base already holds are
/// read from its store instead of being written again, so the new store only
/// carries what changed since. The result keeps `base` alive.
pub(super) async fn snapshot_tree_onto(
    repos: &Repos,
    root: &Path,
    base: Option<&TreeSnapshot>,
) -> Result<TreeSnapshot, EngineError> {
    // The git children are `kill_on_drop`: timing out takes them down too.
    // Boxed because the future embeds several 64 KiB git read buffers, which
    // callers would otherwise carry inline in their own frames.
    let snapshot = Box::pin(write_snapshot(repos, root, base));
    match tokio::time::timeout(SNAPSHOT_TIMEOUT, snapshot).await {
        Ok(result) => result,
        Err(_) => Err(EngineError::Other(format!(
            "turn snapshot timed out after {}s",
            SNAPSHOT_TIMEOUT.as_secs()
        ))),
    }
}

async fn write_snapshot(
    repos: &Repos,
    root: &Path,
    base: Option<&TreeSnapshot>,
) -> Result<TreeSnapshot, EngineError> {
    let store = Arc::new(ObjectStore::create(repos, root, base.map(|base| &base.store)).await?);
    let skipped = untracked_to_skip(root).await?;
    // NUL-separated pathspecs on stdin: no argv limit, no quoting.
    let mut pathspecs = b".\0".to_vec();
    for path in &skipped {
        pathspecs.extend_from_slice(b":(top,exclude,literal)");
        pathspecs.extend_from_slice(path);
        pathspecs.push(0);
    }
    let index = store.dir.join("index");
    let added = snapshot_git(
        root,
        &store,
        &index,
        &[
            "add",
            "-A",
            "--ignore-errors",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        Some(pathspecs),
    )
    .await
    .map_err(|e| EngineError::Other(format!("git add failed: {e}")))?;
    if !added.status.success() {
        return Err(EngineError::Other(format!(
            "git add: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        )));
    }
    let written = snapshot_git(root, &store, &index, &["write-tree"], None)
        .await
        .map_err(|e| EngineError::Other(format!("git write-tree failed: {e}")));
    // The tree is all that is kept; the index can run to tens of megabytes.
    let _ = tokio::fs::remove_file(&index).await;
    let written = written?;
    if !written.status.success() {
        return Err(EngineError::Other(format!(
            "git write-tree: {}",
            String::from_utf8_lossy(&written.stderr).trim()
        )));
    }
    Ok(TreeSnapshot {
        tree: String::from_utf8_lossy(&written.stdout).trim().to_string(),
        store,
    })
}

async fn snapshot_git(
    root: &Path,
    store: &ObjectStore,
    index: &Path,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> std::io::Result<std::process::Output> {
    let mut cmd = tokio::process::Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.as_std_mut().creation_flags(0x08000000);
    }
    cmd.arg("-C").arg(root).args(args);
    cmd.env("GIT_INDEX_FILE", index);
    store.apply_to(&mut cmd);
    cmd.stdin(if stdin.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    });
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    if let Some(input) = stdin
        && let Some(mut pipe) = child.stdin.take()
    {
        // A short pathspec list would drop exclusions, so a failed write is
        // an error rather than something to carry on from.
        pipe.write_all(&input).await?;
        pipe.shutdown().await?;
    }
    child.wait_with_output().await
}

/// The untracked paths `git add` must leave out, as git spells them relative
/// to the checkout root.
///
/// Git lists untracked content as units: a file, or a wholly untracked
/// directory. A directory over the untracked budget goes as one unit (a build
/// tree is useless in part); inside the directories that fit, and among loose
/// files, only files past [`MAX_UNTRACKED_FILE_BYTES`] go. If what remains
/// still exceeds the budget, every untracked unit goes and the snapshot covers
/// tracked files only.
async fn untracked_to_skip(root: &Path) -> Result<Vec<Vec<u8>>, EngineError> {
    let listing = capture_git(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
        MAX_UNTRACKED_LISTING_BYTES,
    )
    .await?;
    if listing.truncated {
        return Err(EngineError::Other(
            "too many untracked paths to snapshot".into(),
        ));
    }
    let units = split_nul(&listing.stdout);
    if units.is_empty() {
        return Ok(Vec::new());
    }

    // Budget-check directories with a walk that stops at the budget, before
    // asking git to enumerate them: git lists a directory in full or not at
    // all, which takes seconds on a build tree. The walk runs on a blocking
    // thread, which outlives this future if the snapshot times out, so the
    // guard tells it to stop.
    let cancel = CancellationToken::new();
    let _stop_walk = cancel.clone().drop_guard();
    let walk_root = root.to_path_buf();
    let walk_units = units.clone();
    let (mut skipped, dirs, mut files) =
        tokio::task::spawn_blocking(move || triage_units(&walk_root, walk_units, &cancel))
            .await
            .map_err(|error| EngineError::Other(format!("untracked walk: {error}")))?;

    // Files inside the directories that fit, in git's own spelling: only git
    // knows which of them are ignored, and the names go back to git as
    // pathspecs.
    for batch in path_batches(&dirs, MAX_PATH_ARGUMENT_BYTES) {
        let mut args: Vec<OsString> = [
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.extend(batch.iter().map(|dir| path_argument(dir)));
        let listed = capture_git(root, &args, MAX_UNTRACKED_LISTING_BYTES).await?;
        if listed.truncated {
            return Ok(units);
        }
        files.extend(split_nul(&listed.stdout));
    }

    let stat_root = root.to_path_buf();
    let (oversized, within_budget) =
        tokio::task::spawn_blocking(move || measure_files(&stat_root, files))
            .await
            .map_err(|error| EngineError::Other(format!("untracked stat: {error}")))?;
    if !within_budget {
        return Ok(units);
    }
    skipped.extend(oversized);
    Ok(skipped)
}

fn split_nul(bytes: &[u8]) -> Vec<Vec<u8>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// Sort untracked units into directories over budget (skipped whole),
/// directories that fit, and loose files. Git marks a directory with a
/// trailing slash.
#[allow(clippy::type_complexity)]
fn triage_units(
    root: &Path,
    units: Vec<Vec<u8>>,
    cancel: &CancellationToken,
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut skipped = Vec::new();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for unit in units {
        if cancel.is_cancelled() {
            break;
        }
        if !unit.ends_with(b"/") {
            files.push(unit);
        } else if dir_exceeds_budget(&root.join(path_argument(&unit))) {
            skipped.push(unit);
        } else {
            dirs.push(unit);
        }
    }
    (skipped, dirs, files)
}

/// True once `dir` holds more unignored content than the untracked budget.
/// Stops at the budget, so the cost is bounded however large the directory
/// is. Files past the per-file cap count as files but not as bytes: they are
/// left out one by one, and must not condemn the directory around them.
fn dir_exceeds_budget(dir: &Path) -> bool {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let walk = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .ignore(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walk.flatten() {
        let Some(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            continue;
        }
        files += 1;
        if kind.is_file()
            && let Ok(metadata) = entry.metadata()
            && metadata.len() <= MAX_UNTRACKED_FILE_BYTES
        {
            bytes += metadata.len();
        }
        if files > MAX_UNTRACKED_FILES || bytes > MAX_UNTRACKED_BYTES {
            return true;
        }
    }
    false
}

/// Split `files` into those past the per-file cap, and whether the rest fit
/// the untracked budget. A path that vanished since git listed it costs
/// nothing; `git add --ignore-errors` copes with it.
fn measure_files(root: &Path, files: Vec<Vec<u8>>) -> (Vec<Vec<u8>>, bool) {
    let mut oversized = Vec::new();
    let mut kept = 0usize;
    let mut bytes = 0u64;
    for file in files {
        let Ok(metadata) = std::fs::symlink_metadata(root.join(path_argument(&file))) else {
            continue;
        };
        if metadata.is_file() && metadata.len() > MAX_UNTRACKED_FILE_BYTES {
            oversized.push(file);
            continue;
        }
        kept += 1;
        if metadata.is_file() {
            bytes += metadata.len();
        }
        if kept > MAX_UNTRACKED_FILES || bytes > MAX_UNTRACKED_BYTES {
            return (oversized, false);
        }
    }
    (oversized, true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::{
        MAX_UNTRACKED_BYTES, MAX_UNTRACKED_FILE_BYTES, STORE_DIR, TreeSnapshot, alternates_value,
        remove_other_runs, run_id, snapshot_tree,
    };
    use crate::repos::Repos;

    async fn git(cwd: &Path, args: &[&str]) {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .output()
            .await
            .expect("git spawns");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A repo with one committed file `a.txt`, and a `Repos` whose data dir
    /// sits next to it.
    async fn fixture(tmp: &Path) -> (PathBuf, Repos, PathBuf) {
        let root = tmp.join("repo");
        std::fs::create_dir_all(&root).expect("repo dir");
        git(&root, &["init", "-b", "main"]).await;
        std::fs::write(root.join("a.txt"), "one\ntwo\n").expect("write a.txt");
        git(&root, &["add", "."]).await;
        git(&root, &["commit", "-m", "initial"]).await;
        let data_dir = tmp.join("data");
        let repos =
            Repos::with_worktrees_root(&data_dir, "device-test", data_dir.join("worktrees"));
        (root, repos, data_dir)
    }

    /// Every file under the checkout's object database, relative to it.
    fn checkout_objects(root: &Path) -> BTreeSet<PathBuf> {
        fn collect(dir: &Path, base: &Path, out: &mut BTreeSet<PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read objects dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, base, out);
                } else {
                    out.insert(path.strip_prefix(base).expect("under base").to_path_buf());
                }
            }
        }
        let objects = root.join(".git").join("objects");
        let mut out = BTreeSet::new();
        collect(&objects, &objects, &mut out);
        out
    }

    /// Paths in the snapshot's tree, read through the snapshot's own store.
    async fn tree_paths(root: &Path, snapshot: &TreeSnapshot) -> BTreeSet<String> {
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("-C")
            .arg(root)
            .args(["ls-tree", "-r", "--name-only", "-z", snapshot.tree()]);
        snapshot.apply_to(&mut cmd);
        let output = cmd.output().await.expect("git ls-tree spawns");
        assert!(
            output.status.success(),
            "ls-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect()
    }

    /// A file of `len` zero bytes. Sparse where the filesystem allows, so the
    /// budget tests do not write hundreds of megabytes.
    fn sized_file(path: &Path, len: u64) {
        std::fs::File::create(path)
            .and_then(|file| file.set_len(len))
            .expect("sized file");
    }

    fn store_dirs(data_dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(data_dir.join(STORE_DIR).join(run_id()))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }

    /// The incident this module exists for: snapshot blobs written into the
    /// checkout's `.git/objects`, where auto-maintenance later repacked them.
    #[tokio::test]
    async fn snapshot_writes_nothing_into_the_checkout_object_database() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, repos, data_dir) = fixture(tmp.path()).await;
        std::fs::write(root.join("a.txt"), "one\ntwo\nedited\n").expect("edit a.txt");
        std::fs::create_dir_all(root.join("build")).expect("build dir");
        std::fs::write(root.join("build/out.bin"), vec![7u8; 64 * 1024]).expect("out.bin");
        let before = checkout_objects(&root);

        let turn = snapshot_tree(&repos, &root).await.expect("snapshot");
        std::fs::write(root.join("later.txt"), "after the snapshot\n").expect("later.txt");
        let diff = crate::diff_sync::capture_turn_diff(&repos, &root, &turn)
            .await
            .expect("turn diff");

        assert_eq!(
            checkout_objects(&root),
            before,
            "snapshot objects leaked into the checkout's object database"
        );
        // The snapshot still saw everything, from its own store.
        assert_eq!(
            tree_paths(&root, &turn).await,
            BTreeSet::from(["a.txt".to_string(), "build/out.bin".to_string()])
        );
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, "later.txt");
        // And the tree is invisible to the checkout without that store.
        let plain = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["cat-file", "-e", turn.tree()])
            .output()
            .await
            .expect("git cat-file spawns");
        assert!(!plain.status.success());

        // The store lives exactly as long as the snapshot.
        assert_eq!(store_dirs(&data_dir).len(), 1);
        drop(turn);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !store_dirs(&data_dir).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "snapshot store was not deleted: {:?}",
                store_dirs(&data_dir)
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn oversized_untracked_files_are_left_out() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, repos, _data_dir) = fixture(tmp.path()).await;
        std::fs::write(root.join("notes.txt"), "small\n").expect("notes.txt");
        sized_file(&root.join("dump.bin"), MAX_UNTRACKED_FILE_BYTES + 1);
        std::fs::create_dir_all(root.join("assets")).expect("assets dir");
        std::fs::write(root.join("assets/logo.svg"), "<svg/>\n").expect("logo.svg");
        sized_file(&root.join("assets/video.mov"), MAX_UNTRACKED_FILE_BYTES + 1);

        let snapshot = snapshot_tree(&repos, &root).await.expect("snapshot");
        assert_eq!(
            tree_paths(&root, &snapshot).await,
            BTreeSet::from([
                "a.txt".to_string(),
                "assets/logo.svg".to_string(),
                "notes.txt".to_string(),
            ])
        );
    }

    #[tokio::test]
    async fn untracked_directory_over_budget_is_left_out_whole() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, repos, _data_dir) = fixture(tmp.path()).await;
        std::fs::write(root.join("a.txt"), "one\ntwo\nedited\n").expect("edit a.txt");
        std::fs::write(root.join("notes.txt"), "small\n").expect("notes.txt");
        // An unignored build tree: every file fits the per-file cap, the
        // directory as a whole does not fit the budget.
        std::fs::create_dir_all(root.join("target-app/debug")).expect("build dir");
        for i in 0..=MAX_UNTRACKED_BYTES / MAX_UNTRACKED_FILE_BYTES {
            sized_file(
                &root.join(format!("target-app/debug/lib{i}.rlib")),
                MAX_UNTRACKED_FILE_BYTES,
            );
        }

        let snapshot = snapshot_tree(&repos, &root).await.expect("snapshot");
        assert_eq!(
            tree_paths(&root, &snapshot).await,
            BTreeSet::from(["a.txt".to_string(), "notes.txt".to_string()])
        );
        // More build output during the turn stays out of the turn diff.
        sized_file(
            &root.join("target-app/debug/late.rlib"),
            MAX_UNTRACKED_FILE_BYTES,
        );
        let diff = crate::diff_sync::capture_turn_diff(&repos, &root, &snapshot)
            .await
            .expect("turn diff");
        assert!(diff.files.is_empty(), "unexpected: {:?}", diff.files);
    }

    #[tokio::test]
    async fn loose_untracked_files_over_budget_leave_tracked_files_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, repos, _data_dir) = fixture(tmp.path()).await;
        std::fs::write(root.join("a.txt"), "one\ntwo\nedited\n").expect("edit a.txt");
        std::fs::create_dir_all(root.join("docs")).expect("docs dir");
        std::fs::write(root.join("docs/readme.md"), "small\n").expect("readme.md");
        for i in 0..=MAX_UNTRACKED_BYTES / MAX_UNTRACKED_FILE_BYTES {
            sized_file(
                &root.join(format!("capture-{i}.raw")),
                MAX_UNTRACKED_FILE_BYTES,
            );
        }

        let snapshot = snapshot_tree(&repos, &root).await.expect("snapshot");
        assert_eq!(
            tree_paths(&root, &snapshot).await,
            BTreeSet::from(["a.txt".to_string()])
        );
        // The tracked edit is still in the tree.
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("-C")
            .arg(&root)
            .args(["cat-file", "blob", &format!("{}:a.txt", snapshot.tree())]);
        snapshot.apply_to(&mut cmd);
        let output = cmd.output().await.expect("git cat-file spawns");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "one\ntwo\nedited\n"
        );
    }

    /// The Changes pane reads a file's turn-start text straight from the
    /// snapshot tree, which only the snapshot's store can resolve.
    #[tokio::test]
    async fn turn_start_text_is_read_from_the_snapshot_store() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, repos, _data_dir) = fixture(tmp.path()).await;
        std::fs::write(root.join("pre.txt"), "before the turn\n").expect("pre.txt");
        let turn = snapshot_tree(&repos, &root).await.expect("snapshot");
        std::fs::write(root.join("pre.txt"), "before the turn\nedited in turn\n")
            .expect("edit pre.txt");

        let diff = crate::diff_sync::capture_turn_diff(&repos, &root, &turn)
            .await
            .expect("turn diff");
        let file = diff
            .files
            .iter()
            .find(|file| file.path == "pre.txt")
            .expect("pre.txt summary");
        let pair =
            crate::diff_sync::read_diff_file_text_at(&root, turn.tree(), Some(&turn), None, file)
                .await
                .expect("file text");
        assert_eq!(pair.old_text.as_deref(), Some("before the turn\n"));
        assert_eq!(
            pair.new_text.as_deref(),
            Some("before the turn\nedited in turn\n")
        );
    }

    #[tokio::test]
    async fn repository_without_commits_or_files_snapshots_to_the_empty_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).expect("repo dir");
        git(&root, &["init", "-b", "main"]).await;
        let data_dir = tmp.path().join("data");
        let repos =
            Repos::with_worktrees_root(&data_dir, "device-test", data_dir.join("worktrees"));

        let snapshot = snapshot_tree(&repos, &root).await.expect("snapshot");
        assert_eq!(snapshot.tree(), crate::diff_sync::EMPTY_TREE_SHA);
    }

    /// A linked worktree keeps its objects in the main checkout's database.
    /// The snapshot has to borrow from there: `a.txt` is unchanged at turn
    /// start, so its blob exists nowhere else.
    #[tokio::test]
    async fn linked_worktree_snapshot_borrows_the_shared_object_database() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, repos, _data_dir) = fixture(tmp.path()).await;
        let linked = tmp.path().join("linked");
        git(
            &root,
            &["worktree", "add", "-b", "side", &linked.to_string_lossy()],
        )
        .await;
        let before = checkout_objects(&root);

        let turn = snapshot_tree(&repos, &linked).await.expect("snapshot");
        std::fs::write(linked.join("a.txt"), "one\ntwo\nedited in turn\n").expect("edit a.txt");
        let diff = crate::diff_sync::capture_turn_diff(&repos, &linked, &turn)
            .await
            .expect("turn diff");
        assert_eq!(diff.files.len(), 1);
        let pair = crate::diff_sync::read_diff_file_text_at(
            &linked,
            turn.tree(),
            Some(&turn),
            None,
            &diff.files[0],
        )
        .await
        .expect("file text");
        assert_eq!(pair.old_text.as_deref(), Some("one\ntwo\n"));
        assert_eq!(checkout_objects(&root), before);
    }

    /// The store's path travels in an environment list that git splits on
    /// `:`; a layered snapshot can only find its base if that survives.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn snapshot_works_from_a_data_dir_containing_the_list_separator() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (root, _repos, _data_dir) = fixture(tmp.path()).await;
        let data_dir = tmp.path().join("da:ta");
        let repos =
            Repos::with_worktrees_root(&data_dir, "device-test", data_dir.join("worktrees"));
        std::fs::write(root.join("pre.txt"), "before the turn\n").expect("pre.txt");
        let turn = snapshot_tree(&repos, &root).await.expect("snapshot");
        std::fs::write(root.join("pre.txt"), "before the turn\nedited in turn\n")
            .expect("edit pre.txt");

        let diff = crate::diff_sync::capture_turn_diff(&repos, &root, &turn)
            .await
            .expect("turn diff");
        assert!(diff.patch.contains("+edited in turn"));
        assert!(!diff.patch.contains("+before the turn"));
    }

    #[test]
    fn stale_run_stores_are_removed_and_the_current_run_is_kept() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let parent = tmp.path().join(STORE_DIR);
        let stale = parent.join("4242-1700000000000000").join("snapshot");
        let current = parent.join(run_id()).join("snapshot");
        std::fs::create_dir_all(stale.join("objects")).expect("stale store");
        std::fs::write(stale.join("objects").join("blob"), "left behind").expect("stale blob");
        std::fs::create_dir_all(current.join("objects")).expect("current store");

        remove_other_runs(&parent, std::ffi::OsStr::new(run_id()));
        assert!(!parent.join("4242-1700000000000000").exists());
        assert!(current.join("objects").is_dir());
        // A data dir that never held a store is fine too.
        remove_other_runs(&tmp.path().join("missing"), std::ffi::OsStr::new(run_id()));
    }

    #[cfg(not(windows))]
    #[test]
    fn alternates_quote_a_directory_containing_the_separator() {
        let plain = Path::new("/repo/.git/objects");
        let colon = Path::new("/data:dir/say \"hi\"/objects");
        assert_eq!(
            alternates_value(&[colon, plain]),
            std::ffi::OsString::from("\"/data:dir/say \\\"hi\\\"/objects\":/repo/.git/objects")
        );
    }
}
