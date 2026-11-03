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
#[derive(Default)]
pub struct Search {
    query: Option<String>,
    pub rows: Vec<usize>,
    pub too_long: bool,
}
impl Search {
    pub fn refresh(&mut self, entries: &[Entry], query: &str) {
        if self.query.as_deref() == Some(query) {
            return;
        }
        self.query = Some(query.into());
        self.rows.clear();
        self.too_long = query.chars().take(129).count() > 128;
        if self.too_long {
            return;
        }
        if query.is_empty() {
            self.rows.extend(0..entries.len());
            return;
        }
        let mut matches = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
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
    fn open_tab_search_keeps_duplicates_prioritizes_names_and_bounds_the_query() {
        let entries = vec![
            entry(ActivePanel::Left, 0, "Elsewhere", "/alpha/path"),
            entry(ActivePanel::Left, 1, "Alpha", "/duplicate"),
            entry(ActivePanel::Right, 0, "Alpha", "/duplicate"),
            entry(ActivePanel::Right, 1, "Работа", "/other"),
        ];
        let mut search = Search::default();
        search.refresh(&entries, "");
        assert_eq!(search.rows, [0, 1, 2, 3]);
        search.refresh(&entries, "alpha");
        assert_eq!(search.rows, [1, 2, 0]);
        search.refresh(&entries, "РБТ");
        assert_eq!(search.rows, [3]);
        search.refresh(&entries, &"a".repeat(129));
        assert!(search.too_long && search.rows.is_empty());
        search.refresh(&entries, "missing");
        assert!(!search.too_long && search.rows.is_empty());
        search.refresh(&entries, "");
        assert_eq!(search.rows.len(), 4);
    }
}
