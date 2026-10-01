//! Moving a chat: workspace scope and git capture/landing against real
//! temporary repositories. Two "devices" are two clones of one bare origin.
//! Every git process (ours and the library's) runs with a hermetic HOME and
//! global config, so the developer's own git setup can't leak in.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use zeron_engine::moves::git::{self, BundleOutcome};
use zeron_engine::moves::scope::{self, ExcludeReason, ScopeKind, ScopeRules};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

static HERMETIC: Once = Once::new();

/// Point git at a throwaway HOME/global config for the whole test process.
/// Runs before any test spawns git (every test calls it first), so no other
/// thread is reading the environment while it is written.
fn hermetic() {
    HERMETIC.call_once(|| {
        let home = tempfile::tempdir().unwrap().keep();
        let config = home.join(".gitconfig");
        std::fs::write(
            &config,
            "[user]\n\tname = Test\n\temail = test@example.com\n\
             [init]\n\tdefaultBranch = main\n\
             [protocol \"file\"]\n\tallow = always\n\
             [advice]\n\tdetachedHead = false\n",
        )
        .unwrap();
        // SAFETY: called once, before any test in this binary spawns a
        // process or reads these variables (see above).
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("GIT_CONFIG_GLOBAL", &config);
            std::env::remove_var("GIT_DIR");
            std::env::remove_var("GIT_WORK_TREE");
            std::env::remove_var("GIT_INDEX_FILE");
        }
    });
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_fails(cwd: &Path, args: &[&str]) -> bool {
    !Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git spawns")
        .status
        .success()
}

