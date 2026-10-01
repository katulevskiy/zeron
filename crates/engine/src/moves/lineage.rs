//! Where each moved chat's work lives on this device, and what it looked like
//! when it last left. A chat that comes back lands exactly where its work is
//! (the original checkout, the earlier landing), and only what changed
//! travels; files the user edited here in the meantime are backed up rather
//! than overwritten (see `target.rs`).
//!
//! One small JSON file per profile (`{store}/moves/lineage.json`), written
//! atomically. It holds paths and content hashes, never file contents.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Chats remembered at most (oldest dropped first).
const MAX_CHATS: usize = 500;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLineage {
    /// The chat's workspace root on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// The workspace was created by a move (a fresh worktree or folder), so
    /// a later move here may delete files in it freely.
    #[serde(default)]
    pub created_by_move: bool,
    /// Extra paths on this device by where they originally came from:
    /// `"<device id>:<path>"` → local path.
    #[serde(default)]
    pub extras: BTreeMap<String, String>,
    /// The last time the chat left this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub departure: Option<Departure>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Departure {
    pub move_id: String,
    pub to_device_id: String,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Workspace files (`rel` → SHA-256) as they were when the chat left.
    #[serde(default)]
    pub files: HashMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LineageFile {
    #[serde(default)]
    chats: HashMap<String, ChatLineage>,
}

pub struct LineageStore {
    path: PathBuf,
    state: Mutex<LineageFile>,
}

impl LineageStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("lineage.json");
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    pub fn get(&self, chat_id: &str) -> Option<ChatLineage> {
        self.lock().chats.get(chat_id).cloned()
    }

    /// Change one chat's record and persist.
    pub fn update(&self, chat_id: &str, f: impl FnOnce(&mut ChatLineage)) {
        let snapshot = {
            let mut state = self.lock();
            let entry = state.chats.entry(chat_id.to_owned()).or_default();
            f(entry);
            entry.updated_at = chrono::Utc::now().timestamp_millis();
            if state.chats.len() > MAX_CHATS {
                let mut by_age: Vec<(String, i64)> = state
                    .chats
                    .iter()
                    .map(|(id, c)| (id.clone(), c.updated_at))
                    .collect();
                by_age.sort_by_key(|(_, at)| *at);
                for (id, _) in by_age.into_iter().take(state.chats.len() - MAX_CHATS) {
                    state.chats.remove(&id);
                }
            }
            serde_json::to_vec(&*state)
        };
        let result = snapshot
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&self.path, &bytes));
        if let Err(error) = result {
            tracing::warn!(%error, "could not save move lineage");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LineageFile> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The key extras are remembered by: where they first came from.
pub fn extra_origin(device_id: &str, path: &str) -> String {
    format!("{device_id}:{path}")
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = LineageStore::open(dir.path());
        store.update("chat-1", |c| {
            c.workspace = Some("/home/bob/dev/proj".into());
            c.created_by_move = true;
            c.extras
                .insert(extra_origin("dev-a", "/Users/alice/Desktop/shot.png"), "/home/bob/Desktop/shot.png".into());
        });
        let reopened = LineageStore::open(dir.path());
        let chat = reopened.get("chat-1").unwrap();
        assert_eq!(chat.workspace.as_deref(), Some("/home/bob/dev/proj"));
        assert!(chat.created_by_move);
        assert_eq!(chat.extras.len(), 1);
        assert!(reopened.get("chat-2").is_none());
    }

    #[test]
    fn a_corrupt_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lineage.json"), b"{not json").unwrap();
        let store = LineageStore::open(dir.path());
        assert!(store.get("chat-1").is_none());
        store.update("chat-1", |c| c.workspace = Some("/x".into()));
        assert!(LineageStore::open(dir.path()).get("chat-1").is_some());
    }
}
