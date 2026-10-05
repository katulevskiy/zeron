//! Forked conversation history: the copy a new child chat inherits from its
//! source. Shared by engines (the `ForkSideChat` RPC) and peer clients so
//! every platform cuts the copy at the same seam.

use crate::parts::{MessagePart, MessageStatus};
use crate::schema::{MessageRole, SessionMessageEntry};

/// The entry a fork's copied history ends at: the source's latest completed
/// assistant response. `None` until the source has one.
pub fn fork_boundary(entries: &[SessionMessageEntry]) -> Option<usize> {
    entries.iter().rposition(|entry| {
        entry.role == MessageRole::Assistant && entry.status == Some(MessageStatus::Complete)
    })
}

/// True when the source has a completed response to fork through. Borrowed
/// twin of [`fork_boundary`] for callers holding shared entry references
/// (the mobile session handle's `can_fork`); a working tail never blocks
/// forking the last complete response.
pub fn has_fork_boundary<'a>(entries: impl IntoIterator<Item = &'a SessionMessageEntry>) -> bool {
    entries.into_iter().any(|entry| {
        entry.role == MessageRole::Assistant && entry.status == Some(MessageStatus::Complete)
    })
}

/// Everything a fork inherits from `source`: the history through
/// [`fork_boundary`], with historical `Input` questions resolved (the source
/// runtime owns them), ending in the `Fork` seam entry. `None` when the
/// source has no completed response to fork through.
///
/// `target_chat_id` names the seam entry (`fork:<target>`), so re-running
/// cannot duplicate it; `source_title` is frozen into the seam so a later
/// rename or delete of the source does not rewrite history.
pub fn fork_entries(
    source: &[SessionMessageEntry],
    target_chat_id: &str,
    source_chat_id: &str,
    source_title: &str,
    device_id: &str,
    now_ms: i64,
) -> Option<Vec<SessionMessageEntry>> {
    let boundary = fork_boundary(source)?;
    let mut entries: Vec<SessionMessageEntry> = source[..=boundary].to_vec();
    for entry in &mut entries {
        for part in &mut entry.parts {
            if let MessagePart::Input { resolved, .. } = part {
                *resolved = true;
            }
        }
    }
    let marker_id = format!("fork:{target_chat_id}");
    entries.push(SessionMessageEntry {
        duration_ms: None,
        id: marker_id.clone(),
        role: MessageRole::System,
        parts: vec![MessagePart::Fork {
            id: marker_id,
            source_chat_id: source_chat_id.to_owned(),
            source_title: source_title.to_owned(),
        }],
        created_at: now_ms,
        device_id: device_id.to_owned(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
    });
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::MessagePart;

    fn entry(id: &str, role: MessageRole, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts,
            created_at: 1_000,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn text(id: &str) -> MessagePart {
        MessagePart::Text {
            id: id.into(),
            text: id.into(),
        }
    }

    #[test]
    fn boundary_is_the_latest_complete_assistant_response() {
        let source = vec![
            entry("u1", MessageRole::User, vec![text("t1")]),
            entry("a1", MessageRole::Assistant, vec![text("t2")]),
            entry("u2", MessageRole::User, vec![text("t3")]),
            entry("a2", MessageRole::Assistant, vec![text("t4")]),
        ];
        assert_eq!(fork_boundary(&source), Some(3));
        // An in-flight tail is not a boundary.
        let streaming = vec![
            source[0].clone(),
            source[1].clone(),
            SessionMessageEntry {
                status: Some(MessageStatus::Streaming),
                ..source[3].clone()
            },
        ];
        assert_eq!(fork_boundary(&streaming), Some(1));
        // Nothing complete yet: nothing to fork through.
        assert_eq!(fork_boundary(&[source[0].clone()]), None);
    }

    #[test]
    fn fork_entries_copies_through_the_boundary_and_appends_the_seam() {
        let source = vec![
            entry("u1", MessageRole::User, vec![text("t1")]),
            entry("a1", MessageRole::Assistant, vec![text("t2")]),
            entry(
                "u2",
                MessageRole::User,
                vec![MessagePart::Input {
                    id: "i1".into(),
                    request_id: "r1".into(),
                    questions: Vec::new(),
                    resolved: false,
                }],
            ),
            entry("a2", MessageRole::Assistant, vec![text("t4")]),
        ];
        let forked = fork_entries(&source, "child", "parent", "Parent title", "phone", 42).unwrap();
        // Everything through the latest completed response, plus the seam.
        assert_eq!(forked.len(), 5);
        assert_eq!(forked[2].id, "u2");
        // The source runtime owns historical questions: the copy resolves them.
        assert!(matches!(
            &forked[2].parts[0],
            MessagePart::Input { resolved: true, .. }
        ));
        // The seam closes the copy and names the source.
        assert_eq!(forked[4].id, "fork:child");
        assert_eq!(forked[4].role, MessageRole::System);
        assert_eq!(forked[4].created_at, 42);
        assert_eq!(forked[4].device_id, "phone");
        assert!(matches!(
            &forked[4].parts[0],
            MessagePart::Fork { id, source_chat_id, source_title }
                if id == "fork:child" && source_chat_id == "parent" && source_title == "Parent title"
        ));
    }

    #[test]
    fn no_complete_response_means_no_fork() {
        let source = vec![entry("u1", MessageRole::User, vec![text("t1")])];
        assert!(fork_entries(&source, "child", "parent", "Parent", "phone", 42).is_none());
    }

    #[test]
    fn has_fork_boundary_ignores_a_working_tail() {
        let source = vec![
            entry("u1", MessageRole::User, vec![text("t1")]),
            entry("a1", MessageRole::Assistant, vec![text("t2")]),
            SessionMessageEntry {
                status: Some(MessageStatus::Streaming),
                ..entry("a2", MessageRole::Assistant, vec![text("t3")])
            },
        ];
        assert!(has_fork_boundary(source.iter()));
        assert!(!has_fork_boundary(source[..1].iter()));
    }
}
