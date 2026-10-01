//! Git state for a moved chat (plan: "The transfer plan", **Git state**).
//!
//! The working tree travels as plain files ([`super::scope`]), but git state
//! travels *as git*: the objects the target lacks as a bundle, the branch name
//! and HEAD as data, and the index as a staged patch. Copying `.git` itself
//! would be both wasteful (the target usually has most objects already) and
//! wrong (it carries device-local config, hooks, worktree registrations).
//!
//! **Source side**: [`capture`] reads HEAD, branch, remotes and root commits;
//! [`create_bundle`] packs only what the target's reported tips don't cover;
//! [`staged_patch`] serialises the index as a binary diff against HEAD.
//!
//! **Target side**: [`find_repo`] picks an existing checkout sharing history
//! (by root commit, preferring the same remote); otherwise [`clone_from_remote`]
//! or [`init_from_bundle`] makes one. [`fetch_bundle`] lands the objects under
//! `refs/zeron/moves/<move>/`, then [`land_new_worktree`] (a fresh `git worktree
//! add`) or [`update_existing_checkout`] (a soft HEAD move, e.g. moving back to
//! where the work started) puts HEAD on the source's commit, and the caller
//! syncs the files and finishes with [`apply_index`].
//!
//! **Branch rules.** A branch is only ever *created* or *fast-forwarded*;
//! a branch that would lose commits, or that another worktree has checked out,
//! is left alone and the work lands on `<branch>-from-<device>` instead, with
//! a human-readable note. Landing never touches a working tree or index other
//! than a newly created worktree's.
//!
//! Every git call is a non-interactive subprocess (never libgit2) that can't
//! prompt; remote URLs are stored without passwords and any URL in an error
//! message is redacted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::repos::git_command;

