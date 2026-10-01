//! Cursor session port. Each Zeron-run Cursor agent owns a JSONL store dir
//! (`agents/<uuid>/`: `agents.ndjson`, `runs.ndjson`, `run_events.ndjson`,
//! `checkpoints.ndjson`, plus the shim's receipt), found through the
//! `by-agent/<agentId>` marker holding the store's ABSOLUTE path. The agent
//! document records `cwd`; the store moves whole and the marker is rewritten
//! for this device. Agents from before per-run stores live in the SDK's
//! workspace-keyed SQLite and are not ported.
//!
//! Checkpoint blobs are opaque conversation state and are carried verbatim;
//! paths inside them still name the old device.
use crate::portable::{
    Format, Import, Placer, PortRoots, Rewriter, Rule, Seg, SessionExport, checked_name,
    list_files, write_atomic,
};
use anyhow::{Context, bail};
use serde_json::json;
use std::path::{Path, PathBuf};
use zeron_proto::HarnessId;

const AGENTS: &str = "agents.ndjson";

const RULES: &[Rule] = &[Rule {
    kind: None,
    path: &[Seg::Key("cwd")],
}];

/// Agent ids are file names in the marker index (as `Lease::acquire` checks).
fn agent_id(id: &str) -> anyhow::Result<&str> {
    if id.is_empty()
        || id.len() > 200
        || id == "."
        || id == ".."
        || id.contains(['/', '\\', ':', '\0'])
    {
        bail!("unsupported Cursor agent id {id:?}");
    }
    Ok(id)
}

fn marker(roots: &PortRoots, id: &str) -> PathBuf {
    roots.cursor_state.join("by-agent").join(id)
}

fn store_of(roots: &PortRoots, id: &str) -> Option<PathBuf> {
    let path = std::fs::read_to_string(marker(roots, id)).ok()?;
    let dir = PathBuf::from(path.trim());
    (!path.trim().is_empty() && dir.is_dir()).then_some(dir)
}

fn recorded_cwd(store: &Path, id: &str) -> Option<String> {
    std::fs::read_to_string(store.join(AGENTS))
        .ok()?
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|doc| doc["agentId"] == id)?["cwd"]
        .as_str()
        .map(str::to_owned)
}

pub(crate) fn export(roots: &PortRoots, id: &str, cwd: &str) -> anyhow::Result<SessionExport> {
    let id = agent_id(id)?;
    let store = store_of(roots, id).with_context(|| {
        format!(
            "Cursor agent {id} has no Zeron store on this device (older agents can't move natively)"
        )
    })?;
    // The owner lease/pid marker belong to this device's processes.
    let files = list_files(&store, "store", |name| {
        name.starts_with(".zeron-owner") || name.contains(".tmp")
    })?;
    if !files.iter().any(|f| f.rel == format!("store/{AGENTS}")) {
        bail!("Cursor agent {id}'s store has no agent document");
    }
    Ok(SessionExport {
        harness: HarnessId::Cursor,
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        files,
        meta: json!({
            "storeName": store.file_name().map(|n| n.to_string_lossy()),
            "recordedCwd": recorded_cwd(&store, id),
        }),
    })
}

