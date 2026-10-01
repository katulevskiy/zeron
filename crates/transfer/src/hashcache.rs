//! Content hashes that survive between sync rounds and moves.
//!
//! Sync manifests carry every file's SHA-256 (and, for large files, the
//! SHA-256 of each block), and a receiver hashes its basis files to find
//! what it already has. Re-reading an unchanged tree each round would
//! dominate a move, so hashes are cached by absolute path and keyed on what
//! a change touches: size, mtime, ctime and inode. A file modified within
//! [`RACY_WINDOW_NS`] of being hashed is not cached — a second write in the
//! same timestamp tick would otherwise go unnoticed.
//!
//! The cache is one binary file under the service's state folder, checked
//! by a trailing SHA-256: a torn or corrupt file loads as empty. It is
//! bounded in entries and block hashes; the least recently used entries go
//! first.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::blocks::BLOCK_SIZE;

/// Files at least this large carry per-block hashes.
pub const BLOCK_LIST_MIN: u64 = 4 * BLOCK_SIZE;
/// Above this many blocks (64 GiB) a file travels without a block list:
/// the manifest frame would grow too large.
pub const BLOCK_LIST_MAX: u64 = 65_536;
const MAGIC: &[u8; 4] = b"ZHC1";
const MAX_ENTRIES: usize = 200_000;
/// Block hashes kept in total (32 MiB of hashes, ~1 TiB of files).
const MAX_BLOCKS: usize = 1 << 20;
const MAX_PATH_BYTES: usize = 16 * 1024;
/// Files changed this recently are hashed but not cached.
pub const RACY_WINDOW_NS: i64 = 2_000_000_000;

/// What identifies one version of a file without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub ino: u64,
    pub dev: u64,
}

impl FileStat {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                size: meta.len(),
                mtime_ns: meta
                    .mtime()
                    .saturating_mul(1_000_000_000)
                    .saturating_add(meta.mtime_nsec()),
                ctime_ns: meta
                    .ctime()
                    .saturating_mul(1_000_000_000)
                    .saturating_add(meta.ctime_nsec()),
                ino: meta.ino(),
                dev: meta.dev(),
            }
        }
        #[cfg(not(unix))]
        {
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
                .unwrap_or_default();
            Self {
                size: meta.len(),
                mtime_ns,
                ctime_ns: 0,
                ino: 0,
                dev: 0,
            }
        }
    }

    /// Same content as far as a stat can tell (ctime aside: a `chmod`
    /// doesn't change the bytes).
    pub fn same_content(&self, other: &FileStat) -> bool {
        self.size == other.size
            && self.mtime_ns == other.mtime_ns
            && self.ino == other.ino
            && self.dev == other.dev
    }

    pub fn mtime_ms(&self) -> i64 {
        self.mtime_ns.div_euclid(1_000_000)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHash {
    pub sha256: [u8; 32],
    /// One hash per [`BLOCK_SIZE`] block (files of [`BLOCK_LIST_MIN`] or more).
    pub blocks: Option<Vec<[u8; 32]>>,
}

/// Whether a file of `size` bytes carries a block list.
pub fn wants_blocks(size: u64) -> bool {
    size >= BLOCK_LIST_MIN && crate::blocks::block_count(size) <= BLOCK_LIST_MAX
}

/// Read `size` bytes of `file` from its start: the whole-file SHA-256 and,
/// when [`wants_blocks`], each block's. A file that turns out shorter or
/// longer than `size` changed while it was read.
pub fn hash_file(file: &std::fs::File, size: u64) -> std::io::Result<FileHash> {
    let mut reader = file;
    let mut whole = Sha256::new();
    let mut blocks = wants_blocks(size).then(Vec::new);
    let mut buffer = vec![0u8; BLOCK_SIZE as usize];
    let mut left = size;
    while left > 0 {
        let want = left.min(BLOCK_SIZE) as usize;
        reader.read_exact(&mut buffer[..want]).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                std::io::Error::other("the file shrank while it was read")
            } else {
                e
            }
        })?;
        whole.update(&buffer[..want]);
        if let Some(blocks) = blocks.as_mut() {
            blocks.push(Sha256::digest(&buffer[..want]).into());
        }
        left -= want as u64;
    }
    let mut probe = [0u8; 1];
    if reader.read(&mut probe)? != 0 {
        return Err(std::io::Error::other("the file grew while it was read"));
    }
    Ok(FileHash {
        sha256: whole.finalize().into(),
        blocks,
    })
}

