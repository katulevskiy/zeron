//! Sync mode: the engine-internal transfers a session move makes
//! (docs/plans/2026-09-30-agent-mobility-and-policy.md, "Rsync-style sync").
//!
//! It differs from inbox delivery in four ways:
//! - **Explicit layout.** The sender lists every file by a relative path
//!   whose first component is a logical root (`ws`, `x0`, `hs`, …). Nothing
//!   is walked or renamed.
//! - **Tickets.** The offer carries a ticket the receiving engine granted
//!   ([`crate::Network::sync_grant`]). The grant — not the sender — decides
//!   where files land: a private staging folder. No prompt, no inbox.
//! - **Content up front.** File entries carry their SHA-256 (and per-block
//!   hashes for large files), computed through the [`crate::hashcache`]. The
//!   receiver compares them with its basis — the folder holding its current
//!   copy of each root — and reports unchanged files and matching blocks as
//!   already there, so only differences travel.
//! - **Tolerance.** A copy-ahead round may skip files that change while
//!   they're sent ([`crate::wire::Msg::Skip`]); the final round carries them.
//!
//! When the transfer completes the receiver writes [`SYNC_RESULT_FILE`] at
//! the staging root: what landed, what was unchanged, what was skipped.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::blocks::{BLOCK_SIZE, RangeSet, block_count, block_len};
use crate::hashcache::{FileHash, FileStat, HashCache, hash_file};
use crate::manifest::{self, Built, Entry, EntryKind, MAX_ENTRIES, Manifest, hex};

/// The result record a completed sync transfer leaves at its staging root.
pub const SYNC_RESULT_FILE: &str = ".zeron-sync.json";
/// Attempts at hashing a file that changes underneath.
const HASH_ATTEMPTS: usize = 3;
const MAX_TICKET: usize = 128;

/// A sync transfer to start ([`crate::Transfers::send_sync`]).
#[derive(Debug, Clone)]
pub struct SyncSend {
    pub to: String,
    /// Granted by the receiving engine for this round.
    pub ticket: String,
    pub files: Vec<SyncSource>,
    /// Skip files that change or vanish while sending instead of failing.
    pub tolerant: bool,
    pub policy: crate::TransportPolicy,
}

#[derive(Debug, Clone)]
pub struct SyncSource {
    /// `/`-separated; the first component is the logical root (`ws`, `x0`…).
    pub rel: String,
    /// Absolute. A file, a symlink (sent as a symlink, never followed) or a
    /// folder (an explicit, possibly empty folder entry — not walked).
    /// Parent folders of every `rel` are implied.
    pub source: PathBuf,
}

/// What the receiving engine grants a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncGrant {
    /// Staging folder chosen by the receiver: files land at
    /// `dest_root/<rel>` exactly.
    pub dest_root: PathBuf,
    /// Root id → the receiver folders holding its current copies (the basis
    /// unchanged files and matching blocks are taken from), in order: for
    /// each file, the first folder with anything at its path is its basis.
    /// A final round lists the earlier round's staging before the landing.
    pub basis: HashMap<String, Vec<PathBuf>>,
}

/// The parsed [`SYNC_RESULT_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub transfer_id: String,
    pub ticket: String,
    pub files: Vec<SyncFile>,
    pub dirs: Vec<SyncDir>,
    pub symlinks: Vec<SyncSymlink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncOutcome {
    /// At `dest_root/<rel>`, verified.
    Landed,
    /// A basis folder already has this content (`basis_index`); nothing
    /// was written.
    Unchanged,
    /// The sender skipped it (tolerant mode); nothing was written.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFile {
    pub rel: String,
    pub outcome: SyncOutcome,
    pub sha256: String,
    pub size: u64,
    /// Unix permission bits; 0 = unknown.
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<i64>,
    /// Why a skipped file was skipped (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unchanged: which of the root's basis folders (position in
    /// [`SyncGrant::basis`]) already holds this content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDir {
    pub rel: String,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSymlink {
    pub rel: String,
    pub target: String,
}

/// Tickets are opaque but tame: ASCII letters, digits, `-`, `_`, `.`.
pub fn valid_ticket(ticket: &str) -> bool {
    (1..=MAX_TICKET).contains(&ticket.len())
        && ticket
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl crate::Transfers {
    /// Read the record a completed sync transfer left at `dest_root`.
    pub fn sync_result(dest_root: &Path) -> anyhow::Result<SyncResult> {
        let path = dest_root.join(SYNC_RESULT_FILE);
        let bytes = std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("Could not read {}: {e}", path.display()))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("Could not parse {}: {e}", path.display()))
    }
}

