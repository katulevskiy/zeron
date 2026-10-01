//! The files a session touched outside its workspace, read from its run
//! journal: paths its tools read, wrote, searched or passed to commands
//! (`~/Desktop/shot.png`, `/data/train.csv`). These travel with a move as
//! extras unless they sit in a secret store, are gone, or are too big.
//!
//! This is the deterministic half of deciding what a move carries; the
//! scout agent (`scout.rs`) adds what only reading the conversation reveals.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use zeron_proto::{AgentEvent, ToolCall};

/// Per-item size cap for harvested extras (a folder counts its contents).
pub const MAX_EXTRA_BYTES: u64 = 512 * 1024 * 1024;
/// Paths considered at most (a long session can mention thousands).
const MAX_CANDIDATES: usize = 2000;

/// A path the session used outside its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub is_dir: bool,
    pub bytes: u64,
}

/// Absolute (or `~`) paths in the session's tool calls that exist outside
/// `workspace` and aren't secret stores, deduplicated so a folder covers the
/// files under it.
pub fn candidates(
    events: &[AgentEvent],
    workspace: Option<&Path>,
    home: Option<&Path>,
    is_secret: impl Fn(&Path) -> bool,
) -> Vec<Candidate> {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for event in events {
        let AgentEvent::ToolCall { call, .. } = event else {
            continue;
        };
        for raw in tool_paths(call) {
            if seen.len() >= MAX_CANDIDATES {
                break;
            }
            if let Some(path) = normalize(&raw, home) {
                seen.insert(path);
            }
        }
    }
    let mut out: Vec<Candidate> = Vec::new();
    for path in seen {
        // Inside the workspace it travels anyway; a folder holding it would
        // drag the workspace (and its neighbours) along a second time.
        if workspace.is_some_and(|ws| path.starts_with(ws) || ws.starts_with(&path)) {
            continue;
        }
        if home.is_some_and(|home| path == home) || path.parent().is_none() {
            continue; // never the whole home folder or `/`
        }
        let in_home = home.is_some_and(|home| path.starts_with(home));
        if is_secret(&path) || (!in_home && system_path(&path)) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue; // gone (or never existed: a path in a command's text)
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let is_dir = meta.is_dir();
        let bytes = if is_dir {
            match folder_bytes(&path, MAX_EXTRA_BYTES) {
                Some(bytes) => bytes,
                None => continue, // too big to carry unasked
            }
        } else if meta.is_file() {
            meta.len()
        } else {
            continue;
        };
        if bytes > MAX_EXTRA_BYTES {
            continue;
        }
        out.push(Candidate {
            path,
            is_dir,
            bytes,
        });
    }
    // A folder already carried covers everything under it.
    let dirs: Vec<PathBuf> = out
        .iter()
        .filter(|c| c.is_dir)
        .map(|c| c.path.clone())
        .collect();
    out.retain(|c| !dirs.iter().any(|d| c.path != *d && c.path.starts_with(d)));
    out
}

/// The raw path strings one tool call names.
fn tool_paths(call: &ToolCall) -> Vec<String> {
    match call {
        ToolCall::ReadFile { path }
        | ToolCall::WriteFile { path, .. }
        | ToolCall::EditFile { path, .. } => vec![path.clone()],
        ToolCall::ApplyPatch { path } | ToolCall::Search { path, .. } => {
            path.iter().cloned().collect()
        }
        ToolCall::Exec { command } => command_paths(command),
        ToolCall::Mcp { input, .. } | ToolCall::Unknown { input, .. } => {
            let mut out = Vec::new();
            if let Some(input) = input {
                json_paths(input, &mut out);
            }
            out
        }
        ToolCall::Glob { .. }
        | ToolCall::WebFetch { .. }
        | ToolCall::WebSearch { .. }
        | ToolCall::Todo { .. } => Vec::new(),
    }
}

/// Path-looking words of a shell command: `/abs/…` or `~/…`, with quotes
/// and trailing punctuation trimmed. Over-matching is harmless: a word that
/// isn't an existing path is dropped later.
fn command_paths(command: &str) -> Vec<String> {
    command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '<' | '>' | '='))
        .map(|word| word.trim_matches(|c| matches!(c, '"' | '\'' | '`' | ',')))
        .filter(|word| word.starts_with('/') || word.starts_with("~/"))
        .map(|word| word.trim_end_matches(['.', ':']).to_owned())
        .filter(|word| word.len() > 1 && !word.contains(['*', '?', '$']))
        .collect()
}

