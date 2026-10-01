//! The agent's checklist as plain data: what the header says and which rows a
//! long list folds to. Shared by the desktop tray and the mobile strip so a
//! phone and a laptop fold the same list the same way (`docs/todo-panel.md`).

use std::ops::Range;

use crate::{TodoItem, TodoStatus};

/// Lists longer than this fold to a [`FOCUS_WINDOW`] around the current item.
pub const FOLD_ABOVE: usize = 6;
pub const FOCUS_WINDOW: usize = 3;

/// What the collapsed header reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TodoSummary {
    pub total: usize,
    pub done: usize,
    /// First in-progress item.
    pub active: Option<usize>,
    /// First item not yet completed.
    pub next: Option<usize>,
}

impl TodoSummary {
    pub fn of(items: &[TodoItem]) -> Self {
        let mut summary = Self {
            total: items.len(),
            done: 0,
            active: None,
            next: None,
        };
        for (ix, item) in items.iter().enumerate() {
            match item.status() {
                TodoStatus::Completed => summary.done += 1,
                TodoStatus::InProgress => {
                    summary.active.get_or_insert(ix);
                    summary.next.get_or_insert(ix);
                }
                TodoStatus::Pending => {
                    summary.next.get_or_insert(ix);
                }
            }
        }
        summary
    }

    pub fn finished(&self) -> bool {
        self.total > 0 && self.done == self.total
    }

    /// The item the header names: what is being worked on, else what is next.
    pub fn headline(&self) -> Option<usize> {
        self.active.or(self.next)
    }
}

