use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

const REMEMBERED_BINDINGS_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Focus {
    Parent,
    Entry(PathBuf),
}

pub(super) struct SelectionState {
    binding: PathBuf,
    has_complete_snapshot: bool,
    selected: HashSet<PathBuf>,
    /// Selections left behind in other folders, by folder.
    remembered: HashMap<PathBuf, HashSet<PathBuf>>,
    remembered_order: VecDeque<PathBuf>,
    focus: Focus,
    cursor_row: usize,
    scroll_to_cursor: bool,
    scroll_anchor: usize,
    page_rows: usize,
    /// Bumped on every change to the selected sets (here or remembered), so
    /// views derived from the selection can tell when to rebuild.
    revision: u64,
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
            remembered: HashMap::new(),
            remembered_order: VecDeque::new(),
            focus: Focus::Parent,
            cursor_row: 0,
            scroll_to_cursor: false,
            scroll_anchor: 0,
            page_rows: 0,
            revision: 0,
        }
    }

    pub(super) const fn revision(&self) -> u64 {
        self.revision
    }

    const fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Switch the active directory without exposing remembered selections.
    ///
    /// Selection for the target is restored only when a complete listing is
    /// published, so an unavailable target cannot report or act on stale rows.
    pub(super) fn bind(&mut self, binding: &Path) -> bool {
        if self.binding == binding {
            return false;
        }
        self.bump();

        self.stash_active();
        self.binding = binding.to_path_buf();
        self.has_complete_snapshot = false;
        self.selected.clear();
        self.reset_position();
        true
    }

    pub(super) fn publish_complete(&mut self, binding: &Path) {
        self.bump();
        self.bind(binding);
        if self.has_complete_snapshot {
            return;
        }

        self.remembered_order.retain(|path| path != binding);
        self.selected = self.remembered.remove(binding).unwrap_or_default();
        self.has_complete_snapshot = true;
    }

    fn stash_active(&mut self) {
        if !self.has_complete_snapshot {
            return;
        }

        let binding = self.binding.clone();
        self.remembered_order.retain(|path| path != &binding);
        if self.selected.is_empty() {
            self.remembered.remove(&binding);
            return;
        }

        self.remembered
            .insert(binding.clone(), std::mem::take(&mut self.selected));
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

    pub(super) const fn selected(&self) -> &HashSet<PathBuf> {
        &self.selected
    }

    pub(super) fn replace_selected(&mut self, selected: HashSet<PathBuf>) {
        self.bump();
        self.selected = selected;
    }

    pub(super) fn retain_present<'a>(&mut self, present: impl IntoIterator<Item = &'a PathBuf>) {
        self.bump();
        let present: HashSet<&PathBuf> = present.into_iter().collect();
        self.selected.retain(|path| present.contains(path));
    }

    pub(super) fn clear_selected(&mut self) {
        self.bump();
        self.selected.clear();
    }

    /// Selections remembered in folders other than the bound one, by folder.
    pub(super) fn selected_elsewhere(&self) -> impl Iterator<Item = (&PathBuf, &HashSet<PathBuf>)> {
        self.remembered
            .iter()
            .filter(|(binding, selected)| *binding != &self.binding && !selected.is_empty())
    }

    /// Clear the selection here and in every remembered folder.
    pub(super) fn clear_selected_everywhere(&mut self) {
        self.bump();
        self.selected.clear();
        self.remembered.clear();
        self.remembered_order.clear();
    }

    pub(super) fn insert_selected(&mut self, path: PathBuf) -> bool {
        self.bump();
        self.selected.insert(path)
    }

    pub(super) fn remove_selected(&mut self, path: &Path) -> bool {
        self.bump();
        self.selected.remove(path)
    }

    pub(super) fn extend_selected(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.bump();
        self.selected.extend(paths);
    }

    pub(super) fn toggle_selected(&mut self, path: PathBuf) {
        self.bump();
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }

    pub(super) const fn cursor(&self) -> usize {
        self.cursor_row
    }

    pub(super) const fn focus(&self) -> &Focus {
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

    pub(super) const fn scroll_to_cursor(&self) -> bool {
        self.scroll_to_cursor
    }

    pub(super) const fn set_scroll_to_cursor(&mut self, value: bool) {
        self.scroll_to_cursor = value;
    }

    pub(super) const fn scroll_anchor(&self) -> usize {
        self.scroll_anchor
    }

    pub(super) const fn set_scroll_anchor(&mut self, anchor: usize) {
        self.scroll_anchor = anchor;
    }

    pub(super) const fn page_rows(&self) -> usize {
        self.page_rows
    }

    pub(super) const fn set_page_rows(&mut self, rows: usize) {
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
    fn binding_switch_hides_state_until_a_complete_snapshot_restores_it() {
        let old_binding = PathBuf::from("/old");
        let old_entry = old_binding.join("selected.txt");
        let mut selection = SelectionState::new(old_binding.clone());
        selection.publish_complete(&old_binding);
        selection.insert_selected(old_entry.clone());
        selection.set_cursor(1, Some(old_entry.clone()));

        assert!(selection.bind(Path::new("/unavailable")));
        assert!(selection.selected().is_empty());
        assert_eq!(selection.focus(), &Focus::Parent);
        assert_eq!(selection.cursor(), 0);

        assert!(selection.bind(&old_binding));
        assert!(
            selection.selected().is_empty(),
            "returning to the path alone must not expose remembered rows"
        );
        selection.publish_complete(&old_binding);
        assert!(selection.selected().contains(&old_entry));
        assert_eq!(selection.focus(), &Focus::Parent);
    }

    /// Visit `folder`, pick `picks` there, and leave for a scratch folder.
    fn pick_in(selection: &mut SelectionState, folder: &Path, picks: &[&str]) {
        selection.publish_complete(folder);
        for pick in picks {
            selection.insert_selected(folder.join(pick));
        }
        selection.bind(Path::new("/scratch"));
    }

    #[test]
    fn remembered_selections_evict_the_least_recently_left_folder() {
        let mut selection = SelectionState::default();
        let folder = |index: usize| PathBuf::from(format!("/f/{index:03}"));
        for index in 0..=REMEMBERED_BINDINGS_LIMIT {
            pick_in(&mut selection, &folder(index), &["pick"]);
        }

        assert_eq!(selection.remembered.len(), REMEMBERED_BINDINGS_LIMIT);
        assert!(!selection.remembered.contains_key(&folder(0)), "oldest");
        assert!(selection.remembered.contains_key(&folder(1)));
        assert_eq!(selection.remembered_order.len(), REMEMBERED_BINDINGS_LIMIT);
        assert_eq!(selection.remembered_order.front(), Some(&folder(1)));

        // Revisiting a folder makes it the most recent: the next overflow
        // evicts folder 2 instead.
        selection.publish_complete(&folder(1));
        assert!(selection.selected().contains(&folder(1).join("pick")));
        selection.bind(Path::new("/scratch"));
        assert_eq!(selection.remembered_order.back(), Some(&folder(1)));
        pick_in(&mut selection, &PathBuf::from("/f/extra"), &["pick"]);
        assert!(selection.remembered.contains_key(&folder(1)));
        assert!(!selection.remembered.contains_key(&folder(2)));
        assert_eq!(selection.remembered.len(), REMEMBERED_BINDINGS_LIMIT);
    }

    #[test]
    fn leaving_a_folder_with_nothing_picked_forgets_its_old_picks() {
        let mut selection = SelectionState::default();
        let folder = PathBuf::from("/a");
        pick_in(&mut selection, &folder, &["x"]);
        assert_eq!(selection.selected_elsewhere().count(), 1);

        selection.publish_complete(&folder);
        selection.clear_selected();
        selection.bind(Path::new("/b"));

        assert!(!selection.remembered.contains_key(&folder));
        assert!(!selection.remembered_order.contains(&folder));
        assert_eq!(selection.selected_elsewhere().count(), 0);
    }

    #[test]
    fn a_folder_still_loading_does_not_count_its_picks_as_elsewhere() {
        let mut selection = SelectionState::default();
        let a = PathBuf::from("/a");
        pick_in(&mut selection, &a, &["x"]);
        selection.publish_complete(Path::new("/b"));
        assert_eq!(selection.selected_elsewhere().count(), 1);

        // Back to A before its listing lands: its picks are neither here
        // (no complete snapshot yet) nor elsewhere.
        selection.bind(&a);
        assert!(selection.selected().is_empty());
        assert_eq!(selection.selected_elsewhere().count(), 0);

        selection.publish_complete(&a);
        assert!(selection.selected().contains(&a.join("x")));
        assert_eq!(selection.selected_elsewhere().count(), 0);
    }
}