/// String values under path-ish keys of a tool's JSON input.
fn json_paths(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_ascii_lowercase();
                match value {
                    serde_json::Value::String(text)
                        if key.contains("path") || key.contains("file") || key == "dir" =>
                    {
                        out.push(text.clone());
                    }
                    other => json_paths(other, out),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                json_paths(item, out);
            }
        }
        _ => {}
    }
}

/// `~` expanded, lexically normalized (no `.`/`..`), absolute only.
fn normalize(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    let raw = raw.trim();
    let path = match raw.strip_prefix("~/") {
        Some(rest) => home?.join(rest),
        None => PathBuf::from(raw),
    };
    if !path.is_absolute() {
        return None;
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    Some(clean)
}

/// OS and toolchain locations a move never carries: they exist on every
/// machine in their own form.
fn system_path(path: &Path) -> bool {
    const ROOTS: &[&str] = &[
        "/bin", "/sbin", "/usr", "/etc", "/dev", "/proc", "/sys", "/System", "/Library",
        "/Applications", "/opt/homebrew", "/private/etc", "/var/run", "/run", "/boot", "/lib",
        "/lib64", "/nix", "/snap", "/Volumes", "/mnt/c", "/tmp", "/private/tmp",
        "/private/var/folders",
    ];
    ROOTS.iter().any(|root| path.starts_with(root))
}

/// Total bytes of a folder's regular files, `None` past `limit` (stops early).
pub(super) fn folder_bytes(dir: &Path, limit: u64) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    let mut entries = 0usize;
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).ok()?;
        for entry in read.flatten() {
            entries += 1;
            if entries > 100_000 {
                return None;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
                if total > limit {
                    return None;
                }
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(call: ToolCall) -> AgentEvent {
        AgentEvent::ToolCall {
            id: "t".into(),
            call,
        }
    }

    #[test]
    fn paths_outside_the_workspace_are_found_in_every_tool_shape() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let ws = home.join("dev/proj");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(home.join("Desktop/shots")).unwrap();
        std::fs::write(home.join("Desktop/shots/a.png"), b"png").unwrap();
        std::fs::write(home.join("Desktop/notes.txt"), b"notes").unwrap();
        std::fs::write(home.join("data.csv"), b"1,2").unwrap();
        std::fs::write(ws.join("src/main.rs"), b"fn main(){}").unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), b"secret").unwrap();
        let events = vec![
            call(ToolCall::ReadFile {
                path: ws.join("src/main.rs").display().to_string(),
            }),
            call(ToolCall::ReadFile {
                path: "~/Desktop/notes.txt".into(),
            }),
            call(ToolCall::Exec {
                command: format!(
                    "convert '{}/Desktop/shots/a.png' out.png && cat ~/data.csv; ls /usr/bin ~/missing",
                    home.display()
                ),
            }),
            call(ToolCall::Exec {
                command: "cp ~/.ssh/id_ed25519 /tmp/x".into(),
            }),
            call(ToolCall::Mcp {
                server: "fs".into(),
                tool: "read".into(),
                input: Some(serde_json::json!({ "filePath": home.join("Desktop/shots").display().to_string() })),
            }),
        ];
        let found = candidates(&events, Some(&ws), Some(&home), |p| {
            p.starts_with(home.join(".ssh"))
        });
        let paths: Vec<PathBuf> = found.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                home.join("Desktop/notes.txt"),
                home.join("Desktop/shots"),
                home.join("data.csv"),
            ],
            "workspace, secrets, system, missing and covered paths are dropped"
        );
        assert!(found.iter().any(|c| c.is_dir && c.bytes == 3));
    }

    #[test]
    fn command_words_are_trimmed() {
        assert_eq!(
            command_paths("cat \"/a/b c\" '/x/y'; echo ~/z: >/out.txt FOO=/etc/x /tmp/*.log"),
            vec!["/a/b", "/x/y", "~/z", "/out.txt", "/etc/x"]
        );
    }

    #[test]
    fn normalization_resolves_dots_and_rejects_relative() {
        let home = Path::new("/home/bob");
        assert_eq!(
            normalize("~/a/../b/./c", Some(home)),
            Some(PathBuf::from("/home/bob/b/c"))
        );
        assert_eq!(normalize("rel/path", Some(home)), None);
        assert_eq!(normalize("~/x", None), None);
    }
}
