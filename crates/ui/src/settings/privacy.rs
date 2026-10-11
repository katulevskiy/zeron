//! Settings → Privacy: the anonymous-usage toggles, what is and isn't
//! collected, the events most recently recorded (exactly as they are
//! uploaded), and the local usage table.
//!
//! Everything lives on the engine (`crate::telemetry` mirrors its status), so
//! this page keeps no settings of its own: a flip sends a patch and the
//! engine's reply repaints it.

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, SharedString, Subscription, Task, Window, div, prelude::*, px};
use serde_json::Value;
use zeron_telemetry::{TelemetrySettingsPatch, TelemetryStatus};

use crate::popover;
use crate::settings::widgets;
use crate::telemetry::{self, UsageRow};
use crate::theme::Theme;

/// Events listed under "Recent events".
const RECENT_LIMIT: usize = 30;
const USAGE_ROW_LIMIT: usize = 20;
/// While the page is on screen the engine is re-read this often.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const DOCS_URL: &str = "https://github.com/zeronsh/comet/blob/main/docs/telemetry.md";

pub struct PrivacyPage {
    scroll: widgets::PageScroll,
    /// `uuid` of the recent event whose JSON is open.
    expanded: Option<String>,
    last_render: Instant,
    _poll: Task<()>,
    _observe: Subscription,
}

#[derive(Clone, Copy)]
enum Toggle {
    Enabled,
    UsageImport,
}