// ── sending ────────────────────────────────────────────────────────────

/// A sync manifest plus what the sender checks its sources against.
pub(crate) struct SyncBuilt {
    pub built: Built,
    /// Parallel to the entries: each file's version when it was hashed.
    pub stats: Vec<Option<FileStat>>,
}

enum Item {
    Dir {
        mode: u32,
        source: PathBuf,
    },
    File {
        source: PathBuf,
    },
    Symlink {
        target: String,
        mode: u32,
        source: PathBuf,
    },
}

/// Lay `files` out as a manifest: implied folders first (pre-order), every
/// file hashed (through `cache`). Sources that vanish or can't be read are
/// left out and counted when `tolerant`, refused otherwise.
pub(crate) fn build(
    files: &[SyncSource],
    tolerant: bool,
    cache: &Mutex<HashCache>,
) -> anyhow::Result<SyncBuilt> {
    anyhow::ensure!(
        files.len() <= MAX_ENTRIES,
        "too many files (more than {MAX_ENTRIES})"
    );
    let mut skipped = 0u64;
    let mut items: BTreeMap<Vec<String>, Item> = BTreeMap::new();
    let mut hashing: Vec<(Vec<String>, PathBuf)> = Vec::new();
    for file in files {
        manifest::validate_relative(&file.rel)?;
        let key: Vec<String> = file.rel.split('/').map(str::to_owned).collect();
        anyhow::ensure!(
            key[0] != SYNC_RESULT_FILE,
            "{} is reserved for the sync result",
            file.rel
        );
        anyhow::ensure!(
            file.source.is_absolute(),
            "sources must be absolute: {}",
            file.source.display()
        );
        anyhow::ensure!(
            !items.contains_key(&key) && !hashing.iter().any(|(k, _)| *k == key),
            "{} is listed twice",
            file.rel
        );
        let meta = match std::fs::symlink_metadata(&file.source) {
            Ok(meta) => meta,
            Err(error) if tolerant => {
                tracing::debug!(%error, source = %file.source.display(), "sync: leaving out a vanished source");
                skipped += 1;
                continue;
            }
            Err(error) => anyhow::bail!("Could not read {}: {error}", file.source.display()),
        };
        let kind = meta.file_type();
        if kind.is_symlink() {
            let target = std::fs::read_link(&file.source)
                .ok()
                .and_then(|t| t.to_str().map(str::to_owned))
                .filter(|t| !t.is_empty());
            match target {
                Some(target) => {
                    items.insert(
                        key,
                        Item::Symlink {
                            target,
                            mode: manifest::mode_of(&meta),
                            source: file.source.clone(),
                        },
                    );
                }
                None if tolerant => skipped += 1,
                None => anyhow::bail!("Could not read the link {}", file.source.display()),
            }
        } else if kind.is_dir() {
            items.insert(
                key,
                Item::Dir {
                    mode: manifest::mode_of(&meta),
                    source: file.source.clone(),
                },
            );
        } else if kind.is_file() {
            hashing.push((key, file.source.clone()));
        } else if tolerant {
            skipped += 1;
        } else {
            anyhow::bail!(
                "{} is not a regular file, folder or symlink",
                file.source.display()
            );
        }
    }

    // Hash files in parallel; each result is a consistent (stat, hash).
    let hashed = parallel(&hashing, |(_, source)| hash_source(source, cache));
    let mut hashes: HashMap<Vec<String>, (FileStat, FileHash, u32)> = HashMap::new();
    for ((key, source), result) in hashing.into_iter().zip(hashed) {
        match result {
            Ok(found) => {
                hashes.insert(key.clone(), found);
                items.insert(key, Item::File { source });
            }
            Err(error) if tolerant => {
                tracing::debug!(%error, source = %source.display(), "sync: leaving out an unreadable source");
                skipped += 1;
            }
            Err(error) => anyhow::bail!("Could not read {}: {error}", source.display()),
        }
    }
    if let Err(error) = cache.lock().unwrap().save() {
        tracing::warn!(%error, "could not save the file hash cache");
    }

    // Implied parent folders; a parent must not be a file or a link.
    let keys: Vec<Vec<String>> = items.keys().cloned().collect();
    for key in keys {
        for depth in 1..key.len() {
            let parent = key[..depth].to_vec();
            match items.get(&parent) {
                Some(Item::Dir { .. }) => {}
                Some(_) => anyhow::bail!(
                    "{} is inside {}, which is not a folder",
                    key.join("/"),
                    parent.join("/")
                ),
                None => {
                    items.insert(
                        parent,
                        Item::Dir {
                            mode: 0,
                            source: PathBuf::new(),
                        },
                    );
                }
            }
        }
    }

    let mut built = Built {
        manifest: Manifest::default(),
        sources: Vec::with_capacity(items.len()),
        skipped,
    };
    let mut stats = Vec::with_capacity(items.len());
    // BTreeMap order over components is pre-order: a folder sorts before
    // everything inside it.
    for (key, item) in items {
        let path = key.join("/");
        let (entry, source, stat) = match item {
            Item::Dir { mode, source } => (
                Entry {
                    path,
                    kind: EntryKind::Dir,
                    size: 0,
                    mode,
                    target: None,
                    sha256: None,
                    blocks: None,
                    mtime_ms: None,
                },
                source,
                None,
            ),
            Item::Symlink {
                target,
                mode,
                source,
            } => (
                Entry {
                    path,
                    kind: EntryKind::Symlink,
                    size: 0,
                    mode,
                    target: Some(target),
                    sha256: None,
                    blocks: None,
                    mtime_ms: None,
                },
                source,
                None,
            ),
            Item::File { source } => {
                let (stat, hash, mode) = hashes.remove(&key).expect("hashed above");
                (
                    Entry {
                        path,
                        kind: EntryKind::File,
                        size: stat.size,
                        mode,
                        target: None,
                        sha256: Some(hex(&hash.sha256)),
                        blocks: hash
                            .blocks
                            .map(|blocks| blocks.iter().map(|b| hex(b)).collect()),
                        mtime_ms: Some(stat.mtime_ms()),
                    },
                    source,
                    Some(stat),
                )
            }
        };
        built.manifest.entries.push(entry);
        built.sources.push(source);
        stats.push(stat);
    }
    built.manifest.validate_sync()?;
    Ok(SyncBuilt { built, stats })
}