/// Ceiling for local git operations (bundle, fetch from a file, worktree add).
const LOCAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Ceiling for a network clone.
const CLONE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Most recent tips reported to the source as bundle exclusions.
pub const MAX_TIPS: usize = 256;
/// Where a move's fetched objects are pinned until [`cleanup_move_refs`].
const MOVE_REFS: &str = "refs/zeron/moves";
/// Temporary source-side refs naming the bundled head.
const BUNDLE_HEAD_REFS: &str = "refs/zeron/move-heads";
const REFLOG_MESSAGE: &str = "zeron: moved chat";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    /// The configured URL with any password (and, for http(s), any user)
    /// removed — safe to log and to send to another device.
    pub url: String,
    /// [`normalize_remote`] of `url`.
    pub normalized: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoState {
    /// The worktree's own toplevel.
    pub root: PathBuf,
    /// `None` for an unborn HEAD (no commits yet).
    pub head: Option<String>,
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `origin/main`-style upstream of `branch`.
    pub upstream: Option<String>,
    pub remotes: Vec<Remote>,
    /// Parentless commits reachable from HEAD: the repository's identity
    /// across devices.
    pub root_commits: Vec<String>,
    pub is_linked_worktree: bool,
    pub common_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleOutcome {
    /// Every commit reachable from head is already reachable from a target tip.
    NotNeeded,
    Written {
        path: PathBuf,
        bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMatch {
    /// Toplevel of the matching checkout.
    pub path: PathBuf,
    /// [`tips`] of it, for the source's bundle exclusions.
    pub tips: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    pub path: PathBuf,
    /// The branch HEAD is on, `None` when detached.
    pub branch: Option<String>,
    /// For the user / the move note, e.g. a renamed landing branch.
    pub notes: Vec<String>,
}

// ── source side ─────────────────────────────────────────────────────────────

/// The git state of the checkout containing `root`; `None` when it isn't in a
/// git work tree.
pub async fn capture(root: &Path) -> anyhow::Result<Option<RepoState>> {
    let output = run(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
        None,
        LOCAL_TIMEOUT,
    )
    .await?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines().map(str::trim);
    let (Some(top), Some(git_dir), Some(common_dir)) = (lines.next(), lines.next(), lines.next())
    else {
        return Ok(None);
    };
    let top = PathBuf::from(top);
    let git_dir = canonical(Path::new(git_dir));
    let common_dir = canonical(Path::new(common_dir));

    let head = rev_parse_commit(&top, "HEAD").await;
    let branch = git_opt(&top, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await;
    let upstream = if branch.is_some() {
        git_opt(
            &top,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .await
    } else {
        None
    };
    let remotes = remotes(&top).await;
    let root_commits = if head.is_some() {
        let out = git_ok(&top, &["rev-list", "--max-parents=0", "HEAD"]).await?;
        dedupe(out.lines().map(str::trim).filter(|l| !l.is_empty()))
    } else {
        Vec::new()
    };
    Ok(Some(RepoState {
        root: top,
        head,
        branch,
        upstream,
        remotes,
        root_commits,
        is_linked_worktree: git_dir != common_dir,
        common_dir,
    }))
}

/// The index as a binary patch against HEAD (against the empty tree for an
/// unborn HEAD); empty when nothing is staged. Every diff knob a user config
/// could change (prefixes, renames, external drivers, submodule format) is
/// pinned so the target's `git apply` always understands it.
pub async fn staged_patch(root: &Path) -> anyhow::Result<Vec<u8>> {
    let base = match rev_parse_commit(root, "HEAD").await {
        Some(head) => head,
        None => empty_tree(root).await?,
    };
    let output = run(
        root,
        &[
            "diff",
            "--cached",
            "--binary",
            "--full-index",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-relative",
            "--submodule=short",
            "--ignore-submodules=none",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &base,
            "--",
        ],
        None,
        LOCAL_TIMEOUT,
    )
    .await?;
    check(&output, "git diff --cached")?;
    Ok(output.stdout)
}

/// Write a bundle at `out` holding the commits reachable from `head` that no
/// tip in `target_tips` reaches. Tips this repository doesn't have are
/// ignored (they can't be negated), which only makes the bundle larger.
/// Bundles carry refs, not bare commits, so `head` is bundled through a
/// temporary `refs/zeron/move-heads/<id>` ref; the checked-out branch's ref
/// is included too when it still points at `head`.
pub async fn create_bundle(
    root: &Path,
    head: &str,
    target_tips: &[String],
    out: &Path,
) -> anyhow::Result<BundleOutcome> {
    require_commit(root, head).await?;
    let known = present_commits(root, target_tips).await?;
    if !known.is_empty() {
        let mut revs = format!("{head}\n");
        for tip in &known {
            revs.push_str(&format!("^{tip}\n"));
        }
        let count =
            git_ok_stdin(root, &["rev-list", "--count", "--stdin"], revs.as_bytes()).await?;
        if count.trim() == "0" {
            return Ok(BundleOutcome::NotNeeded);
        }
    }

    let temp_ref = format!("{BUNDLE_HEAD_REFS}/{}", uuid::Uuid::new_v4());
    git_ok(root, &["update-ref", "--no-deref", &temp_ref, head]).await?;
    let mut refs = vec![temp_ref.clone()];
    if let Some(branch) = git_opt(root, &["symbolic-ref", "--quiet", "HEAD"]).await
        && branch.starts_with("refs/heads/")
        && rev_parse_commit(root, &branch).await.as_deref() == Some(head)
    {
        refs.push(branch);
    }
    let mut stdin = String::new();
    for name in &refs {
        stdin.push_str(name);
        stdin.push('\n');
    }
    for tip in &known {
        stdin.push_str(&format!("^{tip}\n"));
    }
    let written = async {
        if let Some(parent) = out.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let out_arg = out.to_string_lossy();
        git_ok_stdin(
            root,
            &["bundle", "create", &out_arg, "--stdin"],
            stdin.as_bytes(),
        )
        .await?;
        let bytes = tokio::fs::metadata(out).await?.len();
        anyhow::Ok(bytes)
    }
    .await;
    let _ = git_ok(root, &["update-ref", "-d", &temp_ref]).await;
    Ok(BundleOutcome::Written {
        path: out.to_path_buf(),
        bytes: written?,
    })
}

/// `git@github.com:a/b.git`, `https://github.com/a/b`, `ssh://git@github.com:22/a/b.git`
/// → `github.com/a/b`. Credentials, ports, the scheme and a `.git` suffix are
/// dropped and the result is lowercased (hosting services treat owner/repo
/// case-insensitively), so it is only fit for *matching*. Local paths keep
/// their case and lose a trailing `/.git` or `.git`.
pub fn normalize_remote(url: &str) -> String {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return normalize_local(rest);
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        (strip_port(host), path)
    } else if let Some((authority, path)) = scp_like(url) {
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        (host, path)
    } else {
        return normalize_local(url);
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    format!("{host}/{path}").to_lowercase()
}

// ── target side ─────────────────────────────────────────────────────────────

/// The first candidate checkout sharing history with `want` (any common root
/// commit). Among matches, one with the same normalised remote wins, then one
/// that already has `want.head`; otherwise candidate order decides.
/// Missing paths and non-repositories are skipped.
pub async fn find_repo(
    candidates: &[PathBuf],
    want: &RepoState,
) -> anyhow::Result<Option<RepoMatch>> {
    if want.root_commits.is_empty() {
        return Ok(None);
    }
    let want_remotes: HashSet<&str> = want.remotes.iter().map(|r| r.normalized.as_str()).collect();
    let mut seen = HashSet::new();
    let mut best: Option<(u8, PathBuf)> = None;
    for candidate in candidates {
        // A dead network mount must read as "not here", not hang the move.
        let exists =
            tokio::time::timeout(Duration::from_secs(2), tokio::fs::metadata(candidate)).await;
        if !matches!(exists, Ok(Ok(meta)) if meta.is_dir()) {
            continue;
        }
        let Some(top) = git_opt(candidate, &["rev-parse", "--show-toplevel"]).await else {
            continue;
        };
        let top = PathBuf::from(top);
        if !seen.insert(canonical(&top)) {
            continue;
        }
        let Ok(present) = present_commits(&top, &want.root_commits).await else {
            continue;
        };
        if present.is_empty() {
            continue;
        }
        let same_remote = remotes(&top)
            .await
            .iter()
            .any(|r| want_remotes.contains(r.normalized.as_str()));
        let has_head = match &want.head {
            Some(head) => rev_parse_commit(&top, head).await.is_some(),
            None => false,
        };
        let score = u8::from(same_remote) * 2 + u8::from(has_head);
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, top));
        }
    }
    let Some((_, path)) = best else {
        return Ok(None);
    };
    let tips = tips(&path).await?;
    Ok(Some(RepoMatch { path, tips }))
}

/// Commits the source may assume this repository has: HEAD, then local and
/// remote-tracking branch tips, most recently committed first, deduplicated
/// and capped at [`MAX_TIPS`].
pub async fn tips(repo: &Path) -> anyhow::Result<Vec<String>> {
    let count = format!("--count={MAX_TIPS}");
    let refs = git_ok(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            &count,
            "--format=%(objectname)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await?;
    let head = rev_parse_commit(repo, "HEAD").await;
    let mut tips = dedupe(
        head.iter()
            .map(String::as_str)
            .chain(refs.lines().map(str::trim))
            .filter(|l| !l.is_empty()),
    );
    tips.truncate(MAX_TIPS);
    Ok(tips)
}

/// `git clone url dest` that can never prompt: no terminal prompt, no askpass
/// program, no credential-manager UI, ssh in batch mode (unless the user
/// configured their own ssh command), and a hard timeout. Credential helpers
/// still run, so a device that can already fetch the remote clones fine.
/// A failed clone leaves no `dest` behind.
pub async fn clone_from_remote(url: &str, dest: &Path) -> anyhow::Result<()> {
    let safe_url = sanitize_url(url);
    ensure_empty_dest(dest).await?;
    let created = tokio::fs::symlink_metadata(dest).await.is_err();
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .context("clone destination has no parent folder")?;
    tokio::fs::create_dir_all(parent).await?;
    let ssh_configured = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || git_opt(parent, &["config", "--get", "core.sshCommand"])
            .await
            .is_some();

    let mut cmd = command(parent);
    if !ssh_configured {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    cmd.args([
        "-c",
        "credential.interactive=false",
        "clone",
        "--quiet",
        "--",
    ])
    .arg(url)
    .arg(dest);
    let failure = match output_with_timeout(cmd, None, CLONE_TIMEOUT).await {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) => format!("git: {}", stderr_message(&output)),
        Err(err) => redact_text(&err.to_string()),
    };
    if created {
        let _ = tokio::fs::remove_dir_all(dest).await;
    } else {
        // Keep the caller's (empty) folder but not a half clone.
        let _ = tokio::fs::remove_dir_all(dest.join(".git")).await;
    }
    bail!("clone from {safe_url} failed: {failure}")
}

/// A new repository at `dest` from a complete bundle (no prerequisites), for
/// a target with no shared history to build on. Afterwards the bundle's
/// branches exist, HEAD names the bundled branch at the bundled head (or is
/// detached there), and the index is empty: the caller syncs the files and
/// calls [`apply_index`].
pub async fn init_from_bundle(bundle: &Path, dest: &Path) -> anyhow::Result<()> {
    if tokio::fs::symlink_metadata(dest.join(".git")).await.is_ok() {
        bail!("{} is already a git repository", dest.display());
    }
    tokio::fs::create_dir_all(dest).await?;
    let bundle_arg = bundle.to_string_lossy();
    // `bundle list-heads` needs no repository, but must run somewhere.
    let heads = git_ok(dest, &["bundle", "list-heads", &bundle_arg])
        .await
        .context("not a readable git bundle")?;
    let heads: Vec<(String, String)> = heads
        .lines()
        .filter_map(|line| line.trim().split_once(' '))
        .map(|(sha, name)| (sha.to_string(), name.to_string()))
        .collect();
    let head_prefix = format!("{BUNDLE_HEAD_REFS}/");
    let head = heads
        .iter()
        .find(|(_, name)| name.starts_with(&head_prefix))
        .or_else(|| {
            heads
                .iter()
                .find(|(_, name)| name.starts_with("refs/heads/"))
        })
        .or_else(|| heads.iter().find(|(_, name)| name == "HEAD"))
        .map(|(sha, _)| sha.clone())
        .context("the bundle names no commit")?;

    git_ok(dest, &["init", "--quiet"]).await?;
    let staging = format!("+refs/zeron/*:{MOVE_REFS}/init/*");
    git_ok(
        dest,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--update-head-ok",
            &bundle_arg,
            "+refs/heads/*:refs/heads/*",
            &staging,
        ],
    )
    .await?;
    let branch = heads
        .iter()
        .find(|(sha, name)| *sha == head && name.starts_with("refs/heads/"))
        .map(|(_, name)| name.clone());
    match branch {
        Some(branch) => {
            git_ok(
                dest,
                &["symbolic-ref", "-m", REFLOG_MESSAGE, "HEAD", &branch],
            )
            .await?;
        }
        None => {
            git_ok(
                dest,
                &[
                    "update-ref",
                    "--no-deref",
                    "-m",
                    REFLOG_MESSAGE,
                    "HEAD",
                    &head,
                ],
            )
            .await?;
        }
    }
    cleanup_move_refs(dest, "init").await;
    Ok(())
}

/// Fetch a bundle's commits into `repo`, pinned under
/// `refs/zeron/moves/<move_id>/` until [`cleanup_move_refs`].
pub async fn fetch_bundle(repo: &Path, bundle: &Path, move_id: &str) -> anyhow::Result<()> {
    check_move_id(move_id)?;
    let refspec = format!("+refs/*:{MOVE_REFS}/{move_id}/*");
    let bundle_arg = bundle.to_string_lossy();
    git_ok(
        repo,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            &bundle_arg,
            &refspec,
        ],
    )
    .await?;
    Ok(())
}

/// A new linked worktree of `repo` at `dest` (absent or empty) checked out at
/// `head`, on `branch` per the module's branch rules. The repository's other
/// checkouts — working trees, indexes and HEADs — are untouched.
pub async fn land_new_worktree(
    repo: &Path,
    dest: &Path,
    head: &str,
    branch: Option<&str>,
    device_label: &str,
) -> anyhow::Result<Landing> {
    require_commit(repo, head).await?;
    ensure_empty_dest(dest).await?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut notes = Vec::new();
    let plan = plan_branch(repo, head, branch, device_label, None, &mut notes).await?;
    let dest_arg = dest.to_string_lossy();
    let landed = match plan {
        BranchPlan::Detached => {
            git_ok(repo, &["worktree", "add", "--detach", &dest_arg, head]).await?;
            None
        }
        BranchPlan::Use { name, old: None } => {
            git_ok(repo, &["worktree", "add", "-b", &name, &dest_arg, head]).await?;
            Some(name)
        }
        BranchPlan::Use {
            name,
            old: Some(old),
        } => {
            if old != head {
                ref_transaction(repo, &format!("update refs/heads/{name} {head} {old}\n")).await?;
            }
            git_ok(repo, &["worktree", "add", &dest_arg, &name]).await?;
            Some(name)
        }
    };
    Ok(Landing {
        path: dest.to_path_buf(),
        branch: landed,
        notes,
    })
}

/// Point an existing checkout's HEAD at `head` (per the branch rules) without
/// touching its working tree or index — the caller syncs the files and calls
/// [`apply_index`]. Used when the work comes back to where it started: if
/// `branch` is checked out right here and `head` descends from it, the branch
/// simply advances (compare-and-swap on its old value) and HEAD stays on it.
/// `departure_head` is what HEAD was when the chat left this checkout; if it
/// has moved since, a note says so.
pub async fn update_existing_checkout(
    checkout: &Path,
    head: &str,
    branch: Option<&str>,
    departure_head: Option<&str>,
    device_label: &str,
) -> anyhow::Result<Landing> {
    let top = PathBuf::from(
        git_ok(checkout, &["rev-parse", "--show-toplevel"])
            .await
            .with_context(|| format!("{} is not a git checkout", checkout.display()))?,
    );
    require_commit(&top, head).await?;
    let current = rev_parse_commit(&top, "HEAD").await;
    let current_ref = git_opt(&top, &["symbolic-ref", "--quiet", "HEAD"]).await;
    let mut notes = Vec::new();
    if let Some(departure) = departure_head
        && current.as_deref() != Some(departure)
        && current.as_deref() != Some(head)
    {
        notes.push(format!(
            "This checkout changed after the chat left it (HEAD was {}, now {}).",
            short(departure),
            current.as_deref().map_or("unborn", short)
        ));
    }
    let plan = plan_branch(&top, head, branch, device_label, Some(&top), &mut notes).await?;
    let landed = match plan {
        BranchPlan::Detached => {
            if current.as_deref() != Some(head) || current_ref.is_some() {
                git_ok(
                    &top,
                    &[
                        "update-ref",
                        "--no-deref",
                        "-m",
                        REFLOG_MESSAGE,
                        "HEAD",
                        head,
                    ],
                )
                .await?;
            }
            None
        }
        BranchPlan::Use { name, old } => {
            let full = format!("refs/heads/{name}");
            match old {
                None => ref_transaction(&top, &format!("create {full} {head}\n")).await?,
                Some(old) if old != head => {
                    ref_transaction(&top, &format!("update {full} {head} {old}\n")).await?
                }
                Some(_) => {}
            }
            if current_ref.as_deref() != Some(full.as_str()) {
                git_ok(&top, &["symbolic-ref", "-m", REFLOG_MESSAGE, "HEAD", &full]).await?;
            }
            Some(name)
        }
    };
    Ok(Landing {
        path: top,
        branch: landed,
        notes,
    })
}

/// Make the index equal `head` plus the source's staged patch. Only the index
/// changes (`read-tree`, `apply --cached`). If the patch doesn't apply, the
/// index is left equal to `head` and a note explains that the changes are in
/// the working tree but no longer staged. An empty `head` means unborn.
pub async fn apply_index(
    checkout: &Path,
    head: &str,
    staged_patch: &[u8],
) -> anyhow::Result<Vec<String>> {
    let reset: Vec<&str> = if head.is_empty() {
        vec!["read-tree", "--empty"]
    } else {
        require_commit(checkout, head).await?;
        vec!["read-tree", head]
    };
    git_ok(checkout, &reset).await?;
    if staged_patch.is_empty() {
        return Ok(Vec::new());
    }
    let output = run(
        checkout,
        &["apply", "--cached", "--binary", "--whitespace=nowarn"],
        Some(staged_patch),
        LOCAL_TIMEOUT,
    )
    .await?;
    if output.status.success() {
        return Ok(Vec::new());
    }
    let reason = stderr_message(&output);
    git_ok(checkout, &reset).await?;
    Ok(vec![format!(
        "The staged changes could not be restored on this device ({}); they are in the working tree but no longer staged.",
        reason.lines().next().unwrap_or("git apply failed")
    )])
}

/// Drop `refs/zeron/moves/<move_id>/*`. Best-effort.
pub async fn cleanup_move_refs(repo: &Path, move_id: &str) {
    if check_move_id(move_id).is_err() {
        return;
    }
    let prefix = format!("{MOVE_REFS}/{move_id}/");
    let Ok(refs) = git_ok(repo, &["for-each-ref", "--format=%(refname)", &prefix]).await else {
        return;
    };
    let mut stdin = String::new();
    for name in refs.lines().map(str::trim).filter(|l| !l.is_empty()) {
        stdin.push_str(&format!("delete {name}\n"));
    }
    if stdin.is_empty() {
        return;
    }
    if let Err(err) = ref_transaction(repo, &stdin).await {
        tracing::debug!(repo = %repo.display(), error = %err, "move ref cleanup failed");
    }
}

// ── branch rules ────────────────────────────────────────────────────────────

enum BranchPlan {
    Detached,
    /// Land on `name`: create it at head (`old: None`) or advance it from
    /// `old` (a no-op when `old` is head).
    Use {
        name: String,
        old: Option<String>,
    },
}

/// Apply the branch rules. `here` is the checkout being updated (its own
/// HEAD doesn't count as "checked out elsewhere"); `None` for a new worktree.
async fn plan_branch(
    repo: &Path,
    head: &str,
    branch: Option<&str>,
    device_label: &str,
    here: Option<&Path>,
    notes: &mut Vec<String>,
) -> anyhow::Result<BranchPlan> {
    let Some(branch) = branch else {
        return Ok(BranchPlan::Detached);
    };
    check_branch_name(repo, branch).await?;
    let checked_out = checked_out_branches(repo).await?;
    let here = here.map(canonical);
    let checked_out_elsewhere = |name: &str| {
        checked_out
            .iter()
            .find(|(path, b)| b == name && Some(path) != here.as_ref())
            .map(|(path, _)| path.clone())
    };
    let blocker = checked_out_elsewhere(branch);
    if let Some(plan) = usable(repo, head, branch, blocker.is_some()).await? {
        return Ok(plan);
    }
    let reason = match blocker {
        Some(path) => format!("is checked out in {}", path.display()),
        None => "already existed on this device with other commits".to_string(),
    };
    let base = format!("{branch}-from-{}", slug(device_label));
    for attempt in 1..=100 {
        let name = if attempt == 1 {
            base.clone()
        } else {
            format!("{base}-{attempt}")
        };
        check_branch_name(repo, &name).await?;
        let busy = checked_out_elsewhere(&name).is_some();
        if let Some(plan) = usable(repo, head, &name, busy).await? {
            notes.push(format!(
                "Branch {branch} {reason}; the work landed on {name}."
            ));
            return Ok(plan);
        }
    }
    bail!("no free branch name for {branch} on this device")
}

/// `name` can take `head` without losing commits: it's absent, or it's an
/// ancestor of (or equal to) head and not checked out in another worktree.
async fn usable(
    repo: &Path,
    head: &str,
    name: &str,
    checked_out_elsewhere: bool,
) -> anyhow::Result<Option<BranchPlan>> {
    if checked_out_elsewhere {
        return Ok(None);
    }
    let old = rev_parse_commit(repo, &format!("refs/heads/{name}")).await;
    let ok = match &old {
        None => true,
        Some(old) if old == head => true,
        Some(old) => is_ancestor(repo, old, head).await?,
    };
    Ok(ok.then(|| BranchPlan::Use {
        name: name.to_string(),
        old,
    }))
}

/// `(canonical worktree path, branch)` for every checkout of the repository,
/// the main one included.
async fn checked_out_branches(repo: &Path) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let out = git_ok(repo, &["worktree", "list", "--porcelain"]).await?;
    let mut rows = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in out.lines().map(str::trim) {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = Some(canonical(Path::new(p)));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/")
            && let Some(path) = &path
        {
            rows.push((path.clone(), branch.to_string()));
        }
    }
    Ok(rows)
}

async fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> anyhow::Result<bool> {
    let output = run(
        repo,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        None,
        LOCAL_TIMEOUT,
    )
    .await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base: {}", stderr_message(&output)),
    }
}

async fn check_branch_name(repo: &Path, name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.starts_with('-'), "invalid branch name {name:?}");
    let output = run(
        repo,
        &["check-ref-format", "--branch", name],
        None,
        LOCAL_TIMEOUT,
    )
    .await?;
    anyhow::ensure!(output.status.success(), "invalid branch name {name:?}");
    Ok(())
}

/// A device label as a branch-name fragment: `Dan's MacBook Pro` → `dan-s-macbook-pro`.
fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.chars().take(32).collect();
    let out = out.trim_matches('-');
    if out.is_empty() {
        "device".to_string()
    } else {
        out.to_string()
    }
}

// ── plumbing ────────────────────────────────────────────────────────────────

/// `git` in `cwd` that can never prompt. `GIT_ASKPASS` set but empty stops
/// git falling back to `core.askPass`/`SSH_ASKPASS`; with terminal prompts off
/// a missing credential fails instead of waiting.
fn command(cwd: &Path) -> tokio::process::Command {
    let mut cmd = git_command();
    cmd.current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .kill_on_drop(true);
    cmd
}

async fn run(
    cwd: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> anyhow::Result<std::process::Output> {
    let mut cmd = command(cwd);
    cmd.args(args);
    output_with_timeout(cmd, stdin, timeout)
        .await
        .with_context(|| format!("git {}", args.first().unwrap_or(&"")))
}

async fn output_with_timeout(
    mut cmd: tokio::process::Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> anyhow::Result<std::process::Output> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn().context("git spawn failed")?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(data), Some(mut pipe)) => {
            let data = data.to_vec();
            Some(tokio::spawn(async move {
                // A child that exits early closes the pipe; its status says why.
                let _ = pipe.write_all(&data).await;
            }))
        }
        _ => None,
    };
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("timed out after {}s", timeout.as_secs()))??;
    if let Some(writer) = writer {
        let _ = writer.await;
    }
    Ok(output)
}