struct Cached {
    stat: FileStat,
    hash: FileHash,
    used: u64,
}

pub struct HashCache {
    path: PathBuf,
    entries: HashMap<String, Cached>,
    /// Bumped per load; an entry's `used` is the generation it was last hit.
    generation: u64,
    dirty: bool,
    racy_window_ns: i64,
}

impl HashCache {
    /// An empty cache that saves to `path`.
    pub fn empty(path: PathBuf) -> Self {
        Self {
            path,
            entries: HashMap::new(),
            generation: 1,
            dirty: false,
            racy_window_ns: RACY_WINDOW_NS,
        }
    }

    /// The cache saved at `path`; empty when missing, torn or corrupt.
    pub fn load(path: PathBuf) -> Self {
        let mut cache = Self::empty(path);
        let Ok(bytes) = std::fs::read(&cache.path) else {
            return cache;
        };
        match decode(&bytes) {
            Some((generation, entries)) => {
                cache.generation = generation.saturating_add(1);
                cache.entries = entries;
            }
            None => {
                tracing::debug!(path = %cache.path.display(), "discarding a corrupt hash cache");
                cache.dirty = true;
            }
        }
        cache
    }

    #[cfg(test)]
    fn with_racy_window(mut self, ns: i64) -> Self {
        self.racy_window_ns = ns;
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The hash of `path` when it is still at version `stat`.
    pub fn get(&mut self, path: &Path, stat: &FileStat) -> Option<FileHash> {
        let key = path.to_str()?;
        let generation = self.generation;
        let cached = self.entries.get_mut(key)?;
        if cached.stat != *stat {
            return None;
        }
        if cached.used != generation {
            cached.used = generation;
            self.dirty = true;
        }
        Some(cached.hash.clone())
    }

    /// Remember `hash` for `path` at version `stat` (unless the version is
    /// too fresh to trust).
    pub fn put(&mut self, path: &Path, stat: &FileStat, hash: &FileHash) {
        let Some(key) = path.to_str().filter(|k| k.len() <= MAX_PATH_BYTES) else {
            return;
        };
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
            .unwrap_or_default();
        if now_ns.saturating_sub(stat.mtime_ns) < self.racy_window_ns {
            return;
        }
        self.entries.insert(
            key.to_owned(),
            Cached {
                stat: *stat,
                hash: hash.clone(),
                used: self.generation,
            },
        );
        self.dirty = true;
    }

    /// Write the cache back (bounded) when anything changed.
    pub fn save(&mut self) -> anyhow::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.evict();
        let bytes = encode(self.generation, &self.entries);
        crate::service::write_atomic(&self.path, &bytes)?;
        self.dirty = false;
        Ok(())
    }

    /// Drop least recently used entries beyond the bounds.
    fn evict(&mut self) {
        let blocks: usize = self.entries.values().map(block_len).sum();
        if self.entries.len() <= MAX_ENTRIES && blocks <= MAX_BLOCKS {
            return;
        }
        let mut order: Vec<(u64, String)> = self
            .entries
            .iter()
            .map(|(k, c)| (c.used, k.clone()))
            .collect();
        order.sort_unstable_by_key(|(used, _)| std::cmp::Reverse(*used));
        let (mut kept, mut kept_blocks) = (0usize, 0usize);
        for (_, key) in order {
            let size = self.entries.get(&key).map(block_len).unwrap_or_default();
            if kept < MAX_ENTRIES && kept_blocks + size <= MAX_BLOCKS {
                kept += 1;
                kept_blocks += size;
            } else {
                self.entries.remove(&key);
            }
        }
    }
}

