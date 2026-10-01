//! Claude Code session port. A session is `projects/<encoded cwd>/<id>.jsonl`
//! plus an optional `<id>/` sibling (`subagents/*.jsonl` + `*.meta.json`,
//! `tool-results/*.txt`). `--resume <id>` runs with the process cwd set to
//! the new cwd, so the transcript must land under the NEW cwd's project dir.
//!
//! The CLI's encoding (2.1.x): every UTF-16 unit that is not ASCII
//! alphanumeric becomes `-`; names over 200 units are truncated and suffixed
//! with a private hash. Long cwds are refused so the caller replays instead
//! of guessing the hash.
use crate::portable::{
    Format, Import, PortRoots, Rewriter, Rule, Seg, SessionExport, checked_id, list_files,
};
use anyhow::{Context, bail};
use serde_json::json;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use zeron_proto::HarnessId;

const MAX_PROJECT_DIR: usize = 200;

/// Record keys that carry the session's working directory.
const RULES: &[Rule] = &[
    Rule {
        kind: None,
        path: &[Seg::Key("cwd")],
    },
    Rule {
        kind: None,
        path: &[Seg::Key("wireIngestContext"), Seg::Any, Seg::Key("cwd")],
    },
    Rule {
        kind: Some("attachment"),
        path: &[
            Seg::Key("attachment"),
            Seg::Key("snapshot"),
            Seg::Key("workingDirectory"),
        ],
    },
    Rule {
        kind: Some("attachment"),
        path: &[
            Seg::Key("attachment"),
            Seg::Key("snapshot"),
            Seg::Key("additionalWorkingDirectories"),
            Seg::Any,
        ],
    },
];

/// The CLI's project directory name for `cwd`, when it is unhashed.
pub(crate) fn project_dir_name(cwd: &str) -> anyhow::Result<String> {
    if cwd.encode_utf16().count() > MAX_PROJECT_DIR {
        bail!("the cwd is too long for a predictable Claude project directory");
    }
    Ok(cwd
        .chars()
        .flat_map(|c| {
            let n = if c.is_ascii_alphanumeric() {
                0
            } else {
                c.len_utf16()
            };
            std::iter::repeat_n('-', n).chain((n == 0).then_some(c))
        })
        .collect())
}

fn session_id(id: &str) -> anyhow::Result<&str> {
    // Same shape the driver's own history scan accepts.
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("unsupported Claude session id {id:?}");
    }
    checked_id(id)
}

/// Every `projects/*/<id>.jsonl`, the cwd's own directory first.
fn transcripts(roots: &PortRoots, id: &str, cwd: Option<&str>) -> Vec<PathBuf> {
    let projects = roots.claude.join("projects");
    let name = format!("{id}.jsonl");
    let mut found = Vec::new();
    if let Some(cwd) = cwd {
        for cwd in [cwd.to_owned(), crate::portable::physical_cwd(cwd)] {
            if let Ok(dir) = project_dir_name(&cwd) {
                let file = projects.join(dir).join(&name);
                if file.is_file() && !found.contains(&file) {
                    found.push(file);
                }
            }
        }
    }
    for entry in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
        let file = entry.path().join(&name);
        if file.is_file() && !found.contains(&file) {
            found.push(file);
        }
    }
    found
}

/// The first cwd the CLI recorded in a transcript.
fn recorded_cwd(transcript: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Record {
        cwd: Option<String>,
    }
    let file = std::fs::File::open(transcript).ok()?;
    std::io::BufReader::new(file)
        .split(b'\n')
        .take(500)
        .map_while(Result::ok)
        .find_map(|line| serde_json::from_slice::<Record>(&line).ok()?.cwd)
}