fn check(output: &std::process::Output, what: &str) -> anyhow::Result<()> {
    if output.status.success() {
        Ok(())
    } else {
        bail!("{what}: {}", stderr_message(output))
    }
}

fn stderr_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    if message.is_empty() {
        format!("exited {}", output.status)
    } else {
        redact_text(message)
    }
}

/// Trimmed stdout of a git command that must succeed.
async fn git_ok(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = run(cwd, args, None, LOCAL_TIMEOUT).await?;
    check(&output, &format!("git {}", args.first().unwrap_or(&"")))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn git_ok_stdin(cwd: &Path, args: &[&str], stdin: &[u8]) -> anyhow::Result<String> {
    let output = run(cwd, args, Some(stdin), LOCAL_TIMEOUT).await?;
    check(&output, &format!("git {}", args.first().unwrap_or(&"")))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Trimmed stdout when the command succeeds with output, else `None`.
async fn git_opt(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = run(cwd, args, None, LOCAL_TIMEOUT).await.ok()?;
    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !out.is_empty()).then_some(out)
}

async fn rev_parse_commit(cwd: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    git_opt(cwd, &["rev-parse", "--verify", "--quiet", &spec]).await
}

async fn require_commit(repo: &Path, head: &str) -> anyhow::Result<()> {
    anyhow::ensure!(is_object_id(head), "invalid head commit {head:?}");
    if rev_parse_commit(repo, head).await.is_none() {
        bail!(
            "commit {} is not in {} (fetch the move's bundle first)",
            short(head),
            repo.display()
        );
    }
    Ok(())
}

/// The subset of `shas` present here as commits (one `cat-file` call).
async fn present_commits(repo: &Path, shas: &[String]) -> anyhow::Result<Vec<String>> {
    let wanted: Vec<&str> = shas
        .iter()
        .map(String::as_str)
        .filter(|s| is_object_id(s))
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut stdin = wanted.join("\n");
    stdin.push('\n');
    let out = git_ok_stdin(
        repo,
        &["cat-file", "--batch-check=%(objectname) %(objecttype)"],
        stdin.as_bytes(),
    )
    .await?;
    Ok(dedupe(out.lines().filter_map(|line| {
        let (sha, kind) = line.trim().split_once(' ')?;
        (kind == "commit").then_some(sha)
    })))
}

async fn remotes(repo: &Path) -> Vec<Remote> {
    let Ok(output) = run(
        repo,
        &["config", "--null", "--get-regexp", r"^remote\..*\.url$"],
        None,
        LOCAL_TIMEOUT,
    )
    .await
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut remotes = Vec::new();
    for record in output.stdout.split(|b| *b == 0) {
        let record = String::from_utf8_lossy(record);
        let Some((key, url)) = record.split_once('\n') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|k| k.strip_suffix(".url"))
        else {
            continue;
        };
        if !seen.insert(name.to_string()) {
            continue;
        }
        remotes.push(Remote {
            name: name.to_string(),
            url: sanitize_url(url),
            normalized: normalize_remote(url),
        });
    }
    remotes
}

