//! Cloud box checkpoints, control server and boot (docs/cloud.md §4–§5),
//! against an in-process checkpoint store.
#![cfg(unix)]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};

use zeron_engine::cloud::checkpoint::{Checkpointer, CloudPaths, RootSpec, workspace_roots};
use zeron_engine::cloud::control::{Activity, BoxEngine, CloudBox, CONTROL_HEADER};
use zeron_engine::cloud::store::CheckpointStore;
use zeron_engine::cloud::{CloudBoot, CloudEnv, CoreBox};
use zeron_engine::{EngineConfig, EngineCore, HarnessId, default_registry};

// ── the fake store ──────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeStore {
    data: Mutex<HashMap<String, Vec<u8>>>,
    object_puts: AtomicUsize,
    fail_objects: AtomicBool,
    fail_manifests: AtomicBool,
}

impl FakeStore {
    fn head(&self) -> Option<u64> {
        let data = self.data.lock().unwrap();
        let body = data.get("/HEAD")?;
        let head: serde_json::Value = serde_json::from_slice(body).unwrap();
        head["seq"].as_u64()
    }

    fn has(&self, key: &str) -> bool {
        self.data.lock().unwrap().contains_key(key)
    }

    fn puts(&self) -> usize {
        self.object_puts.load(Ordering::SeqCst)
    }
}

async fn serve_store(store: Arc<FakeStore>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let store = store.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                    let store = store.clone();
                    async move {
                        let method = request.method().clone();
                        let path = request.uri().path().to_string();
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        let (status, body) = handle(&store, &method, &path, body);
                        let mut response = Response::new(Full::new(body));
                        *response.status_mut() = status;
                        Ok::<_, std::convert::Infallible>(response)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    format!("http://{addr}")
}

fn handle(store: &FakeStore, method: &Method, path: &str, body: Bytes) -> (StatusCode, Bytes) {
    let mut data = store.data.lock().unwrap();
    match *method {
        Method::PUT => {
            if path.starts_with("/objects/") {
                if store.fail_objects.load(Ordering::SeqCst) {
                    return (StatusCode::BAD_GATEWAY, Bytes::new());
                }
                store.object_puts.fetch_add(1, Ordering::SeqCst);
            }
            if path.starts_with("/manifests/") && store.fail_manifests.load(Ordering::SeqCst) {
                return (StatusCode::BAD_GATEWAY, Bytes::new());
            }
            data.insert(path.to_string(), body.to_vec());
            (StatusCode::OK, Bytes::new())
        }
        Method::GET => match data.get(path) {
            Some(body) => (StatusCode::OK, Bytes::from(body.clone())),
            None => (StatusCode::NOT_FOUND, Bytes::new()),
        },
        Method::HEAD => match data.get(path) {
            Some(_) => (StatusCode::OK, Bytes::new()),
            None => (StatusCode::NOT_FOUND, Bytes::new()),
        },
        _ => (StatusCode::METHOD_NOT_ALLOWED, Bytes::new()),
    }
}

async fn fake_store() -> (Arc<FakeStore>, CheckpointStore) {
    let fake = Arc::new(FakeStore::default());
    let url = serve_store(fake.clone()).await;
    (fake, CheckpointStore::new(url))
}

// ── trees ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    kind: &'static str,
    mode: u32,
    mtime_ms: i64,
    /// Content hash, or a link's target.
    content: String,
}

/// Every entry under `root` (the root itself excluded).
fn snapshot(root: &Path) -> BTreeMap<String, Node> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let mtime_ms = filetime::FileTime::from_last_modification_time(&meta);
            let mtime_ms = mtime_ms.unix_seconds() * 1000 + i64::from(mtime_ms.nanoseconds() / 1_000_000);
            let (kind, content) = if meta.file_type().is_symlink() {
                ("symlink", std::fs::read_link(&path).unwrap().to_string_lossy().into_owned())
            } else if meta.is_dir() {
                stack.push(path.clone());
                ("dir", String::new())
            } else {
                ("file", format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap())))
            };
            out.insert(
                rel,
                Node {
                    kind,
                    mode: meta.permissions().mode() & 0o7777,
                    // Symlink times aren't compared (macOS and Linux differ
                    // in what they keep).
                    mtime_ms: if kind == "symlink" { 0 } else { mtime_ms },
                    content,
                },
            );
        }
    }
    out
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn set_mtime(path: &Path, secs: i64) {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(secs, 123_000_000)).unwrap();
}