fn write(path: &Path, content: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn commit(repo: &Path, file: &str, content: &str, message: &str) -> String {
    write(&repo.join(file), content);
    git(repo, &["add", "--", file]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn head(repo: &Path) -> String {
    git(repo, &["rev-parse", "HEAD"])
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

/// `origin.git` (bare) seeded with one commit on main, cloned as `laptop`
/// and `desktop`.
struct Devices {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    origin: PathBuf,
    laptop: PathBuf,
    desktop: PathBuf,
}

fn devices() -> Devices {
    hermetic();
    let tmp = tempfile::tempdir().unwrap();
    let base = canonical(tmp.path());
    let origin = base.join("origin.git");
    git(&base, &["init", "-q", "--bare", "origin.git"]);
    let seed = base.join("seed");
    git(&base, &["clone", "-q", "origin.git", "seed"]);
    commit(&seed, "README.md", "hello\n", "init");
    commit(&seed, "src/lib.rs", "pub fn a() {}\n", "lib");
    git(&seed, &["push", "-q", "origin", "main"]);
    git(&base, &["clone", "-q", "origin.git", "laptop"]);
    git(&base, &["clone", "-q", "origin.git", "desktop"]);
    Devices {
        _tmp: tmp,
        laptop: base.join("laptop"),
        desktop: base.join("desktop"),
        origin,
        base,
    }
}

/// Laptop: `feature` two commits ahead of origin/main (unpushed); the
/// desktop has fetched the bundle of those commits. Returns the head.
async fn feature_on_laptop_fetched_by_desktop(d: &Devices) -> String {
    git(&d.laptop, &["checkout", "-q", "-b", "feature"]);
    commit(&d.laptop, "src/f1.rs", "1\n", "f1");
    let head = commit(&d.laptop, "src/f2.rs", "2\n", "f2");
    let tips = git::tips(&d.desktop).await.unwrap();
    let out = d.base.join("staging/feature.bundle");
    let outcome = git::create_bundle(&d.laptop, &head, &tips, &out)
        .await
        .unwrap();
    assert!(matches!(outcome, BundleOutcome::Written { .. }));
    git::fetch_bundle(&d.desktop, &out, "move-1").await.unwrap();
    head
}

fn status(repo: &Path) -> String {
    git(repo, &["status", "--porcelain=v1", "--untracked-files=all"])
}

fn ls_files_stage(repo: &Path) -> String {
    git(repo, &["ls-files", "--stage"])
}

// ---------------------------------------------------------------------------
// capture
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capture_reads_branch_detached_unborn_and_linked_worktree() {
    let d = devices();
    let state = git::capture(&d.laptop.join("src")).await.unwrap().unwrap();
    assert_eq!(state.root, d.laptop);
    assert_eq!(state.head.as_deref(), Some(head(&d.laptop).as_str()));
    assert_eq!(state.branch.as_deref(), Some("main"));
    assert_eq!(state.upstream.as_deref(), Some("origin/main"));
    assert_eq!(state.remotes.len(), 1);
    assert_eq!(state.remotes[0].name, "origin");
    assert_eq!(
        state.remotes[0].normalized,
        d.origin.to_string_lossy().trim_end_matches(".git")
    );
    assert_eq!(
        state.root_commits,
        vec![git(&d.laptop, &["rev-list", "--max-parents=0", "HEAD"])]
    );
    assert!(!state.is_linked_worktree);
    assert_eq!(state.common_dir, canonical(&d.laptop.join(".git")));

    git(&d.laptop, &["checkout", "-q", "--detach"]);
    let detached = git::capture(&d.laptop).await.unwrap().unwrap();
    assert_eq!(detached.branch, None);
    assert_eq!(detached.upstream, None);
    assert_eq!(detached.head, state.head);

    let worktree = d.base.join("laptop-wt");
    git(
        &d.laptop,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            worktree.to_str().unwrap(),
        ],
    );
    let linked = git::capture(&worktree).await.unwrap().unwrap();
    assert!(linked.is_linked_worktree);
    assert_eq!(linked.root, worktree);
    assert_eq!(linked.branch.as_deref(), Some("wt"));
    assert_eq!(linked.common_dir, canonical(&d.laptop.join(".git")));
    assert_eq!(scope::workspace_root(&worktree), worktree);

    let unborn = d.base.join("unborn");
    std::fs::create_dir_all(&unborn).unwrap();
    git(&unborn, &["init", "-q"]);
    let state = git::capture(&unborn).await.unwrap().unwrap();
    assert_eq!(state.head, None);
    assert_eq!(state.branch.as_deref(), Some("main"));
    assert!(state.root_commits.is_empty());
    assert!(state.remotes.is_empty());

    let plain = d.base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(git::capture(&plain).await.unwrap(), None);
    assert_eq!(scope::workspace_root(&plain), plain);
    assert_eq!(scope::workspace_root(&d.laptop.join("src")), d.laptop);
}

#[tokio::test]
async fn capture_strips_remote_credentials() {
    let d = devices();
    git(
        &d.laptop,
        &[
            "remote",
            "add",
            "fork",
            "https://user:s3cret@github.com/Someone/Repo.git",
        ],
    );
    let state = git::capture(&d.laptop).await.unwrap().unwrap();
    let fork = state.remotes.iter().find(|r| r.name == "fork").unwrap();
    assert_eq!(fork.url, "https://github.com/Someone/Repo.git");
    assert_eq!(fork.normalized, "github.com/someone/repo");
    assert!(!format!("{state:?}").contains("s3cret"));
}

// ---------------------------------------------------------------------------
// staged patch + apply_index
// ---------------------------------------------------------------------------

#[tokio::test]
async fn staged_patch_roundtrips_text_binary_and_deletes() {
    let d = devices();
    let binary: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    write(&d.laptop.join("README.md"), "hello\nstaged line\n");
    write(&d.laptop.join("assets/logo.bin"), &binary);
    git(&d.laptop, &["add", "README.md", "assets/logo.bin"]);
    git(&d.laptop, &["rm", "-q", "--cached", "src/lib.rs"]);
    // An unstaged edit must not leak into the patch.
    write(
        &d.laptop.join("README.md"),
        "hello\nstaged line\nunstaged\n",
    );

    let patch = git::staged_patch(&d.laptop).await.unwrap();
    assert!(!patch.is_empty());
    assert!(!String::from_utf8_lossy(&patch).contains("unstaged"));

    let notes = git::apply_index(&d.desktop, &head(&d.desktop), &patch)
        .await
        .unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        git(&d.desktop, &["write-tree"]),
        git(&d.laptop, &["write-tree"])
    );
    // The desktop's working tree was not touched.
    assert_eq!(
        std::fs::read_to_string(d.desktop.join("README.md")).unwrap(),
        "hello\n"
    );
    assert!(!d.desktop.join("assets/logo.bin").exists());

    // Nothing staged → empty patch → index is exactly HEAD.
    let clean = git::staged_patch(&d.desktop.join("src")).await;
    assert!(clean.is_ok());
    git::apply_index(&d.desktop, &head(&d.desktop), &[])
        .await
        .unwrap();
    assert_eq!(
        git(&d.desktop, &["write-tree"]),
        git(&d.desktop, &["rev-parse", "HEAD^{tree}"])
    );
    assert!(git::staged_patch(&d.desktop).await.unwrap().is_empty());
}

#[tokio::test]
async fn staged_patch_against_unborn_head() {
    hermetic();
    let tmp = tempfile::tempdir().unwrap();
    let base = canonical(tmp.path());
    let src = base.join("src");
    let dst = base.join("dst");
    for repo in [&src, &dst] {
        std::fs::create_dir_all(repo).unwrap();
        git(repo, &["init", "-q"]);
    }
    write(&src.join("a.txt"), "a\n");
    write(&src.join("bin.dat"), [0u8, 1, 2, 255, 0, 7]);
    git(&src, &["add", "."]);
    let patch = git::staged_patch(&src).await.unwrap();
    let notes = git::apply_index(&dst, "", &patch).await.unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(git(&dst, &["write-tree"]), git(&src, &["write-tree"]));
}

#[tokio::test]
async fn apply_index_failure_leaves_index_at_head_with_a_note() {
    let d = devices();
    // A patch against content the desktop doesn't have.
    write(&d.laptop.join("README.md"), "different base\n");
    git(&d.laptop, &["commit", "-q", "-am", "local"]);
    write(&d.laptop.join("README.md"), "different base\nmore\n");
    git(&d.laptop, &["add", "README.md"]);
    let patch = git::staged_patch(&d.laptop).await.unwrap();

    write(&d.desktop.join("src/lib.rs"), "staged before\n");
    git(&d.desktop, &["add", "src/lib.rs"]);
    let notes = git::apply_index(&d.desktop, &head(&d.desktop), &patch)
        .await
        .unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("no longer staged"));
    assert_eq!(
        git(&d.desktop, &["write-tree"]),
        git(&d.desktop, &["rev-parse", "HEAD^{tree}"])
    );
}

// ---------------------------------------------------------------------------
// bundles
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bundle_not_needed_when_target_has_head() {
    let d = devices();
    let tips = git::tips(&d.desktop).await.unwrap();
    assert!(tips.contains(&head(&d.desktop)));
    let out = d.base.join("x.bundle");
    let outcome = git::create_bundle(&d.laptop, &head(&d.laptop), &tips, &out)
        .await
        .unwrap();
    assert_eq!(outcome, BundleOutcome::NotNeeded);
    assert!(!out.exists());

    // Head is an ancestor of a target tip: still nothing to send.
    let older = git(&d.laptop, &["rev-parse", "HEAD~1"]);
    let outcome = git::create_bundle(&d.laptop, &older, &tips, &out)
        .await
        .unwrap();
    assert_eq!(outcome, BundleOutcome::NotNeeded);
}

#[tokio::test]
async fn bundle_of_unpushed_commits_lands_via_fetch_bundle() {
    let d = devices();
    git(&d.laptop, &["checkout", "-q", "-b", "feature"]);
    commit(&d.laptop, "src/f1.rs", "1\n", "f1");
    let head = commit(&d.laptop, "src/f2.rs", "2\n", "f2");
    assert!(git_fails(&d.desktop, &["cat-file", "-e", &head]));

    let mut tips = git::tips(&d.desktop).await.unwrap();
    // A tip the source has never seen is ignored, not an error.
    tips.push("0123456789abcdef0123456789abcdef01234567".into());
    let out = d.base.join("staging/m.bundle");
    let BundleOutcome::Written { path, bytes } = git::create_bundle(&d.laptop, &head, &tips, &out)
        .await
        .unwrap()
    else {
        panic!("expected a bundle");
    };
    assert_eq!(path, out);
    assert_eq!(bytes, std::fs::metadata(&out).unwrap().len());

    // Only the two unpushed commits travel: the bundle requires origin/main.
    let verify = Command::new("git")
        .args(["bundle", "verify", out.to_str().unwrap()])
        .current_dir(&d.desktop)
        .output()
        .unwrap();
    assert!(verify.status.success());
    let listing = format!(
        "{}{}",
        String::from_utf8_lossy(&verify.stdout),
        String::from_utf8_lossy(&verify.stderr)
    );
    assert!(
        listing.contains(&git(&d.desktop, &["rev-parse", "HEAD"])),
        "{listing}"
    );
    let heads = git(&d.desktop, &["bundle", "list-heads", out.to_str().unwrap()]);
    assert!(heads.contains("refs/heads/feature"), "{heads}");
    // The temporary bundle ref is gone from the source.
    assert_eq!(git(&d.laptop, &["for-each-ref", "refs/zeron"]), "");

    git::fetch_bundle(&d.desktop, &out, "move-1").await.unwrap();
    git(
        &d.desktop,
        &["cat-file", "-e", &format!("{head}^{{commit}}")],
    );
    let pinned = git(
        &d.desktop,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/zeron/moves/move-1",
        ],
    );
    assert!(
        pinned.contains("refs/zeron/moves/move-1/heads/feature"),
        "{pinned}"
    );
    // The desktop's own branches and checkout are untouched.
    assert_eq!(
        git(&d.desktop, &["branch", "--format=%(refname:short)"]),
        "main"
    );

    git::cleanup_move_refs(&d.desktop, "move-1").await;
    assert_eq!(git(&d.desktop, &["for-each-ref", "refs/zeron"]), "");
}