impl crate::Transfers {
    /// The whole-file SHA-256 (hex) of each path, through the same
    /// persistent cache sync sends use; `None` for anything that is not a
    /// readable regular file (symlinks are not followed).
    pub async fn hash_files(&self, paths: Vec<PathBuf>) -> anyhow::Result<Vec<Option<String>>> {
        let cache = self.hash_cache();
        tokio::task::spawn_blocking(move || {
            let hashes = parallel(&paths, |path| {
                path.is_absolute()
                    .then(|| hash_source(path, &cache).ok())
                    .flatten()
                    .map(|(_, hash, _)| hex(&hash.sha256))
            });
            if let Err(error) = cache.lock().unwrap().save() {
                tracing::warn!(%error, "could not save the file hash cache");
            }
            hashes
        })
        .await
        .map_err(|e| anyhow::anyhow!("Could not hash the files: {e}"))
    }
}

/// Hash one source file, retrying while it changes underneath.
fn hash_source(
    source: &Path,
    cache: &Mutex<HashCache>,
) -> std::io::Result<(FileStat, FileHash, u32)> {
    for _ in 0..HASH_ATTEMPTS {
        let file = open_nofollow(source)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        let stat = FileStat::of(&meta);
        let mode = manifest::mode_of(&meta);
        if let Some(hash) = cache.lock().unwrap().get(source, &stat) {
            return Ok((stat, hash, mode));
        }
        let hash = match hash_file(&file, stat.size) {
            Ok(hash) => hash,
            Err(error) if error.kind() == std::io::ErrorKind::Other => continue,
            Err(error) => return Err(error),
        };
        if FileStat::of(&file.metadata()?).same_content(&stat) {
            cache.lock().unwrap().put(source, &stat, &hash);
            return Ok((stat, hash, mode));
        }
    }
    Err(std::io::Error::other("it keeps changing"))
}

