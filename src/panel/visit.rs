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

struct PathTerms(Vec<String>);
impl PathTerms {
    fn parse(query: &str) -> Option<Self> {
        if query.len() > 1024 {
            return None;
        }
        let terms = query
            .split_whitespace()
            .take(33)
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        (terms.len() <= 32).then_some(Self(terms))
    }
    fn matches(&self, path: &Path) -> bool {
        let label = path
            .to_string_lossy()
            .to_lowercase()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let mut tail = label.as_str();
        for term in &self.0 {
            let Some(index) = tail.find(term.as_str()) else {
                return false;
            };
            tail = &tail[index + term.len()..];
        }
        self.0.last().is_none_or(|last| {
            label
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .contains(last.rsplit('/').next().unwrap_or_default())
        })
    }
}

pub fn recent_match_error(query: &str, mode: RecentMatchMode) -> Option<&'static str> {
    (mode == RecentMatchMode::OrderedTerms && PathTerms::parse(query).is_none())
        .then_some("Use at most 32 path terms and 1024 UTF-8 bytes")
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