pub(crate) fn import(import: &mut Import, roots: &PortRoots) -> anyhow::Result<()> {
    let id = agent_id(&import.export.session_id)?.to_owned();
    let name = match import.meta_str("storeName") {
        Some(name) => checked_name(name)?.to_owned(),
        None => uuid::Uuid::new_v4().to_string(),
    };
    let store = std::path::absolute(roots.cursor_state.join("agents").join(name))?;
    let previous = store_of(roots, &id);
    let maps = import.cwd_maps();
    let rewriter = Rewriter::new(&maps, RULES);
    let files: Vec<_> = import.files().map(|(r, p)| (r.to_owned(), p)).collect();
    let mut placer = Placer::new(&roots.cursor_state, &mut import.warnings);
    std::fs::create_dir_all(&store)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700))?;
    }
    for (rel, src) in &files {
        let Some(rest) = rel.strip_prefix("store/") else {
            bail!("unexpected file in a Cursor session export: {rel}");
        };
        let rewrite = (rest == AGENTS).then_some((&rewriter, Format::Lines));
        placer.place(src, &store.join(rest), rewrite)?;
    }
    write_atomic(
        &marker(roots, &id),
        store
            .to_str()
            .context("non-UTF-8 Cursor store path")?
            .as_bytes(),
    )?;
    // An earlier visit's store for the same agent is superseded.
    if let Some(previous) = previous
        && previous != store
        && previous.starts_with(roots.cursor_state.join("agents"))
    {
        placer.retire(&previous)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::export_session;
    use crate::portable::test_support::*;
    use serde_json::Value;

    const ID: &str = "bc-6f1e2d3c-4b5a-4968-8776-655443322110";
    const STORE: &str = "2b7c1d9e-8f60-4a51-b2c3-d4e5f6a7b8c9";

    fn fixture(a: &PortRoots) -> PathBuf {
        let store = a.cursor_state.join("agents").join(STORE);
        let agent = json!({"agentId": ID, "cwd": OLD_CWD, "status": "idle", "activeRunId": null, "createdAt": 1, "updatedAt": 2, "latestCheckpoint": {"schemaVersion": 1, "rootBlobId": "blob-1"}});
        write(&store.join(AGENTS), format!("{agent}\n"));
        write(
            &store.join("runs.ndjson"),
            json!({"runId": "r1", "agentId": ID, "turnNumber": 1, "status": "finished", "createdAt": 1, "updatedAt": 2}).to_string() + "\n",
        );
        write(
            &store.join("run_events.ndjson"),
            json!({"runId": "r1", "seq": 0, "eventType": "tool", "payload": {"cwd": OLD_CWD, "text": "ls /Users/alice/dev/proj"}}).to_string() + "\n",
        );
        write(
            &store.join("checkpoints.ndjson"),
            "{\"blobId\":\"blob-1\",\"data\":\"AAEC\"}\n",
        );
        write(&store.join(".zeron-user-receipt.json"), r#"{"version":1}"#);
        write(
            &store.join(".zeron-owner.json"),
            r#"{"pid":4242,"parentPid":1}"#,
        );
        write(&store.join(".zeron-owner.lock"), "");
        write(
            &a.cursor_state.join("by-agent").join(ID),
            store.to_str().unwrap(),
        );
        store
    }

    #[tokio::test]
    async fn moves_the_store_and_the_driver_lease_finds_it() {
        let (_dir, a, b) = devices();
        let source = fixture(&a);
        let (export, imported) = carry(HarnessId::Cursor, &a, &b, ID, OLD_CWD, NEW_CWD);
        assert_eq!(imported.unwrap().session_id, ID);
        assert_eq!(export.meta["recordedCwd"], OLD_CWD);
        assert!(export.files.iter().all(|f| !f.rel.contains(".zeron-owner")));

        let lease = super::super::state::Lease::acquire(&b.cursor_state, Some(ID))
            .await
            .unwrap();
        let store = b.cursor_state.join("agents").join(STORE);
        assert_eq!(lease.store_dir.as_deref(), Some(store.as_path()));
        let agent: Value = serde_json::from_str(&lines(&store.join(AGENTS))[0]).unwrap();
        assert_eq!(agent["cwd"], NEW_CWD);
        assert_eq!(agent["latestCheckpoint"]["rootBlobId"], "blob-1");
        for name in [
            "runs.ndjson",
            "run_events.ndjson",
            "checkpoints.ndjson",
            ".zeron-user-receipt.json",
        ] {
            assert_eq!(
                std::fs::read(store.join(name)).unwrap(),
                std::fs::read(source.join(name)).unwrap(),
                "{name}"
            );
        }
        assert!(!store.join(".zeron-owner.json").exists());
    }

    #[test]
    fn a_new_store_supersedes_an_earlier_visit() {
        let (_dir, a, b) = devices();
        fixture(&a);
        let old = b.cursor_state.join("agents/old-store");
        write(&old.join(AGENTS), "{}\n");
        write(
            &b.cursor_state.join("by-agent").join(ID),
            old.to_str().unwrap(),
        );
        let imported = carry(HarnessId::Cursor, &a, &b, ID, OLD_CWD, NEW_CWD)
            .1
            .unwrap();
        assert!(!old.exists());
        assert_eq!(imported.warnings.len(), 1);
        assert_eq!(
            std::fs::read_to_string(b.cursor_state.join("by-agent").join(ID)).unwrap(),
            b.cursor_state.join("agents").join(STORE).to_str().unwrap()
        );
    }

    #[test]
    fn markerless_agents_are_not_portable() {
        let (_dir, a, _) = devices();
        assert!(export_session(HarnessId::Cursor, &a, ID, OLD_CWD).is_err());
        assert!(export_session(HarnessId::Cursor, &a, "../x", OLD_CWD).is_err());
    }
}
