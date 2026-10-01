//! Grok session port. A session is the folder
//! `$GROK_HOME/sessions/<percent-encoded cwd>/<session_id>/`
//! (`chat_history.jsonl`, `updates.jsonl`, `summary.json`, terminal logs…);
//! ACP `session/load` looks it up under the cwd it is given, so the folder is
//! re-keyed by the new cwd. `summary.json` (`info.cwd`, `grok_home`) and
//! `prompt_context.json` (`working_directory`) are rewritten; transcripts
//! and the cached system prompt are history and stay verbatim. Subagent
//! sessions named by `subagent_spawned` updates travel along under
//! `children/<id>/`. `sessions/session_search.sqlite` is only a search index.
//!
//! Encoding verified against grok 1.0.44: bytes outside `A-Za-z0-9-._~`
//! become `%XX`, and the cwd is used as given (no symlink resolution).
use crate::portable::{
    Format, Import, Placer, PortRoots, Rewriter, Rule, Seg, SessionExport, checked_id, list_files,
    read_json, rewrite_path, sort_maps,
};
use anyhow::{Context, bail};
use serde_json::{Map, Value, json};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use zeron_proto::HarnessId;

const MAX_CHILDREN: usize = 64;

const RULES: &[Rule] = &[
    Rule {
        kind: None,
        path: &[Seg::Key("info"), Seg::Key("cwd")],
    },
    Rule {
        kind: None,
        path: &[Seg::Key("working_directory")],
    },
    Rule {
        kind: None,
        path: &[Seg::Key("grok_home")],
    },
];