#[tokio::test]
async fn init_from_bundle_without_shared_history() {
    hermetic();
    let tmp = tempfile::tempdir().unwrap();
    let base = canonical(tmp.path());
    let solo = base.join("solo");
    std::fs::create_dir_all(&solo).unwrap();
    git(&solo, &["init", "-q", "-b", "work"]);
    commit(&solo, "a.txt", "a\n", "one");
    let head = commit(&solo, "b.txt", "b\n", "two");
    let state = git::capture(&solo).await.unwrap().unwrap();
    assert!(state.remotes.is_empty());

    let out = base.join("full.bundle");
    assert!(matches!(
        git::create_bundle(&solo, &head, &[], &out).await.unwrap(),
        BundleOutcome::Written { .. }
    ));
    let dest = base.join("landed/solo");
    git::init_from_bundle(&out, &dest).await.unwrap();
    assert_eq!(git(&dest, &["symbolic-ref", "HEAD"]), "refs/heads/work");
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&dest, &["for-each-ref", "refs/zeron"]), "");
    assert!(git(&dest, &["remote"]).is_empty());

    // The caller then syncs the files and restores the index.
    write(&dest.join("a.txt"), "a\n");
    write(&dest.join("b.txt"), "b\n");
    git::apply_index(&dest, &head, &[]).await.unwrap();
    assert_eq!(status(&dest), "");

    // Refuses to clobber an existing repository.
    assert!(git::init_from_bundle(&out, &dest).await.is_err());
}

