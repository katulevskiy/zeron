//! Settings → Devices → Cloud: continue chats in a container in your own
//! Cloudflare account (docs/cloud.md). Connect an API token, create a box,
//! wake, stop or remove it. A box shows up as a device like any other, and
//! a chat is moved to it the usual way.

use gpui::{AnyElement, Context, Entity, Subscription, Task, div, prelude::*, px};
use zeron_proto::{CloudBox, CloudBoxState};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// The box sizes offered (see `zeron_cloud::estimate`), smallest first.
pub const INSTANCE_TYPES: [&str; 5] = ["basic", "standard-1", "standard-2", "standard-3", "standard-4"];
/// How long an idle box stays awake, in minutes.
pub const IDLE_MINUTES: [u32; 4] = [5, 15, 30, 60];
/// The usage the cost hint assumes: a few hours of work a day.
const HINT_ACTIVE_HOURS: f64 = 60.0;

pub fn state_label(state: CloudBoxState) -> &'static str {
    match state {
        CloudBoxState::Asleep => "Asleep",
        CloudBoxState::Waking => "Waking",
        CloudBoxState::Running => "Running",
        CloudBoxState::Checkpointing => "Saving",
        CloudBoxState::Error => "Problem",
    }
}

/// What the box costs a month for about 60 hours of use, in words.
pub fn cost_hint(instance_type: &str) -> Option<String> {
    let estimate = zeron_cloud::estimate::estimate(instance_type, HINT_ACTIVE_HOURS)?;
    let t = estimate.instance_type;
    Some(format!(
        "{} vCPU, {} GiB memory. About ${:.2} a month for {:.0} hours of use, on top of Cloudflare's ${:.0} Workers plan. Idle time is free.",
        trim_number(t.vcpu),
        trim_number(t.memory_gib),
        estimate.usage_usd,
        estimate.active_hours,
        estimate.plan_usd,
    ))
}

fn trim_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else if n < 1.0 {
        format!("{n:.2}")
    } else {
        format!("{n:.1}")
    }
}

/// One line about a box: size, idle time, and what's wrong if something is.
pub fn box_detail(record: &CloudBox) -> String {
    let base = format!(
        "{} · sleeps after {} min idle",
        record.instance_type, record.idle_minutes
    );
    match (&record.state, &record.error) {
        (CloudBoxState::Error, Some(error)) => format!("{base} · {error}"),
        _ => base,
    }
}

