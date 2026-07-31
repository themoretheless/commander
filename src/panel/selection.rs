use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

const REMEMBERED_BINDINGS_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Focus {
    Parent,
    Entry(PathBuf),
}

#[derive(Default)]
struct RememberedSelection {
    selected: HashSet<PathBuf>,
    marked: HashSet<PathBuf>,
}

pub(super) struct SelectionState {
    binding: PathBuf,
    has_complete_snapshot: bool,
    selected: HashSet<PathBuf>,
    marked: HashSet<PathBuf>,
    remembered: HashMap<PathBuf, RememberedSelection>,
    remembered_order: VecDeque<PathBuf>,
    focus: Focus,
    cursor_row: usize,
    scroll_to_cursor: bool,
    scroll_anchor: usize,
    page_rows: usize,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

impl SelectionState {
    pub(super) fn new(binding: PathBuf) -> Self {
        Self {
            binding,
            has_complete_snapshot: false,
            selected: HashSet::new(),
            marked: HashSet::new(),
            remembered: HashMap::new(),
            remembered_order: VecDeque::new(),
            focus: Focus::Parent,
            cursor_row: 0,
            scroll_to_cursor: false,
            scroll_anchor: 0,
            page_rows: 0,
        }
    }

    /// Switch the active directory without exposing remembered selections.
    ///
    /// Selection for the target is restored only when a complete listing is
    /// published, so an unavailable target cannot report or act on stale rows.
    pub(super) fn bind(&mut self, binding: &Path) -> bool {
        if self.binding == binding {
            return false;
        }

        self.stash_active();
        self.binding = binding.to_path_buf();
        self.has_complete_snapshot = false;
        self.selected.clear();
        self.marked.clear();
        self.reset_position();
        true
    }

    pub(super) fn publish_complete(&mut self, binding: &Path) {
        self.bind(binding);
        if self.has_complete_snapshot {
            return;
        }

        self.remembered_order.retain(|path| path != binding);
        let restored = self.remembered.remove(binding).unwrap_or_default();
        self.selected = restored.selected;
        self.marked = restored.marked;
        self.has_complete_snapshot = true;
    }

    fn stash_active(&mut self) {
        if !self.has_complete_snapshot {
            return;
        }

        let binding = self.binding.clone();
        self.remembered_order.retain(|path| path != &binding);
        if self.selected.is_empty() && self.marked.is_empty() {
            self.remembered.remove(&binding);
            return;
        }

        self.remembered.insert(
            binding.clone(),
            RememberedSelection {
                selected: std::mem::take(&mut self.selected),
                marked: std::mem::take(&mut self.marked),
            },
        );
        self.remembered_order.push_back(binding);
        while self.remembered.len() > REMEMBERED_BINDINGS_LIMIT {
            if let Some(oldest) = self.remembered_order.pop_front() {
                self.remembered.remove(&oldest);
            }
        }
    }

    fn reset_position(&mut self) {
        self.focus = Focus::Parent;
        self.cursor_row = 0;
        self.scroll_to_cursor = false;
        self.scroll_anchor = 0;
    }

    pub(super) fn selected(&self) -> &HashSet<PathBuf> {
        &self.selected
    }

    pub(super) fn marked(&self) -> &HashSet<PathBuf> {
        &self.marked
    }

    pub(super) fn replace_selected(&mut self, selected: HashSet<PathBuf>) {
        self.selected = selected;
    }

    #[cfg(test)]
    pub(super) fn replace_marked(&mut self, marked: HashSet<PathBuf>) {
        self.marked = marked;
    }

    pub(super) fn retain_present<'a>(&mut self, present: impl IntoIterator<Item = &'a PathBuf>) {
        let present: HashSet<&PathBuf> = present.into_iter().collect();
        self.selected.retain(|path| present.contains(path));
        self.marked.retain(|path| present.contains(path));
    }

    pub(super) fn clear_selected(&mut self) {
        self.selected.clear();
    }

