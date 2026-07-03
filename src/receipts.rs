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
    /// The action to replay for "Undo", cloned at push time. `None` for a
    /// Delete, which has no undo path in this app today. The undo button is
    /// only live while this is still the exact top of the undo stack (see
    /// [`Receipt::still_undoable`]), so undoing an older receipt out of
    /// order can never desync from what Cmd+Z would actually do.
    pub undo_action: Option<crate::undo::Action>,
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

    /// Whether this receipt's action is still exactly the top of `stack`,
    /// i.e. whether its Undo button should be live right now.
    pub fn still_undoable(&self, top_of_undo_stack: Option<&crate::undo::Action>) -> bool {
        self.undo_action.as_ref() == top_of_undo_stack
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

    fn receipt(verb: &'static str, jump_to: &str, action: Option<crate::undo::Action>) -> Receipt {
        Receipt {
            verb,
            item_count: 1,
            timestamp: 0.0,
            jump_to: PathBuf::from(jump_to),
            undo_action: action,
        }
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
        let r = receipt("Moved", "/b", Some(a.clone()));

        assert!(r.still_undoable(Some(&a)), "matches the live stack top");
        assert!(
            !r.still_undoable(Some(&other)),
            "a different action is now on top (e.g. a newer Move happened)"
        );
        assert!(
            !r.still_undoable(None),
            "the stack was emptied (e.g. this was already undone)"
        );

        let delete_receipt = receipt("Deleted", "/c", None);
        assert!(
            !delete_receipt.still_undoable(Some(&a)),
            "a delete receipt never has an undo action"
        );
    }
}
