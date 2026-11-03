//! Bounded, snapshot-owned open-tab matching. Search text never identifies a tab.
use crate::workspace::ActivePanel;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub side: ActivePanel,
    pub index: usize,
    pub revision: u64,
    pub path: PathBuf,
    pub name: String,
    pub current: bool,
    pub focused: bool,
    pub protected: bool,
    pub pin: Option<PathBuf>,
}
impl Entry {
    pub const fn side_label(&self) -> &'static str {
        match self.side {
            ActivePanel::Left => "Left",
            ActivePanel::Right => "Right",
        }
    }
    pub fn accessible_label(&self) -> String {
        format!(
            "{} tab {}: {}, {}{}",
            self.side_label(),
            self.index + 1,
            self.name.escape_debug(),
            self.path.to_string_lossy().escape_debug(),
            if self.focused {
                ", focused"
            } else if self.current {
                ", current in pane"
            } else {
                ""
            }
        )
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Left,
    Right,
}
impl Scope {
    pub const ALL: [Self; 3] = [Self::All, Self::Left, Self::Right];
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "Both panes",
            Self::Left => "Left pane",
            Self::Right => "Right pane",
        }
    }
    pub fn admits(self, side: ActivePanel) -> bool {
        match self {
            Self::All => true,
            Self::Left => side == ActivePanel::Left,
            Self::Right => side == ActivePanel::Right,
        }
    }
}
#[derive(Default)]
pub struct Search {
    query: Option<(String, Scope)>,
    pub rows: Vec<usize>,
    pub too_long: bool,
}
impl Search {
    pub fn refresh(&mut self, entries: &[Entry], query: &str, scope: Scope) {
        if self
            .query
            .as_ref()
            .is_some_and(|(old, old_scope)| old == query && *old_scope == scope)
        {
            return;
        }
        self.query = Some((query.into(), scope));
        self.rows.clear();
        self.too_long = query.chars().take(129).count() > 128;
        if self.too_long {
            return;
        }
        if query.is_empty() {
            self.rows.extend(
                entries
                    .iter()
                    .enumerate()
                    .filter_map(|(i, e)| scope.admits(e.side).then_some(i)),
            );
            return;
        }
        let mut matches = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if !scope.admits(entry.side) {
                continue;
            }
            let score = crate::fuzzy::score(query, &entry.name)
                .map(|s| (0, s.score))
                .or_else(|| {
                    crate::fuzzy::score(query, &entry.path.to_string_lossy()).map(|s| (1, s.score))
                });
            if let Some((field, score)) = score {
                matches.push((field, std::cmp::Reverse(score), index));
            }
        }
        matches.sort();
        self.rows.extend(matches.into_iter().map(|(_, _, i)| i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(side: ActivePanel, index: usize, name: &str, path: &str) -> Entry {
        Entry {
            side,
            index,
            revision: 1,
            path: path.into(),
            name: name.into(),
            current: false,
            focused: false,
            protected: false,
            pin: None,
        }
    }
    #[test]
    fn tab_scopes_invalidate_same_query_keep_duplicate_slots_and_recover_errors() {
        let entries = vec![
            entry(ActivePanel::Left, 0, "Work", "/same"),
            entry(ActivePanel::Right, 0, "Work", "/same"),
            entry(ActivePanel::Right, 1, "Other", "/work"),
        ];
        let mut search = Search::default();
        for (scope, expected) in [
            (Scope::All, vec![0, 1, 2]),
            (Scope::Right, vec![1, 2]),
            (Scope::Left, vec![0]),
            (Scope::All, vec![0, 1, 2]),
        ] {
            search.refresh(&entries, "work", scope);
            assert_eq!(search.rows, expected);
        }
        search.refresh(&entries, &"x".repeat(129), Scope::Right);
        assert!(search.too_long && search.rows.is_empty());
        search.refresh(&entries, "", Scope::Left);
        assert!(!search.too_long);
        assert_eq!(search.rows, [0]);
        search.refresh(&entries, "", Scope::Right);
        assert_eq!(search.rows, [1, 2]);
        search.refresh(&entries, "/same", Scope::Right);
        assert_eq!(search.rows, [1]);
    }
    #[test]
    fn open_tab_search_keeps_duplicates_prioritizes_names_and_bounds_the_query() {
        let entries = vec![
            entry(ActivePanel::Left, 0, "Elsewhere", "/alpha/path"),
            entry(ActivePanel::Left, 1, "Alpha", "/duplicate"),
            entry(ActivePanel::Right, 0, "Alpha", "/duplicate"),
            entry(ActivePanel::Right, 1, "Работа", "/other"),
        ];
        let mut search = Search::default();
        search.refresh(&entries, "", Scope::All);
        assert_eq!(search.rows, [0, 1, 2, 3]);
        search.refresh(&entries, "alpha", Scope::All);
        assert_eq!(search.rows, [1, 2, 0]);
        search.refresh(&entries, "РБТ", Scope::All);
        assert_eq!(search.rows, [3]);
        search.refresh(&entries, &"a".repeat(129), Scope::All);
        assert!(search.too_long && search.rows.is_empty());
        search.refresh(&entries, "missing", Scope::All);
        assert!(!search.too_long && search.rows.is_empty());
        search.refresh(&entries, "", Scope::All);
        assert_eq!(search.rows.len(), 4);
    }
}
