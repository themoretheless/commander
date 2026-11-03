use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::lock_util::recover;

/// Session-wide most-recent-first list of visited directories (for the
/// Cmd+P quick switcher), distinct from each panel's linear history.
fn visited_log() -> &'static Mutex<Vec<PathBuf>> {
    static V: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(Vec::new()))
}

fn visit_stats() -> &'static Mutex<VisitStats> {
    static STATS: OnceLock<Mutex<VisitStats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(VisitStats::default()))
}

pub const VISITED_CAP: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitUsage {
    pub count: u32,
    pub last: u64,
}

/// Persisted frequency and recency information for the `Cmd+P` destination
/// switcher. Paths remain in a separate ordered list so chronological mode is
/// exact and old session files can default this field independently.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitStats {
    pub uses: HashMap<PathBuf, VisitUsage>,
    pub tick: u64,
}

impl VisitStats {
    pub fn record(&mut self, path: &Path) {
        self.tick = self.tick.saturating_add(1);
        let usage = self.uses.entry(path.to_path_buf()).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last = self.tick;
    }

    fn usage(&self, path: &Path) -> VisitUsage {
        self.uses.get(path).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecentOrder {
    #[default]
    Frecency,
    Chronological,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecentMatchMode {
    #[default]
    Fuzzy,
    OrderedTerms,
}

struct PathTerms {
    groups: Vec<Vec<PathAtom>>,
}
struct PathAtom {
    term: PathTerm,
    inverse: bool,
}
struct PathTerm {
    text: String,
    start: bool,
    end: bool,
}
impl PathTerm {
    fn parse(raw: &str) -> Option<Self> {
        let (text, start) = if let Some(rest) = raw.strip_prefix(r"\!") {
            (format!("!{rest}"), false)
        } else if let Some(rest) = raw.strip_prefix(r"\|") {
            (format!("|{rest}"), false)
        } else if let Some(rest) = raw.strip_prefix(r"\^") {
            (format!("^{rest}"), false)
        } else if let Some(rest) = raw.strip_prefix('^') {
            (rest.to_owned(), true)
        } else {
            (raw.to_owned(), false)
        };
        let (text, end) = if let Some(rest) = text.strip_suffix(r"\$") {
            (format!("{rest}$"), false)
        } else if let Some(rest) = text.strip_suffix('$') {
            (rest.to_owned(), true)
        } else {
            (text, false)
        };
        (!text.is_empty()).then(|| Self {
            text: text.to_lowercase(),
            start,
            end,
        })
    }
    fn find_in(&self, prefix: &str, full_len: usize) -> Option<usize> {
        let index = if self.start {
            prefix.starts_with(&self.text).then_some(0)
        } else {
            prefix.rfind(&self.text)
        }?;
        (!self.end || index + self.text.len() == full_len).then_some(index)
    }
}
impl PathTerms {
    fn parse(query: &str) -> Option<Self> {
        if query.len() > 1024 {
            return None;
        }
        // Only escaped whitespace is consumed here; operator escapes remain for PathTerm.
        let mut terms = Vec::new();
        let mut token = String::new();
        let mut chars = query.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' && chars.peek().is_some_and(|next| next.is_whitespace()) {
                token.push(chars.next().expect("peeked whitespace"));
            } else if ch.is_whitespace() {
                if !token.is_empty() {
                    terms.push(std::mem::take(&mut token));
                    if terms.len() > 63 {
                        return None;
                    }
                }
            } else {
                token.push(ch);
            }
        }
        if !token.is_empty() {
            terms.push(token);
        }
        let mut groups: Vec<Vec<PathAtom>> = Vec::new();
        let mut alternatives = false;
        let mut count = 0;
        for raw in terms {
            if raw == "|" {
                if groups.is_empty() || alternatives {
                    return None;
                }
                alternatives = true;
                continue;
            }
            count += 1;
            if count > 32 {
                return None;
            }
            let (text, inverse) = raw
                .strip_prefix('!')
                .map_or((raw.as_str(), false), |text| (text, true));
            let atom = PathAtom {
                term: PathTerm::parse(text)?,
                inverse,
            };
            if alternatives {
                groups.last_mut()?.push(atom);
            } else {
                groups.push(vec![atom]);
            }
            alternatives = false;
        }
        (!alternatives).then_some(Self { groups })
    }

    fn matches(&self, path: &Path) -> bool {
        let label = path
            .to_string_lossy()
            .to_lowercase()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let normalized = label.trim_end_matches('/');
        let full = if normalized.is_empty() {
            label.as_str()
        } else {
            normalized
        };
        // One frontier has not consumed any positive term, the other has.
        // The greatest remaining prefix dominates smaller prefixes in the
        // same state: earlier ordered terms can use every match they could.
        // This avoids expanding the Cartesian product of OR alternatives.
        let mut frontier = [Some(full.len()), None];
        for group in self.groups.iter().rev() {
            let mut next: [Option<usize>; 2] = [None, None];
            for (state, bound) in frontier.into_iter().enumerate() {
                let Some(bound) = bound else {
                    continue;
                };
                for atom in group {
                    if atom.inverse {
                        let hay = if atom.term.start || atom.term.end {
                            full
                        } else {
                            label.as_str()
                        };
                        if atom.term.find_in(hay, hay.len()).is_none() {
                            next[state] = Some(next[state].map_or(bound, |old| old.max(bound)));
                        }
                    } else if let Some(index) = atom.term.find_in(&full[..bound], full.len()) {
                        if state == 0 && full[index + atom.term.text.len()..].contains('/') {
                            continue;
                        }
                        next[1] = Some(next[1].map_or(index, |old| old.max(index)));
                    }
                }
            }
            frontier = next;
            if frontier.iter().all(Option::is_none) {
                return false;
            }
        }
        frontier.iter().any(Option::is_some)
    }
}

/// Component-aware lexical scope over remembered paths; never probes or resolves links.
pub fn recent_paths_within(paths: &[PathBuf], base: &Path) -> Vec<PathBuf> {
    let safe = |path: &Path| {
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
    };
    if !safe(base) {
        return Vec::new();
    }
    paths
        .iter()
        .filter(|path| safe(path) && path.starts_with(base))
        .cloned()
        .collect()
}

pub fn recent_match_error(query: &str, mode: RecentMatchMode) -> Option<&'static str> {
    (mode == RecentMatchMode::OrderedTerms && PathTerms::parse(query).is_none())
        .then_some(r"Use at most 32 path terms and 1024 UTF-8 bytes; !, ^ and $ need text; | needs alternatives on both sides (escape literal operators with \)")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentMatch {
    pub path: PathBuf,
    pub count: u32,
    pub last: u64,
    pub score: i64,
}

/// Push `path` to the front of `list`, de-duplicating and capping. Pure, so
/// the ordering logic is unit-testable without the global.
pub fn push_visit(list: &mut Vec<PathBuf>, path: &Path, cap: usize) {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(cap);
}

/// Record a visit to `path` in `list` and `stats`, dropping usage for paths
/// that fell off the capped list so the persisted stats stay bounded. Pure,
/// so the pruning is unit-testable without the globals.
pub(super) fn record_visit_into(
    list: &mut Vec<PathBuf>,
    stats: &mut VisitStats,
    path: &Path,
    cap: usize,
) {
    push_visit(list, path, cap);
    stats.record(path);
    if stats.uses.len() > list.len() {
        stats.uses.retain(|used, _| list.contains(used));
    }
}

/// Record a visit to `path` in the global recent list.
pub fn record_visit(path: &Path) {
    let mut log = recover(visited_log());
    let mut stats = recover(visit_stats());
    record_visit_into(&mut log, &mut stats, path, VISITED_CAP);
}

/// Forget one exact destination and its ranking metadata. A future visit can
/// record it again; unrelated destinations and their recency ticks are kept.
fn forget_visit_into(list: &mut Vec<PathBuf>, stats: &mut VisitStats, path: &Path) -> bool {
    let before = list.len();
    list.retain(|entry| entry != path);
    let removed_usage = stats.uses.remove(path).is_some();
    before != list.len() || removed_usage
}

pub fn forget_visit(path: &Path) -> bool {
    let mut log = recover(visited_log());
    let mut stats = recover(visit_stats());
    forget_visit_into(&mut log, &mut stats, path)
}

/// Snapshot of recently visited directories, most recent first.
pub fn visited_paths() -> Vec<PathBuf> {
    recover(visited_log()).clone()
}

pub fn visit_snapshot() -> (Vec<PathBuf>, VisitStats) {
    (visited_paths(), recover(visit_stats()).clone())
}

pub fn restore_visit_snapshot(paths: &[PathBuf], stats: &VisitStats) {
    *recover(visited_log()) = paths.iter().take(VISITED_CAP).cloned().collect();
    let mut current = recover(visit_stats());
    *current = stats.clone();
    current.uses.retain(|path, _| paths.contains(path));
}

/// Rank recent destinations by a bounded frequency/recency score. The fuzzy
/// component only affects a non-empty query; chronological mode preserves the
/// exact most-recent-first ordering of `paths`.
pub fn rank_visited(
    paths: &[PathBuf],
    query: &str,
    order: RecentOrder,
    stats: &VisitStats,
) -> Vec<RecentMatch> {
    rank_visited_with_mode(paths, query, order, stats, RecentMatchMode::Fuzzy)
}

pub fn rank_visited_with_mode(
    paths: &[PathBuf],
    query: &str,
    order: RecentOrder,
    stats: &VisitStats,
    mode: RecentMatchMode,
) -> Vec<RecentMatch> {
    let terms = if mode == RecentMatchMode::OrderedTerms {
        let Some(terms) = PathTerms::parse(query) else {
            return Vec::new();
        };
        Some(terms)
    } else {
        None
    };
    let query = query.trim();
    let mut matches: Vec<(usize, RecentMatch)> = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            let label = path.to_string_lossy();
            let fuzzy = if let Some(terms) = &terms {
                if !terms.matches(path) {
                    return None;
                }
                0
            } else if query.is_empty() {
                0
            } else {
                crate::fuzzy::score(query, &label)?.score as i64
            };
            let usage = stats.usage(path);
            let age = stats.tick.saturating_sub(usage.last).min(64) as i64;
            let recency = 64 - age;
            let frequency = i64::from(usage.count.min(32)) * 4;
            Some((
                index,
                RecentMatch {
                    path: path.clone(),
                    count: usage.count,
                    last: usage.last,
                    score: fuzzy + recency + frequency,
                },
            ))
        })
        .collect();

    if order == RecentOrder::Frecency {
        matches.sort_by(|(index_a, a), (index_b, b)| {
            b.score
                .cmp(&a.score)
                .then(b.last.cmp(&a.last))
                .then(index_a.cmp(index_b))
        });
    }
    matches.into_iter().map(|(_, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_term_alternatives_keep_group_precedence_order_and_inverse_choices() {
        for (query, path, expected) in [
            ("^/core go$ | rb$ | py$", "/core/go", true),
            ("^/core go$ | rb$ | py$", "/core/py", true),
            ("^/core go$ | rb$ | py$", "/other/py", false),
            ("work src$ | tests$", "/work/tests", true),
            ("work src$ | tests$", "/other/tests", false),
            ("work src$ | tests$", "/work/src/nested", false),
            ("work | other src$", "/other/src", true),
            ("work | other src$", "/other/tests", false),
            ("src$ | !src$", "/work/src", true),
            ("src$ | !src$", "/work/tests", true),
            ("work src | !src$", "/work/tests", false),
            ("work src | !src$", "/work/src", true),
            ("work | !missing src$", "/other/src", true),
            ("work | !missing src$", "/missing/src", false),
            ("work src$ | tests$ !archive", "/work/archive/tests", false),
            ("ёж$ | src$", "/work/ЁЖ", true),
            (r"work \|", "/work/|", true),
            ("work a|b", "/work/a|b", true),
            (r"work !\|", "/work/|", false),
            (r"work\ alpha src$ | tests$", "/work alpha/tests/", true),
        ] {
            assert_eq!(
                PathTerms::parse(query).unwrap().matches(Path::new(path)),
                expected,
                "{query} :: {path}"
            );
        }
        for query in [
            "|",
            "| src",
            "src |",
            "src | | tests",
            "src | !",
            "src | ^$",
        ] {
            assert!(PathTerms::parse(query).is_none(), "{query}");
            assert!(recent_match_error(query, RecentMatchMode::OrderedTerms).is_some());
        }
        assert!(PathTerms::parse(&vec!["src"; 32].join(" | ")).is_some());
        assert!(PathTerms::parse(&vec!["src"; 33].join(" | ")).is_none());
    }

    #[test]
    fn alternative_frontiers_equal_exhaustive_branch_expansion_and_preserve_rank() {
        let groups = [
            vec!["a", "bb", "!missing"],
            vec!["a$", "bb$", "!a$"],
            vec!["^/", "!bb", "bb$"],
        ];
        let mut paths = Vec::new();
        for parent in ["a", "bb", "missing", "work"] {
            for name in ["a", "bb", "abb", "aa", "missing"] {
                paths.push(PathBuf::from(format!("/{parent}/{name}")));
            }
        }
        // Evaluate each product branch as a pre-existing conjunction, rather
        // than using the two-state OR implementation as its own oracle.
        for mask in 1..8 {
            let selected: Vec<_> = groups
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, g)| g)
                .collect();
            let query = selected
                .iter()
                .map(|g| g.join(" | "))
                .collect::<Vec<_>>()
                .join(" ");
            let mut branches = vec![String::new()];
            for group in selected {
                branches = branches
                    .into_iter()
                    .flat_map(|prefix| group.iter().map(move |term| format!("{prefix} {term}")))
                    .collect();
            }
            let matcher = PathTerms::parse(&query).unwrap();
            for path in &paths {
                assert_eq!(
                    matcher.matches(path),
                    branches
                        .iter()
                        .any(|branch| PathTerms::parse(branch).unwrap().matches(path)),
                    "{query} :: {}",
                    path.display()
                );
            }
        }
        let mut stats = VisitStats::default();
        for path in &paths {
            stats.record(path);
        }
        let before = stats.clone();
        for order in [RecentOrder::Chronological, RecentOrder::Frecency] {
            let all =
                rank_visited_with_mode(&paths, "", order, &stats, RecentMatchMode::OrderedTerms);
            let selected = rank_visited_with_mode(
                &paths,
                "a$ | bb$",
                order,
                &stats,
                RecentMatchMode::OrderedTerms,
            );
            let expected: Vec<_> = all
                .into_iter()
                .filter(|entry| {
                    entry
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|name| name.ends_with('a') || name.ends_with("bb"))
                })
                .map(|entry| entry.path)
                .collect();
            assert_eq!(
                selected
                    .into_iter()
                    .map(|entry| entry.path)
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert_eq!(stats, before);
    }

    #[test]
    fn recent_path_terms_accept_escaped_whitespace_with_anchors_and_exclusions() {
        for (query, path, expected) in [
            (r"project\ alpha src$", "/work/project alpha/src", true),
            (r"project\ alpha src$", "/work/project/alpha/src", false),
            (r"src !old\ project", "/work/old project/src", false),
            (r"^/my\ work src$", "/my work/src", true),
            (r"src !old\ project$", "/work/old project/src", true),
            (r"\!my\ folder$", "/work/!my folder", true),
            (r"name\ ", "/work/name ", true),
            (r"name\ $", "/work/name ", true),
            (r"name\ $", "/work/name", false),
            (r"name\ \ ", "/work/name  ", true),
            ("ёж\\\tкафе$", "/work/ёж\tкафе", true),
            ("ёж\\\u{2003}кафе$", "/work/ёж\u{2003}кафе", true),
        ] {
            assert_eq!(
                PathTerms::parse(query).unwrap().matches(Path::new(path)),
                expected,
                "{query} :: {path}"
            );
        }
        assert_eq!(
            PathTerms::parse(r"folder\").unwrap().groups[0][0].term.text,
            r"folder\"
        );
        assert!(PathTerms::parse(&vec![r"a\ b"; 32].join(" ")).is_some());
        assert!(PathTerms::parse(&vec![r"a\ b"; 33].join(" ")).is_none());
        assert!(PathTerms::parse(&format!(r"{}\ ", "x".repeat(1023))).is_none());
    }

    #[test]
    fn anchored_recent_terms_bind_full_path_edges_and_preserve_order() {
        let cases = [
            ("^/work src$", "/work/src", true),
            ("^/work src$", "/other/work/src", false),
            ("^/work src$", "/work/src-old", false),
            ("^/work/src$", "/work/src", true),
            ("^/work/src$", "/work/src/nested", false),
            ("^/WORK ЁЖ$", "/work/ёж", true),
            ("work src !^/archive", "/work/src", true),
            ("src !^/work", "/work/src", false),
            ("src !src$", "/work/src", false),
            ("src !src$", "/work/src/", false),
            ("src$", "/work/src/", true),
            ("src !src/", "/work/src/", false),
            ("src !src$", "/work/src-old", true),
            (r"\^literal$", "/work/^literal", true),
            (r"literal\$", "/work/literal$", true),
            (r"\!literal$", "/work/!literal", true),
            (r"literal !\!literal$", "/work/!literal", false),
            ("work$ src", "/work/src", false),
            ("src$ work", "/work/src", false),
        ];
        for (query, path, expected) in cases {
            assert_eq!(
                PathTerms::parse(query).unwrap().matches(Path::new(path)),
                expected,
                "{query} :: {path}"
            );
        }
        for query in ["^", "$", "^$", "!^", "!$", "!^$"] {
            assert!(PathTerms::parse(query).is_none(), "{query}");
            assert!(recent_match_error(query, RecentMatchMode::OrderedTerms).is_some());
        }
        assert!(PathTerms::parse(&vec!["^x$"; 33].join(" ")).is_none());
    }

    #[test]
    fn recent_path_exclusions_filter_full_paths_without_changing_order_or_stats() {
        let paths = vec![
            PathBuf::from("/work/archive/src"),
            PathBuf::from("/work/current/SRC"),
            PathBuf::from("/work/ёж/src"),
            PathBuf::from("/work/!literal"),
        ];
        let mut stats = VisitStats::default();
        for path in &paths {
            stats.record(path);
        }
        let before = stats.clone();
        let ordered = RecentMatchMode::OrderedTerms;
        let matches = |query: &str| {
            rank_visited_with_mode(&paths, query, RecentOrder::Chronological, &stats, ordered)
                .into_iter()
                .map(|r| r.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(matches("work src !ARCHIVE !ёж"), [paths[1].clone()]);
        assert_eq!(matches("!archive work !ёж src"), [paths[1].clone()]);
        assert_eq!(matches("!src"), [paths[3].clone()]);
        assert_eq!(matches(r"\!literal"), [paths[3].clone()]);
        assert!(matches("!!").iter().all(|p| p != &paths[3]));
        assert!(matches("src !work").is_empty());
        assert!(matches("src work !missing").is_empty());
        assert!(recent_match_error("!", ordered).is_some());
        assert!(matches("!").is_empty());
        assert!(PathTerms::parse(&vec!["!x"; 32].join(" ")).is_some());
        assert!(PathTerms::parse(&vec!["!x"; 33].join(" ")).is_none());
        assert!(PathTerms::parse(&format!("!{}", "é".repeat(512))).is_none());
        assert!(recent_match_error("!", RecentMatchMode::Fuzzy).is_none());
        assert_eq!(stats, before);
    }

    #[test]
    fn recent_base_scope_uses_components_and_composes_with_matching_and_ordering() {
        let paths = [
            "/work/project",
            "/work/project/child",
            "/work/project-old/child",
            "/work/project/../other",
            "/other/child",
            "relative/child",
        ]
        .map(PathBuf::from)
        .to_vec();
        let mut stats = VisitStats::default();
        for path in &paths {
            stats.record(path);
        }
        let before = stats.clone();
        let scoped = recent_paths_within(&paths, Path::new("/work/project"));
        assert_eq!(scoped, paths[..2]);
        for mode in [RecentMatchMode::Fuzzy, RecentMatchMode::OrderedTerms] {
            for order in [RecentOrder::Frecency, RecentOrder::Chronological] {
                let matches = rank_visited_with_mode(&scoped, "child", order, &stats, mode);
                assert_eq!(matches.len(), 1);
                assert_eq!(matches[0].path, paths[1]);
            }
        }
        assert!(recent_paths_within(&paths, Path::new("relative")).is_empty());
        assert!(recent_paths_within(&paths, Path::new("/work/project/..")).is_empty());
        assert_eq!(recent_paths_within(&paths, Path::new("/")).len(), 4);
        assert_eq!(stats, before);
    }

    #[cfg(unix)]
    #[test]
    fn recent_base_scope_retains_exact_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;
        let base = PathBuf::from(std::ffi::OsString::from_vec(b"/work/\xff".to_vec()));
        let child = base.join("child");
        let lookalike = PathBuf::from(std::ffi::OsString::from_vec(b"/work/\xfe/child".to_vec()));
        assert_eq!(
            recent_paths_within(&[child.clone(), lookalike], &base),
            [child]
        );
    }

    #[test]
    fn ordered_path_terms_match_documented_examples_unicode_and_boundaries() {
        for (query, path, expected) in [
            ("foo", "/FOO", true),
            ("fo ba", "/foo/bar", true),
            ("fo ba", "/bar/foo", false),
            ("fo / ba", "/foo/bar", true),
            ("fo / ba", "/foobar", false),
            ("bar", "/foo/bar", true),
            ("bar", "/foo/bar/", true),
            ("foo/bar", "/foo/bar/", true),
            ("/", "/", true),
            ("bar", "/bar/foo", false),
            ("foo/bar", "/foo/bar", true),
            ("foo/bar", "/foo/bar/baz", false),
            ("foo/bar", "/foo/bar/other/bar", false),
            ("foo/", "/foo/bar", true),
            ("foo/", "/foo/bar/baz", false),
            ("foo /", "/foo/bar/baz", true),
            ("/foo/ /bar", "/foo/bar", false),
            ("/foo/ /bar", "/foo/baz/bar", true),
            ("foo /", "/foo/child", true),
            ("foo foo", "/foo", false),
            ("  ПРОЕКТ  ДоК  ", "/работа/проект/Документы", true),
            ("fbr", "/foo/bar", false),
            ("", "/any", true),
        ] {
            assert_eq!(
                PathTerms::parse(query).unwrap().matches(Path::new(path)),
                expected,
                "{query:?} {path}"
            );
        }
    }

    #[test]
    fn ordered_terms_preserve_order_rank_identity_and_refuse_oversized_queries() {
        let paths = vec![
            PathBuf::from("/foo/bar"),
            PathBuf::from("/foo/barn"),
            PathBuf::from("/bar/foo"),
        ];
        let mut stats = VisitStats::default();
        stats.record(&paths[0]);
        for _ in 0..8 {
            stats.record(&paths[1]);
        }
        let before = stats.clone();
        let recent = rank_visited_with_mode(
            &paths,
            "fo ba",
            RecentOrder::Chronological,
            &stats,
            RecentMatchMode::OrderedTerms,
        );
        assert_eq!(
            recent.iter().map(|row| &row.path).collect::<Vec<_>>(),
            [&paths[0], &paths[1]]
        );
        let ranked = rank_visited_with_mode(
            &paths,
            "fo ba",
            RecentOrder::Frecency,
            &stats,
            RecentMatchMode::OrderedTerms,
        );
        assert_eq!(
            ranked.iter().map(|row| &row.path).collect::<Vec<_>>(),
            [&paths[1], &paths[0]]
        );
        assert_eq!(stats, before);
        for query in ["x".repeat(1025), "é".repeat(513), vec!["x"; 33].join(" ")] {
            assert!(recent_match_error(&query, RecentMatchMode::OrderedTerms).is_some());
            assert!(
                rank_visited_with_mode(
                    &paths,
                    &query,
                    RecentOrder::Frecency,
                    &stats,
                    RecentMatchMode::OrderedTerms
                )
                .is_empty()
            );
            assert!(recent_match_error(&query, RecentMatchMode::Fuzzy).is_none());
        }
        assert!(PathTerms::parse(&"é".repeat(512)).is_some());
        assert!(PathTerms::parse(&vec!["x"; 32].join(" ")).is_some());
    }

    #[test]
    fn recent_order_changes_projection_without_mutating_visits_or_scores() {
        let newest = PathBuf::from("/project/newest");
        let frequent = PathBuf::from("/project/frequent");
        let paths = vec![newest.clone(), frequent.clone()];
        let stats = VisitStats {
            uses: HashMap::from([
                (newest.clone(), VisitUsage { count: 1, last: 10 }),
                (frequent.clone(), VisitUsage { count: 10, last: 9 }),
            ]),
            tick: 10,
        };
        let before = stats.clone();
        for query in ["", "project"] {
            let ranked = rank_visited(&paths, query, RecentOrder::Frecency, &stats);
            assert_eq!(
                ranked.iter().map(|row| &row.path).collect::<Vec<_>>(),
                [&frequent, &newest]
            );
            let recent = rank_visited(&paths, query, RecentOrder::Chronological, &stats);
            assert_eq!(
                recent.iter().map(|row| &row.path).collect::<Vec<_>>(),
                [&newest, &frequent]
            );
        }
        assert_eq!(stats, before);
        assert_eq!(paths, [newest, frequent]);
    }

    #[test]
    fn forgetting_recent_path_removes_rank_preserves_peers_and_survives_roundtrip() {
        let a = PathBuf::from("/work/alpha");
        let b = PathBuf::from("/work/beta");
        let mut paths = vec![];
        let mut stats = VisitStats::default();
        for path in [&a, &b, &a] {
            record_visit_into(&mut paths, &mut stats, path, 200);
        }
        let peer = stats.uses[&b].clone();
        let tick = stats.tick;
        assert!(forget_visit_into(&mut paths, &mut stats, &a));
        assert_eq!(paths.as_slice(), std::slice::from_ref(&b));
        assert_eq!(stats.uses[&b], peer);
        assert_eq!(stats.tick, tick);
        assert!(!stats.uses.contains_key(&a));
        assert!(!forget_visit_into(&mut paths, &mut stats, &a));
        let encoded = serde_json::to_string(&(paths.clone(), stats.clone())).unwrap();
        let (restored, restored_stats): (Vec<PathBuf>, VisitStats) =
            serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            rank_visited(&restored, "alpha", RecentOrder::Frecency, &restored_stats),
            vec![]
        );
        record_visit_into(&mut paths, &mut stats, &a, 200);
        assert_eq!(stats.uses[&a].count, 1);
        assert_eq!(stats.uses[&a].last, tick + 1);
    }

    #[cfg(unix)]
    #[test]
    fn forgetting_recent_path_uses_exact_non_utf8_identity() {
        use std::os::unix::ffi::OsStringExt;
        let a = PathBuf::from(std::ffi::OsString::from_vec(b"/work/\xff".to_vec()));
        let b = PathBuf::from(std::ffi::OsString::from_vec(b"/work/\xfe".to_vec()));
        let mut paths = vec![a.clone(), b.clone(), a.clone()];
        let mut stats = VisitStats::default();
        stats.record(&a);
        stats.record(&b);
        assert!(forget_visit_into(&mut paths, &mut stats, &a));
        assert_eq!(paths.as_slice(), std::slice::from_ref(&b));
        assert_eq!(stats.uses.len(), 1);
        assert!(stats.uses.contains_key(&b));
    }

    #[test]
    fn a_poisoned_visit_log_keeps_recording() {
        let _ = std::thread::spawn(|| {
            let _log = visited_log().lock();
            let _stats = visit_stats().lock();
            panic!("poison the visit globals");
        })
        .join();
        assert!(visited_log().is_poisoned() && visit_stats().is_poisoned());

        let path = Path::new("/visit-poison-test/recorded");
        record_visit(path);
        let (paths, stats) = visit_snapshot();
        assert!(paths.contains(&path.to_path_buf()));
        assert!(stats.uses.contains_key(path));
        assert!(visited_paths().contains(&path.to_path_buf()));
    }

    fn usage(count: u32, last: u64) -> VisitUsage {
        VisitUsage { count, last }
    }

    #[test]
    fn whitespace_only_query_filters_nothing() {
        let paths = vec![PathBuf::from("/alpha"), PathBuf::from("/beta")];
        let ranked = rank_visited(
            &paths,
            "   ",
            RecentOrder::Chronological,
            &VisitStats::default(),
        );
        let ranked: Vec<_> = ranked.into_iter().map(|item| item.path).collect();
        assert_eq!(ranked, paths);
    }

    #[test]
    fn frecency_frequency_is_capped_at_thirty_two_visits() {
        let fewer = PathBuf::from("/fewer");
        let many = PathBuf::from("/many");
        let stats = VisitStats {
            uses: HashMap::from([(fewer.clone(), usage(32, 5)), (many.clone(), usage(100, 5))]),
            tick: 5,
        };
        // Past the cap, more visits no longer outrank list order.
        let ranked = rank_visited(
            &[fewer.clone(), many.clone()],
            "",
            RecentOrder::Frecency,
            &stats,
        );
        assert_eq!(ranked[0].score, ranked[1].score);
        assert_eq!(ranked[0].path, fewer);
        assert_eq!(ranked[1].count, 100);
    }

    #[test]
    fn frecency_ties_go_to_the_more_recent_visit_then_list_order() {
        let older = PathBuf::from("/older");
        let newer = PathBuf::from("/newer");
        let unused = PathBuf::from("/unused");
        // Both visits are more than 64 ticks old, so recency is capped and
        // the scores tie.
        let stats = VisitStats {
            uses: HashMap::from([(older.clone(), usage(1, 10)), (newer.clone(), usage(1, 20))]),
            tick: 200,
        };
        let ranked = rank_visited(
            &[older.clone(), newer.clone()],
            "",
            RecentOrder::Frecency,
            &stats,
        );
        assert_eq!(ranked[0].score, ranked[1].score);
        assert_eq!(ranked[0].path, newer);

        let second = PathBuf::from("/second");
        let ranked = rank_visited(
            &[unused.clone(), second.clone()],
            "",
            RecentOrder::Frecency,
            &VisitStats::default(),
        );
        let order: Vec<_> = ranked.into_iter().map(|item| item.path).collect();
        assert_eq!(order, [unused, second]);
    }
}