pub struct CloudCard {
    state: Entity<AppState>,
    boxes: Loadable<zeron_proto::CloudBoxesReply>,
    token: Entity<ComposerInput>,
    name: Entity<ComposerInput>,
    instance: usize,
    idle: usize,
    instance_select: widgets::SelectState,
    idle_select: widgets::SelectState,
    /// What's in progress ("Connecting…", "Creating the box…").
    busy: Option<&'static str>,
    error: Option<String>,
    task: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl CloudCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let state_sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.boxes, Loadable::Idle) {
                this.refresh(cx);
            }
        });
        let token = cx.new(|cx| ComposerInput::new("Cloudflare API token", cx));
        let name = cx.new(|cx| ComposerInput::new("Name", cx));
        name.update(cx, |input, cx| input.set_text("Cloud", cx));
        let token_sub = cx.subscribe(&token, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.connect(cx);
            }
        });
        let mut card = Self {
            state,
            boxes: Loadable::Idle,
            token,
            name,
            instance: 2,
            idle: 1,
            instance_select: Default::default(),
            idle_select: Default::default(),
            busy: None,
            error: None,
            task: None,
            _subscriptions: [state_sub, token_sub],
        };
        card.refresh(cx);
        card
    }

    fn engine(&self, cx: &Context<Self>) -> Option<crate::state::EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.run("", methods::CLOUD_BOXES, serde_json::json!({}), cx);
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let token = self.token.read(cx).text().trim().to_string();
        if token.is_empty() {
            return;
        }
        self.token.update(cx, |input, cx| input.set_text("", cx));
        self.run(
            "Checking the token…",
            methods::CLOUD_CONNECT,
            serde_json::json!({ "apiToken": token }),
            cx,
        );
    }

    fn provision(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().to_string();
        let params = serde_json::json!({
            "name": if name.is_empty() { "Cloud".to_string() } else { name },
            "instanceType": INSTANCE_TYPES[self.instance],
            "idleMinutes": IDLE_MINUTES[self.idle],
        });
        self.run(
            "Creating the box in your Cloudflare account. This takes a minute or two…",
            methods::CLOUD_PROVISION,
            params,
            cx,
        );
    }

    fn box_call(&mut self, method: &'static str, id: &str, busy: &'static str, cx: &mut Context<Self>) {
        self.run(busy, method, serde_json::json!({ "deviceId": id }), cx);
    }

    fn destroy(&mut self, id: &str, cx: &mut Context<Self>) {
        self.run(
            "Removing the box…",
            methods::CLOUD_DESTROY,
            serde_json::json!({ "deviceId": id, "keepData": false }),
            cx,
        );
    }

    /// One RPC, then a fresh list: the engine's record is the truth.
    fn run(
        &mut self,
        busy: &'static str,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if matches!(self.boxes, Loadable::Idle) {
            self.boxes = Loadable::Loading;
        }
        self.busy = (!busy.is_empty()).then_some(busy);
        self.task = Some(cx.spawn(async move |this, cx| {
            let acted = if method == methods::CLOUD_BOXES {
                Ok(())
            } else {
                engine
                    .client()
                    .call(method, params)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            };
            let listed = engine
                .client()
                .call(methods::CLOUD_BOXES, serde_json::json!({}))
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<zeron_proto::CloudBoxesReply>(v)
                        .map_err(|e| e.to_string())
                });
            this.update(cx, |card, cx| {
                card.busy = None;
                card.error = acted.err();
                card.boxes = match listed {
                    Ok(reply) => Loadable::Ready(reply),
                    // An engine predating cloud boxes: nothing to show.
                    Err(error) if card.error.is_none() && matches!(card.boxes, Loadable::Ready(_)) => {
                        card.error = Some(error);
                        std::mem::replace(&mut card.boxes, Loadable::Idle)
                    }
                    Err(error) => Loadable::Error(error),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_box(&mut self, theme: &Theme, ix: usize, record: CloudBox, cx: &mut Context<Self>) -> AnyElement {
        let id = record.id.clone();
        let (wake_id, stop_id, destroy_id) = (id.clone(), id.clone(), id);
        let badge = match record.state {
            CloudBoxState::Running => widgets::badge_active(theme, state_label(record.state)),
            _ => widgets::badge(theme, state_label(record.state)),
        };
        let awake = matches!(
            record.state,
            CloudBoxState::Running | CloudBoxState::Waking | CloudBoxState::Checkpointing
        );
        let idle = self.busy.is_some();
        widgets::card_row(theme, false)
            .gap(px(10.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, record.name.clone()))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(box_detail(&record)).into_any_element()],
                    )),
            )
            .child(badge)
            .child(if awake {
                widgets::text_action(theme, widgets::ActionTone::Quiet, "Stop")
                    .id(("cloud-stop", ix))
                    .when(idle, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |card, _, _, cx| {
                        card.box_call(methods::CLOUD_STOP, &stop_id, "Saving and stopping the box…", cx)
                    }))
            } else {
                widgets::text_action(theme, widgets::ActionTone::Quiet, "Wake")
                    .id(("cloud-wake", ix))
                    .when(idle, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |card, _, _, cx| {
                        card.box_call(methods::CLOUD_WAKE, &wake_id, "Waking the box…", cx)
                    }))
            })
            .child(
                widgets::text_action(theme, widgets::ActionTone::Quiet, "Remove")
                    .id(("cloud-remove", ix))
                    .when(idle, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |card, _, _, cx| card.destroy(&destroy_id, cx))),
            )
            .into_any_element()
    }
}

