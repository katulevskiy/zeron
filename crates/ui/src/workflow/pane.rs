//! The run pane: one workflow run, in full. A right-pane surface (the same
//! family as a subagent's transcript), opened from the card's maximize
//! button, a chip, a pill group's "+n more" or a sidebar line.
//!
//! It reads the chat's synced workflow state live, so it is current while the
//! run works. Long lists are paged ("show more") rather than all laid out;
//! nothing here owns a timer.

use std::collections::{HashMap, HashSet};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, prelude::*, px,
};
use zeron_proto::{WorkflowCommand, WorkflowRun, WorkflowStatus};

use super::artifact::ArtifactView;
use super::model::{
    ActorRow, Light, NodeRow, NodeState, PANE_ACTORS_PER_PAGE, PANE_NODES_PER_PAGE, PaneModel,
    PhaseGroup, PillState, StationBase, format_bytes, format_tokens,
};
use super::widgets::{
    icon_button, kind_icon, kind_word, lamp, pill_mark, status_glyph, tone_color,
};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::state::AppState;
use crate::theme::Theme;
use crate::typography::ui_rems;

/// What the pane asks of its host.
#[derive(Debug, Clone)]
pub enum PaneEvent {
    /// Open an agent's chat (a read-only tab).
    OpenActor { child_chat_id: String, title: String },
}

impl EventEmitter<PaneEvent> for WorkflowRunPane {}

enum View {
    Overview,
    Artifact(Entity<ArtifactView>),
}

pub struct WorkflowRunPane {
    state: Entity<AppState>,
    chat_id: String,
    run_id: String,
    view: View,
    /// Pages of node rows shown per phase.
    node_pages: HashMap<String, usize>,
    actor_pages: usize,
    /// The phase the steps list is narrowed to.
    phase_filter: Option<String>,
    scroll: gpui::ScrollHandle,
    answers: HashMap<String, Entity<ComposerInput>>,
    /// Answers sent and not yet gone from the state.
    sent: HashSet<String>,
    result_open: bool,
    /// A copy of the run for a run the live state no longer keeps (fetched
    /// once with `WorkflowGet`).
    fetched: Option<WorkflowRun>,
    fetching: Option<Task<()>>,
    fetch_failed: Option<String>,
    _observe: Subscription,
    _inputs: Vec<Subscription>,
}