/// Grok's directory name for `cwd`.
pub(crate) fn cwd_dir_name(cwd: &str) -> String {
    let mut out = String::with_capacity(cwd.len() * 2);
    for byte in cwd.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn locate(root: &Path, id: &str, cwd: Option<&str>) -> Option<PathBuf> {
    if let Some(cwd) = cwd {
        let dir = root.join(cwd_dir_name(cwd)).join(id);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(id))
        .find(|dir| dir.is_dir())
}

/// Child session ids announced by `subagent_spawned` updates.
fn spawned_children(session: &Path) -> Vec<String> {
    let Ok(file) = std::fs::File::open(session.join("updates.jsonl")) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .split(b'\n')
        .map_while(Result::ok)
        .filter(|line| line.windows(16).any(|w| w == b"subagent_spawned"))
        .filter_map(|line| serde_json::from_slice::<Value>(&line).ok())
        .filter_map(|v| {
            let update = &v["params"]["update"];
            (update["sessionUpdate"] == "subagent_spawned")
                .then(|| update["child_session_id"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect()
}

pub(crate) fn export(roots: &PortRoots, id: &str, cwd: &str) -> anyhow::Result<SessionExport> {
    let id = checked_id(id)?;
    let root = &roots.grok_sessions;
    let session =
        locate(root, id, Some(cwd)).with_context(|| format!("Grok session {id} was not found"))?;
    let skip = |name: &str| name.ends_with(".lock");
    let mut files = list_files(&session, "session", skip)?;
    let mut children = Map::new();
    let mut pending = spawned_children(&session);
    while let Some(child) = pending.pop() {
        if children.len() >= MAX_CHILDREN || child == id || children.contains_key(&child) {
            continue;
        }
        let Ok(child) = checked_id(&child).map(str::to_owned) else {
            continue;
        };
        let Some(dir) = locate(root, &child, Some(cwd)) else {
            continue;
        };
        files.extend(list_files(&dir, &format!("children/{child}"), skip)?);
        children.insert(
            child.clone(),
            read_json(&dir.join("summary.json"))["info"]["cwd"].clone(),
        );
        pending.extend(spawned_children(&dir));
    }
    Ok(SessionExport {
        harness: HarnessId::Grok,
        session_id: id.to_owned(),
        cwd: cwd.to_owned(),
        files,
        meta: json!({
            "recordedCwd": read_json(&session.join("summary.json"))["info"]["cwd"],
            "grokHome": root.parent(),
            "children": children,
        }),
    })
}

pub(crate) fn import(import: &mut Import, roots: &PortRoots) -> anyhow::Result<()> {
    let id = checked_id(&import.export.session_id)?.to_owned();
    let root = &roots.grok_sessions;
    let cwd_maps = import.cwd_maps();
    let mut maps = cwd_maps.clone();
    if let (Some(old), Some(new)) = (
        import.meta_str("grokHome"),
        root.parent().and_then(Path::to_str),
    ) {
        maps.push((old.to_owned(), new.to_owned()));
        sort_maps(&mut maps);
    }
    let rewriter = Rewriter::new(&maps, RULES);
    let children = import.export.meta["children"].clone();
    let session = root.join(cwd_dir_name(import.new_cwd)).join(&id);
    let files: Vec<_> = import.files().map(|(r, p)| (r.to_owned(), p)).collect();
    let mut placer = Placer::new(root.parent().unwrap_or(root), &mut import.warnings);
    for (rel, src) in &files {
        let (dest, rest) = if let Some(rest) = rel.strip_prefix("session/") {
            (session.join(rest), rest)
        } else if let Some((child, rest)) = rel
            .strip_prefix("children/")
            .and_then(|r| r.split_once('/'))
        {
            let child = checked_id(child)?;
            // A child keeps its own cwd, moved along when it was under the chat's.
            let cwd = children[child]
                .as_str()
                .map(|cwd| rewrite_path(cwd, &cwd_maps).unwrap_or_else(|| cwd.to_owned()))
                .unwrap_or_else(|| import.new_cwd.to_owned());
            (root.join(cwd_dir_name(&cwd)).join(child).join(rest), rest)
        } else {
            bail!("unexpected file in a Grok session export: {rel}");
        };
        let rewrite = matches!(rest, "summary.json" | "prompt_context.json")
            .then_some((&rewriter, Format::Document));
        placer.place(src, &dest, rewrite)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::export_session;
    use crate::portable::test_support::*;

    const ID: &str = "01a0ea45-7ea0-7452-95e2-b387dfbde22d";
    const CHILD: &str = "01a0ea46-0000-7000-8000-0000000c41d0";

    fn summary(id: &str, cwd: &str, home: &Path) -> String {
        serde_json::to_string_pretty(&json!({
            "info": {"id": id, "cwd": cwd},
            "agent_id": "agent-1",
            "num_messages": 4,
            "grok_home": home,
            "generated_title": "Fix the build"
        }))
        .unwrap()
    }

    fn fixture(a: &PortRoots) -> PathBuf {
        let home = a.grok_sessions.parent().unwrap();
        let dir = a.grok_sessions.join("%2FUsers%2Falice%2Fdev%2Fproj");
        let session = dir.join(ID);
        write(&session.join("summary.json"), summary(ID, OLD_CWD, home));
        write(
            &session.join("prompt_context.json"),
            serde_json::to_string_pretty(
                &json!({"version": 1, "working_directory": OLD_CWD, "shell_path": "/bin/zsh"}),
            )
            .unwrap(),
        );
        write(
            &session.join("chat_history.jsonl"),
            json!({"type": "system", "content": format!("Working directory: {OLD_CWD}")})
                .to_string()
                + "\n",
        );
        let spawned = json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": ID, "update": {"sessionUpdate": "subagent_spawned", "child_session_id": CHILD}}});
        let task = json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": ID, "update": {"sessionUpdate": "task_backgrounded", "cwd": OLD_CWD}}});
        write(
            &session.join("updates.jsonl"),
            format!("{spawned}\n{task}\n"),
        );
        write(&session.join("terminal/call-1.log"), "$ ls\n");
        write(&session.join("chat_history.jsonl.lock"), "");
        let child = dir.join(CHILD);
        write(&child.join("summary.json"), summary(CHILD, OLD_CWD, home));
        write(
            &child.join("chat_history.jsonl"),
            "{\"type\":\"user\",\"content\":\"hi\"}\n",
        );
        session
    }

    #[test]
    fn encodes_cwds_like_grok() {
        // Observed on grok 1.0.44 for a cwd with every interesting character.
        assert_eq!(
            cwd_dir_name("/tmp/grok-probe/cwd a-b.c_d~e!f(g)h*i+j,k"),
            "%2Ftmp%2Fgrok-probe%2Fcwd%20a-b.c_d~e%21f%28g%29h%2Ai%2Bj%2Ck"
        );
        assert_eq!(cwd_dir_name("/Users/daniel"), "%2FUsers%2Fdaniel");
        assert_eq!(cwd_dir_name("/é"), "%2F%C3%A9");
    }

    #[test]
    fn rekeys_the_session_folder_and_subagent_lookup_finds_children() {
        let (_dir, a, b) = devices();
        let source = fixture(&a);
        let (export, imported) = carry(HarnessId::Grok, &a, &b, ID, OLD_CWD, NEW_CWD);
        assert_eq!(imported.unwrap().session_id, ID);
        assert!(export.files.iter().all(|f| !f.rel.ends_with(".lock")));
        assert_eq!(export.meta["children"][CHILD], OLD_CWD);

        let session = b.grok_sessions.join("%2Fhome%2Fbob%2Fsrc%2Fproj").join(ID);
        let summary_text = std::fs::read_to_string(session.join("summary.json")).unwrap();
        let summary: Value = serde_json::from_str(&summary_text).unwrap();
        assert_eq!(summary["info"]["cwd"], NEW_CWD);
        assert_eq!(
            summary["grok_home"],
            json!(b.grok_sessions.parent().unwrap())
        );
        assert!(summary_text.contains("\n  \"info\": {"), "layout kept");
        let context = read_json(&session.join("prompt_context.json"));
        assert_eq!(context["working_directory"], NEW_CWD);
        for name in ["chat_history.jsonl", "updates.jsonl", "terminal/call-1.log"] {
            assert_eq!(
                std::fs::read(session.join(name)).unwrap(),
                std::fs::read(source.join(name)).unwrap(),
                "{name}"
            );
        }
        assert!(!session.join("chat_history.jsonl.lock").exists());

        let history = crate::acp::subagent::locate_history(&b.grok_sessions, CHILD).unwrap();
        assert_eq!(
            history,
            b.grok_sessions
                .join("%2Fhome%2Fbob%2Fsrc%2Fproj")
                .join(CHILD)
                .join("chat_history.jsonl")
        );
        let child: Value = read_json(&history.with_file_name("summary.json"));
        assert_eq!(child["info"]["cwd"], NEW_CWD);
    }

    #[test]
    fn a_missing_session_is_an_error() {
        let (_dir, a, _) = devices();
        assert!(export_session(HarnessId::Grok, &a, ID, OLD_CWD).is_err());
    }
}
