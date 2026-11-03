//! Ranked lookup over an opening snapshot of Favorites; no path probes.
use crate::bookmarks::Bookmarks;
pub const RESULT_CAP: usize = 100;
#[derive(Default)]
pub struct Matches {
    pub indices: Vec<usize>,
    pub total: usize,
    pub error: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Both,
    Name,
    Path,
}
impl Scope {
    pub const ALL: [Self; 3] = [Self::Both, Self::Name, Self::Path];
    pub fn label(self) -> &'static str {
        match self {
            Self::Both => "Name + path",
            Self::Name => "Name",
            Self::Path => "Full path",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    #[default]
    Fuzzy,
    Literal,
    Glob,
    Regex,
    Exact,
}
impl SearchMode {
    pub const ALL: [Self; 5] = [
        Self::Fuzzy,
        Self::Literal,
        Self::Glob,
        Self::Regex,
        Self::Exact,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Fuzzy => "Fuzzy",
            Self::Literal => "Literal",
            Self::Glob => "Glob",
            Self::Regex => "Regex",
            Self::Exact => "Exact",
        }
    }
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
#[cfg(test)]
pub fn rank_case(store: &Bookmarks, query: &str, mode: CaseMode) -> Matches {
    rank_options(store, query, mode, SearchMode::Fuzzy)
}
#[cfg(test)]
pub fn rank_options(store: &Bookmarks, query: &str, mode: CaseMode, search: SearchMode) -> Matches {
    rank_scope(store, query, mode, search, Scope::Both)
}
pub fn rank_scope(
    store: &Bookmarks,
    query: &str,
    mode: CaseMode,
    search: SearchMode,
    scope: Scope,
) -> Matches {
    let query = query.trim();
    if query.chars().take(129).count() > 128 {
        return Matches {
            error: Some("Favorites search is limited to 128 characters".into()),
            ..Default::default()
        };
    }
    if query.is_empty() {
        return Matches {
            indices: (0..store.items.len().min(RESULT_CAP)).collect(),
            total: store.items.len(),
            error: None,
        };
    }
    let sensitive = mode.sensitive(query);
    let pattern = if search == SearchMode::Fuzzy {
        None
    } else {
        match crate::archive_filter::Pattern::compile(
            query,
            crate::archive_filter::Options {
                glob: search == SearchMode::Glob,
                regex: search == SearchMode::Regex,
                exact: search == SearchMode::Exact,
                case_sensitive: sensitive,
                ..Default::default()
            },
        ) {
            Ok(pattern) => Some(pattern),
            Err(error) => {
                return Matches {
                    error: Some(error),
                    ..Default::default()
                };
            }
        }
    };
    let mut scores: Vec<_> = store
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, bookmark)| {
            if let Some(pattern) = &pattern {
                let name = scope != Scope::Path
                    && pattern.matches(&bookmark.name, &bookmark.name.to_lowercase());
                let path = scope != Scope::Name
                    && !name
                    && bookmark
                        .path
                        .to_str()
                        .is_some_and(|path| pattern.matches(path, &path.to_lowercase()));
                return (name || path).then_some((index, name, 0));
            }
            let name = (scope != Scope::Path)
                .then(|| crate::fuzzy::score_case(query, &bookmark.name, sensitive))
                .flatten();
            let path = if scope != Scope::Name && name.is_none() {
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
        error: None,
    }
}
#[derive(Default)]
pub struct Cache {
    query: Option<(String, CaseMode, SearchMode, Scope)>,
    pub matches: Matches,
}
impl Cache {
    #[cfg(test)]
    pub fn refresh(&mut self, store: &Bookmarks, query: &str) -> bool {
        self.refresh_case(store, query, CaseMode::Ignore)
    }
    #[cfg(test)]
    pub fn refresh_case(&mut self, store: &Bookmarks, query: &str, mode: CaseMode) -> bool {
        self.refresh_options(store, query, mode, SearchMode::Fuzzy)
    }
    #[cfg(test)]
    pub fn refresh_options(
        &mut self,
        store: &Bookmarks,
        query: &str,
        mode: CaseMode,
        search: SearchMode,
    ) -> bool {
        self.refresh_scope(store, query, mode, search, Scope::Both)
    }
    pub fn refresh_scope(
        &mut self,
        store: &Bookmarks,
        query: &str,
        mode: CaseMode,
        search: SearchMode,
        scope: Scope,
    ) -> bool {
        if self
            .query
            .as_ref()
            .is_some_and(|(cached, case, matching, field)| {
                cached == query && *case == mode && *matching == search && *field == scope
            })
        {
            return false;
        }
        self.matches = rank_scope(store, query, mode, search, scope);
        self.query = Some((query.into(), mode, search, scope));
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_apply_to_every_matching_mode_and_keep_original_store_indices() {
        let mut store = Bookmarks::default();
        store.add("Target", "/elsewhere");
        store.add("Alias", "/Target");
        store.add("Target", "/Target-two");
        for mode in SearchMode::ALL {
            let q = match mode {
                SearchMode::Glob => "**/Target*",
                SearchMode::Regex => "Target",
                _ => "Target",
            };
            let run = |scope| rank_scope(&store, q, CaseMode::Ignore, mode, scope).indices;
            assert_eq!(run(Scope::Name), [0, 2]);
            if mode == SearchMode::Exact {
                assert!(run(Scope::Path).is_empty());
                assert_eq!(run(Scope::Both), [0, 2]);
            } else {
                assert_eq!(
                    run(Scope::Path),
                    if mode == SearchMode::Fuzzy {
                        vec![2, 1]
                    } else {
                        vec![1, 2]
                    }
                );
                assert_eq!(run(Scope::Both), [0, 2, 1]);
            }
            assert_eq!(
                rank_scope(&store, "", CaseMode::Match, mode, Scope::Path).indices,
                [0, 1, 2]
            );
        }
        assert_eq!(
            rank_scope(
                &store,
                "/Target",
                CaseMode::Match,
                SearchMode::Exact,
                Scope::Path
            )
            .indices,
            [1]
        );
        assert_eq!(
            rank_scope(
                &store,
                "target",
                CaseMode::Match,
                SearchMode::Literal,
                Scope::Path
            )
            .total,
            0
        );
        assert_eq!(
            rank_scope(
                &store,
                "target",
                CaseMode::Smart,
                SearchMode::Literal,
                Scope::Path
            )
            .indices,
            [1, 2]
        );
        assert_eq!(
            rank_scope(
                &store,
                "TARGET",
                CaseMode::Smart,
                SearchMode::Literal,
                Scope::Path
            )
            .total,
            0
        );
    }
    #[test]
    fn scope_changes_recompute_unchanged_queries_and_keep_caps_errors_and_store_intact() {
        let mut store = Bookmarks::default();
        for i in 0..150 {
            store.add(format!("Saved {i}"), format!("/many/{i}"));
        }
        let before = store.clone();
        let mut cache = Cache::default();
        for scope in [Scope::Both, Scope::Name, Scope::Path] {
            assert!(cache.refresh_scope(
                &store,
                "/many",
                CaseMode::Ignore,
                SearchMode::Literal,
                scope
            ));
            let expected = if scope == Scope::Name { 0 } else { 150 };
            assert_eq!(cache.matches.total, expected);
            assert_eq!(cache.matches.indices.len(), expected.min(100));
            assert!(!cache.refresh_scope(
                &store,
                "/many",
                CaseMode::Ignore,
                SearchMode::Literal,
                scope
            ));
            assert!(cache.refresh_scope(&store, "[", CaseMode::Ignore, SearchMode::Regex, scope));
            assert!(cache.matches.error.is_some() && cache.matches.indices.is_empty());
            assert!(cache.refresh_scope(
                &store,
                "/many",
                CaseMode::Ignore,
                SearchMode::Literal,
                scope
            ));
            assert!(cache.matches.error.is_none());
        }
        assert_eq!(store, before);
    }
    #[test]
    fn pattern_modes_match_complete_fields_or_substrings_without_fuzzy_fallback() {
        let mut store = Bookmarks::default();
        store.add("Report [final]", "/work/one");
        store.add("report final", "/work/two");
        store.add("Elsewhere", "/work/Report [final]");
        store.add("Проект GTK", "/unicode");
        let run = |q: &str, search| rank_options(&store, q, CaseMode::Ignore, search);
        assert_eq!(run("rpt", SearchMode::Fuzzy).total, 3);
        assert_eq!(run("rpt", SearchMode::Literal).total, 0);
        assert_eq!(run("Report [final]", SearchMode::Literal).indices, [0, 2]);
        assert_eq!(run("Report [final]", SearchMode::Exact).indices, [0]);
        assert_eq!(run("/work/Report [final]", SearchMode::Exact).indices, [2]);
        assert_eq!(run("Report*", SearchMode::Glob).indices, [0, 1]);
        assert_eq!(run("**/Report*", SearchMode::Glob).indices, [0, 1, 2]);
        assert_eq!(run("/work/Report*", SearchMode::Glob).indices, [2]);
        assert_eq!(run("Про?кт*", SearchMode::Glob).total, 0);
        assert_eq!(run("Про??кт*", SearchMode::Glob).indices, [3]);
        assert_eq!(run("^report.*final", SearchMode::Regex).indices, [0, 1]);
        assert_eq!(run("(?i)^ПРОЕКТ", SearchMode::Regex).indices, [3]);
        for search in SearchMode::ALL {
            assert_eq!(run("", search).indices, [0, 1, 2, 3]);
            assert!(run(&"a".repeat(129), search).error.is_some());
        }
        for search in [SearchMode::Glob, SearchMode::Regex] {
            let invalid = run("[", search);
            assert!(invalid.error.is_some());
            assert_eq!(invalid.total, 0);
            assert!(invalid.indices.is_empty());
        }
        assert!(run("[", SearchMode::Literal).error.is_none());
        assert_eq!(run("[", SearchMode::Literal).indices, [0, 2]);
    }
    #[test]
    fn mode_cache_refusals_recover_and_match_case_applies_to_every_pattern_kind() {
        let mut store = Bookmarks::default();
        store.add("Проект GTK", "/no-access-required");
        for mode in SearchMode::ALL {
            let q = match mode {
                SearchMode::Glob => "ПРО*",
                SearchMode::Regex => "^ПРО",
                _ => "ПРОЕКТ GTK",
            };
            assert_eq!(rank_options(&store, q, CaseMode::Ignore, mode).indices, [0]);
            assert_eq!(rank_options(&store, q, CaseMode::Smart, mode).total, 0);
            assert_eq!(rank_options(&store, q, CaseMode::Match, mode).total, 0);
        }
        let mut cache = Cache::default();
        assert!(cache.refresh_options(&store, "[", CaseMode::Ignore, SearchMode::Regex));
        assert!(cache.matches.error.is_some());
        assert!(!cache.refresh_options(&store, "[", CaseMode::Ignore, SearchMode::Regex));
        assert!(cache.refresh_options(&store, "[", CaseMode::Ignore, SearchMode::Literal));
        assert!(cache.matches.error.is_none());
        assert!(cache.refresh_options(&store, "Проект GTK", CaseMode::Ignore, SearchMode::Exact));
        assert_eq!(cache.matches.indices, [0]);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            store.add(
                "Other",
                std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/bad-\xff".to_vec())),
            );
            assert_eq!(
                rank_options(&store, "bad-", CaseMode::Ignore, SearchMode::Literal).total,
                0
            );
        }
    }
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