/// Open a regular file for reading without following a symlink at `path`
/// (and without blocking on a FIFO swapped in).
pub(crate) fn open_nofollow(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

/// Run `f` over `items` on a few threads; results keep the input order.
pub(crate) fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .min(items.len());
    if workers <= 1 {
        return items.iter().map(f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut results: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return done;
                        };
                        done.push((index, f(item)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    results.sort_unstable_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, r)| r).collect()
}

// ── receiving ──────────────────────────────────────────────────────────

/// What a receiver's basis holds for one file entry.
pub(crate) enum Basis {
    /// Nothing usable (no root, no file, a symlink, unreadable).
    None,
    /// Same size and SHA-256 in basis folder `index`: nothing travels.
    Unchanged { index: u32 },
    /// Different content; matching blocks can seed the `.part`.
    Seed {
        /// The basis's own block hashes, when known.
        blocks: Option<Vec<[u8; 32]>>,
    },
}

/// One file of a basis folder.
pub(crate) struct BasisFile {
    pub file: File,
    pub path: PathBuf,
    pub stat: FileStat,
    /// Position of its folder in the root's basis list.
    pub index: u32,
}

/// The basis file for `entry`: `<folder>/<rest of path>` in the first of
/// the root's basis folders that has anything at that path. A later folder
/// is never consulted once an earlier one has an entry there — if that
/// entry is not a regular file (a symlink, a folder) there is no basis.
/// Nothing beneath a basis folder is followed through a symlink.
pub(crate) fn basis_file(
    entry: &Entry,
    basis: &HashMap<String, Vec<PathBuf>>,
) -> Option<BasisFile> {
    let (root, rest) = entry.path.split_once('/')?;
    for (index, folder) in basis.get(root)?.iter().enumerate() {
        let file = match open_beneath(folder, rest) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let meta = file.metadata().ok()?;
        return meta.is_file().then(|| BasisFile {
            file,
            path: manifest::join(folder, rest),
            stat: FileStat::of(&meta),
            index: index as u32,
        });
    }
    None
}

/// Compare `entry` with the basis copy of its root, if the grant has one.
/// Never follows a symlink beneath the basis root.
pub(crate) fn examine_basis(
    entry: &Entry,
    basis: &HashMap<String, Vec<PathBuf>>,
    cache: &Mutex<HashCache>,
) -> Basis {
    let Some(expected) = entry.sha256.as_deref() else {
        return Basis::None;
    };
    let Some(BasisFile {
        file,
        path,
        stat,
        index,
    }) = basis_file(entry, basis)
    else {
        return Basis::None;
    };
    let mut blocks = None;
    if stat.size == entry.size {
        let cached = cache.lock().unwrap().get(&path, &stat);
        let hash = match cached {
            Some(hash) => hash,
            None => match hash_file(&file, stat.size) {
                Ok(hash) => {
                    if file
                        .metadata()
                        .is_ok_and(|m| FileStat::of(&m).same_content(&stat))
                    {
                        cache.lock().unwrap().put(&path, &stat, &hash);
                    }
                    hash
                }
                Err(_) => return Basis::None,
            },
        };
        if hex(&hash.sha256) == expected {
            return Basis::Unchanged { index };
        }
        blocks = hash.blocks;
    }
    if entry.blocks.is_none() {
        return Basis::None;
    }
    Basis::Seed { blocks }
}

/// Copy the basis blocks whose hash matches the sender's block at the same
/// index into `part`, fsync it, and return the blocks it now holds. Every
/// copied block is hashed again: the basis may have changed since.
pub(crate) fn seed_part(
    part: &Path,
    entry: &Entry,
    basis: &HashMap<String, Vec<PathBuf>>,
    basis_blocks: Option<&[[u8; 32]]>,
) -> anyhow::Result<RangeSet> {
    let Some(wanted) = entry.blocks.as_ref() else {
        return Ok(RangeSet::default());
    };
    let Some(BasisFile {
        file: basis, stat, ..
    }) = basis_file(entry, basis)
    else {
        return Ok(RangeSet::default());
    };
    let basis_size = stat.size;
    let mut seeded = RangeSet::default();
    let mut out: Option<File> = None;
    let mut buffer = vec![0u8; BLOCK_SIZE as usize];
    for index in 0..block_count(entry.size) {
        let Some(want) = wanted.get(index as usize).and_then(|h| unhex32(h)) else {
            continue;
        };
        let offset = index * BLOCK_SIZE;
        let length = block_len(entry.size, index);
        if offset + length > basis_size {
            break;
        }
        // Known basis hashes rule out blocks without reading them.
        if let Some(have) = basis_blocks.and_then(|b| b.get(index as usize))
            && block_len(basis_size, index) == length
            && *have != want
        {
            continue;
        }
        let data = &mut buffer[..length as usize];
        if crate::sender::read_at(&basis, data, offset).is_err() {
            break;
        }
        if <[u8; 32]>::from(Sha256::digest(&*data)) != want {
            continue;
        }
        let handle = match out.as_ref() {
            Some(handle) => handle,
            None => out.insert(crate::receiver::open_part(part)?),
        };
        crate::receiver::write_at(handle, data, offset)?;
        seeded.insert(index);
    }
    if let Some(handle) = out {
        // Recorded as verified only once the bytes are durable.
        handle.sync_data()?;
    }
    Ok(seeded)
}

/// Open `root/rest` for reading. `root` is the receiver's own folder and may
/// itself be a symlink; no component of `rest` may be one.
pub(crate) fn open_beneath(root: &Path, rest: &str) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        let mut dir = File::open(root)?;
        let names: Vec<&str> = rest.split('/').collect();
        for (index, name) in names.iter().enumerate() {
            let last = index + 1 == names.len();
            let name = std::ffi::CString::new(*name)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            flags |= if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
            let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            dir = unsafe { File::from_raw_fd(fd) };
        }
        Ok(dir)
    }
    #[cfg(not(unix))]
    {
        let mut path = root.to_path_buf();
        for name in rest.split('/') {
            path.push(name);
            if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(std::io::Error::other("a symlink in the basis path"));
            }
        }
        File::open(path)
    }
}

