//! Moving a chat to another device on the desktop (docs/plans/2026-09-30-
//! agent-mobility-and-policy.md, Part 1): the "Move to…" device picker
//! (chat header, chat context menu, command palette) and the move banner
//! above the composer. Presentation strings and state mapping live in
//! [`crate::session_move`]; this file owns rendering and the RPCs.
//!
//! Every move RPC goes through the local engine with `targetDeviceId` = the
//! chat's host, which runs the move and publishes its progress on the chat
//! row the banner renders from.

use super::file_transfers::{icon_button, pill, progress_bar};
use super::*;
use crate::session_move::{self as sm, Banner, BannerAction, BannerTone, PickerRow};
use zeron_proto::{MoveCandidate, MoveWhen};

const PICKER_WIDTH: f32 = 300.0;
const BANNER_RADIUS: f32 = 12.0;
const MOVE_NOW_HINT: &str = "Interrupts the current step and moves right away";

#[derive(Default)]
pub(super) struct SessionMoveUi {
    picker: popover::Popup<MovePicker>,
    /// Finished moves whose banner the user dismissed on this device. Never
    /// written to the registry: other devices keep their own banners.
    dismissed: std::collections::HashSet<String>,
    /// Move ids with a Move now / Cancel in flight (buttons go inert).
    busy: std::collections::HashSet<String>,
    /// The last banner action that failed: `(move id, message)`.
    banner_error: Option<(String, String)>,
    /// Repaint when a lingering line expires: `(expires_at, timer)`.
    expiry: Option<(i64, Task<()>)>,
    /// Bumped per candidates request so a stale reply can't land in a
    /// picker reopened for another chat.
    request: u64,
}

#[derive(Clone)]
pub(super) struct MovePicker {
    chat_id: String,
    /// The engine running the move: the chat's host.
    host: String,
    label: String,
    /// The chat's agent, for the login line ("Uses its own Codex login").
    harness: &'static str,
    position: Point<Pixels>,
    candidates: Loadable<Vec<MoveCandidate>>,
    /// The device a `StartMove` is in flight for.
    starting: Option<String>,
    error: Option<String>,
    request: u64,
}

/// An RPC error in words. Old hosts don't know the methods at all.
fn rpc_error(error: zeron_rpc::RpcError, host: &str) -> String {
    match error {
        zeron_rpc::RpcError::UnknownMethod(_) => {
            format!("{host} can't move chats yet. Update Zeron on it first.")
        }
        other => other.to_string(),
    }
}

impl Shell {
    /// Whether "Move to…" is offered for `chat_id` (see
    /// [`sm::can_offer_move`]). A local-only profile has no other devices.
    pub(super) fn chat_movable(&self, chat_id: &str, cx: &App) -> bool {
        let state = self.state.read(cx);
        if state.workspace_scope == Some(WorkspaceScope::Local) {
            return false;
        }
        state
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_some_and(|chat| {
                sm::can_offer_move(
                    chat,
                    state.device_supports(
                        &chat.device_id,
                        zeron_proto::capabilities::SESSION_MOVE_V1,
                    ),
                )
            })
    }

    /// Open the device picker for `chat_id` at `position` and ask its host
    /// which devices could take it.
    pub(super) fn open_move_picker(
        &mut self,
        chat_id: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.chat_movable(&chat_id, cx) {
            return;
        }
        let resolved = {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .map(|chat| {
                    (
                        chat.device_id.clone(),
                        transcript::single_line(chat.title.as_deref().unwrap_or("New session")),
                        chat.config.as_ref().map_or("the agent", |config| {
                            harness_updates::agent_name(config.harness)
                        }),
                    )
                })
        };
        let Some((host, label, harness)) = resolved else {
            return;
        };
        self.session_move.picker.open(MovePicker {
            chat_id,
            host,
            label,
            harness,
            position,
            candidates: Loadable::Idle,
            starting: None,
            error: None,
            request: 0,
        });
        self.load_move_candidates(cx);
    }

    /// The header button and the command palette: the selected chat, picker
    /// hanging under the titlebar at `x`.
    pub(super) fn open_move_picker_for_selected_chat(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        let x = x.clamp(8.0, (self.viewport_width - PICKER_WIDTH - 8.0).max(8.0));
        let position = gpui::point(px(x), px(Theme::TITLEBAR_HEIGHT + 2.0));
        self.open_move_picker(chat_id, position, cx);
    }