/// The slice of a long list that stays visible when folded: three items with
/// the current one in the middle (one of context before it, one after),
/// clamped to the list ends. A finished list shows its last three. Lists up to
/// [`FOLD_ABOVE`] are never folded.
pub fn focus_window(items: &[TodoItem]) -> Range<usize> {
    let total = items.len();
    if total <= FOLD_ABOVE {
        return 0..total;
    }
    let summary = TodoSummary::of(items);
    let focus = summary.headline().unwrap_or(total - 1);
    let start = focus.saturating_sub(1).min(total - FOCUS_WINDOW);
    start..start + FOCUS_WINDOW
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldSide {
    Earlier,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoRow {
    /// An item, by its index in the agent's list.
    Item(usize),
    /// A fold toggle standing in for `count` hidden items (`open`: they are
    /// currently revealed, and the row hides them again).
    Fold {
        side: FoldSide,
        count: usize,
        open: bool,
    },
}

/// The rows of the expanded list, top to bottom. Items always keep the
/// agent's order; folds sit at the edges of the list they stand for.
pub fn rows(items: &[TodoItem], show_earlier: bool, show_later: bool) -> Vec<TodoRow> {
    let window = focus_window(items);
    let earlier = window.start;
    let later = items.len() - window.end;
    let mut out = Vec::with_capacity(items.len() + 2);
    if earlier > 0 {
        out.push(TodoRow::Fold {
            side: FoldSide::Earlier,
            count: earlier,
            open: show_earlier,
        });
        if show_earlier {
            out.extend((0..earlier).map(TodoRow::Item));
        }
    }
    out.extend(window.clone().map(TodoRow::Item));
    if later > 0 {
        if show_later {
            out.extend((window.end..items.len()).map(TodoRow::Item));
        }
        out.push(TodoRow::Fold {
            side: FoldSide::Later,
            count: later,
            open: show_later,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(spec: &str) -> Vec<TodoItem> {
        // x = completed, > = in progress, . = pending
        spec.chars()
            .enumerate()
            .map(|(i, c)| {
                TodoItem::new(
                    format!("item {i}"),
                    match c {
                        'x' => TodoStatus::Completed,
                        '>' => TodoStatus::InProgress,
                        _ => TodoStatus::Pending,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn summary_counts_and_names_the_current_item() {
        let s = TodoSummary::of(&items("xx>.."));
        assert_eq!(
            (s.total, s.done, s.active, s.next),
            (5, 2, Some(2), Some(2))
        );
        assert_eq!(s.headline(), Some(2));
        assert!(!s.finished());
        // Nothing started: the first unfinished item is the headline.
        let s = TodoSummary::of(&items("x..."));
        assert_eq!((s.active, s.headline()), (None, Some(1)));
        // An in-progress item later in the list still wins over an earlier
        // pending one: the agent is working on it.
        assert_eq!(TodoSummary::of(&items(".>.")).headline(), Some(1));
        let s = TodoSummary::of(&items("xxx"));
        assert!(s.finished());
        assert_eq!(s.headline(), None);
        assert!(!TodoSummary::of(&[]).finished());
    }

    #[test]
    fn short_lists_never_fold() {
        for n in 0..=FOLD_ABOVE {
            let list = items(&".".repeat(n));
            assert_eq!(focus_window(&list), 0..n);
            assert!(
                rows(&list, false, false)
                    .iter()
                    .all(|r| matches!(r, TodoRow::Item(_)))
            );
        }
    }

    #[test]
    fn long_lists_fold_to_three_around_the_current_item() {
        // Current item in the middle: one before, it, one after.
        assert_eq!(focus_window(&items("xxxx>.....")), 3..6);
        // At the start / end the window clamps instead of shrinking.
        assert_eq!(focus_window(&items(">.......")), 0..3);
        assert_eq!(focus_window(&items("xxxxxxx>")), 5..8);
        // No in-progress item: the first unfinished one is the focus.
        assert_eq!(focus_window(&items("xxxx.....")), 3..6);
        // Everything done: the tail.
        assert_eq!(focus_window(&items("xxxxxxxx")), 5..8);
        // Always exactly three when folded.
        for spec in ["xxxxxxx>", ">.......", "xx>.....", "xxxxxxxx", "........"] {
            assert_eq!(focus_window(&items(spec)).len(), FOCUS_WINDOW, "{spec}");
        }
    }

    #[test]
    fn rows_put_folds_at_the_edges_with_counts() {
        let list = items("xxxx>.....");
        assert_eq!(
            rows(&list, false, false),
            vec![
                TodoRow::Fold {
                    side: FoldSide::Earlier,
                    count: 3,
                    open: false
                },
                TodoRow::Item(3),
                TodoRow::Item(4),
                TodoRow::Item(5),
                TodoRow::Fold {
                    side: FoldSide::Later,
                    count: 4,
                    open: false
                },
            ]
        );
        // No fold on a side with nothing hidden.
        let head = rows(&items(">......."), false, false);
        assert!(matches!(head.first(), Some(TodoRow::Item(0))));
        assert!(matches!(
            head.last(),
            Some(TodoRow::Fold {
                side: FoldSide::Later,
                count: 5,
                ..
            })
        ));
    }

    #[test]
    fn expanding_a_fold_reveals_items_without_reordering() {
        let list = items("xxxx>.....");
        let order = |rows: &[TodoRow]| -> Vec<usize> {
            rows.iter()
                .filter_map(|r| match r {
                    TodoRow::Item(ix) => Some(*ix),
                    _ => None,
                })
                .collect()
        };
        for (earlier, later) in [(false, false), (true, false), (false, true), (true, true)] {
            let shown = order(&rows(&list, earlier, later));
            assert!(shown.windows(2).all(|w| w[0] < w[1]), "{earlier} {later}");
        }
        // Both open: the whole list, in order, and each toggle still present
        // so it can be closed again.
        let all = rows(&list, true, true);
        assert_eq!(order(&all), (0..list.len()).collect::<Vec<_>>());
        assert!(matches!(
            all.first(),
            Some(TodoRow::Fold { open: true, .. })
        ));
        assert!(matches!(all.last(), Some(TodoRow::Fold { open: true, .. })));
        // Earlier items appear after their fold row, later items before theirs.
        assert_eq!(all[1], TodoRow::Item(0));
        assert_eq!(all[all.len() - 2], TodoRow::Item(9));
    }
}