// ---------------------------------------------------------------------------
// find_repo / tips
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_repo_matches_by_root_commit_and_prefers_same_remote() {
    let d = devices();
    let want = git::capture(&d.laptop).await.unwrap().unwrap();

    let unrelated = d.base.join("unrelated");
    std::fs::create_dir_all(&unrelated).unwrap();
    git(&unrelated, &["init", "-q"]);
    commit(&unrelated, "x", "x\n", "x");
    // Shares history but points at a different remote.
    git(&d.base, &["clone", "-q", "desktop", "decoy"]);
    let decoy = d.base.join("decoy");
    git(
        &decoy,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.com/other/repo.git",
        ],
    );

    let candidates = vec![
        d.base.join("missing"),
        unrelated.clone(),
        decoy.clone(),
        d.desktop.join("src"),
    ];
    let found = git::find_repo(&candidates, &want).await.unwrap().unwrap();
    assert_eq!(found.path, d.desktop);
    assert_eq!(found.tips.first(), Some(&head(&d.desktop)));

    let found = git::find_repo(&[unrelated.clone(), decoy.clone()], &want)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.path, decoy);

    assert_eq!(git::find_repo(&[unrelated], &want).await.unwrap(), None);
}

#[tokio::test]
async fn tips_are_deduped_and_include_remote_tracking() {
    let d = devices();
    git(&d.desktop, &["branch", "side"]);
    git(&d.desktop, &["checkout", "-q", "-b", "ahead"]);
    let ahead = commit(&d.desktop, "z", "z\n", "z");
    let tips = git::tips(&d.desktop).await.unwrap();
    assert_eq!(tips[0], ahead);
    let main = git(&d.desktop, &["rev-parse", "main"]);
    assert!(tips.contains(&main));
    let unique: BTreeSet<_> = tips.iter().collect();
    assert_eq!(unique.len(), tips.len());
}

// ---------------------------------------------------------------------------
// land_new_worktree
// ---------------------------------------------------------------------------

#[tokio::test]
async fn land_new_worktree_creates_absent_branch_without_touching_main_checkout() {
    let d = devices();
    let head = feature_on_laptop_fetched_by_desktop(&d).await;
    write(&d.desktop.join("README.md"), "dirty edit\n");
    let before = (
        head_of(&d.desktop),
        status(&d.desktop),
        ls_files_stage(&d.desktop),
    );

    let dest = d.base.join("worktrees/desktop/feature");
    let landing = git::land_new_worktree(&d.desktop, &dest, &head, Some("feature"), "Laptop")
        .await
        .unwrap();
    assert_eq!(landing.branch.as_deref(), Some("feature"));
    assert!(landing.notes.is_empty(), "{:?}", landing.notes);
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&dest, &["symbolic-ref", "--short", "HEAD"]), "feature");
    assert_eq!(
        std::fs::read_to_string(dest.join("src/f2.rs")).unwrap(),
        "2\n"
    );
    assert_eq!(status(&dest), "");

    let after = (
        head_of(&d.desktop),
        status(&d.desktop),
        ls_files_stage(&d.desktop),
    );
    assert_eq!(before, after);
}

fn head_of(repo: &Path) -> (String, String) {
    (
        git(repo, &["rev-parse", "HEAD"]),
        git(repo, &["symbolic-ref", "-q", "HEAD"]),
    )
}

#[tokio::test]
async fn land_new_worktree_fast_forwards_an_ancestor_branch() {
    let d = devices();
    let head = feature_on_laptop_fetched_by_desktop(&d).await;
    git(&d.desktop, &["branch", "feature", "origin/main"]);
    let dest = d.base.join("wt-ff");
    let landing = git::land_new_worktree(&d.desktop, &dest, &head, Some("feature"), "Laptop")
        .await
        .unwrap();
    assert_eq!(landing.branch.as_deref(), Some("feature"));
    assert!(landing.notes.is_empty());
    assert_eq!(git(&d.desktop, &["rev-parse", "feature"]), head);
}