async fn empty_tree(repo: &Path) -> anyhow::Result<String> {
    git_ok_stdin(repo, &["hash-object", "-t", "tree", "--stdin"], b"").await
}

/// Apply `update-ref --stdin` instructions atomically.
async fn ref_transaction(repo: &Path, instructions: &str) -> anyhow::Result<()> {
    git_ok_stdin(
        repo,
        &["update-ref", "-m", REFLOG_MESSAGE, "--stdin"],
        instructions.as_bytes(),
    )
    .await
    .map(drop)
}

async fn ensure_empty_dest(dest: &Path) -> anyhow::Result<()> {
    match tokio::fs::read_dir(dest).await {
        Ok(mut entries) => {
            if entries.next_entry().await?.is_some() {
                bail!("{} already exists and is not empty", dest.display());
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("{} is not usable", dest.display())),
    }
}

fn check_move_id(move_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !move_id.is_empty()
            && !move_id.starts_with('.')
            && !move_id.contains("..")
            && move_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "invalid move id {move_id:?}"
    );
    Ok(())
}

/// A full hex object id (SHA-1 or SHA-256), never an option or a rev expression.
fn is_object_id(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn dedupe<'a>(items: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .filter(|s| seen.insert(*s))
        .map(str::to_string)
        .collect()
}