/// Bytes that don't compress (so chunk sizes are real).
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.email=box@zeron.test", "-c", "user.name=Box"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A workspace with every kind of entry the format carries.
fn populate_workspace(ws: &Path) {
    std::fs::create_dir_all(ws).unwrap();
    git(ws, &["init", "-q"]);
    write(&ws.join("src/main.rs"), b"fn main() {}\n");
    write(&ws.join("run.sh"), b"#!/bin/sh\necho hi\n");
    chmod(&ws.join("run.sh"), 0o755);
    write(&ws.join(".env"), b"SECRET=1\n");
    chmod(&ws.join(".env"), 0o600);
    write(&ws.join("empty.txt"), b"");
    write(&ws.join("node_modules/dep/index.js"), b"module.exports = 1;\n");
    write(&ws.join("assets/medium.bin"), &noise(5 * 1024 * 1024, 7));
    write(&ws.join("assets/large.bin"), &noise(66 * 1024 * 1024 + 17, 9));
    std::fs::create_dir_all(ws.join("empty-dir")).unwrap();
    std::fs::create_dir_all(ws.join("private")).unwrap();
    write(&ws.join("private/notes.md"), b"notes\n");
    std::os::unix::fs::symlink("src/main.rs", ws.join("link-rel")).unwrap();
    std::os::unix::fs::symlink("/nonexistent/target", ws.join("link-broken")).unwrap();
    git(ws, &["add", "src", "run.sh"]);
    git(ws, &["commit", "-q", "-m", "first"]);
    // A lock a crashed git left behind must not travel.
    write(&ws.join(".git/index.lock"), b"");
    for (i, rel) in ["src/main.rs", "run.sh", ".env", "empty.txt", "assets/medium.bin", "assets/large.bin"]
        .iter()
        .enumerate()
    {
        set_mtime(&ws.join(rel), 1_600_000_000 + i as i64 * 1000);
    }
    chmod(&ws.join("private"), 0o700);
    set_mtime(&ws.join("private"), 1_500_000_000);
    set_mtime(&ws.join("empty-dir"), 1_500_000_100);
}

fn populate_home(home: &Path) {
    write(&home.join(".claude/projects/-ws/session.jsonl"), b"{\"type\":\"user\"}\n");
    write(&home.join(".claude/.credentials.json"), b"{\"token\":\"t\"}");
    chmod(&home.join(".claude/.credentials.json"), 0o600);
    write(&home.join(".claude.json"), b"{\"hasCompletedOnboarding\":true}");
    write(&home.join(".codex/sessions/2026/rollout.jsonl"), b"{}\n");
    write(&home.join(".config/opencode/opencode.json"), b"{}");
    // Excluded: a local CLI install and an unrelated folder.
    write(&home.join(".claude/local/node_modules/x/big.js"), b"x");
    write(&home.join("Documents/unrelated.txt"), b"no");
}

fn harness_view(home: &Path) -> BTreeMap<String, Node> {
    snapshot(home)
        .into_iter()
        .filter(|(rel, _)| {
            [".claude", ".codex", ".config"]
                .iter()
                .any(|p| rel == p || rel.starts_with(&format!("{p}/")))
                || rel == ".claude.json"
        })
        .filter(|(rel, _)| !rel.starts_with(".claude/local"))
        .collect()
}

fn data_view(data: &Path) -> BTreeMap<String, Node> {
    snapshot(data)
        .into_iter()
        .filter(|(rel, _)| {
            let top = rel.split('/').next().unwrap();
            !["logs", "worktrees", "cloud-checkpoints"].contains(&top)
                && !rel.ends_with(".lock")
                && !rel.ends_with("-wal")
                && !rel.ends_with("-shm")
                && !rel.ends_with(".sqlite3")
        })
        .collect()
}

fn roots(paths: &CloudPaths, folders: Vec<PathBuf>) -> Vec<RootSpec> {
    let mut roots = vec![RootSpec::data(&paths.data_dir), RootSpec::harness(&paths.home)];
    roots.extend(workspace_roots(folders, &paths.home));
    roots
}