fn block_len(cached: &Cached) -> usize {
    cached.hash.blocks.as_ref().map_or(0, Vec::len)
}

fn encode(generation: u64, entries: &HashMap<String, Cached>) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + entries.len() * 128);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (path, cached) in entries {
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        let stat = &cached.stat;
        out.extend_from_slice(&stat.size.to_le_bytes());
        out.extend_from_slice(&stat.mtime_ns.to_le_bytes());
        out.extend_from_slice(&stat.ctime_ns.to_le_bytes());
        out.extend_from_slice(&stat.ino.to_le_bytes());
        out.extend_from_slice(&stat.dev.to_le_bytes());
        out.extend_from_slice(&cached.used.to_le_bytes());
        out.extend_from_slice(&cached.hash.sha256);
        match &cached.hash.blocks {
            Some(blocks) => {
                out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
                for block in blocks {
                    out.extend_from_slice(block);
                }
            }
            None => out.extend_from_slice(&u32::MAX.to_le_bytes()),
        }
    }
    let check: [u8; 32] = Sha256::digest(&out).into();
    out.extend_from_slice(&check);
    out
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn hash(&mut self) -> Option<[u8; 32]> {
        self.take(32)?.try_into().ok()
    }
}

fn decode(bytes: &[u8]) -> Option<(u64, HashMap<String, Cached>)> {
    let body_len = bytes.len().checked_sub(32)?;
    let (body, check) = bytes.split_at(body_len);
    if <[u8; 32]>::from(Sha256::digest(body)) != *check {
        return None;
    }
    let mut cursor = Cursor(body);
    if cursor.take(4)? != MAGIC {
        return None;
    }
    let generation = cursor.u64()?;
    let count = cursor.u32()? as usize;
    if count > MAX_ENTRIES * 2 {
        return None;
    }
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        let path_len = cursor.u32()? as usize;
        if path_len > MAX_PATH_BYTES {
            return None;
        }
        let path = std::str::from_utf8(cursor.take(path_len)?).ok()?.to_owned();
        let stat = FileStat {
            size: cursor.u64()?,
            mtime_ns: cursor.i64()?,
            ctime_ns: cursor.i64()?,
            ino: cursor.u64()?,
            dev: cursor.u64()?,
        };
        let used = cursor.u64()?;
        let sha256 = cursor.hash()?;
        let blocks = match cursor.u32()? {
            u32::MAX => None,
            n if n as u64 <= BLOCK_LIST_MAX => {
                let mut blocks = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    blocks.push(cursor.hash()?);
                }
                Some(blocks)
            }
            _ => return None,
        };
        entries.insert(
            path,
            Cached {
                stat,
                hash: FileHash { sha256, blocks },
                used,
            },
        );
    }
    cursor.0.is_empty().then_some((generation, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_of(path: &Path) -> FileStat {
        FileStat::of(&std::fs::metadata(path).unwrap())
    }

    fn age(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
            .unwrap();
    }

    #[test]
    fn hashes_cover_the_whole_file_and_each_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        let data: Vec<u8> = (0..(4 * BLOCK_SIZE + 10))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&path, &data).unwrap();
        let hash = hash_file(&std::fs::File::open(&path).unwrap(), data.len() as u64).unwrap();
        assert_eq!(hash.sha256, <[u8; 32]>::from(Sha256::digest(&data)));
        let blocks = hash.blocks.unwrap();
        assert_eq!(blocks.len(), 5);
        assert_eq!(
            blocks[4],
            <[u8; 32]>::from(Sha256::digest(&data[4 * BLOCK_SIZE as usize..]))
        );
        let small = dir.path().join("small");
        std::fs::write(&small, b"abc").unwrap();
        let hash = hash_file(&std::fs::File::open(&small).unwrap(), 3).unwrap();
        assert!(hash.blocks.is_none());
        // A size that no longer matches is a change.
        assert!(hash_file(&std::fs::File::open(&small).unwrap(), 4).is_err());
        assert!(hash_file(&std::fs::File::open(&small).unwrap(), 2).is_err());
    }

    #[test]
    fn a_cached_hash_survives_a_reload_until_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("cache.bin");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"hello").unwrap();
        age(&file);
        let stat = stat_of(&file);
        let hash = hash_file(&std::fs::File::open(&file).unwrap(), 5).unwrap();
        let mut cache = HashCache::load(store.clone());
        assert!(cache.get(&file, &stat).is_none());
        cache.put(&file, &stat, &hash);
        cache.save().unwrap();

        let mut cache = HashCache::load(store.clone());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&file, &stat), Some(hash.clone()));
        // Another version of the file misses.
        std::fs::write(&file, b"hellO").unwrap();
        age(&file);
        assert!(cache.get(&file, &stat_of(&file)).is_none());
        let mut other = stat;
        other.ino ^= 1;
        assert!(cache.get(&file, &other).is_none());
    }

    #[test]
    fn freshly_modified_files_are_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hot.txt");
        std::fs::write(&file, b"in flux").unwrap();
        let stat = stat_of(&file);
        let hash = hash_file(&std::fs::File::open(&file).unwrap(), 7).unwrap();
        let mut cache = HashCache::empty(dir.path().join("cache.bin"));
        cache.put(&file, &stat, &hash);
        assert!(
            cache.is_empty(),
            "a file written just now may change again unnoticed"
        );
        let mut cache = cache.with_racy_window(0);
        cache.put(&file, &stat, &hash);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_corrupt_or_truncated_cache_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("cache.bin");
        let mut cache = HashCache::empty(store.clone()).with_racy_window(0);
        let hash = FileHash {
            sha256: [7; 32],
            blocks: Some(vec![[1; 32]; 4]),
        };
        let stat = FileStat {
            size: 4 * BLOCK_SIZE,
            mtime_ns: 1,
            ctime_ns: 2,
            ino: 3,
            dev: 4,
        };
        cache.put(Path::new("/x/y"), &stat, &hash);
        cache.save().unwrap();
        let bytes = std::fs::read(&store).unwrap();
        assert_eq!(HashCache::load(store.clone()).len(), 1);

        let mut flipped = bytes.clone();
        flipped[20] ^= 1;
        std::fs::write(&store, &flipped).unwrap();
        assert!(HashCache::load(store.clone()).is_empty());
        std::fs::write(&store, &bytes[..bytes.len() / 2]).unwrap();
        assert!(HashCache::load(store.clone()).is_empty());
        std::fs::write(&store, b"").unwrap();
        assert!(HashCache::load(store.clone()).is_empty());
        // A corrupt cache is replaced on the next save.
        let mut cache = HashCache::load(store.clone());
        cache.save().unwrap();
        assert!(HashCache::load(store).is_empty());
    }

    #[test]
    fn the_least_recently_used_entries_are_evicted_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = HashCache::empty(dir.path().join("cache.bin")).with_racy_window(0);
        let hash = FileHash {
            sha256: [0; 32],
            blocks: None,
        };
        let stat = FileStat {
            size: 1,
            mtime_ns: 1,
            ctime_ns: 1,
            ino: 1,
            dev: 1,
        };
        for i in 0..MAX_ENTRIES + 10 {
            cache.put(Path::new(&format!("/old/{i}")), &stat, &hash);
        }
        cache.generation += 1;
        cache.put(Path::new("/new"), &stat, &hash);
        cache.save().unwrap();
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.get(Path::new("/new"), &stat).is_some());
    }
}
