//! The in-memory harness the protocol and sync tests share: two transfer
//! services wired back to back over duplex lanes that can be severed,
//! throttled or counted.

#![allow(dead_code)]

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use zeron_proto::{FileTransfer, FileTransferState, FileTransferTransport};
use zeron_transfer::{
    BoxIo, Network, SyncGrant, Transfers, TransfersConfig, TransportPolicy, Tunnel,
};

/// Lanes to one peer's service. `budget` (bytes, shared by every lane of
/// the first `severed` connections) cuts the tunnel mid-transfer.
pub struct Net {
    pub me: String,
    /// Swappable, so a test can restart the other side.
    pub peer: Mutex<Option<Transfers>>,
    pub budget: Arc<AtomicI64>,
    pub connects: AtomicUsize,
    pub severed: usize,
    pub cancels: Mutex<Vec<String>>,
    /// Throttle lanes (so a test can act mid-transfer).
    pub slow: std::sync::atomic::AtomicBool,
    /// Bytes written on connections that were not severed (the resume).
    pub resumed_bytes: Arc<AtomicI64>,
    /// Sync tickets this side grants, and how often it was asked.
    pub grants: Mutex<HashMap<String, SyncGrant>>,
    pub grant_calls: AtomicUsize,
}

impl Net {
    pub fn peer(&self) -> Transfers {
        self.peer.lock().unwrap().clone().expect("wired")
    }

    pub fn set_peer(&self, peer: Transfers) {
        *self.peer.lock().unwrap() = Some(peer);
    }

    pub fn grant(&self, ticket: &str, grant: SyncGrant) {
        self.grants.lock().unwrap().insert(ticket.into(), grant);
    }
}

struct MemTunnel {
    me: String,
    peer: Transfers,
    budget: Option<Arc<AtomicI64>>,
    slow: bool,
    resumed_bytes: Arc<AtomicI64>,
}

#[async_trait::async_trait]
impl Tunnel for MemTunnel {
    fn transport(&self) -> FileTransferTransport {
        FileTransferTransport::P2p
    }
    fn max_lanes(&self) -> usize {
        4
    }
    async fn open_lane(&self) -> anyhow::Result<BoxIo> {
        let (a, b) = tokio::io::duplex(256 * 1024);
        self.peer
            .accept_lane(self.me.clone(), FileTransferTransport::P2p, Box::new(b));
        Ok(match &self.budget {
            Some(budget) => Box::new(Fuse {
                inner: a,
                budget: budget.clone(),
            }),
            None if self.slow => Box::new(Slow {
                inner: a,
                delay: None,
            }),
            None => Box::new(Counted {
                inner: a,
                written: self.resumed_bytes.clone(),
            }),
        })
    }
    async fn close(&self) {}
}

#[async_trait::async_trait]
impl Network for Net {
    async fn connect(
        &self,
        _device: &str,
        _policy: TransportPolicy,
    ) -> anyhow::Result<Box<dyn Tunnel>> {
        let n = self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MemTunnel {
            me: self.me.clone(),
            peer: self.peer(),
            budget: (n < self.severed).then(|| self.budget.clone()),
            slow: self.slow.load(Ordering::SeqCst),
            resumed_bytes: self.resumed_bytes.clone(),
        }))
    }
    async fn notify_cancel(&self, _device: &str, transfer_id: &str) {
        self.cancels.lock().unwrap().push(transfer_id.to_owned());
        let peer = self.peer.lock().unwrap().clone();
        if let Some(peer) = peer {
            let _ = peer.cancel(transfer_id, false);
        }
    }
    fn device_name(&self, device: &str) -> Option<String> {
        Some(format!("{device} name"))
    }
    fn destination_roots(&self) -> Vec<PathBuf> {
        Vec::new()
    }
    fn sync_grant(&self, peer: &str, ticket: &str) -> Option<SyncGrant> {
        self.grant_calls.fetch_add(1, Ordering::SeqCst);
        if peer != "laptop" && peer != "phone" {
            return None;
        }
        self.grants.lock().unwrap().get(ticket).cloned()
    }
}

/// Fails every read and write once the shared byte budget is spent — a
/// tunnel dying mid-block.
struct Fuse<T> {
    inner: T,
    budget: Arc<AtomicI64>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Fuse<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.budget.load(Ordering::SeqCst) <= 0 {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Fuse<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.budget.fetch_sub(data.len() as i64, Ordering::SeqCst) <= 0 {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionReset.into()));
        }
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Counts the bytes written through it.
struct Counted<T> {
    inner: T,
    written: Arc<AtomicI64>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Counted<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Counted<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, data));
        if let Ok(n) = &result {
            self.written.fetch_add(*n as i64, Ordering::SeqCst);
        }
        Poll::Ready(result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// ~12 MB/s per lane: 64 KiB writes 5 ms apart.
struct Slow<T> {
    inner: T,
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<T: AsyncRead + Unpin> AsyncRead for Slow<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Slow<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(delay) = self.delay.as_mut() {
            std::task::ready!(delay.as_mut().poll(cx));
            self.delay = None;
        }
        let n = data.len().min(64 * 1024);
        let result = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, &data[..n]));
        self.delay = Some(Box::pin(tokio::time::sleep(Duration::from_millis(5))));
        Poll::Ready(result)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub struct Pair {
    pub sender: Transfers,
    pub receiver: Transfers,
    pub sender_net: Arc<Net>,
    pub receiver_net: Arc<Net>,
    pub home: PathBuf,
    pub _dir: tempfile::TempDir,
}

