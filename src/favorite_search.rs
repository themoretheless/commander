//! Ranked lookup over an opening snapshot of Favorites; no path probes.
use crate::bookmarks::Bookmarks;
pub const RESULT_CAP: usize = 100;
#[derive(Default)]
pub struct Matches {
    pub indices: Vec<usize>,
    pub total: usize,
}
pub fn rank(store: &Bookmarks, query: &str) -> Matches {
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
    let mut scores: Vec<_> = store
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, bookmark)| {
            let name = crate::fuzzy::score(query, &bookmark.name);
            let path = if name.is_none() {
                crate::fuzzy::score(query, &bookmark.path.to_string_lossy())
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
    query: Option<String>,
    pub matches: Matches,
}
impl Cache {
    pub fn refresh(&mut self, store: &Bookmarks, query: &str) -> bool {
        if self.query.as_deref() == Some(query) {
            return false;
        }
        self.matches = rank(store, query);
        self.query = Some(query.into());
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