impl Render for CloudCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let header_copy = "Run a chat in a container in your own Cloudflare account, and move it back when you're \
             back at your desk. The box sleeps when idle and wakes when you send it work.";
        let reply = self.boxes.ready().cloned();
        let connected = reply.as_ref().is_some_and(|r| r.connected);
        let boxes = reply.map(|r| r.boxes).unwrap_or_default();
        let header = widgets::card_row(&theme, true).child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .child(widgets::row_title(&theme, "Continue in the cloud"))
                .child(widgets::meta_line(
                    &theme,
                    vec![div().child(header_copy).into_any_element()],
                )),
        );
        let mut card = widgets::section_card(&theme).mt(px(8.0)).child(header);
        match &self.boxes {
            Loadable::Idle | Loadable::Loading => {
                card = card.child(
                    widgets::card_row(&theme, false)
                        .child(div().text_color(theme.text_muted).child("Loading…")),
                );
            }
            Loadable::Error(error) => {
                card = card.child(
                    widgets::card_row(&theme, false)
                        .child(div().text_color(theme.text_muted).child(error.clone())),
                );
            }
            Loadable::Ready(_) => {}
        }
        for (ix, record) in boxes.iter().cloned().enumerate() {
            card = card.child(self.render_box(&theme, ix, record, cx));
        }
        if matches!(self.boxes, Loadable::Ready(_)) && !connected && boxes.is_empty() {
            card = card.child(
                widgets::card_row(&theme, false).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(widgets::field_label(&theme, "Cloudflare API token"))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(8.0))
                                .items_center()
                                .child(
                                    div()
                                        .flex_1()
                                        .child(crate::popover::dialog_field(self.token.clone().into_any_element())),
                                )
                                .child(
                                    widgets::text_action(&theme, widgets::ActionTone::Solid, "Connect")
                                        .id("cloud-connect")
                                        .on_click(cx.listener(|card, _, _, cx| card.connect(cx))),
                                ),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_muted)
                                .child(
                                    "Create an account API token with Workers Scripts: Edit, Containers: Edit, \
                                     Workers R2 Storage: Edit and Account Settings: Read. It's kept on this \
                                     device only (Keychain on macOS).",
                                ),
                        ),
                ),
            );
        } else if matches!(self.boxes, Loadable::Ready(_)) && connected && boxes.is_empty() {
            let instance = INSTANCE_TYPES[self.instance];
            card = card.child(
                widgets::card_row(&theme, false).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(widgets::field_label(&theme, "New box"))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(8.0))
                                .items_center()
                                .child(
                                    div()
                                        .w(px(160.0))
                                        .child(crate::popover::dialog_field(self.name.clone().into_any_element())),
                                )
                                .child(
                                    widgets::select(
                                        "cloud-instance",
                                        "Box size",
                                        &theme,
                                        |card: &mut Self| &mut card.instance_select,
                                    )
                                    .options(
                                        INSTANCE_TYPES.iter().map(|t| widgets::SelectOption::new(*t)),
                                        self.instance,
                                    )
                                    .width(140.0)
                                    .on_select(|card, ix, _, cx| {
                                        card.instance = ix;
                                        cx.notify();
                                    })
                                    .render(&self.instance_select, cx),
                                )
                                .child(
                                    widgets::select(
                                        "cloud-idle",
                                        "Sleep after",
                                        &theme,
                                        |card: &mut Self| &mut card.idle_select,
                                    )
                                    .options(
                                        IDLE_MINUTES
                                            .iter()
                                            .map(|m| widgets::SelectOption::new(format!("{m} min idle"))),
                                        self.idle,
                                    )
                                    .width(150.0)
                                    .on_select(|card, ix, _, cx| {
                                        card.idle = ix;
                                        cx.notify();
                                    })
                                    .render(&self.idle_select, cx),
                                )
                                .child(
                                    widgets::text_action(&theme, widgets::ActionTone::Solid, "Create box")
                                        .id("cloud-create")
                                        .on_click(cx.listener(|card, _, _, cx| card.provision(cx))),
                                ),
                        )
                        .children(cost_hint(instance).map(|hint| {
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_muted)
                                .child(hint)
                        })),
                ),
            );
        }
        card.when_some(self.busy, |card, busy| {
            card.child(
                widgets::card_row(&theme, false)
                    .child(div().text_color(theme.text_muted).child(busy)),
            )
        })
        .when_some(self.error.clone(), |card, error| {
            card.child(widgets::card_row(&theme, false).child(widgets::error_strip(&theme, error)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(state: CloudBoxState, error: Option<&str>) -> CloudBox {
        CloudBox {
            id: "box-1".into(),
            provider: "cloudflare".into(),
            name: "Cloud".into(),
            account_id: "acc".into(),
            worker_url: "https://zeron-cloud.example.workers.dev".into(),
            wake_key: "k".into(),
            instance_type: "standard-2".into(),
            idle_minutes: 15,
            created_at: 0,
            state,
            state_at: 0,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn every_offered_size_has_a_price_hint_and_unknown_ones_none() {
        for t in INSTANCE_TYPES {
            let hint = cost_hint(t).unwrap_or_else(|| panic!("{t} has no estimate"));
            assert!(hint.contains("vCPU") && hint.contains("a month"), "{hint}");
        }
        assert!(cost_hint("huge").is_none());
        // 1 vCPU, 6 GiB: the documented standard-2 numbers.
        let hint = cost_hint("standard-2").unwrap();
        assert!(hint.starts_with("1 vCPU, 6 GiB memory."), "{hint}");
        assert!(hint.contains("$5 Workers plan"), "{hint}");
    }

    #[test]
    fn states_and_details_read_in_plain_words() {
        assert_eq!(state_label(CloudBoxState::Asleep), "Asleep");
        assert_eq!(state_label(CloudBoxState::Checkpointing), "Saving");
        assert_eq!(
            box_detail(&record(CloudBoxState::Asleep, None)),
            "standard-2 · sleeps after 15 min idle"
        );
        assert!(
            box_detail(&record(CloudBoxState::Error, Some("the Worker said no")))
                .ends_with("the Worker said no")
        );
        assert_eq!(IDLE_MINUTES[1], 15, "the default is 15 minutes");
        assert_eq!(INSTANCE_TYPES[2], "standard-2", "the default is standard-2");
    }
}
