//! Pi session port. Pi keeps one JSONL per session whose `session` header
//! carries the cwd (Pi restores it as the session cwd, so it must be
//! rewritten), under `sessions/--<cwd with separators as ->--/`. Zeron's
//! index (`zeron-sessions/<sha256(id)>.json`) maps the id to that absolute
//! file. A proven-empty session (no file yet) moves as its index record.
use super::sessions::Store;
use crate::portable::{
    ExportFile, Format, Import, Placer, PortRoots, Rewriter, Rule, Seg, SessionExport, checked_id,
    checked_name,
};
use anyhow::{Context, bail};
use serde_json::json;
use std::path::Path;
use zeron_proto::HarnessId;

const RULES: &[Rule] = &[Rule {
    kind: Some("session"),
    path: &[Seg::Key("cwd")],
}];

/// Pi's default session directory name for `cwd` (`getDefaultSessionDirPath`).
pub(crate) fn session_dir_name(cwd: &str) -> String {
    let trimmed = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
    format!("--{}--", trimmed.replace(['/', '\\', ':'], "-"))
}

pub(crate) fn export(roots: &PortRoots, id: &str, cwd: &str) -> anyhow::Result<SessionExport> {
    let id = checked_id(id)?;
    let store = Store::at(&roots.pi_agent, &roots.home);
    let record = store.record(id);
    let (files, name, recorded) = match store.resolve(id, Path::new(cwd)) {
        Ok(file) => {
            let recorded = crate::portable::first_record(&file)
                .and_then(|header| header["cwd"].as_str().map(str::to_owned));
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
            let files = vec![ExportFile {
                rel: "session.jsonl".into(),
                path: file,
            }];
            (files, name, recorded)
        }
        // Only Zeron's own post-ACK proof of an empty session moves without a file.
        Err(error) if record["emptyState"].is_object() => {
            let name = record["sessionFile"]
                .as_str()
                .and_then(|f| Path::new(f).file_name())
                .map(|n| n.to_string_lossy().into_owned());
            if name.is_none() {
                return Err(error.into());
            }
            (Vec::new(), name, record["cwd"].as_str().map(str::to_owned))
        }
        Err(error) => return Err(error.into()),
    };
    Ok(SessionExport {
        harness: HarnessId::Pi,
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        files,
        meta: json!({
            "fileName": name,
            "recordedCwd": recorded,
            "emptyState": record["emptyState"],
        }),
    })
}

