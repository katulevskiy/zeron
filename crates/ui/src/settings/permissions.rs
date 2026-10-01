//! Settings → General → Default permission mode: the mode new chats on this
//! device start in (Bypass unless changed). Stored by the engine with the
//! device's harness preferences (`GetPolicySettings` / `SetPolicySettings`),
//! so agent-created chats and other clients start from the same default.

use gpui::{AnyElement, Context, Entity, Subscription, Task, div, prelude::*, px};
use zeron_proto::{PermissionMode, PolicyRule, RuleEffect};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub struct DefaultModeCard {
    state: Entity<AppState>,
    mode: Loadable<PermissionMode>,
    select: widgets::SelectState,
    error: Option<String>,
    task: Option<Task<()>>,
    _state: Subscription,
}

impl DefaultModeCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Settings can open before the engine connects: load once it does.
        let state_sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.mode, Loadable::Idle) {
                this.call(None, cx);
            }
        });
        let mut card = Self {
            state,
            mode: Loadable::Idle,
            select: widgets::SelectState::default(),
            error: None,
            task: None,
            _state: state_sub,
        };
        card.call(None, cx);
        card
    }

    /// Read the device's settings, or store a new default; either reply is
    /// the device's authoritative value, which composers then start from.
    fn call(&mut self, save: Option<PermissionMode>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (method, params) = match save {
            Some(mode) => (
                methods::SET_POLICY_SETTINGS,
                serde_json::json!({ "defaultMode": mode }),
            ),
            None => (methods::GET_POLICY_SETTINGS, serde_json::json!({})),
        };
        if matches!(self.mode, Loadable::Idle) {
            self.mode = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    v.get("defaultMode")
                        .cloned()
                        .map(serde_json::from_value::<PermissionMode>)
                        .unwrap_or(Ok(PermissionMode::Bypass))
                        .map_err(|e| e.to_string())
                });
            this.update(cx, |card, cx| {
                match result {
                    Ok(mode) => {
                        crate::permission_mode::set_device_default(mode, cx);
                        card.mode = Loadable::Ready(mode);
                        card.error = None;
                    }
                    Err(error) if matches!(card.mode, Loadable::Ready(_)) => {
                        card.error = Some(error);
                    }
                    Err(error) => card.mode = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for DefaultModeCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let control: AnyElement = match &self.mode {
            Loadable::Ready(current) => {
                let current = *current;
                widgets::select(
                    "default-permission-mode",
                    "Default permission mode",
                    &theme,
                    |card: &mut Self| &mut card.select,
                )
                .options(
                    PermissionMode::ALL
                        .iter()
                        .map(|mode| widgets::SelectOption::new(mode.label())),
                    PermissionMode::ALL
                        .iter()
                        .position(|mode| *mode == current)
                        .unwrap_or_default(),
                )
                .width(184.0)
                .on_select(|card, ix, _, cx| {
                    if let Some(mode) = PermissionMode::ALL.get(ix) {
                        card.call(Some(*mode), cx);
                    }
                })
                .render(&self.select, cx)
                .into_any_element()
            }
            Loadable::Error(error) => div()
                .text_color(theme.text_muted)
                .child(error.clone())
                .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        let description = self
            .mode
            .ready()
            .map(|mode| mode.description())
            .unwrap_or("The mode new chats start in.");
        widgets::section_card(&theme)
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(&theme, "Default permission mode"))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(format!(
                                            "New chats start here. {description}."
                                        ))
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(control),
            )
            .when_some(self.error.clone(), |card, error| {
                card.child(widgets::card_row(&theme, false).child(widgets::error_strip(&theme, error)))
            })
    }
}

