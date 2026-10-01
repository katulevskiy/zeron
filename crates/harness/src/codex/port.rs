//! Codex session port. A thread is its rollout,
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<ts>-<threadId>.jsonl`; subagent
//! threads are separate rollouts whose `session_meta.source` names the parent.
//! Files keep their relative path, so the id stays the same.
//!
//! The SQLite stores beside them (`state_*.sqlite` thread index,
//! `thread_history_*.sqlite` paginated-history projection) are caches:
//! verified on codex-cli 0.159.2, `thread/resume` on a home with no rows for
//! the thread finds the rollout by id and rebuilds both. A target that
//! already indexed an EARLIER copy of the thread keeps its projection byte
//! offsets; that stays valid when the new rollout extends the old bytes (the
//! byte-preserving rewrite makes a move back to the original cwd do so), and
//! the importer warns when it doesn't.
use crate::portable::{
    ExportFile, Format, Import, Placed, Placer, PortRoots, Rewriter, Rule, Seg, SessionExport,
    checked_id, checked_rel, first_record,
};
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use zeron_proto::HarnessId;

/// Records that state the thread's working directory or workspace roots.
const RULES: &[Rule] = &[
    Rule {
        kind: Some("session_meta"),
        path: &[Seg::Key("payload"), Seg::Key("cwd")],
    },
    Rule {
        kind: Some("session_meta"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("runtime_workspace_roots"),
            Seg::Any,
        ],
    },
    Rule {
        kind: Some("turn_context"),
        path: &[Seg::Key("payload"), Seg::Key("cwd")],
    },
    Rule {
        kind: Some("turn_context"),
        path: &[Seg::Key("payload"), Seg::Key("workspace_roots"), Seg::Any],
    },
    Rule {
        kind: Some("turn_context"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("sandbox_policy"),
            Seg::Key("writable_roots"),
            Seg::Any,
        ],
    },
    Rule {
        kind: Some("turn_context"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("permission_profile"),
            Seg::Key("file_system"),
            Seg::Key("entries"),
            Seg::Any,
            Seg::Key("path"),
            Seg::Key("path"),
        ],
    },
    Rule {
        kind: Some("turn_context"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("file_system_sandbox_policy"),
            Seg::Key("entries"),
            Seg::Any,
            Seg::Key("path"),
            Seg::Key("path"),
        ],
    },
    Rule {
        kind: Some("world_state"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("state"),
            Seg::Key("environments"),
            Seg::Key("environments"),
            Seg::Any,
            Seg::Key("cwd"),
        ],
    },
    Rule {
        kind: Some("event_msg"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("thread_settings"),
            Seg::Key("cwd"),
        ],
    },
    Rule {
        kind: Some("event_msg"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("thread_settings"),
            Seg::Key("runtime_workspace_roots"),
            Seg::Any,
        ],
    },
    Rule {
        kind: Some("event_msg"),
        path: &[
            Seg::Key("payload"),
            Seg::Key("thread_settings"),
            Seg::Key("permission_profile"),
            Seg::Key("file_system"),
            Seg::Key("entries"),
            Seg::Any,
            Seg::Key("path"),
            Seg::Key("path"),
        ],
    },
];

/// Every rollout as (`YYYY/MM/DD`, path).
fn rollouts(sessions: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let dirs = |dir: &Path| -> Vec<(String, PathBuf)> {
        let mut dirs: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path())))
            .collect();
        dirs.sort();
        dirs
    };
    for (year, year_dir) in dirs(sessions) {
        for (month, month_dir) in dirs(&year_dir) {
            for (day, day_dir) in dirs(&month_dir) {
                for entry in std::fs::read_dir(&day_dir)?.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                        found.push((format!("{year}/{month}/{day}"), entry.path()));
                    }
                    if found.len() > 200_000 {
                        bail!("too many Codex rollouts to search");
                    }
                }
            }
        }
    }
    Ok(found)
}

fn is_rollout_of(path: &Path, id: &str) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&format!("-{id}.jsonl")))
}

/// The parent thread named by a `session_meta.source` (any casing/nesting).
fn parent_of(source: &Value) -> Option<&str> {
    match source {
        Value::Object(map) => map
            .get("parent_thread_id")
            .or_else(|| map.get("parentThreadId"))
            .and_then(Value::as_str)
            .or_else(|| map.values().find_map(parent_of)),
        _ => None,
    }
}

pub(crate) fn export(roots: &PortRoots, id: &str, cwd: &str) -> anyhow::Result<SessionExport> {
    let id = checked_id(id)?;
    let all = rollouts(&roots.codex.join("sessions"))?;
    let (day, main) = all
        .iter()
        .find(|(_, path)| is_rollout_of(path, id))
        .cloned()
        .with_context(|| format!("Codex rollout for thread {id} was not found"))?;
    let recorded =
        first_record(&main).and_then(|meta| meta["payload"]["cwd"].as_str().map(str::to_owned));

    // Children are created after their parent: only later rollouts can be one.
    let candidates: Vec<(String, String, PathBuf)> = all
        .iter()
        .filter(|(d, path)| *d >= day && *path != main)
        .filter_map(|(_, path)| {
            let meta = first_record(path)?;
            let payload = &meta["payload"];
            let child = payload["id"].as_str()?.to_owned();
            let parent = parent_of(&payload["source"])?.to_owned();
            Some((child, parent, path.clone()))
        })
        .collect();
    let mut threads = vec![id.to_owned()];
    let mut files = vec![main];
    let mut grew = true;
    while grew {
        grew = false;
        for (child, parent, path) in &candidates {
            if threads.contains(parent) && !threads.contains(child) {
                threads.push(child.clone());
                files.push(path.clone());
                grew = true;
            }
        }
    }
    let files = files
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&roots.codex)?
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            Ok(ExportFile { rel, path })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(SessionExport {
        harness: HarnessId::Codex,
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        files,
        meta: json!({ "recordedCwd": recorded, "children": &threads[1..] }),
    })
}

pub(crate) fn import(import: &mut Import, roots: &PortRoots) -> anyhow::Result<()> {
    let id = checked_id(&import.export.session_id)?.to_owned();
    let maps = import.cwd_maps();
    let rewriter = Rewriter::new(&maps, RULES);
    let files: Vec<_> = import.files().map(|(r, p)| (r.to_owned(), p)).collect();
    let mut placer = Placer::new(&roots.codex, &mut import.warnings);
    let mut has_main = false;
    for (rel, src) in &files {
        let parts: Vec<_> = rel.split('/').collect();
        let ["sessions", year, month, day, name] = parts.as_slice() else {
            bail!("unexpected file in a Codex session export: {rel}");
        };
        if ![year, month, day]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
            || !name.starts_with("rollout-")
            || !name.ends_with(".jsonl")
        {
            bail!("unexpected file in a Codex session export: {rel}");
        }
        let dest = roots.codex.join(checked_rel(rel)?);
        has_main |= is_rollout_of(&dest, &id);
        if placer.place(src, &dest, Some((&rewriter, Format::Lines)))?
            == (Placed::Replaced { prefix: false })
        {
            placer.warn(format!(
                "{} diverged from this device's earlier copy; Codex may show a stale \
                 history page for it until its cache refreshes",
                dest.display()
            ));
        }
    }
    if !has_main {
        bail!("the Codex session export has no rollout for {id}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::export_session;
    use crate::portable::test_support::*;

    const ID: &str = "01a0f46f-7c9f-7520-8a72-7f1adebec5f0";
    const CHILD: &str = "01a0f470-0000-7000-8000-00000000c41d";
    const GRANDCHILD: &str = "01a0f471-0000-7000-8000-0000000000g1";
    const OTHER: &str = "01a0f472-0000-7000-8000-000000000123";

    fn rollout(codex: &Path, day: &str, ts: &str, id: &str) -> PathBuf {
        codex.join(format!("sessions/{day}/rollout-{ts}-{id}.jsonl"))
    }

    fn meta(id: &str, cwd: &str, source: Value) -> String {
        json!({"timestamp": "2026-09-30T15:29:02.000Z", "ordinal": 0, "type": "session_meta", "payload": {
            "session_id": id, "id": id, "cwd": cwd, "runtime_workspace_roots": [cwd],
            "originator": "zeron-native", "cli_version": "0.159.2", "source": source,
            "model_provider": "openai", "history_mode": "paginated",
            "base_instructions": {"text": format!("You run in {cwd}.")},
            "git": {"branch": "main"}
        }})
        .to_string()
    }

    /// Rollout records shaped like codex-cli 0.159.2's.
    fn fixture(a: &PortRoots) -> (PathBuf, Vec<String>) {
        let lines = vec![
            meta(ID, OLD_CWD, json!("vscode")),
            json!({"timestamp": "t", "ordinal": 1, "type": "turn_context", "payload": {
                "turn_id": "turn-1", "cwd": OLD_CWD, "workspace_roots": [OLD_CWD],
                "sandbox_policy": {"type": "workspace-write", "writable_roots": ["/Users/alice/dev/proj/.git", "/tmp"]},
                "permission_profile": {"type": "custom", "file_system": {"type": "restricted", "entries": [
                    {"path": {"type": "special", "value": {"kind": "project_roots"}}, "access": "write"},
                    {"path": {"type": "path", "path": "/Users/alice/dev/proj/build"}, "access": "read"}
                ]}},
                "file_system_sandbox_policy": {"kind": "restricted", "entries": [
                    {"path": {"type": "path", "path": OLD_CWD}, "access": "write"}
                ]},
                "model": "gpt-5.6"
            }}).to_string(),
            json!({"timestamp": "t", "ordinal": 2, "type": "world_state", "payload": {"full": true, "state": {"environments": {"environments": {"local": {"cwd": OLD_CWD, "shell": "zsh"}}}}}}).to_string(),
            json!({"timestamp": "t", "ordinal": 3, "type": "event_msg", "payload": {"type": "thread_settings_applied", "thread_settings": {"cwd": OLD_CWD, "runtime_workspace_roots": [OLD_CWD], "model": "gpt-5.6"}}}).to_string(),
            json!({"timestamp": "t", "ordinal": 4, "type": "response_item", "payload": {"type": "function_call", "name": "exec_command", "arguments": "{\"cmd\":\"ls\",\"workdir\":\"/Users/alice/dev/proj\"}", "call_id": "c1"}}).to_string(),
            json!({"timestamp": "t", "ordinal": 5, "type": "response_item", "payload": {"type": "function_call_output", "call_id": "c1", "output": "/Users/alice/dev/proj/README.md"}}).to_string(),
            json!({"timestamp": "t", "ordinal": 6, "type": "event_msg", "payload": {"type": "exec_command_end", "cwd": OLD_CWD}}).to_string(),
        ];
        let main = rollout(&a.codex, "2026/09/30", "2026-09-30T15-29-02", ID);
        write(&main, lines.join("\n") + "\n");
        let spawn = |parent: &str| json!({"subagent": {"thread_spawn": {"parent_thread_id": parent, "depth": 1}}});
        write(
            &rollout(&a.codex, "2026/09/30", "2026-09-30T15-30-00", CHILD),
            meta(CHILD, OLD_CWD, spawn(ID)) + "\n",
        );
        write(
            &rollout(&a.codex, "2026/10/01", "2026-10-01T00-01-00", GRANDCHILD),
            meta(GRANDCHILD, OLD_CWD, spawn(CHILD)) + "\n",
        );
        write(
            &rollout(&a.codex, "2026/09/30", "2026-09-30T16-00-00", OTHER),
            meta(OTHER, OLD_CWD, json!("cli")) + "\n",
        );
        (main, lines)
    }

    #[test]
    fn moves_the_rollout_and_its_subagents_with_the_same_ids() {
        let (_dir, a, b) = devices();
        let (_, lines) = fixture(&a);
        let (export, imported) = carry(HarnessId::Codex, &a, &b, ID, OLD_CWD, NEW_CWD);
        let imported = imported.unwrap();
        assert_eq!(imported.session_id, ID);
        assert!(imported.warnings.is_empty());
        assert_eq!(export.meta["children"], json!([CHILD, GRANDCHILD]));
        assert_eq!(export.files.len(), 3);

        let dest = rollout(&b.codex, "2026/09/30", "2026-09-30T15-29-02", ID);
        let moved = lines_of(&dest);
        let records: Vec<Value> = moved
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let meta = &records[0]["payload"];
        assert_eq!(meta["cwd"], NEW_CWD);
        assert_eq!(meta["runtime_workspace_roots"], json!([NEW_CWD]));
        assert_eq!(
            meta["base_instructions"]["text"],
            format!("You run in {OLD_CWD}."),
            "instructions are history"
        );
        let turn = &records[1]["payload"];
        assert_eq!(turn["cwd"], NEW_CWD);
        assert_eq!(turn["workspace_roots"], json!([NEW_CWD]));
        assert_eq!(
            turn["sandbox_policy"]["writable_roots"],
            json!(["/home/bob/src/proj/.git", "/tmp"])
        );
        assert_eq!(
            turn["permission_profile"]["file_system"]["entries"][1]["path"]["path"],
            "/home/bob/src/proj/build"
        );
        assert_eq!(
            turn["file_system_sandbox_policy"]["entries"][0]["path"]["path"],
            NEW_CWD
        );
        assert_eq!(
            records[2]["payload"]["state"]["environments"]["environments"]["local"]["cwd"],
            NEW_CWD
        );
        assert_eq!(records[3]["payload"]["thread_settings"]["cwd"], NEW_CWD);
        // Tool calls, outputs and per-command events are untouched bytes.
        assert_eq!(moved[4..], lines[4..]);

        for (day, ts, id) in [
            ("2026/09/30", "2026-09-30T15-30-00", CHILD),
            ("2026/10/01", "2026-10-01T00-01-00", GRANDCHILD),
        ] {
            let child: Value =
                serde_json::from_str(&lines_of(&rollout(&b.codex, day, ts, id))[0]).unwrap();
            assert_eq!(child["payload"]["cwd"], NEW_CWD);
        }
        assert!(!rollout(&b.codex, "2026/09/30", "2026-09-30T16-00-00", OTHER).exists());
    }

    fn lines_of(path: &Path) -> Vec<String> {
        lines(path)
    }

    #[test]
    fn moving_back_restores_the_original_bytes() {
        let (_dir, a, b) = devices();
        let (main, _) = fixture(&a);
        let original = std::fs::read(&main).unwrap();
        carry(HarnessId::Codex, &a, &b, ID, OLD_CWD, NEW_CWD)
            .1
            .unwrap();
        // B grows the thread, then the chat moves back to A's cwd.
        let on_b = rollout(&b.codex, "2026/09/30", "2026-09-30T15-29-02", ID);
        let appended = json!({"timestamp": "t", "ordinal": 7, "type": "turn_context", "payload": {"cwd": NEW_CWD}}).to_string();
        let mut grown = std::fs::read(&on_b).unwrap();
        grown.extend_from_slice(format!("{appended}\n").as_bytes());
        std::fs::write(&on_b, grown).unwrap();
        let imported = carry(HarnessId::Codex, &b, &a, ID, NEW_CWD, OLD_CWD)
            .1
            .unwrap();

        let back = std::fs::read(&main).unwrap();
        assert!(
            back.starts_with(&original),
            "A's indexed byte offsets stay valid"
        );
        assert!(
            String::from_utf8_lossy(&back[original.len()..])
                .contains(&format!("\"cwd\":\"{OLD_CWD}\""))
        );
        // The grown rollout replaced A's copy (backed up), without a divergence warning.
        assert_eq!(imported.warnings.len(), 1, "{:?}", imported.warnings);
        assert_eq!(backups(&a.codex).len(), 1);
        assert!(!backups(&a.codex)[0].starts_with(a.codex.join("sessions")));
    }

    #[test]
    fn a_diverged_copy_is_backed_up_with_a_warning() {
        let (_dir, a, b) = devices();
        fixture(&a);
        let dest = rollout(&b.codex, "2026/09/30", "2026-09-30T15-29-02", ID);
        write(
            &dest,
            meta(ID, "/home/bob/elsewhere", json!("vscode")) + "\n",
        );
        let imported = carry(HarnessId::Codex, &a, &b, ID, OLD_CWD, NEW_CWD)
            .1
            .unwrap();
        assert_eq!(imported.warnings.len(), 2, "{:?}", imported.warnings);
        assert!(imported.warnings[1].contains("stale history"));
    }

    #[test]
    fn a_missing_rollout_is_an_error() {
        let (_dir, a, _) = devices();
        assert!(export_session(HarnessId::Codex, &a, ID, OLD_CWD).is_err());
    }
}