pub(crate) fn import(import: &mut Import, roots: &PortRoots) -> anyhow::Result<()> {
    let id = checked_id(&import.export.session_id)?.to_owned();
    let name = checked_name(
        import
            .meta_str("fileName")
            .context("missing Pi file name")?,
    )?;
    if !name.ends_with(".jsonl") {
        bail!("unexpected Pi session file name {name:?}");
    }
    let dest = roots
        .pi_agent
        .join("sessions")
        .join(session_dir_name(import.new_cwd))
        .join(name);
    let store = Store::at(&roots.pi_agent, &roots.home);
    let maps = import.cwd_maps();
    let rewriter = Rewriter::new(&maps, RULES);
    let files: Vec<_> = import.files().map(|(r, p)| (r.to_owned(), p)).collect();
    let empty_state = import.export.meta["emptyState"].clone();
    let mut placer = Placer::new(&roots.pi_agent, &mut import.warnings);
    match files.as_slice() {
        [(rel, src)] if rel == "session.jsonl" => {
            placer.place(src, &dest, Some((&rewriter, Format::Lines)))?;
            store.remember(&id, &dest)?;
        }
        [] if empty_state.is_object() => {
            // Recreating needs the record's cwd to be the canonical new cwd.
            let cwd = std::fs::canonicalize(import.new_cwd)
                .map(|p| json!(p))
                .unwrap_or_else(|_| json!(import.new_cwd));
            let mut state = empty_state;
            state["sessionFile"] = json!(dest);
            store.put_record(
                &id,
                &json!({"sessionId": id, "sessionFile": dest, "emptyState": state, "cwd": cwd}),
            )?;
        }
        _ => bail!("unexpected files in a Pi session export"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::export_session;
    use crate::portable::test_support::*;
    use serde_json::Value;

    const ID: &str = "01a0e9d7-d3b1-7436-aac0-9abbd5ad5532";
    const NAME: &str = "2026-09-28T21-07-10-899Z_01a0e9d7-d3b1-7436-aac0-9abbd5ad5532.jsonl";

    #[test]
    fn encodes_session_dirs_like_pi() {
        // Observed for a session started in `/Users/daniel`.
        assert_eq!(session_dir_name("/Users/daniel"), "--Users-daniel--");
        assert_eq!(session_dir_name(OLD_CWD), "--Users-alice-dev-proj--");
        assert_eq!(session_dir_name(r"C:\work\proj"), "--C--work-proj--");
    }

    #[test]
    fn moves_the_file_rewrites_the_header_and_resolve_finds_it() {
        let (_dir, a, b) = devices();
        let source = a
            .pi_agent
            .join("sessions")
            .join(session_dir_name(OLD_CWD))
            .join(NAME);
        let header = json!({"type": "session", "version": 3, "id": ID, "timestamp": "2026-09-28T21:07:10.899Z", "cwd": OLD_CWD});
        let message = r#"{"type":"message","id":"m1","message":{"role":"user","content":[{"type":"text","text":"cwd is /Users/alice/dev/proj"}]}}"#;
        write(&source, format!("{header}\n{message}\n"));
        Store::at(&a.pi_agent, &a.home)
            .remember(ID, &source)
            .unwrap();

        let (export, imported) = carry(HarnessId::Pi, &a, &b, ID, OLD_CWD, NEW_CWD);
        assert_eq!(imported.unwrap().session_id, ID);
        assert_eq!(export.meta["recordedCwd"], OLD_CWD);

        let store = Store::at(&b.pi_agent, &b.home);
        let found = store.resolve(ID, Path::new(NEW_CWD)).unwrap();
        let dest = b.pi_agent.join("sessions/--home-bob-src-proj--").join(NAME);
        assert_eq!(found, dest.canonicalize().unwrap());
        let moved = lines(&dest);
        let header: Value = serde_json::from_str(&moved[0]).unwrap();
        assert_eq!(header["cwd"], NEW_CWD);
        assert_eq!(header["id"], ID);
        assert_eq!(moved[1], message);
        assert_eq!(
            store.resume_args(ID, Path::new(NEW_CWD)).unwrap(),
            vec!["--session".to_owned(), found.display().to_string()]
        );
    }

    #[test]
    fn an_unsubmitted_empty_session_moves_as_its_index_record() {
        let (dir, a, b) = devices();
        // Recreating an empty session checks the canonical cwd, which must exist.
        let new_cwd = dir.path().join("bob-proj");
        std::fs::create_dir_all(&new_cwd).unwrap();
        let file = a
            .pi_agent
            .join("sessions/--Users-alice-dev-proj--")
            .join(NAME);
        let state = json!({"sessionId": ID, "sessionFile": file, "model": {"provider": "mock", "id": "m"}, "thinkingLevel": "off"});
        let store = Store::at(&a.pi_agent, &a.home);
        store
            .put_record(
                ID,
                &json!({"sessionId": ID, "sessionFile": file, "emptyState": state, "cwd": OLD_CWD}),
            )
            .unwrap();

        let (export, imported) = carry(
            HarnessId::Pi,
            &a,
            &b,
            ID,
            OLD_CWD,
            new_cwd.to_str().unwrap(),
        );
        imported.unwrap();
        assert!(export.files.is_empty());
        let args = Store::at(&b.pi_agent, &b.home)
            .resume_args(ID, &new_cwd)
            .unwrap();
        let expected_dir = b
            .pi_agent
            .join("sessions")
            .join(session_dir_name(new_cwd.to_str().unwrap()));
        assert_eq!(
            args[..4],
            [
                "--session-id".to_owned(),
                ID.to_owned(),
                "--session-dir".to_owned(),
                expected_dir.display().to_string()
            ]
        );
    }

    #[test]
    fn an_unknown_session_is_an_error() {
        let (_dir, a, _) = devices();
        assert!(export_session(HarnessId::Pi, &a, ID, OLD_CWD).is_err());
    }
}