    /// The command palette's entry: centered under the titlebar.
    pub(super) fn open_move_picker_centered(&mut self, cx: &mut Context<Self>) {
        let x = (self.viewport_width - PICKER_WIDTH) * 0.5;
        self.open_move_picker_for_selected_chat(x, cx);
    }

    fn load_move_candidates(&mut self, cx: &mut Context<Self>) {
        self.session_move.request += 1;
        let request = self.session_move.request;
        let engine = self.state.read(cx).engine().cloned();
        let Some(picker) = self.session_move.picker.open_mut() else {
            return;
        };
        let Some(engine) = engine else {
            picker.candidates = Loadable::Error("Zeron isn't connected.".into());
            cx.notify();
            return;
        };
        picker.candidates = Loadable::Loading;
        picker.request = request;
        let host = picker.host.clone();
        let host_name = self
            .state
            .read(cx)
            .device_name(&host)
            .unwrap_or("This device")
            .to_string();
        let params = serde_json::json!({
            "chatId": picker.chat_id,
            "targetDeviceId": host,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::MOVE_CANDIDATES, params)
                .await
                .map_err(|error| rpc_error(error, &host_name))
                .and_then(sm::parse_candidates);
            this.update(cx, |shell, cx| {
                if let Some(picker) = shell.session_move.picker.open_mut()
                    && picker.request == request
                {
                    picker.candidates = match result {
                        Ok(candidates) => Loadable::Ready(candidates),
                        Err(error) => Loadable::Error(error),
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn close_move_picker(&mut self, cx: &mut Context<Self>) {
        if self.session_move.picker.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.session_move.picker);
        }
        cx.notify();
    }

    /// Escape closes the picker first. Returns whether it did.
    pub(super) fn dismiss_move_picker(&mut self, cx: &mut Context<Self>) -> bool {
        if self.session_move.picker.is_open() {
            self.close_move_picker(cx);
            return true;
        }
        false
    }

    /// `StartMove` on the chat's host. The picker stays up (inert) until the
    /// host answers, so a refusal can be read where it was asked.
    fn start_move(&mut self, to: PickerRow, when: MoveWhen, cx: &mut Context<Self>) {
        let engine = self.state.read(cx).engine().cloned();
        let Some(picker) = self.session_move.picker.open_mut() else {
            return;
        };
        if picker.starting.is_some() {
            return;
        }
        let Some(engine) = engine else {
            picker.error = Some("Zeron isn't connected.".into());
            cx.notify();
            return;
        };
        picker.starting = Some(to.device_id.clone());
        picker.error = None;
        let chat_id = picker.chat_id.clone();
        let host = picker.host.clone();
        let host_name = self
            .state
            .read(cx)
            .device_name(&host)
            .unwrap_or("This device")
            .to_string();
        let mut params = serde_json::to_value(zeron_proto::StartMoveParams {
            chat_id: chat_id.clone(),
            to_device_id: to.device_id.clone(),
            when,
        })
        .unwrap_or_default();
        params["targetDeviceId"] = serde_json::json!(host);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::START_MOVE, params).await;
            this.update(cx, |shell, cx| {
                let Some(picker) = shell
                    .session_move
                    .picker
                    .open_mut()
                    .filter(|picker| picker.chat_id == chat_id)
                else {
                    return;
                };
                picker.starting = None;
                match result {
                    // The banner takes over from the row's `move` field.
                    Ok(_) => shell.close_move_picker(cx),
                    Err(error) => {
                        picker.error = Some(format!(
                            "Couldn't move to {}: {}",
                            to.name,
                            rpc_error(error, &host_name)
                        ));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// The banner's controls. Dismissal is device-local.
    fn run_move_action(
        &mut self,
        chat_id: String,
        host: String,
        banner: &Banner,
        action: BannerAction,
        cx: &mut Context<Self>,
    ) {
        let move_id = banner.move_id.clone();
        let method = match action {
            BannerAction::Dismiss => {
                self.session_move.dismissed.insert(move_id.clone());
                if self
                    .session_move
                    .banner_error
                    .as_ref()
                    .is_some_and(|(id, _)| *id == move_id)
                {
                    self.session_move.banner_error = None;
                }
                cx.notify();
                return;
            }
            BannerAction::MoveNow => methods::MOVE_NOW,
            BannerAction::Cancel => methods::CANCEL_MOVE,
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !self.session_move.busy.insert(move_id.clone()) {
            return;
        }
        self.session_move.banner_error = None;
        let host_name = self
            .state
            .read(cx)
            .device_name(&host)
            .unwrap_or("This device")
            .to_string();
        let params = serde_json::json!({ "chatId": chat_id, "targetDeviceId": host });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |shell, cx| {
                shell.session_move.busy.remove(&move_id);
                if let Err(error) = result {
                    let what = match action {
                        BannerAction::MoveNow => "Couldn't move now",
                        _ => "Couldn't cancel the move",
                    };
                    shell.session_move.banner_error =
                        Some((move_id, format!("{what}: {}", rpc_error(error, &host_name))));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Repaint once a lingering line's time is up (the row itself won't
    /// change again to trigger one).
    fn schedule_move_banner_expiry(&mut self, expires_at: Option<i64>, cx: &mut Context<Self>) {
        let Some(expires_at) = expires_at else {
            return;
        };
        if self
            .session_move
            .expiry
            .as_ref()
            .is_some_and(|(at, _)| *at == expires_at)
        {
            return;
        }
        let wait = (expires_at - Utc::now().timestamp_millis()).max(0) as u64 + 50;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            this.update(cx, |shell, cx| {
                shell.session_move.expiry = None;
                cx.notify();
            })
            .ok();
        });
        self.session_move.expiry = Some((expires_at, task));
    }

    /// The move banner for the selected chat, shown above the composer while
    /// a move runs and briefly after it ends.
    pub(super) fn render_move_banner(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (chat_id, host, chat_move) = {
            let state = self.state.read(cx);
            let chat = state.selected_chat_row()?;
            (
                chat.id.clone(),
                chat.device_id.clone(),
                chat.move_state.clone()?,
            )
        };
        let now = Utc::now().timestamp_millis();
        let dismissed = self.session_move.dismissed.contains(&chat_move.id);
        let banner = sm::banner(&chat_move, now, dismissed)?;
        self.schedule_move_banner_expiry(banner.expires_at, cx);
        let theme = Theme::of(cx).clone();
        let busy = self.session_move.busy.contains(&banner.move_id);
        let action_error = self
            .session_move
            .banner_error
            .as_ref()
            .filter(|(id, _)| *id == banner.move_id)
            .map(|(_, message)| message.clone());

        let (glyph, tint) = match banner.tone {
            BannerTone::Progress => (icons::ARROW_RIGHT, theme.accent),
            BannerTone::Success => (icons::CHECK, theme.success),
            BannerTone::Failure => (icons::DANGER_TRIANGLE, theme.danger),
            BannerTone::Quiet => (icons::CLOSE_CIRCLE, theme.text_faint),
        };
        let tile = div()
            .size(px(24.0))
            .flex_none()
            .rounded(px(7.0))
            .bg(tint.opacity(0.12))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(glyph).size(px(13.0)).text_color(tint));

        let mut actions = div().flex_none().flex().items_center().gap(px(4.0));
        for action in banner.actions.iter().copied() {
            let label = sm::action_label(action);
            let control = match action {
                BannerAction::Dismiss => icon_button(&theme, icons::CLOSE),
                BannerAction::MoveNow => pill(&theme, true).child(label),
                BannerAction::Cancel => pill(&theme, false).child(label),
            };
            let hint = match action {
                BannerAction::MoveNow => Some(MOVE_NOW_HINT),
                BannerAction::Dismiss => Some("Dismiss"),
                BannerAction::Cancel => None,
            };
            let (chat_id, host, banner_for_click) = (chat_id.clone(), host.clone(), banner.clone());
            let inert = busy && action != BannerAction::Dismiss;
            actions = actions.child(
                control
                    .id(SharedString::from(format!("move-banner-{action:?}")))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .when_some(hint, |el, hint| {
                        el.tooltip(settings::widgets::text_tooltip(hint))
                    })
                    .when(inert, |el| el.opacity(0.45).cursor_default())
                    .when(!inert, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.run_move_action(
                                chat_id.clone(),
                                host.clone(),
                                &banner_for_click,
                                action,
                                cx,
                            );
                        }))
                    }),
            );
        }

        let failed = banner.tone == BannerTone::Failure;
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.5))
                    .line_height(crate::typography::ui_rems(16.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .truncate()
                    .child(banner.title.clone()),
            )
            .when_some(banner.subtitle.clone(), |el, subtitle| {
                let line = div()
                    .text_size(crate::typography::ui_rems(11.0))
                    .line_height(crate::typography::ui_rems(14.0))
                    .text_color(if failed {
                        theme.danger
                    } else {
                        theme.text_muted
                    });
                // A failure's reason is the one line worth wrapping.
                el.child(if failed {
                    line.child(subtitle)
                } else {
                    line.truncate().child(subtitle)
                })
            });

        let surface =
            if theme.is_frost() && matches!(theme.appearance, crate::theme::Appearance::Dark) {
                theme.composer_sidebar_tint()
            } else {
                theme.input_glass_bg()
            };
        let small = |color: gpui::Hsla| {
            div()
                .text_size(crate::typography::ui_rems(11.0))
                .line_height(crate::typography::ui_rems(14.0))
                .text_color(color)
        };
        let card = div()
            .id("move-banner")
            .occlude()
            .rounded(px(BANNER_RADIUS))
            .bg(surface)
            .border_1()
            .border_color(theme.border)
            .when(!theme.is_frost(), |el| el.shadow_md())
            .px(px(10.0))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .role(gpui::Role::Status)
            .aria_label(banner.title.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(tile)
                    .child(text)
                    .child(actions),
            )
            .when_some(banner.progress.clone(), |el, progress| {
                el.child(
                    div()
                        .pl(px(34.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .child(progress_bar(progress.fraction, theme.accent)),
                        )
                        .child(small(theme.text_faint).flex_none().child(progress.label)),
                )
            })
            .when(!banner.notes.is_empty(), |el| {
                el.child(
                    div().pl(px(34.0)).flex().flex_col().gap(px(2.0)).children(
                        banner
                            .notes
                            .iter()
                            .map(|note| small(theme.text_muted).child(format!("\u{2022} {note}"))),
                    ),
                )
            })
            .when_some(action_error, |el, message| {
                el.child(small(theme.danger).pl(px(34.0)).child(message))
            });
        Some(
            motion::fade_in(
                SharedString::from(format!("move-banner-{}-{:?}", banner.move_id, banner.tone)),
                div()
                    .mx(px(Theme::SPACE_LG))
                    .mb(px(6.0))
                    .child(crate::frost::frosted(
                        BANNER_RADIUS,
                        crate::frost::MENU_BLUR,
                        card,
                    )),
            )
            .into_any_element(),
        )
    }

    /// The "Move to…" device picker (context-menu overlay).
    pub(super) fn render_move_picker(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.session_move.picker.get()?.clone();
        let closing = self.session_move.picker.closing_since();
        let theme = Theme::of(cx).for_popup();
        let small = |color: gpui::Hsla| {
            div()
                .text_size(crate::typography::ui_rems(11.0))
                .line_height(crate::typography::ui_rems(14.0))
                .text_color(color)
        };
        let note = |text: String, color: gpui::Hsla| {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let heading = div()
            .px(px(8.0))
            .pt(px(6.0))
            .pb(px(4.0))
            .text_size(crate::typography::ui_rems(11.5))
            .text_color(theme.text_muted)
            .truncate()
            .child(format!("Move \u{201c}{}\u{201d} to", picker.label));
        let body: Vec<AnyElement> = match &picker.candidates {
            Loadable::Idle | Loadable::Loading => vec![
                div()
                    .px(px(8.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(loaders::gradient_spinner(
                        "move-candidates-loading",
                        &theme,
                        2.5,
                        cx.entity_id(),
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("Checking your devices\u{2026}"),
                    )
                    .into_any_element(),
            ],
            Loadable::Error(error) => vec![
                note(error.clone(), theme.danger),
                settings::widgets::text_action(
                    &theme,
                    settings::widgets::ActionTone::Quiet,
                    "Try again",
                )
                .id("move-candidates-retry")
                .mx(px(4.0))
                .mb(px(4.0))
                .min_h(px(24.0))
                .py(px(2.0))
                .text_size(crate::typography::ui_rems(11.5))
                .role(gpui::Role::Button)
                .on_click(cx.listener(|this, _, _, cx| this.load_move_candidates(cx)))
                .into_any_element(),
            ],
            Loadable::Ready(candidates) => {
                let rows =
                    sm::picker_rows(candidates, &self.state.read(cx).devices, picker.harness);
                if rows.is_empty() {
                    vec![note(
                        "No other devices yet. Sign in to Zeron on another device to move chats to it."
                            .into(),
                        theme.text_faint,
                    )]
                } else {
                    let starting = picker.starting.clone();
                    rows.into_iter()
                        .enumerate()
                        .map(|(ix, row)| {
                            let this_starting = starting.as_deref() == Some(row.device_id.as_str());
                            let inert = !row.selectable || starting.is_some();
                            let group =
                                SharedString::from(format!("move-target-{}", row.device_id));
                            let move_now = row.selectable.then(|| {
                                let pick = row.clone();
                                pill(&theme, false)
                                    .id(("move-target-now", ix))
                                    .role(gpui::Role::Button)
                                    .aria_label(format!("Move to {} now", row.name))
                                    .tooltip(settings::widgets::text_tooltip(MOVE_NOW_HINT))
                                    .opacity(0.0)
                                    .group_hover(group.clone(), |s| s.opacity(1.0))
                                    .when(!inert, |el| {
                                        el.on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.start_move(pick.clone(), MoveWhen::Now, cx)
                                        }))
                                    })
                                    .child("Move now")
                            });
                            let trailing: Option<AnyElement> = if this_starting {
                                Some(
                                    loaders::gradient_spinner(
                                        "move-target-starting",
                                        &theme,
                                        2.0,
                                        cx.entity_id(),
                                        cx,
                                    )
                                    .into_any_element(),
                                )
                            } else if !row.online {
                                Some(
                                    icon(icons::WIFI_OFF)
                                        .size(px(12.0))
                                        .flex_none()
                                        .text_color(theme.warning.opacity(0.8))
                                        .into_any_element(),
                                )
                            } else {
                                move_now.map(IntoElement::into_any_element)
                            };
                            let pick = row.clone();
                            popover::menu_row(
                                &theme,
                                false,
                                format!("move-target-{}", row.device_id),
                            )
                            .id(("move-target", ix))
                            .group(group)
                            .role(gpui::Role::MenuItem)
                            .aria_label(format!("Move to {}", row.name))
                            .when(!row.selectable, |el| el.opacity(0.45).cursor_default())
                            .when(
                                row.selectable && starting.is_some() && !this_starting,
                                |el| el.opacity(0.6).cursor_default(),
                            )
                            .when(!inert, |el| {
                                el.on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_move(pick.clone(), MoveWhen::SafePoint, cx)
                                }))
                            })
                            .child(
                                icon(crate::file_transfers::platform_icon(&row.platform))
                                    .size(px(16.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().truncate().child(row.name.clone()))
                                    .when_some(row.secondary.clone(), |el, secondary| {
                                        el.child(
                                            small(theme.text_faint).truncate().child(secondary),
                                        )
                                    }),
                            )
                            .children(trailing)
                            .into_any_element()
                        })
                        .collect()
                }
            }
        };
        let ready = matches!(picker.candidates, Loadable::Ready(ref c) if !c.is_empty());
        let card = popover::popover_card(&theme)
            .id("move-to-device-menu")
            .w(px(PICKER_WIDTH))
            .flex()
            .flex_col()
            .role(gpui::Role::Menu)
            .aria_label("Move to device")
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_move_picker(cx)))
            .child(heading)
            .children(body)
            .when_some(picker.error.clone(), |el, error| {
                el.child(small(theme.danger).px(px(8.0)).py(px(4.0)).child(error))
            })
            .when(ready, |el| {
                el.child(popover::menu_separator()).child(
                    small(theme.text_faint)
                        .px(px(8.0))
                        .pt(px(4.0))
                        .pb(px(6.0))
                        .child(
                            "The agent keeps working while its files copy, then moves \
                             between steps. Move now interrupts the current step.",
                        ),
                )
            })
            .into_any_element();
        Some(popover::menu_at(
            "move-to-device-menu-layer",
            picker.position,
            card,
            closing,
        ))
    }
}

/// Review-fixture hooks (`examples/move-fixture.rs`).
#[doc(hidden)]
impl Shell {
    /// Open the picker over canned candidates (no engine round trip).
    pub fn fixture_move_picker(
        &mut self,
        chat_id: &str,
        candidates: Vec<MoveCandidate>,
        error: Option<String>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_move_picker(chat_id.to_string(), position, cx);
        // Supersede the real request.
        self.session_move.request += 1;
        if let Some(picker) = self.session_move.picker.open_mut() {
            picker.candidates = Loadable::Ready(candidates);
            picker.error = error;
        }
        cx.notify();
    }
}
