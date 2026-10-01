//! Engine ⇄ engine messages of a move (`MovePrepare`, `MoveStage`,
//! `MoveCommit`, `MoveAbort`, `MoveProbe`). The source drives; the target
//! answers and decides where everything lands on its own disk — no message
//! names an absolute path on the receiving device.

use serde::{Deserialize, Serialize};
use zeron_proto::HarnessId;

/// Logical roots files travel under (`<root>/<rel>` in a sync transfer).
pub const ROOT_WORKSPACE: &str = "ws";
pub const ROOT_SESSION: &str = "hs";
pub const ROOT_UPLOADS: &str = "up";
pub const ROOT_GIT: &str = "git";
pub const ROOT_LOGIN: &str = "login";
/// Extra paths outside the workspace: `x0`, `x1`, …
pub fn extra_root(index: usize) -> String {
    format!("x{index}")
}

/// `MoveProbe`: can this device take a chat running `harness`?
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeParams {
    pub harness: HarnessId,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReply {
    pub harness_installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_signed_in: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
}

/// What kind of folder the chat works in on the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceKind {
    /// A git checkout: objects travel as a bundle, the tree as files.
    Git,
    /// A plain folder: the tree travels as files.
    Folder,
    /// Too broad to carry (the home folder itself): only the files the
    /// session used travel, as extras.
    None,
}

/// The repository the workspace belongs to, as the target needs to find or
/// make its own copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDesc {
    pub root_commits: Vec<String>,
    /// `(name, url)`; urls already stripped of credentials.
    pub remotes: Vec<(String, String)>,
    pub head: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDesc {
    pub kind: WorkspaceKind,
    /// The workspace root's folder name (`zeron`).
    pub name: String,
    /// The root relative to the source's home folder, `/`-separated, when it
    /// lives under it (`dev/zeron`): the target prefers the same place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_rel: Option<String>,
    /// The chat's cwd relative to the root (`""` = the root itself).
    #[serde(default)]
    pub cwd_rel: String,
    /// Absolute root on the source (shown in notes; never used as a path on
    /// the target).
    pub source_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoDesc>,
}

/// One extra path outside the workspace (a screenshot folder, a dataset).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraDesc {
    /// Its logical root (`x0`…); the item travels as `<root>/<name>`.
    pub root: String,
    pub name: String,
    /// Absolute path on the source.
    pub source_path: String,
    /// Its parent relative to the source's home, when under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_home_rel: Option<String>,
    pub is_dir: bool,
    /// Where it first came from (`"<device id>:<path>"`), when it arrived on
    /// the source with an earlier move: a chat going back puts it back there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareParams {
    pub move_id: String,
    pub chat_id: String,
    pub from_device_id: String,
    pub from_device_name: String,
    #[serde(default)]
    pub chat_title: Option<String>,
    pub harness: HarnessId,
    pub workspace: WorkspaceDesc,
    #[serde(default)]
    pub extras: Vec<ExtraDesc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareReply {
    pub harness_installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_signed_in: Option<bool>,
    /// Commits the target's repository already has (bundle exclusions).
    #[serde(default)]
    pub repo_tips: Vec<String>,
    /// The target has landed this chat before (move back): only what changed
    /// since needs to travel.
    #[serde(default)]
    pub returning: bool,
    /// Where the workspace will land (for the source's notes and banner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<String>,
}

/// `MoveStage`: a git bundle (round 0) has landed in staging; build the
/// workspace landing now so later rounds compare against real content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageParams {
    pub move_id: String,
    /// The staging round the bundle travelled in, if one was needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_round: Option<u32>,
    pub head: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageReply {
    pub landing: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Final git state, captured with the agent stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFinal {
    pub head: Option<String>,
    pub branch: Option<String>,
    /// The round carrying a bundle of commits made since the stage, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_round: Option<u32>,
    /// `git diff --cached --binary`, base64.
    #[serde(default)]
    pub staged_patch_b64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitParams {
    pub move_id: String,
    pub chat_id: String,
    /// The staging rounds that carry files, oldest first (apply order).
    pub rounds: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitFinal>,
    /// The harness session as exported on the source; its files travelled
    /// under `hs/` in the last round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<zeron_harness::portable::SessionExport>,
    /// Ledger claims for the chat's commands.
    #[serde(default)]
    pub processed_commands: Vec<(String, i64)>,
    /// A turn was running (or waiting on a question) when the agent stopped:
    /// the target continues it.
    pub was_active: bool,
    /// The question the agent was waiting on, to ask again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_question: Option<String>,
    /// Lines for the agent's move note (processes left behind, the scout's
    /// advice). The target adds its own path mappings.
    #[serde(default)]
    pub note_lines: Vec<String>,
    /// For the transcript seam.
    pub started_at: i64,
    pub files_sent: u64,
    pub bytes_sent: u64,
    /// The login travelled under `login/` (target had none).
    #[serde(default)]
    pub login_included: bool,
    /// Every workspace file and symlink on the source when the agent
    /// stopped (`/`-separated, relative to the root): the target deletes
    /// only files missing from THIS list, never ones a transfer merely
    /// skipped or left out.
    #[serde(default)]
    pub workspace_rels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitReply {
    pub cwd: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// What a target did with a move it was asked to abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AbortOutcome {
    /// Forgotten (or never known): the source keeps the chat.
    Aborted,
    /// The commit is being applied; the abort stops it before the handover
    /// if it still can. Ask again.
    Applying,
    /// The target took the chat over: the move happened.
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortReply {
    pub outcome: AbortOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortParams {
    pub move_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