impl WorkflowRunPane {
    pub fn new(
        state: Entity<AppState>,
        chat_id: String,
        run_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&state, |this, _, cx| this.on_state(cx));
        Self {
            state,
            chat_id,
            run_id,
            view: View::Overview,
            node_pages: HashMap::new(),
            actor_pages: 1,
            phase_filter: None,
            scroll: gpui::ScrollHandle::new(),
            answers: HashMap::new(),
            sent: HashSet::new(),
            result_open: false,
            fetched: None,
            fetching: None,
            fetch_failed: None,
            _observe: observe,
            _inputs: Vec::new(),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The run's name for the tab strip.
    pub fn title(&self, cx: &gpui::App) -> SharedString {
        self.state
            .read(cx)
            .workflows
            .run(&self.run_id)
            .map(|r| r.header.name.clone())
            .or_else(|| self.fetched.as_ref().map(|r| r.header.name.clone()))
            .filter(|n| !n.is_empty())
            .map_or_else(|| "Workflow".into(), SharedString::from)
    }

    /// Whether the run is still going (the tab shows a spinner).
    pub fn is_live(&self, cx: &gpui::App) -> bool {
        self.state
            .read(cx)
            .workflows
            .run(&self.run_id)
            .is_some_and(|r| !r.header.status.is_settled())
    }

    /// Land on a phase (the card's "+n more").
    pub fn land_on(&mut self, phase: Option<String>, cx: &mut Context<Self>) {
        self.view = View::Overview;
        if let Some(phase) = phase {
            let pages = self.node_pages.entry(phase.clone()).or_insert(1);
            *pages = (*pages).max(1);
            self.phase_filter = Some(phase);
        }
        cx.notify();
    }

    pub fn open_artifact(&mut self, artifact_id: &str, cx: &mut Context<Self>) {
        let Some(run) = self.run(cx) else {
            return;
        };
        let (kind, title) = match run.artifacts.iter().find(|a| a.id == artifact_id) {
            Some(a) => (a.kind, a.title.clone()),
            None => return,
        };
        let (state, run_id, id) = (self.state.clone(), self.run_id.clone(), artifact_id.to_owned());
        let view = cx.new(|cx| ArtifactView::new(state, run_id, id, kind, title, cx));
        self.view = View::Artifact(view);
        cx.notify();
    }

    fn run(&self, cx: &gpui::App) -> Option<WorkflowRun> {
        self.state
            .read(cx)
            .workflows
            .run(&self.run_id)
            .cloned()
            .or_else(|| self.fetched.clone())
    }

    fn on_state(&mut self, cx: &mut Context<Self>) {
        let (live, here, replayed) = {
            let state = self.state.read(cx);
            (
                state.workflows.run(&self.run_id).cloned(),
                state.selected_chat.as_deref() == Some(self.chat_id.as_str()),
                state.transcript_replayed,
            )
        };
        // Follow newly published versions of the artifact on screen.
        if let (View::Artifact(view), Some(run)) = (&self.view, &live) {
            let id = view.read(cx).artifact_id().to_owned();
            if let Some(a) = run.artifacts.iter().find(|a| a.id == id) {
                let latest = a.version;
                view.update(cx, |v, cx| v.refresh_if_newer(latest, cx));
            }
        }
        if let Some(run) = &live {
            let waiting: HashSet<&str> = run
                .pending_questions
                .iter()
                .map(|q| q.qid.as_str())
                .collect();
            self.sent.retain(|q| waiting.contains(q.as_str()));
            self.answers.retain(|q, _| waiting.contains(q.as_str()));
        } else if here
            && replayed
            && self.fetched.is_none()
            && self.fetching.is_none()
            && self.fetch_failed.is_none()
        {
            self.fetch_missing(cx);
        }
        cx.notify();
    }

    /// The run fell out of the live state (older than the 8 it keeps):
    /// read its record once.
    fn fetch_missing(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let run_id = self.run_id.clone();
        self.fetching = Some(cx.spawn(async move |this, cx| {
            let reply = engine
                .client()
                .call(
                    zeron_rpc::methods::WORKFLOW_GET,
                    serde_json::json!({ "runId": run_id, "include": ["nodes", "reports"] }),
                )
                .await;
            let parsed = reply.map_err(|e| e.to_string()).and_then(|v| {
                serde_json::from_value::<WorkflowRun>(v.get("run").cloned().unwrap_or_default())
                    .map_err(|e| e.to_string())
            });
            this.update(cx, |pane, cx| {
                pane.fetching = None;
                match parsed {
                    Ok(run) => pane.fetched = Some(run),
                    Err(err) => pane.fetch_failed = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn answer_input(&mut self, qid: &str, cx: &mut Context<Self>) -> Entity<ComposerInput> {
        if let Some(input) = self.answers.get(qid) {
            return input.clone();
        }
        let input = cx.new(|cx| ComposerInput::new("Answer the agent…", cx));
        let q = qid.to_owned();
        let sub = cx.subscribe(&input, move |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.send_answer(&q, cx);
            }
        });
        self._inputs.push(sub);
        self.answers.insert(qid.to_owned(), input.clone());
        input
    }

    fn send_answer(&mut self, qid: &str, cx: &mut Context<Self>) {
        let Some(input) = self.answers.get(qid).cloned() else {
            return;
        };
        let answer = input.read(cx).text().trim().to_owned();
        if answer.is_empty() || self.sent.contains(qid) {
            return;
        }
        self.sent.insert(qid.to_owned());
        let (chat_id, run_id, qid) = (self.chat_id.clone(), self.run_id.clone(), qid.to_owned());
        self.state.update(cx, |state, cx| {
            state.send_workflow_command(
                &chat_id,
                WorkflowCommand::Answer {
                    run_id,
                    qid,
                    answer,
                },
                cx,
            )
        });
        cx.notify();
    }

    fn command(&mut self, command: WorkflowCommand, cx: &mut Context<Self>) {
        let chat_id = self.chat_id.clone();
        self.state
            .update(cx, |state, cx| state.send_workflow_command(&chat_id, command, cx));
    }
}

// ── rendering ─────────────────────────────────────────────────────────────

const PAD: f32 = 16.0;

fn section_title(text: &str, count: Option<String>, theme: &Theme) -> gpui::Div {
    div()
        .pt(px(18.0))
        .pb(px(6.0))
        .flex()
        .items_baseline()
        .gap(px(8.0))
        .child(
            div()
                .text_size(ui_rems(11.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(SharedString::from(text.to_uppercase())),
        )
        .when_some(count, |el, count| {
            el.child(
                div()
                    .text_size(ui_rems(11.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(count)),
            )
        })
}

fn link_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let accent = theme.accent;
    div()
        .id(id)
        .role(gpui::Role::Button)
        .h(px(24.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(ui_rems(11.5))
        .text_color(theme.text_muted)
        .cursor_pointer()
        .tab_index(0)
        .hover(|s| s.bg(crate::theme::ink(0.06)))
        .focus_visible(move |s| s.bg(accent.opacity(0.18)))
        .on_click(on_click)
        .child(label.into())
        .into_any_element()
}

impl Render for WorkflowRunPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let Some(run) = self.run(cx) else {
            return self.render_missing(&theme).into_any_element();
        };
        if let View::Artifact(view) = &self.view {
            let view = view.clone();
            return self.render_artifact_view(view, &theme, cx);
        }
        let node_pages = self.node_pages.clone();
        let model = PaneModel::build(
            &run,
            &|phase| node_pages.get(phase).copied().unwrap_or(1),
            self.actor_pages,
        );
        let _ = window;

        let mut body = div()
            .w_full()
            .flex()
            .flex_col()
            .px(px(PAD))
            .pt(px(PAD))
            .pb(px(PAD * 2.0))
            .child(self.render_header(&run, &model, &theme, cx));
        for notice in &model.card.notices {
            body = body.child(
                div()
                    .pt(px(8.0))
                    .text_size(ui_rems(12.0))
                    .line_height(px(17.0))
                    .text_color(match notice.tone {
                        super::model::Tone::Muted => theme.text_muted,
                        tone => tone_color(tone, &theme),
                    })
                    .child(SharedString::from(notice.text.clone())),
            );
        }
        if let Some((id, text)) = self.state.read(cx).workflow_failure.clone()
            && id == self.run_id
        {
            body = body.child(
                div()
                    .pt(px(8.0))
                    .text_size(ui_rems(12.0))
                    .text_color(theme.danger)
                    .child(SharedString::from(text)),
            );
        }
        if !model.questions.is_empty() {
            body = body.child(self.render_questions(&model, &theme, cx));
        }
        // What the run produced comes before how it ran.
        if !model.artifacts.is_empty() {
            body = body.child(self.render_artifacts(&model, &theme, cx));
        }
        if model.result_preview.is_some() {
            body = body.child(self.render_result(&model, &theme, cx));
        }
        if !model.rail.is_empty() {
            body = body.child(self.render_rail(&model, &theme, cx));
        }
        if !model.actors.is_empty() {
            body = body.child(self.render_actors(&model, &theme, cx));
        }
        body = body.child(self.render_steps(&model, &theme, cx));
        if !model.reports.is_empty() {
            body = body.child(self.render_reports(&model, &theme, cx));
        }
        body = body.child(self.render_details(&run, &model, &theme));

        div()
            .id("workflow-run-pane")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(body)
            .into_any_element()
    }
}

impl WorkflowRunPane {
    fn render_missing(&self, theme: &Theme) -> gpui::Div {
        let text = if let Some(err) = &self.fetch_failed {
            format!("This run is no longer tracked, and its record could not be read: {err}")
        } else if self.fetching.is_some() {
            "Loading the run…".to_owned()
        } else {
            "This run is not available in this chat.".to_owned()
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(PAD))
            .text_size(ui_rems(12.5))
            .text_color(theme.text_faint)
            .child(SharedString::from(text))
    }

    fn render_artifact_view(
        &mut self,
        view: Entity<ArtifactView>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let run_name = self.title(cx);
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .px(px(PAD))
            .pt(px(PAD))
            .child(
                div()
                    .pb(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(icon_button(
                        "workflow-artifact-back",
                        icons::ARROW_LEFT,
                        "Back to the run",
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.view = View::Overview;
                            cx.notify();
                        }),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(12.0))
                            .text_color(theme.text_faint)
                            .child(run_name),
                    ),
            )
            .child(div().flex_1().min_h_0().pb(px(PAD)).child(view))
            .into_any_element()
    }

    fn render_header(
        &mut self,
        run: &WorkflowRun,
        model: &PaneModel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = &model.card;
        let tone = tone_color(card.tone, theme);
        let view = cx.entity_id();
        let run_id = run.header.run_id.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .w(px(14.0))
                            .flex()
                            .justify_center()
                            .child(status_glyph(
                                card.status,
                                card.tone,
                                "pane-status".into(),
                                theme,
                                view,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(tone)
                            .child(card.kind_word),
                    )
                    .when(!card.counts.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_rems(12.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(format!("· {}", card.counts))),
                        )
                    })
                    .when(card.counts.is_empty(), |el| el.child(div().flex_1()))
                    .when(card.can_stop, |el| {
                        let id = run_id.clone();
                        el.child(icon_button(
                            "pane-stop",
                            icons::STOP,
                            "Stop workflow (it can be resumed)",
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                this.command(
                                    WorkflowCommand::Stop {
                                        run_id: id.clone(),
                                        reason: None,
                                    },
                                    cx,
                                )
                            }),
                        ))
                    })
                    .when(card.can_resume, |el| {
                        let id = run_id.clone();
                        let accent = theme.accent;
                        el.child(
                            div()
                                .id("pane-resume")
                                .role(gpui::Role::Button)
                                .aria_label("Resume workflow")
                                .flex_none()
                                .h(px(26.0))
                                .px(px(10.0))
                                .rounded(px(6.0))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .border_1()
                                .border_color(crate::theme::hairline(0.14))
                                .text_size(ui_rems(12.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .cursor_pointer()
                                .tab_index(0)
                                .hover(|s| s.bg(crate::theme::ink(0.07)))
                                .focus_visible(move |s| s.border_color(accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.command(WorkflowCommand::Resume { run_id: id.clone() }, cx)
                                }))
                                .child(
                                    icon(icons::RESTART)
                                        .size(px(12.0))
                                        .text_color(theme.text_muted),
                                )
                                .child("Resume"),
                        )
                    }),
            )
            .child(
                div()
                    .text_size(ui_rems(17.0))
                    .line_height(px(22.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(card.name.clone())),
            )
            .when(!card.meta.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(ui_rems(12.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(card.meta.clone())),
                )
            })
            .into_any_element()
    }

    fn render_questions(
        &mut self,
        model: &PaneModel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let warning = theme.warning;
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section_title(
                "Waiting for you",
                Some(model.questions.len().to_string()),
                theme,
            ));
        for q in &model.questions {
            let input = self.answer_input(&q.qid, cx);
            let sent = self.sent.contains(&q.qid);
            let qid = q.qid.clone();
            col = col.child(
                div()
                    .w_full()
                    .p(px(12.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(warning.opacity(0.35))
                    .bg(warning.opacity(0.06))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                icon(icons::CHAT_ROUND_LINE)
                                    .size(px(12.0))
                                    .text_color(warning),
                            )
                            .child(
                                div()
                                    .text_size(ui_rems(11.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(warning)
                                    .child(SharedString::from(format!("{} asks", q.actor))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(13.0))
                            .line_height(px(19.0))
                            .text_color(theme.text)
                            .child(SharedString::from(q.question.clone())),
                    )
                    .when(!q.context.trim().is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(ui_rems(11.5))
                                .line_height(px(16.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(super::one_line(&q.context))),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h(px(32.0))
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(theme.border)
                                    .bg(theme.bg)
                                    .text_size(ui_rems(13.0))
                                    .child(input),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("answer-{qid}")))
                                    .role(gpui::Role::Button)
                                    .aria_label("Send the answer")
                                    .flex_none()
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .flex()
                                    .items_center()
                                    .text_size(ui_rems(12.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .bg(if sent { theme.element_hover } else { theme.solid })
                                    .text_color(if sent { theme.text_muted } else { theme.on_solid })
                                    .when(!sent, |el| {
                                        el.cursor_pointer().tab_index(0).on_click(cx.listener(
                                            move |this, _, _, cx| this.send_answer(&qid, cx),
                                        ))
                                    })
                                    .child(if sent { "Sent" } else { "Answer" }),
                            ),
                    ),
            );
        }
        col.into_any_element()
    }

    fn render_rail(&mut self, model: &PaneModel, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.phase_filter.clone();
        let all_selected = selected.is_none();
        let chip = |id: SharedString,
                    label: SharedString,
                    on: bool,
                    base: Option<&StationBase>,
                    click: Box<dyn Fn(&mut Self, &mut Context<Self>)>| {
            let accent = theme.accent;
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(ui_rems(12.0))
                .cursor_pointer()
                .tab_index(0)
                .border_1()
                .border_color(if on {
                    crate::theme::hairline(0.2)
                } else {
                    gpui::transparent_black()
                })
                .when(on, |el| el.bg(crate::theme::ink(0.07)))
                .hover(|s| s.bg(crate::theme::ink(0.05)))
                .focus_visible(move |s| s.border_color(accent))
                .on_click(cx.listener(move |this, _, _, cx| click(this, cx)))
                .when_some(base, |el, base| {
                    el.child(lamp(base.light, 7.0, theme))
                })
                .child(
                    div()
                        .max_w(px(160.0))
                        .truncate()
                        .text_color(if on { theme.text } else { theme.text_muted })
                        .child(label),
                )
                .when_some(base.and_then(StationBase::fraction), |el, f| {
                    el.child(
                        div()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(f)),
                    )
                })
        };
        let mut row = div().flex().flex_row().flex_wrap().gap(px(4.0)).child(chip(
            "pane-phase-all".into(),
            "All phases".into(),
            all_selected,
            None,
            Box::new(|this, cx| {
                this.phase_filter = None;
                cx.notify();
            }),
        ));
        for (ix, base) in model.rail.iter().enumerate() {
            let name = base.name.clone();
            let on = selected.as_deref() == Some(base.name.as_str());
            row = row.child(chip(
                SharedString::from(format!("pane-phase-{ix}")),
                SharedString::from(base.name.clone()),
                on,
                Some(base),
                Box::new(move |this, cx| {
                    this.phase_filter = if this.phase_filter.as_deref() == Some(name.as_str()) {
                        None
                    } else {
                        Some(name.clone())
                    };
                    cx.notify();
                }),
            ));
        }
        div()
            .flex()
            .flex_col()
            .child(section_title("Phases", None, theme))
            .child(row)
            .into_any_element()
    }

    fn render_actors(&mut self, model: &PaneModel, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity_id();
        let mut col = div().flex().flex_col().child(section_title(
            "Agents",
            Some(model.actors_total.to_string()),
            theme,
        ));
        for (ix, actor) in model.actors.iter().enumerate() {
            col = col.child(self.actor_row(ix, actor, theme, view, cx));
        }
        if (model.actors.len() as u32) < model.actors_total {
            let more = (model.actors_total as usize - model.actors.len()).min(PANE_ACTORS_PER_PAGE);
            col = col.child(link_button(
                "pane-actors-more",
                format!("Show {more} more"),
                theme,
                cx.listener(|this, _, _, cx| {
                    this.actor_pages += 1;
                    cx.notify();
                }),
            ));
        }
        col.into_any_element()
    }

    fn actor_row(
        &mut self,
        ix: usize,
        actor: &ActorRow,
        theme: &Theme,
        view: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut sub: Vec<String> = Vec::new();
        if let Some(label) = &actor.label {
            sub.push(label.clone());
        }
        if !actor.phases.is_empty() {
            sub.push(actor.phases.join(", "));
        }
        if let Some(a) = &actor.activity {
            sub.push(a.clone());
        }
        let asks = match (actor.asks, actor.failed_asks) {
            (0, _) => String::new(),
            (n, 0) => format!("{n} {}", if n == 1 { "ask" } else { "asks" }),
            (n, f) => format!("{n} asks · {f} failed"),
        };
        let name_color = match actor.state {
            PillState::Pending => theme.text_faint,
            PillState::Done | PillState::Cancelled => theme.text_muted,
            _ => theme.text,
        };
        let open = actor.child_chat_id.clone().map(|child| (child, actor.name.clone()));
        let accent = theme.accent;
        let row = div()
            .id(SharedString::from(format!("pane-actor-{ix}")))
            .w_full()
            .min_w_0()
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(7.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(pill_mark(
                        actor.state,
                        SharedString::from(format!("pane-actor-mark-{ix}")),
                        theme,
                        view,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(name_color)
                            .child(SharedString::from(actor.name.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(asks)),
                    ),
            )
            .when(!sub.is_empty(), |el| {
                el.child(
                    div()
                        .pl(px(20.0))
                        .truncate()
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(sub.join(" · "))),
                )
            });
        match open {
            Some((child, title)) => row
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Open {title}'s chat")))
                .cursor_pointer()
                .tab_index(0)
                .hover(|s| s.bg(crate::theme::ink(0.05)))
                .focus_visible(move |s| s.bg(accent.opacity(0.16)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(PaneEvent::OpenActor {
                        child_chat_id: child.clone(),
                        title: title.clone(),
                    })
                }))
                .into_any_element(),
            None => row.into_any_element(),
        }
    }

    fn render_steps(&mut self, model: &PaneModel, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity_id();
        let filter = self.phase_filter.clone();
        let mut col = div().flex().flex_col().child(section_title("Steps", None, theme));
        let mut any = false;
        for (gix, group) in model.groups.iter().enumerate() {
            if filter.as_deref().is_some_and(|f| f != group.base.name) {
                continue;
            }
            if group.total == 0 && filter.is_none() && model.groups.len() > 1 {
                continue;
            }
            any = true;
            col = col.child(self.group_block(gix, group, theme, view, cx));
        }
        if !any {
            col = col.child(
                div()
                    .text_size(ui_rems(12.0))
                    .text_color(theme.text_faint)
                    .child("No steps yet."),
            );
        }
        col.into_any_element()
    }

    fn group_block(
        &mut self,
        gix: usize,
        group: &PhaseGroup,
        theme: &Theme,
        view: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = group.base.name.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .pb(px(10.0))
            .child(
                div()
                    .h(px(24.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(lamp(group.base.light, 7.0, theme))
                    .child(
                        div()
                            .text_size(ui_rems(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(match group.base.light {
                                Light::Pending => theme.text_faint,
                                _ => theme.text,
                            })
                            .child(SharedString::from(group.base.name.clone())),
                    )
                    .when_some(group.base.fraction(), |el, f| {
                        el.child(
                            div()
                                .text_size(ui_rems(11.5))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(f)),
                        )
                    })
                    .child(div().flex_1()),
            );
        for (rix, row) in group.rows.iter().enumerate() {
            col = col.child(self.node_row(gix, rix, row, theme, view, cx));
        }
        if (group.rows.len() as u32) < group.total {
            let more = (group.total as usize - group.rows.len()).min(PANE_NODES_PER_PAGE);
            col = col.child(link_button(
                SharedString::from(format!("pane-nodes-more-{gix}")),
                format!("Show {more} more of {}", group.total - group.rows.len() as u32),
                theme,
                cx.listener(move |this, _, _, cx| {
                    *this.node_pages.entry(name.clone()).or_insert(1) += 1;
                    cx.notify();
                }),
            ));
        }
        col.into_any_element()
    }

    fn node_row(
        &mut self,
        gix: usize,
        rix: usize,
        row: &NodeRow,
        theme: &Theme,
        view: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dim = matches!(row.state, NodeState::Queued);
        let subline = row
            .activity
            .clone()
            .map(|a| (a, theme.text_faint))
            .or_else(|| {
                row.detail.clone().map(|d| match row.state {
                    NodeState::Failed => (d, theme.danger_muted),
                    _ => (d, theme.text_faint),
                })
            });
        let open = row
            .child_chat_id
            .clone()
            .map(|child| (child, row.who.clone()));
        let accent = theme.accent;
        let el = div()
            .id(SharedString::from(format!("pane-node-{gix}-{rix}")))
            .w_full()
            .min_w_0()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(7.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(pill_mark(
                        row.state.pill(),
                        SharedString::from(format!("pane-node-mark-{gix}-{rix}")),
                        theme,
                        view,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(140.0))
                            .truncate()
                            .text_size(ui_rems(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if dim { theme.text_faint } else { theme.text })
                            .child(SharedString::from(row.who.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(row.head.clone())),
                    )
                    .when(row.cached, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .px(px(5.0))
                                .rounded(px(4.0))
                                .text_size(ui_rems(10.5))
                                .text_color(theme.text_faint)
                                .bg(crate::theme::ink(0.06))
                                .child("cached"),
                        )
                    })
                    .when(row.tokens > 0, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(ui_rems(11.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(format!(
                                    "{} tok",
                                    format_tokens(row.tokens)
                                ))),
                        )
                    }),
            )
            .when_some(subline, |el, (text, color)| {
                el.child(
                    div()
                        .pl(px(20.0))
                        .truncate()
                        .text_size(ui_rems(11.5))
                        .text_color(color)
                        .child(SharedString::from(text)),
                )
            });
        match open {
            Some((child, title)) => el
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Open {title}'s chat")))
                .cursor_pointer()
                .tab_index(0)
                .hover(|s| s.bg(crate::theme::ink(0.05)))
                .focus_visible(move |s| s.bg(accent.opacity(0.16)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(PaneEvent::OpenActor {
                        child_chat_id: child.clone(),
                        title: title.clone(),
                    })
                }))
                .into_any_element(),
            None => el.into_any_element(),
        }
    }

    fn render_reports(&mut self, model: &PaneModel, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().flex().flex_col().gap(px(6.0)).child(section_title(
            "Reports",
            Some(model.reports.len().to_string()),
            theme,
        ));
        for report in model.reports.iter().rev().take(20) {
            let artifact = report.artifact_id.clone();
            col = col.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(ui_rems(12.0))
                            .line_height(px(17.0))
                            .text_color(theme.text)
                            .child(SharedString::from(super::one_line(&report.text))),
                    )
                    .when_some(artifact, |el, id| {
                        el.child(
                            div().child(link_button(
                                SharedString::from(format!("pane-report-{}", report.index)),
                                format!("Open {id}"),
                                theme,
                                cx.listener(move |this, _, _, cx| this.open_artifact(&id, cx)),
                            )),
                        )
                    }),
            );
        }
        col.into_any_element()
    }

    fn render_artifacts(&mut self, model: &PaneModel, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().flex().flex_col().child(section_title(
            "Artifacts",
            Some(model.artifacts.len().to_string()),
            theme,
        ));
        for (ix, art) in model.artifacts.iter().enumerate() {
            let id = art.chip.id.clone();
            let accent = theme.accent;
            let mut meta = vec![kind_word(art.chip.kind).to_owned()];
            if art.items > 0 {
                meta.push(format!("{} {}", art.items, match art.chip.kind {
                    zeron_proto::ArtifactKind::Table => "rows",
                    _ => "items",
                }));
            }
            if art.bytes > 0 {
                meta.push(format_bytes(art.bytes));
            }
            if art.chip.version > 1 {
                meta.push(format!("v{}", art.chip.version));
            }
            col = col.child(
                div()
                    .id(SharedString::from(format!("pane-artifact-{ix}")))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(format!("Open artifact {}", art.chip.title)))
                    .w_full()
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .cursor_pointer()
                    .tab_index(0)
                    .hover(|s| s.bg(crate::theme::ink(0.05)))
                    .focus_visible(move |s| s.bg(accent.opacity(0.16)))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_artifact(&id, cx)))
                    .child(
                        icon(kind_icon(art.chip.kind))
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(12.5))
                            .text_color(theme.text)
                            .child(SharedString::from(art.chip.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(meta.join(" · "))),
                    ),
            );
        }
        col.into_any_element()
    }

    fn render_result(&mut self, model: &PaneModel, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let text = model.result_preview.clone().unwrap_or_default();
        let open = self.result_open;
        let long = text.chars().count() > 280 || text.lines().count() > 4;
        div()
            .flex()
            .flex_col()
            .child(section_title("Result", None, theme))
            .child(
                div()
                    .px(px(8.0))
                    .text_size(ui_rems(12.0))
                    .line_height(px(18.0))
                    .text_color(theme.text)
                    .when(!open, |el| el.max_h(px(18.0 * 4.0)).overflow_hidden())
                    .child(SharedString::from(text)),
            )
            .when(model.result_truncated, |el| {
                el.child(
                    div()
                        .px(px(8.0))
                        .pt(px(4.0))
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_faint)
                        .child("The result was longer; the agent received it in full."),
                )
            })
            .when(long, |el| {
                el.child(link_button(
                    "pane-result-toggle",
                    if open { "Show less" } else { "Show more" },
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.result_open = !this.result_open;
                        cx.notify();
                    }),
                ))
            })
            .into_any_element()
    }

    fn render_details(&mut self, run: &WorkflowRun, model: &PaneModel, theme: &Theme) -> AnyElement {
        let h = &run.header;
        let mut rows: Vec<(String, String)> = Vec::new();
        if let Some(c) = &model.concurrency {
            rows.push(("Concurrency".into(), c.clone()));
        }
        let u = &h.usage;
        if u.total_tokens() > 0 {
            rows.push((
                "Tokens".into(),
                format!(
                    "{} in · {} out",
                    format_tokens(u.input_tokens),
                    format_tokens(u.output_tokens)
                ),
            ));
        }
        if h.status == WorkflowStatus::Pending {
            rows.push(("Status".into(), "Waiting for approval".into()));
        }
        rows.extend(model.ids.iter().cloned());
        div()
            .flex()
            .flex_col()
            .child(section_title("Details", None, theme))
            .children(rows.into_iter().map(|(k, v)| {
                div()
                    .px(px(8.0))
                    .py(px(2.0))
                    .flex()
                    .gap(px(10.0))
                    .text_size(ui_rems(11.5))
                    .child(
                        div()
                            .flex_none()
                            .w(px(96.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(k)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(v)),
                    )
            }))
            .into_any_element()
    }
}