#[tokio::test]
async fn land_new_worktree_never_rewrites_a_diverged_branch() {
    let d = devices();
    let head = feature_on_laptop_fetched_by_desktop(&d).await;
    git(&d.desktop, &["checkout", "-q", "-b", "feature"]);
    let theirs = commit(&d.desktop, "desktop-only.txt", "mine\n", "desktop work");
    git(&d.desktop, &["checkout", "-q", "main"]);

    let dest = d.base.join("wt-diverged");
    let landing = git::land_new_worktree(&d.desktop, &dest, &head, Some("feature"), "Dan's Laptop")
        .await
        .unwrap();
    assert_eq!(landing.branch.as_deref(), Some("feature-from-dan-s-laptop"));
    assert_eq!(landing.notes.len(), 1);
    assert!(
        landing.notes[0].contains("other commits"),
        "{:?}",
        landing.notes
    );
    assert!(landing.notes[0].contains("feature-from-dan-s-laptop"));
    assert_eq!(git(&d.desktop, &["rev-parse", "feature"]), theirs);
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), head);

    // A second landing reuses the fallback (it's an ancestor of the new head)
    // only if it isn't checked out; here it is, so the next free name wins.
    let dest2 = d.base.join("wt-diverged-2");
    let landing =
        git::land_new_worktree(&d.desktop, &dest2, &head, Some("feature"), "Dan's Laptop")
            .await
            .unwrap();
    assert_eq!(
        landing.branch.as_deref(),
        Some("feature-from-dan-s-laptop-2")
    );
}

#[tokio::test]
async fn land_new_worktree_respects_a_branch_checked_out_elsewhere() {
    let d = devices();
    let head = feature_on_laptop_fetched_by_desktop(&d).await;
    // The desktop's main checkout has `feature` (an ancestor) checked out.
    git(&d.desktop, &["checkout", "-q", "-b", "feature"]);
    let before = head_of(&d.desktop);

    let dest = d.base.join("wt-elsewhere");
    let landing = git::land_new_worktree(&d.desktop, &dest, &head, Some("feature"), "laptop")
        .await
        .unwrap();
    assert_eq!(landing.branch.as_deref(), Some("feature-from-laptop"));
    assert!(
        landing.notes[0].contains("checked out in"),
        "{:?}",
        landing.notes
    );
    assert_eq!(head_of(&d.desktop), before);
}

