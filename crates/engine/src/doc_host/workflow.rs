//! What dynamic workflows need from the doc host: the service slot, machine
//! messages into the parent chat's queue, transcript markers, and a nudge for
//! the goal controller. The run logic itself lives in `crate::workflow`.

use zeron_proto::MessageOrigin;

use super::*;

impl DocHost {
    /// Wire the workflow runner (engine assembly).
    pub fn set_workflows(&self, workflows: crate::workflow::WorkflowService) {
        *lock(&self.inner.workflows) = Some(workflows);
    }

    pub fn workflows(&self) -> Option<crate::workflow::WorkflowService> {
        lock(&self.inner.workflows).clone()
    }

    /// Queue a machine-origin user message into `chat_id`, delivered through
    /// the normal queue (so ordering and "never interrupt a running turn"
    /// are the queue's own rules). Idempotent by `id`: a message already
    /// queued or already in the transcript is not queued again. `true` when
    /// this call queued it.
    pub(crate) fn enqueue_machine_message(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let queue = handle.doc.read_queue()?;
        let delivered = handle.doc.read_entries()?.iter().any(|e| e.id == id);
        if delivered || queue.iter().any(|row| row.id == id) {
            return Ok(false);
        }
        let mut row = QueuedMessage::new(
            id.to_owned(),
            text.to_owned(),
            self.inner.config.device_id.clone(),
        );
        row.issued_at = now_ms();
        row.origin = Some(origin);
        handle.doc.push_queued(&row)?;
        // A queue frozen by an earlier Stop would hold this forever; thaw it
        // only when no person's row is in it (those are never reached).
        if queue.is_empty() {
            handle.queue_paused.store(false, Ordering::Release);
        }
        handle.publish_queue();
        Ok(true)
    }

    /// Append a system-role marker row (idempotent by `id`).
    pub(crate) fn push_system_marker(
        &self,
        chat_id: &str,
        id: &str,
        text: &str,
        origin: MessageOrigin,
    ) -> Result<(), EngineError> {
        let handle = self.open(chat_id)?;
        if handle.doc.read_entries()?.iter().any(|e| e.id == id) {
            return Ok(());
        }
        handle.doc.push_message(&SessionMessageEntry {
            origin: Some(origin),
            id: id.to_owned(),
            role: MessageRole::System,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_owned(),
            }],
            created_at: now_ms(),
            device_id: handle.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })?;
        Ok(())
    }

    /// The models a harness offers (cached catalog; `None` when it cannot be
    /// listed — callers then trust the caller's pick rather than guess).
    pub(crate) async fn list_harness_models(
        &self,
        harness: zeron_proto::HarnessId,
    ) -> Option<Vec<zeron_proto::Model>> {
        let sessions = self.sessions()?;
        let repos = self.inner.repos.get()?;
        let harness = sessions.resolve_harness(harness).ok()?;
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            crate::model_catalogs::list_with_lease(repos.data_dir(), harness, false, None),
        )
        .await
        .ok()?
        .ok()
    }

    /// Ask the goal controller to look again (a workflow it was waiting on
    /// just settled).
    pub async fn goal_nudge(&self, chat_id: &str) {
        if let Ok(handle) = self.open(chat_id) {
            self.goal_tick(&handle).await;
        }
    }
}