/// Canonical (macOS temp dirs sit behind `/var` → `/private/var`, and git
/// reports resolved paths).
fn box_paths(dir: &Path) -> CloudPaths {
    let dir = dir.canonicalize().unwrap();
    CloudPaths {
        data_dir: dir.join("data"),
        home: dir.join("home"),
    }
}

// ── checkpoints ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_restores_an_identical_tree() {
    let (fake, store) = fake_store().await;
    let one = tempfile::tempdir().unwrap();
    let paths = box_paths(one.path());
    let ws = paths.home.join("dev/app");
    populate_workspace(&ws);
    populate_home(&paths.home);
    write(&paths.data_dir.join("device-id"), b"box-1");
    write(&paths.data_dir.join("ui-settings.json"), b"{}");
    write(&paths.data_dir.join("engine.lock"), b"123");
    write(&paths.data_dir.join("logs/zeron-headless.log"), b"log");
    write(&paths.data_dir.join("worktrees/app/x"), b"x");
    let db_path = paths.data_dir.join("orgs/o/u/docs.sqlite3");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE docs(id INTEGER PRIMARY KEY, body TEXT);",
    )
    .unwrap();
    for i in 0..500 {
        db.execute("INSERT INTO docs(body) VALUES (?1)", [format!("doc {i}")])
            .unwrap();
    }
    // A writer mid-transaction while the checkpoint copies the database.
    let writer = rusqlite::Connection::open(&db_path).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; INSERT INTO docs(body) VALUES ('uncommitted');")
        .unwrap();

    let ws_before = snapshot(&ws);
    let harness_before = harness_view(&paths.home);
    let data_before = data_view(&paths.data_dir);
    assert!(ws_before.contains_key(".git/HEAD"));

    let checkpointer = Checkpointer::new(store.clone(), paths.clone());
    let stats = checkpointer
        .checkpoint(roots(&paths, vec![ws.join("src")]))
        .await
        .unwrap();
    assert!(stats.changed);
    assert_eq!(stats.seq, 1);
    assert_eq!(fake.head(), Some(1));
    writer.execute_batch("ROLLBACK").unwrap();

    // A fresh box: data and harness at boot, the workspace on first use.
    let two = tempfile::tempdir().unwrap();
    let paths_two = box_paths(two.path());
    std::fs::remove_dir_all(&ws).unwrap();
    let restorer = Checkpointer::new(store.clone(), paths_two.clone());
    let boot = restorer.restore_boot().await.unwrap();
    assert_eq!(boot.seq, Some(1));
    assert_eq!(boot.pending, vec![ws.clone()]);
    assert!(!ws.exists(), "workspaces restore lazily");

    assert_eq!(harness_view(&paths_two.home), harness_before);
    assert!(!paths_two.home.join("Documents").exists());
    assert!(!paths_two.home.join(".claude/local").exists());
    assert_eq!(data_view(&paths_two.data_dir), data_before);
    assert!(!paths_two.data_dir.join("engine.lock").exists());
    assert!(!paths_two.data_dir.join("logs").exists());
    assert!(!paths_two.data_dir.join("worktrees").exists());
    let restored_db = rusqlite::Connection::open(paths_two.data_dir.join("orgs/o/u/docs.sqlite3")).unwrap();
    let rows: i64 = restored_db
        .query_row("SELECT count(*) FROM docs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 500, "committed rows (still in the WAL) only");

    let files = restorer.restore_workspace(&ws.join("src")).await.unwrap();
    assert!(files > 0);
    let mut expected = ws_before.clone();
    expected.remove(".git/index.lock");
    assert_eq!(snapshot(&ws), expected);
    assert_eq!(git(&ws, &["log", "--format=%s"]).trim(), "first");
    assert!(git(&ws, &["status", "--porcelain", "--untracked-files=no"]).is_empty());
    assert_eq!(restorer.restore_workspace(&ws).await.unwrap(), 0, "restored once");
    assert!(restorer.pending_workspaces().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn incremental_checkpoints_upload_only_what_changed() {
    let (fake, store) = fake_store().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = box_paths(dir.path());
    let ws = paths.home.join("proj");
    for i in 0..50 {
        write(&ws.join(format!("src/file{i}.txt")), format!("file {i}\n").as_bytes());
    }
    write(&ws.join("big.bin"), &noise(5 * 1024 * 1024, 3));
    write(&paths.home.join(".claude/settings.json"), b"{}");
    write(&paths.data_dir.join("device-id"), b"box");
    let roots = || roots(&paths, vec![ws.clone()]);
    let checkpointer = Checkpointer::new(store.clone(), paths.clone());

    let first = checkpointer.checkpoint(roots()).await.unwrap();
    assert!(first.changed);
    let uploaded = fake.puts();
    assert_eq!(first.objects as usize, uploaded);

    let unchanged = checkpointer.checkpoint(roots()).await.unwrap();
    assert!(!unchanged.changed);
    assert_eq!((unchanged.seq, unchanged.objects), (1, 0));
    assert_eq!(fake.puts(), uploaded);
    assert!(!fake.has("/manifests/2.json"));

    write(&ws.join("src/file7.txt"), b"changed\n");
    let small = checkpointer.checkpoint(roots()).await.unwrap();
    assert_eq!((small.seq, small.objects), (2, 1), "one new pack");
    assert_eq!(fake.puts(), uploaded + 1);

    write(&ws.join("big.bin"), &noise(5 * 1024 * 1024, 4));
    let big = checkpointer.checkpoint(roots()).await.unwrap();
    assert_eq!((big.seq, big.objects), (3, 1), "one new chunk");

    // A restarted checkpointer (no local index) re-hashes but uploads
    // nothing the store has.
    std::fs::remove_dir_all(paths.data_dir.join("cloud-checkpoints")).unwrap();
    let restarted = Checkpointer::new(store.clone(), paths.clone());
    restarted.restore_boot().await.unwrap();
    restarted.restore_workspace(&ws).await.unwrap();
    let again = restarted.checkpoint(roots()).await.unwrap();
    assert_eq!(again.objects, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_deleted_on_the_box_is_skipped_not_fatal() {
    let (fake, store) = fake_store().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = box_paths(dir.path());
    let ws = paths.home.join("proj");
    write(&ws.join("a.txt"), b"one\n");
    write(&paths.data_dir.join("device-id"), b"box");
    let roots = || roots(&paths, vec![ws.clone()]);
    let checkpointer = Checkpointer::new(store.clone(), paths.clone());
    checkpointer.checkpoint(roots()).await.unwrap();
    std::fs::remove_dir_all(&ws).unwrap();
    // The project is gone; the next checkpoint still happens.
    write(&paths.data_dir.join("device-id"), b"box-2");
    let after = checkpointer.checkpoint(roots()).await.unwrap();
    assert!(after.changed);
    assert_eq!(fake.head(), Some(after.seq));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_checkpoint_leaves_the_previous_head() {
    let (fake, store) = fake_store().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = box_paths(dir.path());
    let ws = paths.home.join("proj");
    write(&ws.join("a.txt"), b"one\n");
    write(&paths.data_dir.join("device-id"), b"box");
    let roots = || roots(&paths, vec![ws.clone()]);
    let checkpointer = Checkpointer::new(store.clone(), paths.clone());
    checkpointer.checkpoint(roots()).await.unwrap();
    assert_eq!(fake.head(), Some(1));

    write(&ws.join("a.txt"), b"two\n");
    fake.fail_objects.store(true, Ordering::SeqCst);
    assert!(checkpointer.checkpoint(roots()).await.is_err());
    assert_eq!(fake.head(), Some(1));
    assert!(!fake.has("/manifests/2.json"));

    fake.fail_objects.store(false, Ordering::SeqCst);
    fake.fail_manifests.store(true, Ordering::SeqCst);
    assert!(checkpointer.checkpoint(roots()).await.is_err());
    assert_eq!(fake.head(), Some(1), "objects alone don't change HEAD");

    // The store still restores the first checkpoint.
    let other = tempfile::tempdir().unwrap();
    std::fs::remove_dir_all(&ws).unwrap();
    let restorer = Checkpointer::new(store.clone(), box_paths(other.path()));
    restorer.restore_boot().await.unwrap();
    restorer.restore_workspace(&ws).await.unwrap();
    assert_eq!(std::fs::read(ws.join("a.txt")).unwrap(), b"one\n");

    write(&ws.join("a.txt"), b"two\n");
    fake.fail_manifests.store(false, Ordering::SeqCst);
    let stats = checkpointer.checkpoint(roots()).await.unwrap();
    assert_eq!(stats.seq, 2);
    assert_eq!(fake.head(), Some(2));
}

// ── control server ──────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeEngine {
    activity: Mutex<Activity>,
    folders: Mutex<Vec<PathBuf>>,
    held: Mutex<Vec<String>>,
    released: Mutex<Vec<String>>,
}

impl BoxEngine for FakeEngine {
    fn activity(&self) -> Activity {
        *self.activity.lock().unwrap()
    }
    fn workspace_folders(&self) -> Vec<PathBuf> {
        self.folders.lock().unwrap().clone()
    }
    fn hold_all(&self) -> Vec<String> {
        let chats = vec!["chat-a".to_string(), "chat-b".to_string()];
        self.held.lock().unwrap().extend(chats.clone());
        chats
    }
    fn release(&self, chats: &[String]) {
        self.released.lock().unwrap().extend(chats.iter().cloned());
    }
    fn flush(&self) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn control_server_checks_the_token_and_reports_status() {
    let (fake, store) = fake_store().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = box_paths(dir.path());
    let ws = paths.home.join("proj");
    write(&ws.join("a.txt"), b"a");
    write(&paths.data_dir.join("device-id"), b"box");
    let engine = Arc::new(FakeEngine::default());
    engine.folders.lock().unwrap().push(ws.clone());
    let cloud = CloudBox::new(engine.clone(), Checkpointer::new(store, paths));
    let (addr, server) = cloud
        .serve("127.0.0.1:0".parse().unwrap(), "secret-token-123".into())
        .await
        .unwrap();
    let base = format!("http://{addr}/zeron/cloud");
    let http = reqwest::Client::new();

    let anonymous = http.get(format!("{base}/status")).send().await.unwrap();
    assert_eq!(anonymous.status(), 401);
    let wrong = http
        .get(format!("{base}/status"))
        .header(CONTROL_HEADER, "secret-token-12")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    let get = |path: &'static str| {
        http.get(format!("{base}/{path}"))
            .header(CONTROL_HEADER, "secret-token-123")
            .send()
    };
    let idle: serde_json::Value = get("status").await.unwrap().json().await.unwrap();
    assert_eq!(idle["busy"], false);
    assert_eq!(idle["runs"], 0);
    let idle_since = idle["lastActivityAt"].as_i64().unwrap();
    assert!(idle_since > 0);

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    engine.activity.lock().unwrap().terminals = 1;
    let busy: serde_json::Value = get("status").await.unwrap().json().await.unwrap();
    assert_eq!(busy["busy"], true);
    assert_eq!(busy["terminals"], 1);
    assert!(busy["lastActivityAt"].as_i64().unwrap() > idle_since);
    engine.activity.lock().unwrap().terminals = 0;

    assert_eq!(get("checkpoint").await.unwrap().status(), 405);
    let missing = http
        .get(format!("http://{addr}/elsewhere"))
        .header(CONTROL_HEADER, "secret-token-123")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    let post = |query: &'static str| {
        http.post(format!("{base}/checkpoint?{query}"))
            .header(CONTROL_HEADER, "secret-token-123")
            .send()
    };
    let first: serde_json::Value = post("final=0").await.unwrap().json().await.unwrap();
    assert_eq!(first["seq"], 1);
    assert!(first["objects"].as_u64().unwrap() >= 1);
    assert!(engine.held.lock().unwrap().is_empty(), "an incremental checkpoint holds nothing");
    assert_eq!(fake.head(), Some(1));

    let last: serde_json::Value = post("final=1").await.unwrap().json().await.unwrap();
    assert_eq!(last["seq"], 1);
    assert_eq!(last["changed"], false);
    assert_eq!(*engine.held.lock().unwrap(), vec!["chat-a", "chat-b"]);
    assert!(engine.released.lock().unwrap().is_empty(), "a final checkpoint keeps chats held");
    cloud.release_holds();
    assert_eq!(*engine.released.lock().unwrap(), vec!["chat-a", "chat-b"]);
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_counts_terminals_and_viewers_of_a_real_engine() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        &dir.path().join("engine"),
        Arc::new(default_registry()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let probe = CoreBox::new(&core);
    assert!(!probe.activity().busy());
    assert!(probe.hold_all().is_empty());

    let cwd = dir.path().to_string_lossy().into_owned();
    let terminal = core.terminals.open(&cwd, 80, 24).unwrap();
    let open = probe.activity();
    assert_eq!(open.terminals, 1);
    assert!(open.busy());
    core.terminals.close(&terminal.id).unwrap();
    assert!(!probe.activity().busy());

    let rpc = zeron_engine::cloud::activity::TrackedRpc::wrap(
        core.rpc_service(),
        core.rpc_activity.clone(),
    );
    let stream = rpc
        .handle(zeron_rpc::methods::WATCH_SESSIONS, serde_json::json!({}))
        .await
        .unwrap();
    let viewing = probe.activity();
    assert_eq!(viewing.viewers, 1);
    assert!(viewing.busy());
    assert!(viewing.last_rpc_ms > 0);
    drop(stream);
    assert!(!probe.activity().busy());
    core.shutdown().await;
}

// ── zeron cloud-boot ────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn cloud_boot_restores_then_checkpoints_on_stop() {
    let (fake, store) = fake_store().await;
    // A box that ran before: a device id, an agent's state and a workspace.
    let before = tempfile::tempdir().unwrap();
    let old = box_paths(before.path());
    let ws = before.path().canonicalize().unwrap().join("workspaces/proj");
    write(&old.data_dir.join("device-id"), b"cloud-box-device-1");
    write(&old.home.join(".claude/settings.json"), b"{\"model\":\"x\"}");
    write(&ws.join("README.md"), b"hello\n");
    Checkpointer::new(store.clone(), old.clone())
        .checkpoint(roots(&old, vec![ws.clone()]))
        .await
        .unwrap();
    std::fs::remove_dir_all(&ws).unwrap();

    let now = tempfile::tempdir().unwrap();
    let paths = box_paths(now.path());
    let config = EngineConfig {
        data_dir: paths.data_dir.clone(),
        edge_url: "http://127.0.0.1:9".into(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        workos_client_id: None,
        dev_user_id: None,
        device_credential: None,
    };
    let env = CloudEnv {
        device_id: Some("cloud-box-device-1".into()),
        checkpoint_url: store.base().to_string(),
        control_port: Some(0),
        control_token: Some("control-token-xyz".into()),
    };
    let mut boot = CloudBoot::new(config, env);
    boot.paths = paths.clone();
    boot.drain_wait = std::time::Duration::from_secs(1);
    let booted = boot.start().await.unwrap();

    assert_eq!(booted.runtime().core().device_id, "cloud-box-device-1");
    assert_eq!(
        std::fs::read(paths.home.join(".claude/settings.json")).unwrap(),
        b"{\"model\":\"x\"}"
    );
    assert!(!ws.exists());
    // A run's first dispatch restores its workspace.
    booted
        .runtime()
        .core()
        .sessions
        .prepare_cwd(&ws.to_string_lossy())
        .await;
    assert_eq!(std::fs::read(ws.join("README.md")).unwrap(), b"hello\n");

    let port = booted.control_addr().unwrap().port();
    let status = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/zeron/cloud/status"))
        .header(CONTROL_HEADER, "control-token-xyz")
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 200);
    let status: serde_json::Value = status.json().await.unwrap();
    assert_eq!(status["busy"], false);

    write(&ws.join("README.md"), b"edited on the box\n");
    let stats = booted.stop().await.unwrap();
    assert!(stats.changed);
    assert_eq!(fake.head(), Some(2));

    // The next wake sees the edit.
    let after = tempfile::tempdir().unwrap();
    std::fs::remove_dir_all(&ws).unwrap();
    let next = Checkpointer::new(store, box_paths(after.path()));
    next.restore_boot().await.unwrap();
    next.restore_workspace(&ws).await.unwrap();
    assert_eq!(std::fs::read(ws.join("README.md")).unwrap(), b"edited on the box\n");
}