pub(crate) fn unhex32(text: &str) -> Option<[u8; 32]> {
    if !manifest::is_sha256_hex(text) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(dir: &Path) -> Mutex<HashCache> {
        Mutex::new(HashCache::empty(dir.join("cache.bin")))
    }

    #[test]
    fn layouts_imply_parent_folders_in_pre_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        std::fs::write(&a, b"a").unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        let files = vec![
            SyncSource {
                rel: "ws/src/deep/a.txt".into(),
                source: a.clone(),
            },
            SyncSource {
                rel: "ws/empty".into(),
                source: empty,
            },
            SyncSource {
                rel: "hs/session.jsonl".into(),
                source: a.clone(),
            },
        ];
        let built = build(&files, false, &cache(dir.path())).unwrap();
        let paths: Vec<&str> = built
            .built
            .manifest
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "hs",
                "hs/session.jsonl",
                "ws",
                "ws/empty",
                "ws/src",
                "ws/src/deep",
                "ws/src/deep/a.txt"
            ]
        );
        let file = &built.built.manifest.entries[6];
        assert_eq!(
            file.sha256.as_deref(),
            Some(hex(&Sha256::digest(b"a")).as_str())
        );
        assert!(file.blocks.is_none() && file.mtime_ms.is_some());
        assert!(built.stats[6].is_some() && built.stats[2].is_none());

        // A file can't also be a folder, and nothing is listed twice.
        let clash = vec![
            SyncSource {
                rel: "ws/a".into(),
                source: a.clone(),
            },
            SyncSource {
                rel: "ws/a/b".into(),
                source: a.clone(),
            },
        ];
        assert!(build(&clash, false, &cache(dir.path())).is_err());
        let twice = vec![
            SyncSource {
                rel: "ws/a".into(),
                source: a.clone(),
            },
            SyncSource {
                rel: "ws/a".into(),
                source: a.clone(),
            },
        ];
        assert!(build(&twice, false, &cache(dir.path())).is_err());
        for bad in ["../x", "ws/../x", "/abs", ".zeron-sync.json"] {
            let files = vec![SyncSource {
                rel: bad.into(),
                source: a.clone(),
            }];
            assert!(build(&files, true, &cache(dir.path())).is_err(), "{bad}");
        }
    }

    #[test]
    fn vanished_sources_fail_unless_tolerant() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        std::fs::write(&a, b"a").unwrap();
        let files = vec![
            SyncSource {
                rel: "ws/a.txt".into(),
                source: a,
            },
            SyncSource {
                rel: "ws/gone.txt".into(),
                source: dir.path().join("gone.txt"),
            },
        ];
        assert!(build(&files, false, &cache(dir.path())).is_err());
        let built = build(&files, true, &cache(dir.path())).unwrap();
        assert_eq!(built.built.skipped, 1);
        assert_eq!(built.built.manifest.file_count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn basis_lookups_never_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"s").unwrap();
        std::fs::write(root.join("real/f"), b"f").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("dirlink")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), root.join("filelink")).unwrap();
        assert!(open_beneath(&root, "real/f").is_ok());
        assert!(open_beneath(&root, "dirlink/secret").is_err());
        assert!(open_beneath(&root, "filelink").is_err());
        // The root itself may be reached through a link.
        let root_link = dir.path().join("root-link");
        std::os::unix::fs::symlink(&root, &root_link).unwrap();
        assert!(open_beneath(&root_link, "real/f").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_first_basis_folder_with_an_entry_wins() {
        let dir = tempfile::tempdir().unwrap();
        let (r1, landing) = (dir.path().join("r1"), dir.path().join("landing"));
        std::fs::create_dir_all(r1.join("sub")).unwrap();
        std::fs::create_dir_all(landing.join("sub")).unwrap();
        std::fs::write(r1.join("a"), b"r1").unwrap();
        std::fs::write(landing.join("a"), b"landing").unwrap();
        std::fs::write(landing.join("b"), b"landing").unwrap();
        std::os::unix::fs::symlink("b", r1.join("c")).unwrap();
        std::fs::write(landing.join("c"), b"landing").unwrap();
        let basis = HashMap::from([("ws".to_string(), vec![r1.clone(), landing.clone()])]);
        let found = |path: &str| {
            let entry = Entry {
                path: path.into(),
                kind: EntryKind::File,
                size: 0,
                mode: 0,
                target: None,
                sha256: None,
                blocks: None,
                mtime_ms: None,
            };
            basis_file(&entry, &basis).map(|b| (b.index, b.path))
        };
        assert_eq!(found("ws/a"), Some((0, r1.join("a"))));
        assert_eq!(found("ws/b"), Some((1, landing.join("b"))));
        assert_eq!(
            found("ws/c"),
            None,
            "a link in the first folder hides the landing"
        );
        assert_eq!(found("ws/missing"), None);
        assert_eq!(found("other/a"), None);
    }

    #[test]
    fn an_empty_layout_is_a_valid_round() {
        let dir = tempfile::tempdir().unwrap();
        let built = build(&[], false, &cache(dir.path())).unwrap();
        assert!(built.built.manifest.entries.is_empty());
        let gone = vec![SyncSource {
            rel: "ws/gone".into(),
            source: dir.path().join("gone"),
        }];
        let built = build(&gone, true, &cache(dir.path())).unwrap();
        assert!(built.built.manifest.entries.is_empty());
        assert_eq!(built.built.skipped, 1);
    }

    #[test]
    fn tickets_are_tame() {
        assert!(valid_ticket("move-1234_ab.c"));
        assert!(!valid_ticket(""));
        assert!(!valid_ticket("a/b"));
        assert!(!valid_ticket(&"x".repeat(200)));
        assert_eq!(unhex32(&hex(&[0xab; 32])), Some([0xab; 32]));
        assert_eq!(unhex32("zz"), None);
    }
}