#[tokio::test]
async fn land_new_worktree_detached_and_validation() {
    let d = devices();
    let head = feature_on_laptop_fetched_by_desktop(&d).await;
    let dest = d.base.join("wt-detached");
    let landing = git::land_new_worktree(&d.desktop, &dest, &head, None, "laptop")
        .await
        .unwrap();
    assert_eq!(landing.branch, None);
    assert!(git_fails(&dest, &["symbolic-ref", "-q", "HEAD"]));
    assert_eq!(git(&dest, &["rev-parse", "HEAD"]), head);

    // A non-empty destination, an unknown commit and a bad branch name fail.
    assert!(
        git::land_new_worktree(&d.desktop, &dest, &head, None, "laptop")
            .await
            .is_err()
    );
    let missing = "1111111111111111111111111111111111111111";
    assert!(
        git::land_new_worktree(&d.desktop, &d.base.join("x"), missing, None, "l")
            .await
            .is_err()
    );
    assert!(
        git::land_new_worktree(&d.desktop, &d.base.join("y"), &head, Some("-rf"), "l")
            .await
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// update_existing_checkout (moving back)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn moving_back_fast_forwards_in_place_without_touching_the_tree() {
    let d = devices();
    // The chat started on the laptop on `feature`, with uncommitted work.
    git(&d.laptop, &["checkout", "-q", "-b", "feature"]);
    let departure = commit(&d.laptop, "src/f1.rs", "1\n", "f1");
    write(&d.laptop.join("src/f1.rs"), "1\nworking edit\n");
    write(&d.laptop.join("staged.txt"), "staged\n");
    git(&d.laptop, &["add", "staged.txt"]);

    // On the desktop it committed once more; that commit comes back.
    let tips = git::tips(&d.desktop).await.unwrap();
    let out = d.base.join("there.bundle");
    git::create_bundle(&d.laptop, &departure, &tips, &out)
        .await
        .unwrap();
    git::fetch_bundle(&d.desktop, &out, "m-out").await.unwrap();
    let wt = d.base.join("desktop-wt");
    git::land_new_worktree(&d.desktop, &wt, &departure, Some("feature"), "laptop")
        .await
        .unwrap();
    let back_head = commit(&wt, "src/desktop.rs", "d\n", "on desktop");

    let tips = git::tips(&d.laptop).await.unwrap();
    let back = d.base.join("back.bundle");
    git::create_bundle(&wt, &back_head, &tips, &back)
        .await
        .unwrap();
    git::fetch_bundle(&d.laptop, &back, "m-back").await.unwrap();

    let index_before = ls_files_stage(&d.laptop);
    let landing = git::update_existing_checkout(
        &d.laptop,
        &back_head,
        Some("feature"),
        Some(&departure),
        "desktop",
    )
    .await
    .unwrap();
    assert_eq!(landing.path, d.laptop);
    assert_eq!(landing.branch.as_deref(), Some("feature"));
    assert!(landing.notes.is_empty(), "{:?}", landing.notes);
    assert_eq!(git(&d.laptop, &["rev-parse", "feature"]), back_head);
    assert_eq!(
        git(&d.laptop, &["symbolic-ref", "HEAD"]),
        "refs/heads/feature"
    );
    // Working tree and index are exactly as they were.
    assert_eq!(
        std::fs::read_to_string(d.laptop.join("src/f1.rs")).unwrap(),
        "1\nworking edit\n"
    );
    assert!(!d.laptop.join("src/desktop.rs").exists());
    assert_eq!(ls_files_stage(&d.laptop), index_before);
    git::cleanup_move_refs(&d.laptop, "m-back").await;

    // The caller then syncs the files and restores the index.
    write(&d.laptop.join("src/desktop.rs"), "d\n");
    let notes = git::apply_index(&d.laptop, &back_head, &[]).await.unwrap();
    assert!(notes.is_empty());
    assert_eq!(git(&d.laptop, &["diff", "--cached", "--name-only"]), "");
}

#[tokio::test]
async fn moving_back_onto_a_branch_that_moved_lands_beside_it() {
    let d = devices();
    git(&d.laptop, &["checkout", "-q", "-b", "feature"]);
    let departure = commit(&d.laptop, "src/f1.rs", "1\n", "f1");
    // Someone kept committing on the laptop after the chat left.
    git(&d.laptop, &["branch", "departed"]);
    let local = commit(&d.laptop, "local.txt", "l\n", "local");

    // The desktop's continuation of `departure`.
    git(
        &d.desktop,
        &["fetch", "-q", d.laptop.to_str().unwrap(), "departed"],
    );
    git(&d.desktop, &["checkout", "-q", "--detach", &departure]);
    let back_head = commit(&d.desktop, "src/desktop.rs", "d\n", "on desktop");
    let tips = git::tips(&d.laptop).await.unwrap();
    let back = d.base.join("back.bundle");
    git::create_bundle(&d.desktop, &back_head, &tips, &back)
        .await
        .unwrap();
    git::fetch_bundle(&d.laptop, &back, "m").await.unwrap();

    let landing = git::update_existing_checkout(
        &d.laptop,
        &back_head,
        Some("feature"),
        Some(&departure),
        "Desktop",
    )
    .await
    .unwrap();
    assert_eq!(landing.branch.as_deref(), Some("feature-from-desktop"));
    assert_eq!(git(&d.laptop, &["rev-parse", "feature"]), local);
    assert_eq!(git(&d.laptop, &["rev-parse", "HEAD"]), back_head);
    assert_eq!(
        git(&d.laptop, &["symbolic-ref", "HEAD"]),
        "refs/heads/feature-from-desktop"
    );
    assert_eq!(landing.notes.len(), 2, "{:?}", landing.notes);
    assert!(
        landing
            .notes
            .iter()
            .any(|n| n.contains("changed after the chat left"))
    );

    // Detached on the source → detached here too.
    let landing = git::update_existing_checkout(&d.laptop, &departure, None, None, "Desktop")
        .await
        .unwrap();
    assert_eq!(landing.branch, None);
    assert!(git_fails(&d.laptop, &["symbolic-ref", "-q", "HEAD"]));
    assert_eq!(head(&d.laptop), departure);
}

// ---------------------------------------------------------------------------
// clone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clone_from_remote_is_non_interactive_and_cleans_up() {
    let d = devices();
    let dest = d.base.join("cloned/repo");
    git::clone_from_remote(d.origin.to_str().unwrap(), &dest)
        .await
        .unwrap();
    assert_eq!(head(&dest), head(&d.desktop));

    let bad = d.base.join("cloned/bad");
    let err = git::clone_from_remote("https://user:hunter2@127.0.0.1:9/nope.git", &bad)
        .await
        .unwrap_err()
        .to_string();
    assert!(!err.contains("hunter2"), "{err}");
    assert!(!bad.exists());
}

// ---------------------------------------------------------------------------
// scope
// ---------------------------------------------------------------------------

