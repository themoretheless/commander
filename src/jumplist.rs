//! Per-pane jump list: a bounded navigation history with a cursor, modelled on
//! the vim jumplist. Unlike linear back/forward over a single trail, a new jump
//! made after stepping back TRUNCATES the forward tail, so the history always
//! reflects the path actually travelled.
//!
//! Pure and standalone (no I/O); it backs each panel's back/forward history.

use std::path::{Path, PathBuf};

/// Default maximum remembered jumps; older entries are evicted past this.
pub const DEFAULT_CAP: usize = 64;

/// A bounded trail of visited directories with a movable cursor.
#[derive(Debug)]
pub struct JumpList {
    trail: Vec<PathBuf>,
    /// Index of the focused entry, or `None` while the trail is empty.
    cursor: Option<usize>,
    cap: usize,
}

impl Default for JumpList {
    fn default() -> Self {
        Self::with_cap(DEFAULT_CAP)
    }
}

impl JumpList {
    pub fn new() -> Self {
        Self::default()
    }

    /// A jump list keeping at most `cap` entries (clamped to >= 1).
    pub fn with_cap(cap: usize) -> Self {
        JumpList {
            trail: Vec::new(),
            cursor: None,
            cap: cap.max(1),
        }
    }

    /// Record a jump to `path`.
    ///
    /// - If it equals the focused entry, it is a no-op (consecutive repeats are
    ///   collapsed), so re-entering the current directory does not grow history.
    /// - Otherwise any forward tail (entries after the cursor, reachable only by
    ///   [`forward`](Self::forward)) is discarded, `path` is appended, and the
    ///   cursor moves to it. The oldest entry is evicted once the cap is hit.
    pub fn push(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if self.current() == Some(path.as_path()) {
            return;
        }
        // Drop the forward tail: a new jump invalidates any "redo" path.
        if let Some(c) = self.cursor {
            self.trail.truncate(c + 1);
        }
        self.trail.push(path);
        // Evict from the front until within cap, keeping the cursor on the end.
        while self.trail.len() > self.cap {
            self.trail.remove(0);
        }
        self.cursor = Some(self.trail.len() - 1);
    }

    /// Step back one entry, returning the now-focused path, or `None` if already
    /// at the oldest entry (the cursor does not move past the start).
    pub fn back(&mut self) -> Option<&Path> {
        match self.cursor {
            Some(c) if c > 0 => {
                self.cursor = Some(c - 1);
                self.current()
            }
            _ => None,
        }
    }

    /// Step forward one entry, returning the now-focused path, or `None` if
    /// already at the newest entry.
    pub fn forward(&mut self) -> Option<&Path> {
        match self.cursor {
            Some(c) if c + 1 < self.trail.len() => {
                self.cursor = Some(c + 1);
                self.current()
            }
            _ => None,
        }
    }

    /// The currently-focused path, or `None` when the trail is empty.
    pub fn current(&self) -> Option<&Path> {
        self.cursor.map(|c| self.trail[c].as_path())
    }

    pub fn can_back(&self) -> bool {
        matches!(self.cursor, Some(c) if c > 0)
    }

    pub fn can_forward(&self) -> bool {
        matches!(self.cursor, Some(c) if c + 1 < self.trail.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trail(j: &JumpList) -> Vec<String> {
        j.trail
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    fn cur(j: &JumpList) -> Option<String> {
        j.current().map(|p| p.to_string_lossy().to_string())
    }

    #[test]
    fn empty_list_has_no_current_and_cannot_move() {
        let mut j = JumpList::new();
        assert_eq!(j.current(), None);
        assert_eq!(j.back(), None);
        assert_eq!(j.forward(), None);
    }

    #[test]
    fn push_appends_and_focuses_the_newest() {
        let mut j = JumpList::new();
        j.push("/a");
        j.push("/b");
        j.push("/c");
        assert_eq!(trail(&j), vec!["/a", "/b", "/c"]);
        assert_eq!(cur(&j), Some("/c".to_string()));
    }

    #[test]
    fn consecutive_duplicates_are_collapsed() {
        let mut j = JumpList::new();
        j.push("/a");
        j.push("/a"); // same as current -> no-op
        assert_eq!(trail(&j), vec!["/a"]);
        // After stepping back, re-pushing the focused entry is still a no-op.
        j.push("/b");
        j.back(); // focus /a
        j.push("/a"); // equals current -> no-op, forward tail (/b) survives
        assert_eq!(trail(&j), vec!["/a", "/b"]);
        assert_eq!(cur(&j), Some("/a".to_string()));
    }

    #[test]
    fn back_and_forward_walk_without_mutating_the_trail() {
        let mut j = JumpList::new();
        j.push("/a");
        j.push("/b");
        j.push("/c");
        assert_eq!(
            j.back().map(|p| p.to_string_lossy().to_string()),
            Some("/b".into())
        );
        assert_eq!(
            j.back().map(|p| p.to_string_lossy().to_string()),
            Some("/a".into())
        );
        // Trail unchanged by navigation.
        assert_eq!(trail(&j), vec!["/a", "/b", "/c"]);
        assert_eq!(
            j.forward().map(|p| p.to_string_lossy().to_string()),
            Some("/b".into())
        );
        assert_eq!(
            j.forward().map(|p| p.to_string_lossy().to_string()),
            Some("/c".into())
        );
    }

    #[test]
    fn cursor_is_bounded_at_both_ends() {
        let mut j = JumpList::new();
        j.push("/a");
        j.push("/b");
        // At the newest: cannot go forward.
        assert!(!j.can_forward());
        assert_eq!(j.forward(), None);
        // Walk to the oldest: cannot go back further.
        j.back();
        assert!(!j.can_back());
        assert_eq!(j.back(), None);
        assert_eq!(cur(&j), Some("/a".to_string()));
    }

    #[test]
    fn a_new_jump_truncates_the_forward_tail() {
        let mut j = JumpList::new();
        j.push("/a");
        j.push("/b");
        j.push("/c");
        j.back(); // focus /b, forward tail is [/c]
        j.push("/d"); // new jump discards /c
        assert_eq!(trail(&j), vec!["/a", "/b", "/d"]);
        assert_eq!(cur(&j), Some("/d".to_string()));
        assert!(!j.can_forward());
    }

    #[test]
    fn cap_evicts_oldest_and_keeps_cursor_valid() {
        let mut j = JumpList::with_cap(3);
        for p in ["/a", "/b", "/c", "/d"] {
            j.push(p);
        }
        // "/a" evicted; newest stays focused.
        assert_eq!(trail(&j), vec!["/b", "/c", "/d"]);
        assert_eq!(cur(&j), Some("/d".to_string()));
        // Cursor still walks the retained range correctly.
        assert_eq!(
            j.back().map(|p| p.to_string_lossy().to_string()),
            Some("/c".into())
        );
    }

    #[test]
    fn cap_is_clamped_to_at_least_one() {
        let mut j = JumpList::with_cap(0);
        j.push("/a");
        j.push("/b");
        assert_eq!(trail(&j), vec!["/b"]);
        assert_eq!(cur(&j), Some("/b".to_string()));
    }
}
