//! Experimental transcript editing (see [`crate::transcript_edit`]): the
//! editor takes the selected chat's transcript slot, and its Save bar takes
//! the composer's.
use super::*;
use crate::transcript_edit::{TranscriptEditor, TranscriptEditorEvent};

pub(super) struct TranscriptEditing {
    pub(super) editor: Entity<TranscriptEditor>,
    _events: Subscription,
}

impl Shell {
    /// Whether the chat's menu offers "Edit transcript": the experiment is
    /// on and the chat's agent keeps a history the host can rewrite.
    pub(super) fn can_edit_transcript(&self, chat_id: &str, cx: &App) -> bool {
        crate::settings::experimental_transcript_editing(cx)
            && self
                .state
                .read(cx)
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .and_then(|chat| chat.config.as_ref())
                .is_some_and(|config| crate::transcript_edit::supports(config.harness))
    }

    /// Open the editor over `chat_id`, selecting it in the main area first.
    pub(super) fn open_transcript_editor(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.close_chat_menu(cx);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(chat) = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .cloned()
        else {
            return;
        };
        let Some(harness) = chat.config.as_ref().map(|config| config.harness) else {
            return;
        };
        if self.state.read(cx).selected_chat.as_deref() != Some(chat_id.as_str()) {
            self.open_chat(chat_id.clone(), cx);
        }
        let editor = cx.new(|cx| {
            TranscriptEditor::new(chat.id.clone(), chat.device_id.clone(), harness, engine, cx)
        });
        let events = cx.subscribe(&editor, |this, _, event, cx| match event {
            TranscriptEditorEvent::Close => this.close_transcript_editor(cx),
        });
        self.transcript_editing = Some(TranscriptEditing {
            editor,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_transcript_editor(&mut self, cx: &mut Context<Self>) {
        if self.transcript_editing.take().is_some() {
            self.focus_composer(cx);
            cx.notify();
        }
    }

    /// The open editor, when it belongs to the chat on screen. An editor
    /// left behind by navigating away waits for its chat to come back.
    pub(super) fn visible_transcript_editor(&self, cx: &App) -> Option<Entity<TranscriptEditor>> {
        let editing = self.transcript_editing.as_ref()?;
        let selected = self.state.read(cx).selected_chat.clone()?;
        (editing.editor.read(cx).chat_id() == selected).then(|| editing.editor.clone())
    }
}