    pub(super) fn insert_selected(&mut self, path: PathBuf) -> bool {
        self.selected.insert(path)
    }

    pub(super) fn remove_selected(&mut self, path: &Path) -> bool {
        self.selected.remove(path)
    }

    pub(super) fn extend_selected(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selected.extend(paths);
    }

    pub(super) fn toggle_selected(&mut self, path: PathBuf) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }

    pub(super) fn toggle_marked(&mut self, path: PathBuf) {
        if !self.marked.remove(&path) {
            self.marked.insert(path);
        }
    }

    pub(super) fn clear_marked(&mut self) {
        self.marked.clear();
    }

    pub(super) fn cursor(&self) -> usize {
        self.cursor_row
    }

    pub(super) fn focus(&self) -> &Focus {
        &self.focus
    }

    pub(super) fn set_cursor(&mut self, row: usize, path: Option<PathBuf>) {
        match (row, path) {
            (0, _) => {
                self.cursor_row = 0;
                self.focus = Focus::Parent;
            }
            (row, Some(path)) => {
                self.cursor_row = row;
                self.focus = Focus::Entry(path);
            }
            (_, None) => {
                self.cursor_row = 0;
                self.focus = Focus::Parent;
            }
        }
    }

    pub(super) fn focused_path(&self) -> Option<&Path> {
        match &self.focus {
            Focus::Parent => None,
            Focus::Entry(path) => Some(path),
        }
    }

    pub(super) fn scroll_to_cursor(&self) -> bool {
        self.scroll_to_cursor
    }

    pub(super) fn set_scroll_to_cursor(&mut self, value: bool) {
        self.scroll_to_cursor = value;
    }

    pub(super) fn scroll_anchor(&self) -> usize {
        self.scroll_anchor
    }

    pub(super) fn set_scroll_anchor(&mut self, anchor: usize) {
        self.scroll_anchor = anchor;
    }

    pub(super) fn page_rows(&self) -> usize {
        self.page_rows
    }

    pub(super) fn set_page_rows(&mut self, rows: usize) {
        self.page_rows = rows;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_focus_never_carries_an_entry_path() {
        let mut selection = SelectionState::default();
        selection.set_cursor(1, Some(PathBuf::from("/first")));
        assert_eq!(selection.focused_path(), Some(Path::new("/first")));

        selection.set_cursor(0, Some(PathBuf::from("/must-be-ignored")));
        assert_eq!(selection.focus(), &Focus::Parent);
        assert_eq!(selection.focused_path(), None);
    }

    #[test]
    fn selection_and_marks_are_independent_path_sets() {
        let mut selection = SelectionState::default();
        let path = PathBuf::from("/a");
        selection.insert_selected(path.clone());
        selection.toggle_marked(path.clone());
        selection.clear_selected();

        assert!(selection.selected().is_empty());
        assert!(selection.marked().contains(&path));
    }

    #[test]
    fn binding_switch_hides_state_until_a_complete_snapshot_restores_it() {
        let old_binding = PathBuf::from("/old");
        let old_entry = old_binding.join("selected.txt");
        let mut selection = SelectionState::new(old_binding.clone());
        selection.publish_complete(&old_binding);
        selection.insert_selected(old_entry.clone());
        selection.toggle_marked(old_entry.clone());
        selection.set_cursor(1, Some(old_entry.clone()));

        assert!(selection.bind(Path::new("/unavailable")));
        assert!(selection.selected().is_empty());
        assert!(selection.marked().is_empty());
        assert_eq!(selection.focus(), &Focus::Parent);
        assert_eq!(selection.cursor(), 0);

        assert!(selection.bind(&old_binding));
        assert!(
            selection.selected().is_empty(),
            "returning to the path alone must not expose remembered rows"
        );
        selection.publish_complete(&old_binding);
        assert!(selection.selected().contains(&old_entry));
        assert!(selection.marked().contains(&old_entry));
        assert_eq!(selection.focus(), &Focus::Parent);
    }
}