impl PrivacyPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        telemetry::refresh_status(cx);
        telemetry::refresh_usage(cx);
        let observe = cx.observe_global::<telemetry::TelemetryState>(|_, cx| cx.notify());
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let alive = this
                    .update(cx, |page, cx| {
                        // Only chase the engine while someone is looking.
                        if page.last_render.elapsed() < POLL_INTERVAL * 2 {
                            telemetry::refresh_status(cx);
                            telemetry::refresh_usage(cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        Self {
            scroll: widgets::PageScroll::default(),
            expanded: None,
            last_render: Instant::now(),
            _poll: poll,
            _observe: observe,
        }
    }

    fn flip(&mut self, toggle: Toggle, cx: &mut Context<Self>) {
        let Some(settings) = telemetry::status(cx).map(|status| status.settings.clone()) else {
            return;
        };
        let patch = match toggle {
            Toggle::Enabled => TelemetrySettingsPatch {
                enabled: Some(!settings.enabled),
                ..Default::default()
            },
            Toggle::UsageImport => TelemetrySettingsPatch {
                usage_import: Some(!settings.usage_import),
                ..Default::default()
            },
        };
        telemetry::set_settings(patch, cx);
        cx.notify();
    }

    fn toggle_event(&mut self, uuid: &str, cx: &mut Context<Self>) {
        self.expanded = if self.expanded.as_deref() == Some(uuid) {
            None
        } else {
            Some(uuid.to_owned())
        };
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn switch_row(
        &self,
        theme: &Theme,
        first: bool,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        on: bool,
        interactive: bool,
        toggle: Toggle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let mut switch = div()
            .id(id)
            .flex_none()
            .w(px(widgets::SWITCH_WIDTH))
            .h(px(40.0))
            .flex()
            .items_center()
            .justify_center()
            .role(gpui::Role::Switch)
            .aria_label(title)
            .aria_toggled(if on {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .child(widgets::toggle_switch(theme, on, id));
        if interactive {
            switch = switch
                .tab_index(0)
                .focus_visible(move |style| style.border_2().border_color(accent))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.flip(toggle, cx)))
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        this.flip(toggle, cx);
                    }
                }));
        }
        widgets::card_row(theme, first)
            .when(!interactive, |el| el.opacity(0.55))
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, title))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(description).into_any_element()],
                    )),
            )
            .child(switch)
            .into_any_element()
    }

    fn render_status_rows(&self, theme: &Theme, status: &TelemetryStatus) -> AnyElement {
        let mut facts: Vec<(&'static str, String)> = vec![(
            "Anonymous ID",
            if status.account_linked {
                format!("{} (linked to your account)", status.distinct_id)
            } else {
                status.distinct_id.clone()
            },
        )];
        facts.push(("Events sent", status.sent.to_string()));
        facts.push(("Events dropped", status.dropped.to_string()));
        if let Some(endpoint) = &status.endpoint {
            facts.push(("Sent to", endpoint.clone()));
        } else {
            facts.push(("Sent to", "Nowhere (kept on this device)".into()));
        }
        if let Some(error) = &status.last_error {
            facts.push(("Last error", error.clone()));
        }
        let card = facts.into_iter().enumerate().fold(
            widgets::section_card(theme).mt_0(),
            |card, (ix, (label, value))| {
                card.child(
                    widgets::card_row(theme, ix == 0)
                        .min_h(px(44.0))
                        .child(
                            div()
                                .flex_none()
                                .w(px(120.0))
                                .text_size(crate::typography::ui_rems(widgets::ROW_TITLE_SIZE))
                                .text_color(theme.text)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(crate::typography::ui_rems(
                                    widgets::ROW_DESCRIPTION_SIZE,
                                ))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(value)),
                        ),
                )
            },
        );
        widgets::section(theme, "Status", card).into_any_element()
    }

    fn render_recent(
        &self,
        theme: &Theme,
        status: &TelemetryStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = chrono::Utc::now();
        let code_font = crate::typography::code_effective_family_name(cx);
        let mut card = widgets::section_card(theme).mt_0().py(px(6.0));
        if status.recent.is_empty() {
            card = card.child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                    .text_color(theme.text_muted)
                    .child("Nothing recorded yet."),
            );
        }
        for (ix, event) in status.recent.iter().rev().take(RECENT_LIMIT).enumerate() {
            let name = event
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or("event")
                .to_owned();
            let uuid = event
                .get("uuid")
                .and_then(Value::as_str)
                .map_or_else(|| format!("row-{ix}"), str::to_owned);
            let when = event
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                .map(|stamp| crate::state::format_time_ago(stamp.to_utc(), now))
                .unwrap_or_default();
            let open = self.expanded.as_deref() == Some(uuid.as_str());
            let toggle_uuid = uuid.clone();
            card = card.child(
                div()
                    .id(("privacy-event", ix))
                    .mx(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(crate::theme::ink(0.03)))
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label(format!("Show {name} event"))
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_event(&toggle_uuid, cx);
                    }))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(widgets::ROW_TITLE_SIZE))
                                    .text_color(theme.text)
                                    .child(SharedString::from(name)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(when)),
                            ),
                    )
                    .when(open, |el| {
                        let json = serde_json::to_string_pretty(event).unwrap_or_default();
                        el.child(
                            div()
                                .mt(px(6.0))
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(6.0))
                                .bg(widgets::block_fill(theme))
                                .font_family(code_font.clone())
                                .text_size(crate::typography::ui_rems(11.5))
                                .line_height(crate::typography::ui_rems(16.0))
                                .text_color(theme.text_muted)
                                .whitespace_normal()
                                .child(SharedString::from(json)),
                        )
                    }),
            );
        }
        widgets::section(theme, "Recent events", card).into_any_element()
    }

    fn render_usage(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let import_enabled = telemetry::usage(cx).is_some_and(|stats| stats.import_enabled);
        let rows = telemetry::usage(cx)
            .map(|stats| telemetry::aggregate_usage(stats, chrono::Utc::now().date_naive()))
            .unwrap_or_default();
        let hidden = rows.len().saturating_sub(USAGE_ROW_LIMIT);
        let cell = |text: SharedString| {
            div()
                .flex_none()
                .w(px(64.0))
                .text_right()
                .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                .text_color(theme.text_muted)
                .child(text)
        };
        let header = div()
            .mx(px(16.0))
            .pt(px(12.0))
            .pb(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text_muted)
            .child(div().flex_1().min_w_0().child("Model"))
            .child(cell("Requests".into()))
            .child(cell("Input".into()))
            .child(cell("Output".into()))
            .child(cell("Cache".into()))
            .child(cell("Cost".into()));
        let mut card = widgets::section_card(theme).mt_0();
        if rows.is_empty() {
            card = card.child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                    .text_color(theme.text_muted)
                    .child(if import_enabled {
                        "No usage recorded in the last 30 days."
                    } else {
                        "No usage recorded in the last 30 days. Turn on \"Include usage from \
                         other agent CLIs\" to add Claude Code, Codex, Pi and OpenCode."
                    }),
            );
        } else {
            card = card.child(header);
            for row in rows.iter().take(USAGE_ROW_LIMIT) {
                card = card.child(self.usage_row(theme, row, &cell));
            }
            if hidden > 0 {
                card = card.child(
                    div()
                        .px(px(16.0))
                        .py(px(8.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format!("and {hidden} more"))),
                );
            }
        }
        widgets::section(theme, "Usage, last 30 days", card).into_any_element()
    }

    fn usage_row(
        &self,
        theme: &Theme,
        row: &UsageRow,
        cell: &impl Fn(SharedString) -> gpui::Div,
    ) -> gpui::Div {
        let cost = row
            .cost_usd
            .map_or_else(|| "\u{2014}".to_owned(), |cost| format!("${cost:.2}"));
        let model = if row.model.is_empty() {
            "unknown"
        } else {
            row.model.as_str()
        };
        div()
            .mx(px(16.0))
            .py(px(8.0))
            .border_t_1()
            .border_color(widgets::row_divider(theme))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(widgets::ROW_TITLE_SIZE))
                            .text_color(theme.text)
                            .child(SharedString::from(model.to_owned())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(source_label(&row.source)))
                            .when(row.in_zeron, |el| {
                                el.child(widgets::badge(theme, "in Zeron"))
                            }),
                    ),
            )
            .child(cell(compact_count(row.requests).into()))
            .child(cell(compact_count(row.input_tokens).into()))
            .child(cell(compact_count(row.output_tokens).into()))
            .child(cell(compact_count(row.cache_read_tokens).into()))
            .child(cell(cost.into()))
    }
}