fn rels(scope: &scope::Scope) -> Vec<&str> {
    scope.files.iter().map(|f| f.rel.as_str()).collect()
}

fn find<'a>(scope: &'a scope::Scope, rel: &str) -> &'a scope::ScopeFile {
    scope
        .files
        .iter()
        .find(|f| f.rel == rel)
        .unwrap_or_else(|| panic!("{rel} missing from {:?}", rels(scope)))
}

fn excluded_reason(scope: &scope::Scope, rel: &str) -> Option<ExcludeReason> {
    scope
        .excluded
        .iter()
        .find(|e| e.rel == rel)
        .map(|e| e.reason)
}

/// Untracked, ignored, heavy and odd files on top of a clone.
fn dress(repo: &Path) {
    write(
        &repo.join(".gitignore"),
        "node_modules/\n.env\nbuild/\n*.log\n!keep.log\n",
    );
    write(&repo.join(".env"), "SECRET=1\n");
    write(&repo.join("notes.txt"), "untracked\n");
    write(&repo.join("debug.log"), "x\n");
    write(&repo.join("keep.log"), "x\n");
    write(
        &repo.join("node_modules/pkg/index.js"),
        "module.exports = 1\n",
    );
    write(&repo.join("dist/bundle.js"), "untracked heavy\n");
    write(&repo.join("build/tracked.txt"), "tracked in build\n");
    git(repo, &["add", "-f", ".gitignore", "build/tracked.txt"]);
    write(&repo.join("build/out.o"), "object\n");
    write(&repo.join("build/gen/deep.o"), "object\n");
    write(&repo.join("big.bin"), vec![7u8; 4096]);
    std::fs::create_dir_all(repo.join("empty/dir")).unwrap();
    // A nested repository: its files travel, its `.git` doesn't.
    let nested = repo.join("third_party/lib");
    std::fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    write(&nested.join("lib.c"), "int x;\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink("src", repo.join("src-link")).unwrap();
}

fn small_rules() -> ScopeRules {
    ScopeRules {
        max_file_bytes: 1024,
        secret_home: None,
        ..ScopeRules::default()
    }
}

#[tokio::test]
async fn scope_walk_includes_ignored_and_prunes_heavy_and_special() {
    let d = devices();
    dress(&d.laptop);
    #[cfg(unix)]
    let _socket = std::os::unix::net::UnixListener::bind(d.laptop.join("agent.sock")).unwrap();

    let scope = scope::walk_async(d.laptop.clone(), small_rules())
        .await
        .unwrap();
    assert!(!scope.truncated);
    assert_eq!(scope.root, d.laptop);
    let all = rels(&scope);
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted, "deterministic order");

    // Ignored local state is included and classified.
    assert!(find(&scope, ".env").ignored);
    assert!(!find(&scope, ".env").tracked);
    assert!(find(&scope, "debug.log").ignored);
    assert!(!find(&scope, "keep.log").ignored, "negation re-includes");
    assert!(!find(&scope, "notes.txt").ignored);
    let lib = find(&scope, "src/lib.rs");
    assert!(lib.tracked && !lib.ignored);
    assert_eq!(lib.kind, ScopeKind::File);
    assert_eq!(lib.size, "pub fn a() {}\n".len() as u64);
    assert!(lib.mtime_ms > 0);
    #[cfg(unix)]
    assert_eq!(lib.mode & 0o600, 0o600);

    // Heavy folders: pruned unless they hold tracked files.
    assert_eq!(
        excluded_reason(&scope, "node_modules"),
        Some(ExcludeReason::Heavy)
    );
    assert_eq!(excluded_reason(&scope, "dist"), Some(ExcludeReason::Heavy));
    assert!(
        !all.iter()
            .any(|r| r.starts_with("node_modules") || r.starts_with("dist"))
    );
    let kept = find(&scope, "build/tracked.txt");
    assert!(kept.tracked);
    assert!(kept.ignored, "under an ignored folder, yet tracked");
    assert_eq!(
        excluded_reason(&scope, "build/out.o"),
        Some(ExcludeReason::Heavy)
    );
    assert_eq!(
        excluded_reason(&scope, "build/gen"),
        Some(ExcludeReason::Heavy)
    );

    // Too large, special, `.git`, nested repos, empty folders, symlinks.
    assert_eq!(
        excluded_reason(&scope, "big.bin"),
        Some(ExcludeReason::TooLarge)
    );
    assert_eq!(
        scope
            .excluded
            .iter()
            .find(|e| e.rel == "big.bin")
            .unwrap()
            .bytes_estimate,
        Some(4096)
    );
    #[cfg(unix)]
    assert_eq!(
        excluded_reason(&scope, "agent.sock"),
        Some(ExcludeReason::Special)
    );
    assert!(
        !all.iter()
            .any(|r| *r == ".git" || r.starts_with(".git/") || r.contains("/.git"))
    );
    assert!(all.contains(&"third_party/lib/lib.c"));
    assert_eq!(find(&scope, "empty/dir").kind, ScopeKind::Dir);
    assert!(
        !all.contains(&"empty"),
        "only leaf empty folders are listed"
    );
    #[cfg(unix)]
    {
        assert_eq!(find(&scope, "src-link").kind, ScopeKind::Symlink);
        assert!(
            !all.iter().any(|r| r.starts_with("src-link/")),
            "never followed"
        );
    }

    let expected_bytes: u64 = scope
        .files
        .iter()
        .filter(|f| f.kind == ScopeKind::File)
        .map(|f| f.size)
        .sum();
    assert_eq!(scope.total_bytes, expected_bytes);
}

