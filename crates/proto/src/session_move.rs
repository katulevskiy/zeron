//! Moving a chat — its run, workspace and agent session — from the engine
//! hosting it to another of the account's engines (docs/session-move.md).
//!
//! The hosting engine drives a move and publishes its progress on the chat's
//! registry row ([`ChatMove`] in `Chat.move_state`), so every device and the
//! phones render the same banner from the rows they already sync.

use serde::{Deserialize, Serialize};

/// When the source stops the agent to hand the chat over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MoveWhen {
    /// Copy everything while the agent keeps working, then stop it between
    /// steps (no tool running) and move the last changes.
    #[default]
    SafePoint,
    /// Stop the agent now, even mid-step, and move.
    Now,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MovePhase {
    /// Asking the target and working out what has to travel.
    Preparing,
    /// Copying the workspace while the agent keeps working.
    Copying,
    /// Waiting for the agent to finish its current step.
    Waiting,
    /// Agent stopped: copying the last changes and its session.
    Finishing,
    /// The target is taking the chat over.
    Handover,
    /// The chat runs on the target.
    Done,
    Failed,
    Cancelled,
}

impl MovePhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// The agent has been (or is being) stopped on the source: cancelling now
    /// resumes it there.
    pub fn agent_stopped(self) -> bool {
        matches!(self, Self::Finishing | Self::Handover)
    }
}

/// A move's progress, published by the engine running it on the chat's
/// registry row (`move` field). Written only by that engine, and only with
/// targeted updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMove {
    pub id: String,
    pub from_device_id: String,
    pub to_device_id: String,
    pub to_device_name: String,
    pub phase: MovePhase,
    #[serde(default)]
    pub when: MoveWhen,
    /// One line for the banner: what the move is doing or waiting for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Bytes that must travel (unchanged files excluded) and how many have.
    #[serde(default)]
    pub bytes_total: u64,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub files_total: u64,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Things the user should know afterwards (a branch landed under another
    /// name, a file was backed up, a process didn't come along).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl ChatMove {
    pub fn is_live(&self) -> bool {
        !self.phase.is_terminal()
    }
}

/// `StartMove` (forwardable: `targetDeviceId` = the chat's host).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartMoveParams {
    pub chat_id: String,
    pub to_device_id: String,
    #[serde(default)]
    pub when: MoveWhen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartMoveReply {
    pub move_id: String,
}

/// `MoveNow` / `CancelMove` (forwardable to the host).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveControlParams {
    pub chat_id: String,
}

/// `MoveCandidates`: what each device can offer a chat before moving it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveCandidate {
    pub device_id: String,
    pub device_name: String,
    pub online: bool,
    /// The device runs an engine that can take moved chats.
    pub supported: bool,
    /// The chat's harness is installed there.
    #[serde(default)]
    pub harness_installed: bool,
    /// The harness is signed in there (`None` = unknown). When it isn't, the
    /// move carries this device's login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_signed_in: Option<bool>,
    /// Why the device can't take the chat, when it can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// A cloud box that is asleep: selectable, and woken when the move
    /// starts (docs/cloud.md §8).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asleep: bool,
    /// One line for the picker when nothing is wrong ("Asleep — wakes when
    /// you move").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