fn source_label(source: &str) -> String {
    match source {
        "claude-code" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "pi" => "Pi".into(),
        "opencode" => "OpenCode".into(),
        other => other.to_owned(),
    }
}

/// `987`, `12.3k`, `4.5M`, `1.2B`.
fn compact_count(value: u64) -> String {
    let (scaled, suffix) = match value {
        0..1_000 => return value.to_string(),
        1_000..1_000_000 => (value as f64 / 1e3, "k"),
        1_000_000..1_000_000_000 => (value as f64 / 1e6, "M"),
        _ => (value as f64 / 1e9, "B"),
    };
    if scaled >= 100.0 {
        format!("{scaled:.0}{suffix}")
    } else {
        format!("{scaled:.1}{suffix}")
    }
}

impl popover::ScrollRailHost for PrivacyPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for PrivacyPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render = Instant::now();
        let theme = Theme::of(cx).for_settings_surface();
        let status = telemetry::status(cx).cloned();

        let mut column = widgets::page_column()
            .child(widgets::page_header(&theme, "Privacy", None))
            .child(widgets::page_subtitle(
                &theme,
                "Zeron shares anonymous usage data to help improve the app. It never \
                 includes your prompts, code, file names, paths or repository names.",
            ));

        match status {
            None => {
                column = column.child(
                    div()
                        .mt(px(24.0))
                        .px(px(8.0))
                        .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                        .text_color(theme.text_muted)
                        .child(
                            "Usage sharing isn't available: this Zeron engine doesn't report \
                             telemetry, so nothing is collected or sent.",
                        ),
                );
            }
            Some(status) => {
                let forced_off = status.override_reason.is_some() && !status.collecting;
                if let Some(reason) = &status.override_reason {
                    column = column.child(
                        widgets::warning_strip(&theme, SharedString::from(reason.clone()))
                            .mt(px(16.0)),
                    );
                }
                let enabled = status.settings.enabled && !forced_off;
                let sharing = widgets::section_card(&theme)
                    .mt_0()
                    .child(self.switch_row(
                        &theme,
                        true,
                        "privacy-share-usage-toggle",
                        "Share anonymous usage data",
                        "Which features you use and how the app performs, tied to a random ID.",
                        enabled,
                        !forced_off,
                        Toggle::Enabled,
                        cx,
                    ))
                    .child(self.switch_row(
                        &theme,
                        false,
                        "privacy-import-usage-toggle",
                        "Include usage from other agent CLIs",
                        "Daily per-model token totals read from your local Claude Code, Codex, \
                         Pi and OpenCode logs. Never content or paths.",
                        status.settings.usage_import && enabled,
                        enabled,
                        Toggle::UsageImport,
                        cx,
                    ));
                let collected = div()
                    .mx(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                    .line_height(crate::typography::ui_rems(17.0))
                    .text_color(theme.text_muted)
                    .child(
                        "Collected: which features and settings are used, app version and \
                         platform, counts, durations, token totals and error categories.",
                    )
                    .child(
                        "Never collected: prompts, messages, code, file contents, file or \
                         folder names, paths, repository or branch names, URLs or terminal \
                         output.",
                    )
                    .child(
                        widgets::ghost_action(&theme)
                            .id("privacy-read-more")
                            .self_start()
                            .tab_index(0)
                            .role(gpui::Role::Link)
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(|_, _, cx| cx.open_url(DOCS_URL))
                            .child("Read the full event catalog (docs/telemetry.md)"),
                    );
                column = column
                    .child(widgets::section(&theme, "Sharing", sharing).mt(px(24.0)))
                    .child(widgets::section(&theme, "What is collected", collected))
                    .child(self.render_status_rows(&theme, &status))
                    .child(self.render_recent(&theme, &status, cx))
                    .child(self.render_usage(&theme, cx));
            }
        }

        let scrollbar = popover::rail(self, "privacy-page-scrollbar", &theme, cx);
        div()
            .id("privacy-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("privacy-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(column),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_compact_to_three_significant_figures() {
        assert_eq!(compact_count(0), "0");
        assert_eq!(compact_count(987), "987");
        assert_eq!(compact_count(1_000), "1.0k");
        assert_eq!(compact_count(12_345), "12.3k");
        assert_eq!(compact_count(123_456), "123k");
        assert_eq!(compact_count(4_560_000), "4.6M");
        assert_eq!(compact_count(2_000_000_000), "2.0B");
    }

    #[test]
    fn sources_get_their_product_names() {
        assert_eq!(source_label("claude-code"), "Claude Code");
        assert_eq!(source_label("opencode"), "OpenCode");
        assert_eq!(source_label("custom"), "custom");
    }
}