// ── URLs ────────────────────────────────────────────────────────────────────

/// `[user@]host:path` (git's scp-like syntax), not a Windows drive (`C:\…`)
/// or a local path with a colon after a slash.
fn scp_like(url: &str) -> Option<(&str, &str)> {
    let (authority, path) = url.split_once(':')?;
    if authority.is_empty()
        || authority.contains('/')
        || authority.contains('\\')
        || (authority.len() == 1 && authority.as_bytes()[0].is_ascii_alphabetic())
    {
        return None;
    }
    Some((authority, path))
}

fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').map_or(rest, |(h, _)| h);
    }
    host.split_once(':').map_or(host, |(h, _)| h)
}

fn normalize_local(path: &str) -> String {
    let path = path.trim_end_matches(['/', '\\']);
    let path = path
        .strip_suffix("/.git")
        .or_else(|| path.strip_suffix("\\.git"))
        .or_else(|| path.strip_suffix(".git"))
        .unwrap_or(path);
    path.trim_end_matches(['/', '\\']).to_string()
}

/// The URL without a password, and for http(s) without any user (a token is
/// often passed as the user name).
pub fn sanitize_url(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };
    let http = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let user = userinfo.split_once(':').map_or(userinfo, |(user, _)| user);
    if http || user.is_empty() {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{user}@{host}{path}")
    }
}

