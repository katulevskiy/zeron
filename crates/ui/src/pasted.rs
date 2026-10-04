//! Long pastes as attachments: the composer stages a paste past
//! [`MIN_CHARS`] as a chip instead of flooding the input, the sent prompt
//! carries each one as a trailing `<pasted-text>` block the agent reads, and
//! [`extract_badge`] turns those blocks back into a pill in the transcript.

use gpui::{SharedString, div, prelude::*, px};

/// Shorter pastes go into the input as typed text.
pub const MIN_CHARS: usize = 1_500;

const OPEN: &str = "\n\n<pasted-text>\n";
const CLOSE: &str = "\n</pasted-text>";
/// The words a prompt carries when the pastes are all there is.
const ONLY_TEXT: &str = "See the pasted text below.";
const PREVIEW_CHARS: usize = 280;

/// One pill for every paste in a message, with a preview row for each. The
/// composer's chip is the same pill.
pub fn badge(blocks: &[impl AsRef<str>]) -> crate::badges::MessageBadge {
    let label = match blocks {
        [one] => format!("Pasted text · {} chars", grouped(one.as_ref())),
        many => format!("{} pasted texts", many.len()),
    };
    crate::badges::MessageBadge {
        icon: crate::icons::DOCUMENT,
        label: label.into(),
        details: blocks
            .iter()
            .enumerate()
            .map(|(ix, text)| crate::badges::BadgeDetail {
                location: format!("Paste {}", ix + 1).into(),
                tag: Some(format!("{} chars", grouped(text.as_ref())).into()),
                body: preview(text.as_ref()).into(),
            })
            .collect(),
        full: Vec::new(),
    }
}

pub fn is_long(text: &str) -> bool {
    text.chars().nth(MIN_CHARS - 1).is_some()
}

/// `text` with every paste appended, in order. Folded before review comments,
/// whose block must stay last.
pub fn with_pasted(text: &str, pasted: &[String]) -> String {
    if pasted.is_empty() {
        return text.to_string();
    }
    let mut out = if text.trim().is_empty() {
        ONLY_TEXT.to_string()
    } else {
        text.to_string()
    };
    for paste in pasted {
        out.push_str(OPEN);
        out.push_str(paste);
        out.push_str(CLOSE);
    }
    out
}

/// [`crate::badges::Extractor`] for trailing paste blocks.
pub fn extract_badge(text: &str) -> Option<(String, crate::badges::MessageBadge)> {
    let mut rest = text;
    let mut blocks = Vec::new();
    while let Some(body) = rest.strip_suffix(CLOSE) {
        let Some(at) = body.rfind(OPEN) else {
            break;
        };
        blocks.push(&body[at + OPEN.len()..]);
        rest = &body[..at];
    }
    if blocks.is_empty() {
        return None;
    }
    blocks.reverse();
    let rest = if rest == ONLY_TEXT { "" } else { rest };
    Some((
        rest.to_string(),
        crate::badges::MessageBadge {
            full: blocks
                .iter()
                .map(|block| SharedString::from(block.to_string()))
                .collect(),
            ..badge(&blocks)
        },
    ))
}

/// The read-only full text a transcript pill opens, with Copy. Escape or a
/// click outside the card closes it.
pub fn viewer(
    viewport: gpui::Size<gpui::Pixels>,
    pill: &crate::badges::MessageBadge,
    focus: &gpui::FocusHandle,
    on_close: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
    cx: &gpui::App,
) -> gpui::AnyElement {
    let theme = crate::theme::Theme::of(cx).for_popup();
    let all: SharedString = pill
        .full
        .iter()
        .map(SharedString::as_ref)
        .collect::<Vec<_>>()
        .join("\n\n")
        .into();
    let copy = all.clone();
    let on_close = std::rc::Rc::new(on_close);
    let close_on_key = on_close.clone();
    let card = crate::popover::dialog_card(&theme)
        .w(px((f32::from(viewport.width) * 0.9).min(760.0)))
        .max_h(px(f32::from(viewport.height) * 0.8))
        .track_focus(focus)
        .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                close_on_key(window, cx);
            }
        })
        .on_mouse_down_out(move |_, window, cx| on_close(window, cx))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    crate::popover::dialog_title(&theme, &pill.label)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .child(
                    crate::popover::btn_ghost(&theme, "Copy", "paste-viewer-copy")
                        .id("paste-viewer-copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                copy.to_string(),
                            ));
                        }),
                ),
        )
        .child(
            div()
                .id("paste-viewer-body")
                .mt(px(12.0))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .text_size(crate::typography::ui_rems(13.0))
                .line_height(crate::typography::ui_rems(20.0))
                .child(all),
        )
        .into_any_element();
    crate::popover::modal("paste-viewer", viewport, card)
}

/// Character count with separators: `12,400`.
fn grouped(text: &str) -> String {
    crate::context_usage::with_separators(text.chars().count() as u64)
}

fn preview(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pastes_round_trip_into_one_pill() {
        let long = "x".repeat(12_400);
        let staged = [long, "second\nblock".to_string()];
        let sent = with_pasted("summarize these", &staged);
        assert!(sent.starts_with("summarize these\n\n<pasted-text>\n"));
        let (rest, pill) = extract_badge(&sent).unwrap();
        assert_eq!(rest, "summarize these");
        assert_eq!(pill.label, "2 pasted texts");
        assert_eq!(pill.details.len(), 2);
        assert_eq!(pill.details[0].tag.as_deref(), Some("12,400 chars"));
        assert!(pill.details[0].body.ends_with('…'));
        assert_eq!(pill.details[1].body, "second\nblock");
        assert_eq!(pill.full.len(), 2);
        assert_eq!(pill.full[0].len(), 12_400);
        // The composer's chip reads the same as the sent pill.
        assert_eq!(badge(&staged).label, pill.label);
        assert_eq!(badge(&staged).details, pill.details);
    }

    #[test]
    fn a_paste_alone_is_a_message_and_shows_no_filler() {
        let sent = with_pasted("  ", &["only this".into()]);
        let (rest, badge) = extract_badge(&sent).unwrap();
        assert_eq!(rest, "");
        assert_eq!(badge.label, "Pasted text · 9 chars");
    }

    #[test]
    fn review_comments_still_trail_the_pastes() {
        let comment = crate::comments::ReviewComment::file("src/a.rs", 3, "fix this");
        let sent = crate::comments::with_comments(&with_pasted("go", &["log".into()]), &[comment]);
        let (badges_text, badges) = crate::badges::split(&sent);
        assert_eq!(badges_text, "go");
        assert_eq!(badges.len(), 2);
    }

    #[test]
    fn plain_messages_and_quoted_tags_are_left_alone() {
        assert!(extract_badge("no pastes here").is_none());
        assert!(extract_badge("talking about </pasted-text>").is_none());
        assert!(!is_long(&"y".repeat(MIN_CHARS - 1)));
        assert!(is_long(&"y".repeat(MIN_CHARS)));
    }
}
