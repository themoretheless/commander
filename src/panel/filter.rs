//! Filtering: glob/substring search + quick facet chips (kind, size, age).
//! SRP small piece extracted from panel.rs so the matching rules are readable
//! in isolation, easy to test, and separate from PanelState lifecycle.
//! DRY: the pure predicates used by mask, search box, and ensure_filter_cache.

use std::time::SystemTime;

use super::FileEntry;

/// Glob match supporting `*` (any run) and `?` (one char). Inputs are
/// expected lowercased; matching is greedy with backtracking.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Parse a select-by-mask line into `(term, is_subtract)` pairs. Terms are
/// comma-separated; a leading `!` or `-` marks subtraction. Terms are
/// lowercased for case-insensitive matching.
pub fn parse_mask(mask: &str) -> Vec<(String, bool)> {
    mask.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(
            |t| match t.strip_prefix('!').or_else(|| t.strip_prefix('-')) {
                Some(rest) => (rest.trim().to_lowercase(), true),
                None => (t.to_lowercase(), false),
            },
        )
        .collect()
}

/// Whether a single lowercased `term` matches an entry. A term with no
/// wildcard and no `.` matches by extension ("jpg" selects all *.jpg);
/// otherwise it is globbed against the full name.
pub(crate) fn term_matches(term: &str, name_lower: &str, ext: &str) -> bool {
    let wild = term.contains('*') || term.contains('?');
    if !wild && !term.contains('.') {
        return ext == term;
    }
    glob_match(term, name_lower)
}

/// A category facet for the quick-filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindFacet {
    Folders,
    Images,
    Docs,
    Archives,
    Code,
}

/// Active quick-filter facets, ANDed with the substring filter. Default is
/// "no facets" (everything passes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FacetSet {
    pub kind: Option<KindFacet>,
    /// Minimum file size in bytes (folders are not filtered by size).
    pub min_size: Option<u64>,
    /// Maximum age in days by mtime (entries with unknown mtime fail this).
    pub max_age_days: Option<u64>,
}

impl FacetSet {
    pub fn is_empty(&self) -> bool {
        self.kind.is_none() && self.min_size.is_none() && self.max_age_days.is_none()
    }
}

/// Whether `entry` passes all active facets, relative to `now`. Pure.
pub fn facet_matches(entry: &FileEntry, facets: &FacetSet, now: SystemTime) -> bool {
    if let Some(kind) = facets.kind {
        let ok = match kind {
            KindFacet::Folders => entry.is_dir,
            KindFacet::Images => entry.is_image(),
            KindFacet::Docs => matches!(
                entry.extension.as_str(),
                "pdf" | "txt" | "md" | "rtf" | "doc" | "docx" | "pages" | "odt" | "tex"
            ),
            KindFacet::Archives => matches!(
                entry.extension.as_str(),
                "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz" | "zst"
            ),
            KindFacet::Code => matches!(
                entry.extension.as_str(),
                "rs" | "py"
                    | "js"
                    | "ts"
                    | "jsx"
                    | "tsx"
                    | "c"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "go"
                    | "java"
                    | "kt"
                    | "swift"
                    | "rb"
                    | "php"
                    | "sh"
                    | "toml"
                    | "json"
                    | "yaml"
                    | "yml"
            ),
        };
        if !ok {
            return false;
        }
    }
    if let Some(min) = facets.min_size {
        // Folders are not filtered by size (their byte size is 0 here).
        if !entry.is_dir && entry.size < min {
            return false;
        }
    }
    if let Some(days) = facets.max_age_days {
        let Some(modified) = entry.modified else {
            return false;
        };
        let cutoff = std::time::Duration::from_secs(days * 24 * 60 * 60);
        match now.duration_since(modified) {
            Ok(age) if age <= cutoff => {}
            Ok(_) => return false,
            // modified in the future: treat as fresh (passes).
            Err(_) => {}
        }
    }
    true
}

/// Cached filtered view: indices into `entries` matching `query` and facets.
/// Valid while `generation`, `query` and `facets` are unchanged.
pub(crate) struct FilterCache {
    pub generation: u64,
    pub query: String,
    pub facets: FacetSet,
    pub search_regex: bool,
    pub indices: Vec<usize>,
}

impl FilterCache {
    pub(crate) fn stale() -> Self {
        FilterCache {
            generation: u64::MAX, // sentinel: never computed
            query: String::new(),
            facets: FacetSet::default(),
            search_regex: false,
            indices: Vec::new(),
        }
    }
}

/// Rebuild the cached filtered indices if entries or query changed.
/// Moved here for SRP (filter concern).
pub(crate) fn ensure_filter_cache(panel: &super::PanelState) {
    let mut cache = panel.filter_cache.borrow_mut();
    if cache.generation == panel.entries_gen
        && cache.query == panel.search_query
        && cache.facets == panel.facets
        && cache.search_regex == panel.search_regex
    {
        return;
    }
    cache.generation = panel.entries_gen;
    cache.query = panel.search_query.clone();
    cache.facets = panel.facets;
    cache.search_regex = panel.search_regex;
    cache.indices.clear();

    let query = panel.search_query.as_str();
    let facets = panel.facets;
    let no_facets = facets.is_empty();
    let now = SystemTime::now();
    let is_regex = panel.search_regex;

    cache.indices.extend(
        panel.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if is_regex {
                    // Case-insensitive regex match (idea 91/74). On bad pattern, no match (graceful).
                    if let Ok(re) = regex::RegexBuilder::new(query).case_insensitive(true).build() {
                        re.is_match(&e.name)
                    } else {
                        false
                    }
                } else {
                    crate::fuzzy::is_match(query, &e.name)
                }
            })
            .filter(|(_, e)| no_facets || facet_matches(e, &facets, now))
            .map(|(i, _)| i),
    );
}