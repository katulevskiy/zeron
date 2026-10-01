//! What the agent is told when it wakes up on the new device, and what was
//! left running on the old one.
//!
//! The note is prepended once to the next prompt its harness receives
//! (`SessionDoc::move_note`); the user sees only the transcript's move seam.

use std::path::Path;

/// Commands still running with their working folder inside the workspace on
/// the source device (dev servers, watchers, builds). They don't move; the
/// agent is told so it can restart what it needs.
pub fn processes_in(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let me = std::process::id();
        let Ok(read) = std::fs::read_dir("/proc") else {
            return found;
        };
        for entry in read.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == me {
                continue;
            }
            let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) else {
                continue;
            };
            if !cwd.starts_with(root) {
                continue;
            }
            let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let command = cmdline
                .split(|b| *b == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            if !command.is_empty() {
                found.push(format!("{command} (pid {pid})"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // `lsof` reports every process's cwd in one pass.
        let output = std::process::Command::new("lsof")
            .args(["-a", "-d", "cwd", "-Fpn", "-w"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let Ok(output) = output else {
            return found;
        };
        let me = std::process::id();
        let mut pid: Option<u32> = None;
        let mut pids = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(value) = line.strip_prefix('p') {
                pid = value.parse().ok();
            } else if let Some(value) = line.strip_prefix('n')
                && let Some(pid) = pid
                && pid != me
                && Path::new(value).starts_with(root)
            {
                pids.push(pid);
            }
        }
        if pids.is_empty() {
            return found;
        }
        let list = pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "pid=,command=", "-p", &list])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                if let Some((pid, command)) = line.split_once(' ') {
                    found.push(format!("{} (pid {pid})", command.trim()));
                }
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = root;
    found.sort();
    found.dedup();
    found.truncate(20);
    found
}

/// Everything the agent should know after the move, as one message.
pub struct NoteInput<'a> {
    pub from_device: &'a str,
    pub to_device: &'a str,
    /// `(old path, new path)` for the workspace and every extra that landed
    /// somewhere else than where it was.
    pub path_map: &'a [(String, String)],
    pub processes: &'a [String],
    pub pending_question: Option<&'a str>,
    pub was_active: bool,
    /// The scout's advice and the move's own notes.
    pub lines: &'a [String],
}

pub fn compose(input: &NoteInput<'_>) -> String {
    let mut out = format!(
        "[Zeron] This session just moved from the computer \"{}\" to \"{}\". \
         Your conversation, the project folder and the files you were using came along.",
        input.from_device, input.to_device
    );
    let moved: Vec<&(String, String)> = input.path_map.iter().filter(|(a, b)| a != b).collect();
    if !moved.is_empty() {
        out.push_str("\nPaths are different here — use the new ones:");
        for (from, to) in moved {
            out.push_str(&format!("\n- {from} → {to}"));
        }
    }
    if !input.processes.is_empty() {
        out.push_str(
            "\nThese were still running on the old computer and did not move \
             (restart any you need):",
        );
        for process in input.processes {
            out.push_str(&format!("\n- {process}"));
        }
    }
    out.push_str(
        "\nBuild outputs and installed dependencies (node_modules, target, virtualenvs) were \
         not copied: rebuild or reinstall them if you need them.",
    );
    for line in input.lines {
        out.push_str(&format!("\n- {line}"));
    }
    if let Some(question) = input.pending_question {
        out.push_str(&format!(
            "\nYou were waiting for the user to answer this question; ask it again: {question}"
        ));
    }
    if input.was_active {
        out.push_str(
            "\nYou were interrupted mid-task by the move. Check the state of the files, then \
             continue exactly where you left off.",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_lists_only_what_changed() {
        let note = compose(&NoteInput {
            from_device: "Laptop",
            to_device: "Desktop",
            path_map: &[
                ("/Users/a/dev/p".into(), "/home/b/dev/p".into()),
                ("/same".into(), "/same".into()),
            ],
            processes: &["npm run dev (pid 7)".into()],
            pending_question: Some("Which port?"),
            was_active: true,
            lines: &["Run npm install first.".into()],
        });
        assert!(note.contains("/Users/a/dev/p → /home/b/dev/p"));
        assert!(!note.contains("/same →"));
        assert!(note.contains("npm run dev (pid 7)"));
        assert!(note.contains("Which port?"));
        assert!(note.contains("Run npm install first."));
        assert!(note.contains("continue exactly where you left off"));
    }

    #[test]
    fn processes_in_an_empty_folder_are_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(processes_in(dir.path()).is_empty());
    }
}
