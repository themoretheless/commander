//! Pure recursive-find query: a conjunction of predicates over file entries.
//! No I/O and no UI, so matching is unit-tested directly. The walk that feeds
//! it lives in `workspace::run_find`.

use crate::panel::FileEntry;
use crate::selection_summary::{Kind, kind_of};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

const SECONDS_PER_DAY: u64 = 86_400;

/// One condition a found entry must satisfy.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Predicate {
    /// Name contains this substring (case-insensitive).
    NameContains(String),
    /// Entry classifies as this kind.
    Kind(Kind),
    /// At least this many bytes.
    MinSize(u64),
    /// Modified within this many days (unknown mtime never matches).
    MaxAgeDays(u64),
}

impl Predicate {
    fn matches(&self, e: &FileEntry, now: SystemTime) -> bool {
        match self {
            Predicate::NameContains(s) => e.name_lower.contains(&s.to_lowercase()),
            Predicate::Kind(k) => kind_of(e) == *k,
            Predicate::MinSize(min) => e.size >= *min,
            Predicate::MaxAgeDays(days) => match e.modified {
                Some(m) => now
                    .duration_since(m)
                    .map(|d| d.as_secs() <= days * SECONDS_PER_DAY)
                    .unwrap_or(false),
                None => false,
            },
        }
    }
}

/// A conjunction (AND) of predicates. An empty query matches everything.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Query {
    pub predicates: Vec<Predicate>,
}

impl Query {
    pub fn matches(&self, e: &FileEntry, now: SystemTime) -> bool {
        self.predicates.iter().all(|p| p.matches(e, now))
    }
}

/// Convert the active panel's lightweight filter controls into a reusable
/// smart-folder query.
pub fn from_panel_filter(search: &str, facets: &crate::panel::FacetSet) -> Query {
    let mut predicates = Vec::new();
    let trimmed = search.trim();
    if !trimmed.is_empty() {
        predicates.push(Predicate::NameContains(trimmed.to_string()));
    }
    if let Some(facet) = facets.kind {
        predicates.push(Predicate::Kind(kind_from_facet(facet)));
    }
    if let Some(min) = facets.min_size {
        predicates.push(Predicate::MinSize(min));
    }
    if let Some(days) = facets.max_age_days {
        predicates.push(Predicate::MaxAgeDays(days));
    }
    Query { predicates }
}

fn kind_from_facet(facet: crate::panel::KindFacet) -> Kind {
    match facet {
        crate::panel::KindFacet::Folders => Kind::Folder,
        crate::panel::KindFacet::Images => Kind::Image,
        crate::panel::KindFacet::Docs => Kind::Document,
        crate::panel::KindFacet::Archives => Kind::Archive,
        crate::panel::KindFacet::Code => Kind::Code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn entry(name: &str, is_dir: bool, size: u64, modified: Option<SystemTime>) -> FileEntry {
        let ext = PathBuf::from(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/x/{name}")),
            is_dir,
            size,
            extension: ext,
            modified,
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn q(preds: Vec<Predicate>) -> Query {
        Query { predicates: preds }
    }

    #[test]
    fn empty_query_matches_everything() {
        let now = SystemTime::now();
        assert!(q(vec![]).matches(&entry("a.txt", false, 1, None), now));
    }

    #[test]
    fn name_contains_is_case_insensitive() {
        let now = SystemTime::now();
        let p = q(vec![Predicate::NameContains("RE".into())]);
        assert!(p.matches(&entry("Report.txt", false, 1, None), now));
        assert!(!p.matches(&entry("notes.txt", false, 1, None), now));
    }

    #[test]
    fn kind_predicate_uses_classifier() {
        let now = SystemTime::now();
        let imgs = q(vec![Predicate::Kind(Kind::Image)]);
        assert!(imgs.matches(&entry("a.png", false, 1, None), now));
        assert!(!imgs.matches(&entry("a.rs", false, 1, None), now));
        let folders = q(vec![Predicate::Kind(Kind::Folder)]);
        assert!(folders.matches(&entry("dir", true, 0, None), now));
    }

    #[test]
    fn min_size_is_inclusive() {
        let now = SystemTime::now();
        let big = q(vec![Predicate::MinSize(1000)]);
        assert!(big.matches(&entry("a", false, 1000, None), now));
        assert!(!big.matches(&entry("a", false, 999, None), now));
    }

    #[test]
    fn max_age_days_window_and_unknown_mtime() {
        let now = SystemTime::now();
        let recent = now - Duration::from_secs(2 * SECONDS_PER_DAY);
        let old = now - Duration::from_secs(40 * SECONDS_PER_DAY);
        let within = q(vec![Predicate::MaxAgeDays(7)]);
        assert!(within.matches(&entry("a", false, 1, Some(recent)), now));
        assert!(!within.matches(&entry("a", false, 1, Some(old)), now));
        // Unknown mtime never matches an age predicate.
        assert!(!within.matches(&entry("a", false, 1, None), now));
    }

    #[test]
    fn predicates_are_anded() {
        let now = SystemTime::now();
        let p = q(vec![
            Predicate::NameContains("log".into()),
            Predicate::MinSize(500),
        ]);
        assert!(p.matches(&entry("server.log", false, 800, None), now));
        assert!(!p.matches(&entry("server.log", false, 100, None), now)); // too small
        assert!(!p.matches(&entry("readme.md", false, 800, None), now)); // wrong name
    }

    #[test]
    fn panel_filter_converts_to_smart_folder_query() {
        let facets = crate::panel::FacetSet {
            kind: Some(crate::panel::KindFacet::Images),
            min_size: Some(1 << 20),
            max_age_days: Some(7),
        };
        let query = from_panel_filter(" raw ", &facets);
        assert_eq!(
            query.predicates,
            vec![
                Predicate::NameContains("raw".into()),
                Predicate::Kind(Kind::Image),
                Predicate::MinSize(1 << 20),
                Predicate::MaxAgeDays(7),
            ]
        );
    }
}
