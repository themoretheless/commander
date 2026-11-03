//! Ranked lookup over an opening snapshot of Favorites; no path probes.
use crate::bookmarks::Bookmarks;
pub const RESULT_CAP: usize = 100;
#[derive(Default)]
pub struct Matches {
    pub indices: Vec<usize>,
    pub total: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaseMode {
    #[default]
    Ignore,
    Smart,
    Match,
}
impl CaseMode {
    pub fn sensitive(self, query: &str) -> bool {
        self == Self::Match || (self == Self::Smart && query.chars().any(char::is_uppercase))
    }
}
#[cfg(test)]
pub fn rank(store: &Bookmarks, query: &str) -> Matches {
    rank_case(store, query, CaseMode::Ignore)
}
pub fn rank_case(store: &Bookmarks, query: &str, mode: CaseMode) -> Matches {
    let query = query.trim();
    if query.chars().take(129).count() > 128 {
        return Matches::default();
    }
    if query.is_empty() {
        return Matches {
            indices: (0..store.items.len().min(RESULT_CAP)).collect(),
            total: store.items.len(),
        };
    }
    let sensitive = mode.sensitive(query);
    let mut scores: Vec<_> = store
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, bookmark)| {
            let name = crate::fuzzy::score_case(query, &bookmark.name, sensitive);
            let path = if name.is_none() {
                crate::fuzzy::score_case(query, &bookmark.path.to_string_lossy(), sensitive)
            } else {
                None
            };
            name.as_ref()
                .or(path.as_ref())
                .map(|score| (index, name.is_some(), score.score))
        })
        .collect();
    scores.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    let total = scores.len();
    Matches {
        indices: scores
            .into_iter()
            .take(RESULT_CAP)
            .map(|(index, _, _)| index)
            .collect(),
        total,
    }
}
#[derive(Default)]
pub struct Cache {
    query: Option<(String, CaseMode)>,
    pub matches: Matches,
}
impl Cache {
    #[cfg(test)]
    pub fn refresh(&mut self, store: &Bookmarks, query: &str) -> bool {
        self.refresh_case(store, query, CaseMode::Ignore)
    }
    pub fn refresh_case(&mut self, store: &Bookmarks, query: &str, mode: CaseMode) -> bool {
        if self
            .query
            .as_ref()
            .is_some_and(|(cached, case)| cached == query && *case == mode)
        {
            return false;
        }
        self.matches = rank_case(store, query, mode);
        self.query = Some((query.into(), mode));
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn case_modes_cover_unicode_names_path_fallback_stable_ties_and_cache_changes() {
        let mut store = Bookmarks::default();
        store.add("Alpha", "/one");
        store.add("alpha", "/two");
        store.add("Elsewhere", "/Alpha");
        store.add("ПРОЕКТ", "/three");
        store.add("проект", "/four");
        for mode in [CaseMode::Ignore, CaseMode::Smart] {
            assert_eq!(rank_case(&store, "alpha", mode).indices, [0, 1, 2]);
            assert_eq!(rank_case(&store, "пркт", mode).indices, [3, 4]);
        }
        for mode in [CaseMode::Smart, CaseMode::Match] {
            assert_eq!(rank_case(&store, "Alpha", mode).indices, [0, 2]);
            assert_eq!(rank_case(&store, "ПРКТ", mode).indices, [3]);
            assert_eq!(rank_case(&store, "aLP", mode).total, 0);
        }
        assert_eq!(rank_case(&store, "alpha", CaseMode::Match).indices, [1]);
        assert_eq!(
            rank_case(&store, "Alpha", CaseMode::Ignore).indices,
            [0, 1, 2]
        );
        let mut cache = Cache::default();
        for (mode, expected) in [
            (CaseMode::Ignore, 3),
            (CaseMode::Smart, 2),
            (CaseMode::Match, 2),
        ] {
            assert!(cache.refresh_case(&store, "Alpha", mode));
            assert_eq!(cache.matches.total, expected);
            assert!(!cache.refresh_case(&store, "Alpha", mode));
        }
        assert!(cache.refresh_case(&store, "alpha", CaseMode::Match));
        assert_eq!(cache.matches.indices, [1]);
        for mode in [CaseMode::Ignore, CaseMode::Smart, CaseMode::Match] {
            assert_eq!(rank_case(&store, " ", mode).indices, [0, 1, 2, 3, 4]);
            assert_eq!(rank_case(&store, &"A".repeat(129), mode).total, 0);
        }
    }
    #[test]
    fn matches_all_snapshot_rows_then_caps_with_stable_order_and_unicode_name_priority() {
        let mut store = Bookmarks::default();
        store.add("Elsewhere", "/work/target");
        store.add("Target", "/different");
        store.add("Проект GTK", "/unicode");
        store.add("Target", "/different-two");
        assert_eq!(rank(&store, "tgt").indices, [1, 3, 0]);
        assert_eq!(rank(&store, "ПРКТ").indices, [2]);
        assert_eq!(rank(&store, "unicode").indices, [2]);
        for index in 0..200 {
            store.add(format!("Folder {index}"), format!("/many/{index}"));
        }
        let all = rank(&store, "");
        assert_eq!(all.total, 204);
        assert_eq!(all.indices, (0..100).collect::<Vec<_>>());
        assert_eq!(rank(&store, "/many/199").indices, [203]);
        assert!(rank(&store, &"x".repeat(129)).indices.is_empty());
    }
    #[test]
    fn empty_and_nonempty_searches_are_cached_for_the_immutable_opening_snapshot() {
        let mut store = Bookmarks::default();
        store.add("Work", "/work");
        let mut cache = Cache::default();
        assert!(cache.refresh(&store, "missing"));
        assert_eq!(cache.matches.total, 0);
        assert!(!cache.refresh(&store, "missing"));
        assert!(cache.refresh(&store, "wk"));
        assert_eq!(cache.matches.indices, [0]);
        assert!(!cache.refresh(&store, "wk"));
    }
}
