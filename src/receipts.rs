//! Operation receipts: a searchable, session-lifetime log of completed
//! moves, deletes and batch renames, each with a jump-back path and (for
//! moves/renames) a live undo affordance. No I/O and no UI here, so the log
//! and its search are unit-tested directly.

use std::path::PathBuf;

/// One completed operation, in the order it happened.
#[derive(Clone, PartialEq, Debug)]
pub struct Receipt {
    pub verb: &'static str,
    pub item_count: usize,
    /// Seconds, from the same injected clock as toasts (`ctx.input(|i| i.time)`).
    pub timestamp: f64,
    /// Where "Jump" navigates the active panel.
    pub jump_to: PathBuf,
    /// The history entry "Undo" reverses, captured at push time. `None` for
    /// a delete that did not preserve restore versions. The undo button is
    /// only live while this entry is still the top of the undo stack (see
    /// [`Receipt::still_undoable`]), so undoing an older receipt out of
    /// order can never desync from what Cmd+Z would actually do, even when
    /// an older entry recorded an equal action (A->B, B->A, A->B).
    pub undo_entry: Option<crate::undo::HistoryEntryId>,
}

impl Receipt {
    pub fn label(&self) -> String {
        let item = if self.item_count == 1 {
            "item"
        } else {
            "items"
        };
        format!("{} {} {}", self.verb, self.item_count, item)
    }

    /// Whether this receipt's entry is still the top of the undo stack,
    /// i.e. whether its Undo button should be live right now.
    pub fn still_undoable(&self, top_of_undo_stack: Option<crate::undo::HistoryEntryId>) -> bool {
        self.undo_entry.is_some() && self.undo_entry == top_of_undo_stack
    }
}

/// A short "how long ago" label for `then` relative to `now` (both seconds
/// on the same injected clock, e.g. `ctx.input(|i| i.time)`).
pub fn format_elapsed(now: f64, then: f64) -> String {
    let secs = (now - then).max(0.0);
    if secs < 60.0 {
        "just now".to_string()
    } else if secs < 3600.0 {
        format!("{}m ago", (secs / 60.0) as u64)
    } else if secs < 86_400.0 {
        format!("{}h ago", (secs / 3600.0) as u64)
    } else {
        format!("{}d ago", (secs / 86_400.0) as u64)
    }
}

/// Session-lifetime receipt history, oldest first. Capped so a very long
/// session doesn't grow it without bound.
#[derive(Default)]
pub struct ReceiptLog {
    entries: Vec<Receipt>,
}

const MAX_ENTRIES: usize = 200;