pub(crate) fn export(roots: &PortRoots, id: &str, cwd: &str) -> anyhow::Result<SessionExport> {
    let id = session_id(id)?;
    let transcript = transcripts(roots, id, Some(cwd))
        .into_iter()
        .next()
        .with_context(|| format!("Claude session {id} was not found"))?;
    let mut files = vec![crate::portable::ExportFile {
        rel: "transcript.jsonl".into(),
        path: transcript.clone(),
    }];
    let sidecar = transcript.with_extension("");
    if sidecar.is_dir() {
        files.extend(list_files(&sidecar, "session", |name| {
            name.ends_with(".lock")
        })?);
    }
    Ok(SessionExport {
        harness: HarnessId::ClaudeCode,
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        files,
        meta: json!({ "recordedCwd": recorded_cwd(&transcript) }),
    })
}

pub(crate) fn import(import: &mut Import, roots: &PortRoots) -> anyhow::Result<()> {
    let id = session_id(&import.export.session_id)?.to_owned();
    // The CLI keys the project dir by `process.cwd()`, i.e. symlinks resolved.
    let dir =
        roots
            .claude
            .join("projects")
            .join(project_dir_name(&crate::portable::physical_cwd(
                import.new_cwd,
            ))?);
    let transcript = dir.join(format!("{id}.jsonl"));
    let maps = import.cwd_maps();
    let rewriter = Rewriter::new(&maps, RULES);
    let files: Vec<_> = import.files().map(|(r, p)| (r.to_owned(), p)).collect();
    let mut placer = crate::portable::Placer::new(&roots.claude, &mut import.warnings);
    let mut placed_transcript = false;
    for (rel, src) in &files {
        let (dest, rewrite) = if rel == "transcript.jsonl" {
            placed_transcript = true;
            (transcript.clone(), true)
        } else if let Some(rest) = rel.strip_prefix("session/") {
            (dir.join(&id).join(rest), rest.ends_with(".jsonl"))
        } else {
            bail!("unexpected file in a Claude session export: {rel}");
        };
        placer.place(src, &dest, rewrite.then_some((&rewriter, Format::Lines)))?;
    }
    if !placed_transcript {
        bail!("the Claude session export has no transcript");
    }
    // Session ids are global: another project dir holding the same id (an
    // older visit of this chat) would shadow or confuse the resume lookup.
    for other in transcripts(roots, &id, None) {
        if other != transcript {
            placer.retire(&other)?;
            let sidecar = other.with_extension("");
            if sidecar.is_dir() {
                placer.retire(&sidecar)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::test_support::*;
    use crate::portable::{export_session, import_session, snapshot_export};
    use serde_json::Value;

    const ID: &str = "4f9d2c1e-0000-4000-8000-00000000c1a0";

    fn line(value: Value) -> String {
        serde_json::to_string(&value).unwrap()
    }

    /// A transcript shaped like the 2.1.x CLI's: records carry `cwd`, the
    /// environment attachment snapshots the working directory, tool calls
    /// record their ingest context, and an Agent spawn binds to its agent id.
    fn fixture(roots: &PortRoots) -> (PathBuf, Vec<String>) {
        let dir = roots.claude.join("projects/-Users-alice-dev-proj");
        let base = json!({"sessionId": ID, "cwd": OLD_CWD, "version": "2.1.285", "gitBranch": "main", "isSidechain": false});
        let with = |extra: Value| {
            let mut record = base.clone();
            record
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            line(record)
        };
        let lines = vec![
            with(
                json!({"type": "attachment", "attachment": {"type": "environment", "snapshot": {"workingDirectory": OLD_CWD, "isGitRepo": true, "additionalWorkingDirectories": ["/Users/alice/dev/proj/vendor", "/opt/shared"], "platform": "darwin"}}}),
            ),
            with(
                json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": "fix /Users/alice/dev/proj/src/main.rs"}}),
            ),
            with(
                json!({"type": "assistant", "uuid": "a1", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_spawn", "name": "Agent", "input": {"prompt": "look around"}},
                {"type": "tool_use", "id": "toolu_read", "name": "Read", "input": {"file_path": "/Users/alice/dev/proj/src/main.rs"}}
            ]}, "wireIngestContext": {"toolu_read": {"cwd": OLD_CWD}}}),
            ),
            with(
                json!({"type": "user", "uuid": "u2", "toolUseResult": {"agentId": "a7f3"}, "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_spawn", "content": "ran in /Users/alice/dev/proj"}
                ]}}),
            ),
            line(
                json!({"type": "last-prompt", "lastPrompt": "cd /Users/alice/dev/proj", "sessionId": ID}),
            ),
            "{\"type\":\"user\",\"cwd\":\"/Users/alice/dev/proj\",\"torn".to_owned(),
        ];
        let transcript = dir.join(format!("{ID}.jsonl"));
        write(&transcript, lines.join("\n") + "\n");
        write(
            &dir.join(ID).join("subagents/agent-a7f3.jsonl"),
            line(json!({"type": "user", "cwd": OLD_CWD, "isSidechain": true, "agentId": "a7f3"}))
                + "\n",
        );
        write(
            &dir.join(ID).join("subagents/agent-a7f3.meta.json"),
            r#"{"agentType":"general-purpose","description":"look around","toolUseId":"toolu_spawn"}"#,
        );
        write(
            &dir.join(ID).join("tool-results/b1.txt"),
            "listing of /Users/alice/dev/proj\n",
        );
        (transcript, lines)
    }

    #[test]
    fn encodes_project_dirs_like_the_cli() {
        assert_eq!(
            project_dir_name("/Users/daniel/dev/zeron").unwrap(),
            "-Users-daniel-dev-zeron"
        );
        // Observed: `/private/var/.../T/.tmpXYZ` → `-private-var-...-T--tmpXYZ`.
        assert_eq!(
            project_dir_name("/private/var/folders/0d/T/.tmp0pk1LI").unwrap(),
            "-private-var-folders-0d-T--tmp0pk1LI"
        );
        assert_eq!(
            project_dir_name("/a.b/my_proj dir").unwrap(),
            "-a-b-my-proj-dir"
        );
        assert_eq!(project_dir_name(r"C:\Users\bob").unwrap(), "C--Users-bob");
        // One `-` per UTF-16 unit, as JS replaces them.
        assert_eq!(project_dir_name("/é/😀").unwrap(), "-----");
        assert!(project_dir_name(&format!("/{}", "x".repeat(200))).is_err());
        assert!(project_dir_name(&format!("/{}", "x".repeat(199))).is_ok());
    }

    #[tokio::test]
    async fn moves_a_transcript_under_the_new_cwd_and_resume_finds_it() {
        let (_dir, a, b) = devices();
        let (source, lines) = fixture(&a);
        let (export, imported) = carry(HarnessId::ClaudeCode, &a, &b, ID, OLD_CWD, NEW_CWD);
        let imported = imported.unwrap();
        assert_eq!(imported.session_id, ID);
        assert!(imported.warnings.is_empty());
        assert_eq!(export.meta["recordedCwd"], OLD_CWD);
        let mut rels: Vec<_> = export.files.iter().map(|f| f.rel.as_str()).collect();
        rels.sort();
        assert_eq!(
            rels,
            [
                "session/subagents/agent-a7f3.jsonl",
                "session/subagents/agent-a7f3.meta.json",
                "session/tool-results/b1.txt",
                "transcript.jsonl"
            ]
        );

        let dir = b.claude.join("projects/-home-bob-src-proj");
        let moved = lines_of(&dir.join(format!("{ID}.jsonl")));
        assert_eq!(moved.len(), lines.len());
        let records: Vec<Value> = moved
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        for record in &records[..4] {
            assert_eq!(record["cwd"], NEW_CWD);
        }
        let snapshot = &records[0]["attachment"]["snapshot"];
        assert_eq!(snapshot["workingDirectory"], NEW_CWD);
        assert_eq!(
            snapshot["additionalWorkingDirectories"],
            json!(["/home/bob/src/proj/vendor", "/opt/shared"])
        );
        assert_eq!(
            records[2]["wireIngestContext"]["toolu_read"]["cwd"],
            NEW_CWD
        );
        // Conversation content and tool inputs/outputs are history: untouched.
        assert_eq!(
            records[1]["message"]["content"],
            "fix /Users/alice/dev/proj/src/main.rs"
        );
        assert_eq!(
            records[2]["message"]["content"][1]["input"]["file_path"],
            "/Users/alice/dev/proj/src/main.rs"
        );
        assert_eq!(
            records[3]["message"]["content"][0]["content"],
            "ran in /Users/alice/dev/proj"
        );
        // Lines without a cwd key, and torn lines, are byte-identical.
        assert_eq!(moved[4], lines[4]);
        assert_eq!(moved[5], lines[5]);

        let sub = dir.join(ID);
        assert!(
            std::fs::read_to_string(sub.join("subagents/agent-a7f3.jsonl"))
                .unwrap()
                .contains(&format!("\"cwd\":\"{NEW_CWD}\""))
        );
        for rel in ["subagents/agent-a7f3.meta.json", "tool-results/b1.txt"] {
            assert_eq!(
                std::fs::read(sub.join(rel)).unwrap(),
                std::fs::read(source.with_extension("").join(rel)).unwrap()
            );
        }

        // The driver's own resume scan finds it and restores spawn identity.
        let norm = super::super::normalize::Normalizer::for_resume(&b.claude, ID).await;
        assert_eq!(norm.restored_spawn("a7f3"), Some("toolu_spawn"));
    }

    fn lines_of(path: &Path) -> Vec<String> {
        lines(path)
    }

    #[test]
    fn reimport_backs_up_a_different_copy_and_retires_other_project_dirs() {
        let (_dir, a, b) = devices();
        fixture(&a);
        // B already has this chat: once under the destination (an older
        // state) and once under another project dir (an earlier cwd).
        let dest = b
            .claude
            .join(format!("projects/-home-bob-src-proj/{ID}.jsonl"));
        let stale = b.claude.join(format!("projects/-home-bob-old/{ID}.jsonl"));
        write(
            &dest,
            "{\"type\":\"user\",\"cwd\":\"/home/bob/src/proj\"}\n",
        );
        write(&stale, "{\"type\":\"user\",\"cwd\":\"/home/bob/old\"}\n");
        write(&stale.with_extension("").join("tool-results/x.txt"), "x");

        let (_, imported) = carry(HarnessId::ClaudeCode, &a, &b, ID, OLD_CWD, NEW_CWD);
        let imported = imported.unwrap();
        assert_eq!(imported.warnings.len(), 3, "{:?}", imported.warnings);
        assert!(!stale.exists() && !stale.with_extension("").exists());
        assert!(lines(&dest).len() > 1);
        let saved = backups(&b.claude);
        assert_eq!(saved.len(), 3);
        assert!(
            saved
                .iter()
                .all(|p| p.to_string_lossy().contains(".zeron-backups"))
        );
        // Backups sit outside `projects/`, where the CLI and driver scan by id.
        assert_eq!(transcripts(&b, ID, None), vec![dest.clone()]);

        // Importing the same state again changes nothing.
        let export = export_session(HarnessId::ClaudeCode, &a, ID, OLD_CWD)
            .unwrap()
            .unwrap();
        let staged = a.home.join("../again");
        snapshot_export(&export, &staged).unwrap();
        let again = import_session(&export, &staged, &b, NEW_CWD).unwrap();
        assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    }

    #[test]
    fn a_hashed_long_cwd_is_refused_so_the_chat_replays() {
        let (_dir, a, b) = devices();
        fixture(&a);
        let long = format!("/home/bob/{}", "deep/".repeat(50));
        let (_, imported) = carry(HarnessId::ClaudeCode, &a, &b, ID, OLD_CWD, &long);
        assert!(imported.is_err());
        assert!(!b.claude.join("projects").exists());
    }

    #[test]
    fn a_missing_session_is_an_error() {
        let (_dir, a, _) = devices();
        assert!(export_session(HarnessId::ClaudeCode, &a, ID, OLD_CWD).is_err());
        assert!(export_session(HarnessId::ClaudeCode, &a, "../x", OLD_CWD).is_err());
    }
}
