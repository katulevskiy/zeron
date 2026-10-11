//! Pseudonymous identity. Nothing sent is reversible to a person: the
//! distinct id is a salted SHA-256 of either a random per-install UUID or,
//! once signed in, the Zeron account id — so one person's devices collapse
//! into one user without the account id ever leaving the device.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

const SALT: &str = "zeron-telemetry-v1";

/// Truncated salted hash: 128 bits is plenty to stay collision-free across
/// installs while keeping payloads small.
pub fn hash(kind: &str, value: &str) -> String {
    let mut h = Sha256::new();
    h.update(SALT.as_bytes());
    h.update([0]);
    h.update(kind.as_bytes());
    h.update([0]);
    h.update(value.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Debug, Clone)]
pub(crate) struct Identity {
    /// Hash of the install UUID — stable across sign-in/out.
    pub install: Arc<str>,
    /// What events are attributed to: the account hash when signed in, else
    /// the install hash.
    pub distinct: Arc<str>,
    pub account_linked: bool,
}

impl Identity {
    pub fn new(install_id: &str) -> Self {
        let install: Arc<str> = hash("install", install_id).into();
        Identity {
            distinct: install.clone(),
            install,
            account_linked: false,
        }
    }

    pub fn with_account(&self, account_id: Option<&str>) -> Self {
        match account_id.filter(|id| !id.trim().is_empty()) {
            Some(id) => Identity {
                install: self.install.clone(),
                distinct: hash("account", id).into(),
                account_linked: true,
            },
            None => Identity {
                install: self.install.clone(),
                distinct: self.install.clone(),
                account_linked: false,
            },
        }
    }
}

/// Reads `<dir>/install-id`, minting (atomically) a fresh v4 UUID on first
/// run. Falls back to an ephemeral id if the disk refuses — telemetry must
/// never block startup.
pub(crate) fn load_or_create_install_id(dir: &Path) -> String {
    let path = dir.join("install-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if uuid::Uuid::parse_str(existing).is_ok() {
            return existing.to_string();
        }
    }
    let fresh = uuid::Uuid::new_v4().to_string();
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("install-id.{}.tmp", std::process::id()));
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(fresh.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(err) = write() {
        tracing::debug!(%err, "telemetry: install id not persisted");
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_salted_stable_and_kind_scoped() {
        assert_eq!(hash("install", "x"), hash("install", "x"));
        assert_ne!(hash("install", "x"), hash("account", "x"));
        assert_eq!(hash("install", "x").len(), 32);
    }

    #[test]
    fn install_id_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create_install_id(dir.path());
        let b = load_or_create_install_id(dir.path());
        assert_eq!(a, b);
    }

    #[test]
    fn account_link_switches_distinct_id_but_keeps_install() {
        let base = Identity::new("install-1");
        let linked = base.with_account(Some("user_123"));
        assert!(linked.account_linked);
        assert_eq!(linked.install, base.install);
        assert_ne!(linked.distinct, base.distinct);
        let unlinked = linked.with_account(None);
        assert_eq!(unlinked.distinct, base.distinct);
    }
}