impl ReceiptLog {
    pub fn push(&mut self, receipt: Receipt) {
        self.entries.push(receipt);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// All receipts matching `query` (case-insensitive substring of the
    /// label or the jump-back path), newest first. An empty query matches
    /// everything.
    pub fn search(&self, query: &str) -> Vec<&Receipt> {
        let q = query.trim().to_lowercase();
        self.entries
            .iter()
            .rev()
            .filter(|r| {
                q.is_empty()
                    || r.label().to_lowercase().contains(&q)
                    || r.jump_to.to_string_lossy().to_lowercase().contains(&q)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(
        verb: &'static str,
        jump_to: &str,
        entry: Option<crate::undo::HistoryEntryId>,
    ) -> Receipt {
        Receipt {
            verb,
            item_count: 1,
            timestamp: 0.0,
            jump_to: PathBuf::from(jump_to),
            undo_entry: entry,
        }
    }

    /// Record `action` and return the receipt entry id now on top.
    fn record(
        center: &mut crate::undo::UndoCenter,
        action: &crate::undo::Action,
    ) -> Option<crate::undo::HistoryEntryId> {
        center.record(action.clone()).unwrap();
        center.top_undo_entry()
    }

    #[test]
    fn format_elapsed_buckets_by_magnitude() {
        assert_eq!(format_elapsed(100.0, 100.0), "just now");
        assert_eq!(format_elapsed(100.0, 59.5), "just now");
        assert_eq!(format_elapsed(100.0, 40.0), "1m ago");
        assert_eq!(format_elapsed(4000.0, 100.0), "1h ago");
        assert_eq!(format_elapsed(200_000.0, 100.0), "2d ago");
        // A clock that hasn't advanced yet never reports negative elapsed.
        assert_eq!(format_elapsed(100.0, 150.0), "just now");
    }

    #[test]
    fn search_matches_the_label_or_the_jump_path_case_insensitively() {
        let mut log = ReceiptLog::default();
        log.push(receipt("Moved", "/Users/me/Downloads", None));
        log.push(receipt("Deleted", "/Users/me/Desktop", None));

        assert_eq!(log.search("moved").len(), 1);
        assert_eq!(log.search("DOWNLOADS").len(), 1);
        assert_eq!(log.search("desktop")[0].verb, "Deleted");
        assert_eq!(log.search("").len(), 2, "empty query matches everything");
        assert!(log.search("nonexistent").is_empty());
    }

    #[test]
    fn search_returns_newest_first() {
        let mut log = ReceiptLog::default();
        log.push(receipt("Moved", "/a", None));
        log.push(receipt("Deleted", "/b", None));
        let results = log.search("");
        assert_eq!(results[0].verb, "Deleted");
        assert_eq!(results[1].verb, "Moved");
    }

    #[test]
    fn push_caps_the_log_dropping_the_oldest() {
        let mut log = ReceiptLog::default();
        for i in 0..(MAX_ENTRIES + 5) {
            log.push(receipt("Moved", &format!("/dir{i}"), None));
        }
        assert_eq!(log.search("").len(), MAX_ENTRIES);
        // The 5 oldest were dropped; the newest survives.
        assert!(log.search(&format!("dir{}", MAX_ENTRIES + 4)).len() == 1);
        assert!(log.search("dir0").is_empty());
    }

    #[test]
    fn still_undoable_only_while_this_is_exactly_the_undo_stack_top() {
        let a = crate::undo::Action::Move {
            pairs: vec![(PathBuf::from("/a/x"), PathBuf::from("/b/x"))],
        };
        let other = crate::undo::Action::Move {
            pairs: vec![(PathBuf::from("/a/y"), PathBuf::from("/b/y"))],
        };
        let mut center = crate::undo::UndoCenter::default();
        let r = receipt("Moved", "/b", record(&mut center, &a));

        assert!(
            r.still_undoable(center.top_undo_entry()),
            "matches the live stack top"
        );
        let newer = record(&mut center, &other);
        assert!(
            !r.still_undoable(newer),
            "a different action is now on top (e.g. a newer Move happened)"
        );
        assert!(
            !r.still_undoable(None),
            "the stack was emptied (e.g. this was already undone)"
        );

        let delete_receipt = receipt("Deleted", "/c", None);
        assert!(
            !delete_receipt.still_undoable(newer),
            "a delete receipt never has an undo action"
        );
        assert!(!delete_receipt.still_undoable(None));
    }

    #[test]
    fn an_older_receipt_for_an_equal_action_is_not_undoable() {
        // A->B, B->A, A->B: the first and last moves are equal actions, but
        // only the newest receipt is the entry Cmd+Z would reverse.
        let there = crate::undo::Action::Move {
            pairs: vec![(PathBuf::from("/a/x"), PathBuf::from("/b/x"))],
        };
        let back = crate::undo::Action::Move {
            pairs: vec![(PathBuf::from("/b/x"), PathBuf::from("/a/x"))],
        };
        let mut center = crate::undo::UndoCenter::default();
        let first = receipt("Moved", "/b", record(&mut center, &there));
        let _ = record(&mut center, &back);
        let last = receipt("Moved", "/b", record(&mut center, &there));

        let top = center.top_undo_entry();
        assert!(last.still_undoable(top));
        assert!(!first.still_undoable(top));
    }
}