/// Settings → General → Standing rules: what the agent may or may not do
/// whatever its mode, first match wins. "Always allow" answers in a permission
/// prompt land here; the user can add their own (a deny that holds even in
/// Bypass) and forget any of them.
pub struct PolicyRulesCard {
    state: Entity<AppState>,
    rules: Loadable<Vec<PolicyRule>>,
    effect_select: widgets::SelectState,
    kind_select: widgets::SelectState,
    effect: usize,
    kind: usize,
    pattern: Entity<ComposerInput>,
    error: Option<String>,
    task: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl PolicyRulesCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let state_sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.rules, Loadable::Idle) {
                this.call(methods::LIST_POLICY_RULES, None, cx);
            }
        });
        let pattern = cx.new(|cx| ComposerInput::new("Command, path or tool, e.g. cargo test*", cx));
        let input_sub = cx.subscribe(&pattern, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.add(cx);
            }
        });
        let mut card = Self {
            state,
            rules: Loadable::Idle,
            effect_select: widgets::SelectState::default(),
            kind_select: widgets::SelectState::default(),
            effect: 0,
            kind: 0,
            pattern,
            error: None,
            task: None,
            _subscriptions: [state_sub, input_sub],
        };
        card.call(methods::LIST_POLICY_RULES, None, cx);
        card
    }

    /// Run one of the rules RPCs; each replies with the full list.
    fn call(&mut self, method: &'static str, rule: Option<PolicyRule>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = match rule {
            Some(rule) => serde_json::json!({ "rule": rule }),
            None => serde_json::json!({}),
        };
        if matches!(self.rules, Loadable::Idle) {
            self.rules = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<Vec<PolicyRule>>(
                        v.get("rules").cloned().unwrap_or_default(),
                    )
                    .map_err(|e| e.to_string())
                });
            this.update(cx, |card, cx| {
                match result {
                    Ok(rules) => {
                        card.rules = Loadable::Ready(rules);
                        card.error = None;
                    }
                    Err(error) if matches!(card.rules, Loadable::Ready(_)) => {
                        card.error = Some(error);
                    }
                    Err(error) => card.rules = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let text = self.pattern.read(cx).text();
        let Some(rule) = crate::permission_mode::new_rule(
            crate::permission_mode::RULE_EFFECTS[self.effect],
            crate::permission_mode::RULE_KINDS[self.kind],
            &text,
        ) else {
            return;
        };
        self.pattern.update(cx, |input, cx| input.set_text("", cx));
        self.call(methods::ADD_POLICY_RULE, Some(rule), cx);
    }
}

impl Render for PolicyRulesCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::permission_mode::{
            RULE_EFFECTS, RULE_KINDS, effect_description, effect_label, kind_label,
        };
        let theme = Theme::of(cx).for_settings_surface();
        let effect = RULE_EFFECTS[self.effect];
        let add_row = widgets::card_row(&theme, true).child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .child(widgets::row_title(&theme, "Standing rules"))
                .child(widgets::meta_line(
                    &theme,
                    vec![
                        div()
                            .child(format!(
                                "Checked before the mode; the first match wins. A matching rule {}. \
                                 In Bypass, Ask and Deny rules apply to Claude Code and OpenCode.",
                                effect_description(effect)
                            ))
                            .into_any_element(),
                    ],
                ))
                .child(
                    div()
                        .mt(px(10.0))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            widgets::select(
                                "rule-effect",
                                "Rule effect",
                                &theme,
                                |card: &mut Self| &mut card.effect_select,
                            )
                            .options(
                                RULE_EFFECTS
                                    .iter()
                                    .map(|e| widgets::SelectOption::new(effect_label(*e))),
                                self.effect,
                            )
                            .width(96.0)
                            .on_select(|card, ix, _, cx| {
                                card.effect = ix;
                                cx.notify();
                            })
                            .render(&self.effect_select, cx),
                        )
                        .child(
                            widgets::select(
                                "rule-kind",
                                "Rule applies to",
                                &theme,
                                |card: &mut Self| &mut card.kind_select,
                            )
                            .options(
                                RULE_KINDS
                                    .iter()
                                    .map(|k| widgets::SelectOption::new(kind_label(*k))),
                                self.kind,
                            )
                            .width(116.0)
                            .on_select(|card, ix, _, cx| {
                                card.kind = ix;
                                cx.notify();
                            })
                            .render(&self.kind_select, cx),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(180.0))
                                .child(crate::popover::dialog_field(self.pattern.clone().into_any_element())),
                        )
                        .child(
                            widgets::text_action(&theme, widgets::ActionTone::Solid, "Add")
                                .id("rule-add")
                                .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                        ),
                ),
        );
        let rows: Vec<AnyElement> = match &self.rules {
            Loadable::Ready(rules) if rules.is_empty() => vec![
                widgets::card_row(&theme, false)
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .child("No rules yet. \"Always allow\" in a permission prompt saves one here."),
                    )
                    .into_any_element(),
            ],
            Loadable::Ready(rules) => rules
                .iter()
                .cloned()
                .enumerate()
                .map(|(ix, rule)| {
                    let badge = match rule.effect {
                        RuleEffect::Allow => widgets::badge_active(&theme, effect_label(rule.effect)),
                        _ => widgets::badge(&theme, effect_label(rule.effect)),
                    };
                    let forget = rule.clone();
                    widgets::card_row(&theme, false)
                        .gap(px(10.0))
                        .child(badge)
                        .child(widgets::badge(&theme, kind_label(rule.kind)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme.font_mono.clone())
                                .text_size(crate::typography::ui_rems(12.5))
                                .child(rule.pattern.clone()),
                        )
                        .child(
                            widgets::text_action(&theme, widgets::ActionTone::Quiet, "Remove")
                                .id(("rule-remove", ix))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.call(
                                        methods::REMOVE_POLICY_RULE,
                                        Some(forget.clone()),
                                        cx,
                                    )
                                })),
                        )
                        .into_any_element()
                })
                .collect(),
            Loadable::Error(error) => vec![
                widgets::card_row(&theme, false)
                    .child(div().text_color(theme.text_muted).child(error.clone()))
                    .into_any_element(),
            ],
            Loadable::Idle | Loadable::Loading => vec![
                widgets::card_row(&theme, false)
                    .child(div().text_color(theme.text_muted).child("Loading…"))
                    .into_any_element(),
            ],
        };
        widgets::section_card(&theme)
            .child(add_row)
            .children(rows)
            .when_some(self.error.clone(), |card, error| {
                card.child(widgets::card_row(&theme, false).child(widgets::error_strip(&theme, error)))
            })
    }
}