/// [`sanitize_url`] applied to every `scheme://…` inside free text (git error
/// messages quote the URL they failed on).
fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let scheme_start = rest[..pos]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
            .map_or(0, |i| i + 1);
        let url_end = rest[pos..]
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '<' | '>' | '`'))
            .map_or(rest.len(), |i| pos + i);
        out.push_str(&rest[..scheme_start]);
        out.push_str(&sanitize_url(&rest[scheme_start..url_end]));
        rest = &rest[url_end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remote_spellings() {
        for url in [
            "git@github.com:Owner/Repo.git",
            "git@github.com:owner/repo",
            "https://github.com/owner/repo",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo/",
            "https://user:tok@github.com/owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
            "ssh://git@github.com:22/owner/repo.git",
            "  git@github.com:/owner/repo.git  ",
        ] {
            assert_eq!(normalize_remote(url), "github.com/owner/repo", "{url}");
        }
        assert_eq!(
            normalize_remote("https://gitlab.com/group/sub/proj.git"),
            "gitlab.com/group/sub/proj"
        );
        assert_eq!(normalize_remote("ssh://git@[::1]:2222/a/b.git"), "::1/a/b");
        assert_eq!(normalize_remote("/srv/git/Proj.git"), "/srv/git/Proj");
        assert_eq!(normalize_remote("/home/u/proj/.git"), "/home/u/proj");
        assert_eq!(normalize_remote("file:///srv/git/x.git"), "/srv/git/x");
        assert_eq!(normalize_remote(r"C:\repos\x.git"), r"C:\repos\x");
    }

    #[test]
    fn sanitizes_and_redacts_credentials() {
        assert_eq!(
            sanitize_url("https://user:secret@github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        assert_eq!(
            sanitize_url("https://ghp_token@github.com/a/b"),
            "https://github.com/a/b"
        );
        assert_eq!(sanitize_url("ssh://git:pw@host/a/b"), "ssh://git@host/a/b");
        assert_eq!(sanitize_url("git@github.com:a/b"), "git@github.com:a/b");
        let message = "fatal: repository 'https://me:hunter2@example.com/x.git/' not found";
        let redacted = redact_text(message);
        assert!(!redacted.contains("hunter2") && !redacted.contains("me:"));
        assert!(redacted.contains("https://example.com/x.git/"));
    }

    #[test]
    fn slugs_device_labels() {
        assert_eq!(slug("Dan's MacBook Pro"), "dan-s-macbook-pro");
        assert_eq!(slug("  "), "device");
        assert_eq!(slug("Ünïcode"), "n-code");
    }

    #[test]
    fn move_ids_are_ref_safe() {
        assert!(check_move_id("0b5f-uuid_1.2").is_ok());
        for bad in ["", "..", "a/b", ".x", "a..b", "a b", "-x/"] {
            assert!(check_move_id(bad).is_err(), "{bad}");
        }
    }
}