pub fn net(me: &str, severed: usize, budget: i64) -> Arc<Net> {
    Arc::new(Net {
        me: me.into(),
        peer: Mutex::new(None),
        budget: Arc::new(AtomicI64::new(budget)),
        connects: AtomicUsize::new(0),
        severed,
        cancels: Mutex::new(Vec::new()),
        slow: Default::default(),
        resumed_bytes: Arc::new(AtomicI64::new(0)),
        grants: Mutex::new(HashMap::new()),
        grant_calls: AtomicUsize::new(0),
    })
}

pub fn service(dir: &Path, id: &str, home: Option<PathBuf>, net: Arc<Net>) -> Transfers {
    Transfers::new(
        TransfersConfig {
            device_id: id.into(),
            device_name: format!("{id} name"),
            state_dir: dir.join(id).join("state"),
            settings_file: dir.join(id).join("settings.json"),
            home_dir: home,
        },
        net,
    )
}

pub fn pair(severed: usize, budget: i64) -> Pair {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let sender_net = net("laptop", severed, budget);
    let receiver_net = net("phone", 0, 0);
    let sender = service(dir.path(), "laptop", None, sender_net.clone());
    let receiver = service(
        dir.path(),
        "phone",
        Some(home.clone()),
        receiver_net.clone(),
    );
    sender_net.set_peer(receiver.clone());
    receiver_net.set_peer(sender.clone());
    Pair {
        sender,
        receiver,
        sender_net,
        receiver_net,
        home,
        _dir: dir,
    }
}

pub async fn wait_for(
    transfers: &Transfers,
    id: &str,
    what: impl Fn(&FileTransfer) -> bool,
) -> FileTransfer {
    let mut watch = transfers.watch();
    let found = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            if let Some(row) = watch.borrow_and_update().iter().find(|r| r.id == id)
                && what(row)
            {
                return row.clone();
            }
            watch.changed().await.unwrap();
        }
    })
    .await;
    found.unwrap_or_else(|_| panic!("timed out; rows: {:#?}", transfers.list()))
}

pub fn state_is(state: FileTransferState) -> impl Fn(&FileTransfer) -> bool {
    move |row| row.state == state
}

/// Deterministic, position-dependent bytes: a misplaced block shows.
pub fn pattern(size: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..size)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

pub fn project(root: &Path) -> PathBuf {
    let project = root.join("src-project");
    std::fs::create_dir_all(project.join("app/src")).unwrap();
    std::fs::create_dir_all(project.join("empty-dir")).unwrap();
    std::fs::write(project.join("README.md"), b"# hello\n").unwrap();
    std::fs::write(
        project.join("app/src/main.rs"),
        pattern(3 * 1024 * 1024 + 17, 1),
    )
    .unwrap();
    std::fs::write(project.join("app/empty.txt"), b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(project.join("run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(
            project.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("app/src/main.rs", project.join("main-link")).unwrap();
    }
    project
}

pub fn assert_same_tree(a: &Path, b: &Path) {
    for entry in std::fs::read_dir(a).unwrap() {
        let entry = entry.unwrap();
        let other = b.join(entry.file_name());
        let meta = std::fs::symlink_metadata(entry.path()).unwrap();
        let other_meta = std::fs::symlink_metadata(&other)
            .unwrap_or_else(|_| panic!("{} missing", other.display()));
        if meta.file_type().is_symlink() {
            assert!(
                other_meta.file_type().is_symlink(),
                "{} must stay a symlink",
                other.display()
            );
            assert_eq!(
                std::fs::read_link(entry.path()).unwrap(),
                std::fs::read_link(&other).unwrap()
            );
        } else if meta.is_dir() {
            assert!(other_meta.is_dir());
            assert_same_tree(&entry.path(), &other);
        } else {
            assert_eq!(
                std::fs::read(entry.path()).unwrap(),
                std::fs::read(&other).unwrap(),
                "{}",
                other.display()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    meta.permissions().mode() & 0o777,
                    other_meta.permissions().mode() & 0o777
                );
            }
        }
    }
}

pub fn no_parts(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(!name.ends_with(".part"), "leftover {name}");
        if entry.file_type().unwrap().is_dir() {
            no_parts(&entry.path());
        }
    }
}