#[tokio::test]
async fn scope_file_sets_line_up_on_both_devices() {
    let d = devices();
    dress(&d.laptop);
    dress(&d.desktop);
    // Device-local heavy content differs; it must not matter.
    write(&d.desktop.join("node_modules/other/x.js"), "x\n");
    write(&d.desktop.join("target/debug/app"), "bin\n");

    let rules = small_rules();
    let a = scope::walk(&d.laptop, &rules).unwrap();
    let b = scope::walk(&d.desktop, &rules).unwrap();
    let shape = |s: &scope::Scope| -> Vec<(String, ScopeKind, u64, bool, bool)> {
        s.files
            .iter()
            .map(|f| (f.rel.clone(), f.kind, f.size, f.ignored, f.tracked))
            .collect()
    };
    assert_eq!(shape(&a), shape(&b));
}

#[tokio::test]
async fn scope_walk_guards_and_secrets() {
    hermetic();
    let tmp = tempfile::tempdir().unwrap();
    let root = canonical(tmp.path());
    for i in 0..20 {
        write(&root.join(format!("f{i}.txt")), "0123456789");
    }
    write(&root.join(".ssh/id_ed25519"), "key");
    write(&root.join(".aws/credentials"), "key");
    write(&root.join("proj/id_rsa"), "key");
    write(&root.join(".netrc"), "machine x");

    // Treat the root as a home folder: credential stores are never walked.
    let rules = ScopeRules {
        secret_home: Some(root.clone()),
        ..ScopeRules::default()
    };
    let scope = scope::walk(&root, &rules).unwrap();
    for secret in [".ssh", ".aws", ".netrc", "proj/id_rsa"] {
        assert_eq!(
            excluded_reason(&scope, secret),
            Some(ExcludeReason::Secret),
            "{secret}"
        );
    }
    assert!(
        !rels(&scope)
            .iter()
            .any(|r| r.contains("id_") || r.starts_with(".ssh"))
    );
    assert!(scope::walk(&root.join(".ssh"), &rules).is_err());

    let capped = scope::walk(
        &root,
        &ScopeRules {
            max_files: 5,
            ..rules.clone()
        },
    )
    .unwrap();
    assert!(capped.truncated);
    let capped = scope::walk(
        &root,
        &ScopeRules {
            max_total_bytes: 50,
            ..rules
        },
    )
    .unwrap();
    assert!(capped.truncated);

    // A symlink into a secret store is caught after resolution.
    #[cfg(unix)]
    {
        write(&root.join(".ssh/config"), "Host x");
        std::os::unix::fs::symlink(root.join(".ssh"), root.join("innocent")).unwrap();
        assert!(scope::is_secret_store(&root.join("innocent/config"), &root));
    }
    assert!(scope::is_secret_store(
        &root.join(".aws/credentials"),
        &root
    ));
    assert!(!scope::is_secret_store(&root.join("f1.txt"), &root));
}

/// Informal benchmark: `cargo test -p zeron-engine --test move_git --release
/// -- --ignored --nocapture scope_walk_benchmark`.
#[test]
#[ignore]
fn scope_walk_benchmark() {
    hermetic();
    let tmp = tempfile::tempdir().unwrap();
    let root = canonical(tmp.path());
    git(&root, &["init", "-q"]);
    write(&root.join(".gitignore"), "*.gen\nnode_modules/\n");
    for dir in 0..1000 {
        for file in 0..100 {
            let ext = if file % 10 == 0 { "gen" } else { "rs" };
            write(&root.join(format!("d{dir}/f{file}.{ext}")), "x");
        }
    }
    write(&root.join("node_modules/a.js"), "x");
    git(&root, &["add", "-A"]);
    let rules = ScopeRules {
        secret_home: None,
        ..ScopeRules::default()
    };
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let scope = scope::walk(&root, &rules).unwrap();
        println!(
            "walked {} entries ({} ignored) in {:?}",
            scope.files.len(),
            scope.files.iter().filter(|f| f.ignored).count(),
            started.elapsed()
        );
        assert_eq!(scope.files.len(), 100_001);
    }
}
