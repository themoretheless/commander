//! Streaming recursive search with cancellation, stable identities and match
//! explanations. The UI owns only a [`SearchRun`] receiver; disk walking and
//! bounded content reads stay off the frame thread.

use crate::panel::FileEntry;
use crate::query::{MatchMode, Predicate, Query, QueryError};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

const BATCH_SIZE: usize = 32;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const MAX_CONTENT_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_RESULT_CAP: usize = 10_000;
pub const HISTORY_CAP: usize = 50;

pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileIdentity {
    Native { volume: u64, file: u64 },
    Path(PathBuf),
}

impl FileIdentity {
    fn from_metadata(path: &Path, metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let volume = metadata.dev();
            let file = metadata.ino();
            if file != 0 {
                return Self::Native { volume, file };
            }
        }
        Self::Path(path.to_path_buf())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchComponent {
    pub field: &'static str,
    pub detail: String,
    pub score: i32,
    pub matched_ranges: Vec<(usize, usize)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchExplanation {
    pub total_score: i32,
    pub components: Vec<MatchComponent>,
}

impl MatchExplanation {
    pub fn summary(&self) -> String {
        self.components
            .iter()
            .map(|component| {
                if component.score == 0 {
                    component.detail.clone()
                } else {
                    format!("{} ({:+})", component.detail, component.score)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub entry: FileEntry,
    pub identity: FileIdentity,
    pub accessed: Option<SystemTime>,
    pub explanation: MatchExplanation,
}

impl SearchHit {
    pub fn relative_to<'a>(&'a self, root: &'a Path) -> &'a Path {
        self.entry
            .path
            .strip_prefix(root)
            .unwrap_or(&self.entry.path)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchSummary {
    pub scanned: usize,
    pub matched: usize,
    pub content_skipped: usize,
    pub elapsed: Duration,
    pub truncated: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug)]
pub enum SearchEvent {
    Batch {
        generation: u64,
        hits: Vec<SearchHit>,
        scanned: usize,
    },
    Progress {
        generation: u64,
        scanned: usize,
        matched: usize,
    },
    Complete {
        generation: u64,
        summary: SearchSummary,
    },
}

pub struct SearchRun {
    pub generation: u64,
    receiver: Receiver<SearchEvent>,
}

impl SearchRun {
    pub fn try_recv(&self) -> Result<SearchEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

/// Starts one active generation at a time. Starting a new run invalidates all
/// older workers; they observe the token during traversal and stop before
/// emitting more batches.
#[derive(Default)]
pub struct SearchEngine {
    next_generation: AtomicU64,
    active_generation: Arc<AtomicU64>,
}

impl SearchEngine {
    pub fn start(
        &self,
        root: PathBuf,
        query: Query,
        cap: usize,
        notify: Notify,
    ) -> Result<SearchRun, QueryError> {
        let compiled = CompiledQuery::new(query)?;
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.active_generation.store(generation, Ordering::Release);
        let active = Arc::clone(&self.active_generation);
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            run_worker(
                generation,
                active,
                root,
                compiled,
                cap.max(1),
                sender,
                notify,
            );
        });
        Ok(SearchRun {
            generation,
            receiver,
        })
    }

    pub fn cancel(&self) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.active_generation.store(generation, Ordering::Release);
    }

    #[cfg(test)]
    fn active_generation(&self) -> u64 {
        self.active_generation.load(Ordering::Acquire)
    }
}

struct CompiledQuery {
    predicates: Vec<CompiledPredicate>,
    requires_content: bool,
}

enum CompiledPredicate {
    Name(TextMatcher),
    Path(TextMatcher),
    Content(TextMatcher),
    Metadata(Predicate),
}

enum TextMatcher {
    Exact {
        original: String,
        regex: regex::Regex,
    },
    Fuzzy(String),
    Regex {
        original: String,
        regex: regex::Regex,
    },
}

struct TextMatch {
    score: i32,
    matched_ranges: Vec<(usize, usize)>,
    detail: String,
}

impl TextMatcher {
    fn new(mode: MatchMode, value: String) -> Result<Self, QueryError> {
        match mode {
            MatchMode::Exact => {
                let regex = regex::RegexBuilder::new(&regex::escape(&value))
                    .case_insensitive(true)
                    .build()
                    .expect("an escaped literal is always a valid regex");
                Ok(Self::Exact {
                    original: value,
                    regex,
                })
            }
            MatchMode::Fuzzy => Ok(Self::Fuzzy(value)),
            MatchMode::Regex => {
                let regex = regex::RegexBuilder::new(&value)
                    .case_insensitive(true)
                    .build()
                    .map_err(|error| QueryError {
                        token: value.clone(),
                        message: error.to_string(),
                    })?;
                Ok(Self::Regex {
                    original: value,
                    regex,
                })
            }
        }
    }

    fn evaluate(&self, candidate: &str) -> Option<TextMatch> {
        match self {
            TextMatcher::Exact { original, regex } => {
                let found = regex.find(candidate)?;
                let ranges = vec![byte_range_to_char_range(
                    candidate,
                    found.start(),
                    found.end(),
                )];
                let position = ranges[0].0.min(100) as i32;
                Some(TextMatch {
                    score: 120 - position,
                    matched_ranges: ranges,
                    detail: format!("literal {original:?}"),
                })
            }
            TextMatcher::Fuzzy(value) => {
                crate::fuzzy::score(value, candidate).map(|matched| TextMatch {
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                    detail: format!("fuzzy {value:?}"),
                })
            }
            TextMatcher::Regex { original, regex } => {
                regex.find(candidate).map(|found| TextMatch {
                    score: 100,
                    matched_ranges: vec![byte_range_to_char_range(
                        candidate,
                        found.start(),
                        found.end(),
                    )],
                    detail: format!("regex {original:?}"),
                })
            }
        }
    }
}

impl CompiledQuery {
    fn new(query: Query) -> Result<Self, QueryError> {
        query.validate()?;
        let requires_content = query.requires_content();
        let mut predicates = Vec::with_capacity(query.predicates.len());
        for predicate in query.predicates {
            let compiled = match predicate {
                Predicate::NameContains(value) => {
                    CompiledPredicate::Name(TextMatcher::new(query.mode, value)?)
                }
                Predicate::PathContains(value) => {
                    CompiledPredicate::Path(TextMatcher::new(query.mode, value)?)
                }
                Predicate::ContentContains(value) => {
                    CompiledPredicate::Content(TextMatcher::new(query.mode, value)?)
                }
                metadata => CompiledPredicate::Metadata(metadata),
            };
            predicates.push(compiled);
        }
        Ok(Self {
            predicates,
            requires_content,
        })
    }

    fn evaluate_metadata(&self, entry: &FileEntry, now: SystemTime) -> Option<MatchExplanation> {
        let mut explanation = MatchExplanation::default();
        for predicate in &self.predicates {
            match predicate {
                CompiledPredicate::Name(matcher) => {
                    let matched = matcher.evaluate(&entry.name)?;
                    explanation.total_score += matched.score;
                    explanation.components.push(MatchComponent {
                        field: "name",
                        detail: matched.detail,
                        score: matched.score,
                        matched_ranges: matched.matched_ranges,
                    });
                }
                CompiledPredicate::Path(matcher) => {
                    let path = entry.path.to_string_lossy();
                    let matched = matcher.evaluate(&path)?;
                    explanation.total_score += matched.score;
                    explanation.components.push(MatchComponent {
                        field: "path",
                        detail: matched.detail,
                        score: matched.score,
                        matched_ranges: matched.matched_ranges,
                    });
                }
                CompiledPredicate::Content(_) => {}
                CompiledPredicate::Metadata(predicate) => {
                    let query = Query {
                        predicates: vec![predicate.clone()],
                        mode: MatchMode::Exact,
                    };
                    if !query.matches_metadata(entry, now) {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: predicate.chip(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
            }
        }
        Some(explanation)
    }

    fn evaluate_content(
        &self,
        entry: &FileEntry,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        if !self.requires_content {
            return ContentVerdict::Matched;
        }
        if entry.is_dir {
            return ContentVerdict::NotMatched;
        }
        let Ok(file) = std::fs::File::open(&entry.path) else {
            return ContentVerdict::Skipped;
        };
        let mut bytes = Vec::with_capacity(entry.size.min(MAX_CONTENT_BYTES) as usize);
        if file
            .take(MAX_CONTENT_BYTES)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return ContentVerdict::Skipped;
        }
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            return ContentVerdict::Skipped;
        }
        let content = String::from_utf8_lossy(&bytes);
        for predicate in &self.predicates {
            if let CompiledPredicate::Content(matcher) = predicate {
                let Some(matched) = matcher.evaluate(&content) else {
                    return ContentVerdict::NotMatched;
                };
                explanation.total_score += matched.score;
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: if entry.size > MAX_CONTENT_BYTES {
                        format!("{}, first 16MB", matched.detail)
                    } else {
                        matched.detail
                    },
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                });
            }
        }
        ContentVerdict::Matched
    }
}

enum ContentVerdict {
    Matched,
    NotMatched,
    Skipped,
}

fn run_worker(
    generation: u64,
    active: Arc<AtomicU64>,
    root: PathBuf,
    query: CompiledQuery,
    cap: usize,
    sender: mpsc::Sender<SearchEvent>,
    notify: Notify,
) {
    let started = Instant::now();
    let now = SystemTime::now();
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut scanned = 0usize;
    let mut matched = 0usize;
    let mut content_skipped = 0usize;
    let mut truncated = false;
    let mut cancelled = false;
    let mut last_progress = Instant::now();

    for item in jwalk::WalkDir::new(&root)
        .skip_hidden(false)
        .into_iter()
        .flatten()
    {
        if active.load(Ordering::Acquire) != generation {
            cancelled = true;
            break;
        }
        let path = item.path();
        if path == root {
            continue;
        }
        scanned += 1;
        let Ok(metadata) = item.metadata() else {
            continue;
        };
        let identity = FileIdentity::from_metadata(&path, &metadata);
        let accessed = metadata.accessed().ok();
        let Some(entry) = FileEntry::from_meta(path, &metadata) else {
            continue;
        };
        let Some(mut explanation) = query.evaluate_metadata(&entry, now) else {
            if !maybe_send_progress(
                generation,
                scanned,
                matched,
                &mut last_progress,
                &sender,
                &notify,
            ) {
                return;
            }
            continue;
        };
        match query.evaluate_content(&entry, &mut explanation) {
            ContentVerdict::NotMatched => continue,
            ContentVerdict::Skipped => {
                content_skipped += 1;
                continue;
            }
            ContentVerdict::Matched => {}
        }
        batch.push(SearchHit {
            entry,
            identity,
            accessed,
            explanation,
        });
        matched += 1;
        if batch.len() >= BATCH_SIZE {
            sort_new_hits(&mut batch);
            let hits = std::mem::take(&mut batch);
            if sender
                .send(SearchEvent::Batch {
                    generation,
                    hits,
                    scanned,
                })
                .is_err()
            {
                return;
            }
            notify();
        }
        if matched >= cap {
            truncated = true;
            break;
        }
        if !maybe_send_progress(
            generation,
            scanned,
            matched,
            &mut last_progress,
            &sender,
            &notify,
        ) {
            return;
        }
    }

    if !batch.is_empty() && !cancelled {
        sort_new_hits(&mut batch);
        if sender
            .send(SearchEvent::Batch {
                generation,
                hits: batch,
                scanned,
            })
            .is_err()
        {
            return;
        }
        notify();
    }
    let summary = SearchSummary {
        scanned,
        matched,
        content_skipped,
        elapsed: started.elapsed(),
        truncated,
        cancelled,
    };
    let _ = sender.send(SearchEvent::Complete {
        generation,
        summary,
    });
    notify();
}

fn maybe_send_progress(
    generation: u64,
    scanned: usize,
    matched: usize,
    last_progress: &mut Instant,
    sender: &mpsc::Sender<SearchEvent>,
    notify: &Notify,
) -> bool {
    if last_progress.elapsed() < PROGRESS_INTERVAL {
        return true;
    }
    if sender
        .send(SearchEvent::Progress {
            generation,
            scanned,
            matched,
        })
        .is_err()
    {
        return false;
    }
    *last_progress = Instant::now();
    notify();
    true
}

fn byte_range_to_char_range(candidate: &str, start: usize, end: usize) -> (usize, usize) {
    let start_chars = candidate[..start].chars().count();
    let length_chars = candidate[start..end].chars().count();
    (start_chars, start_chars + length_chars)
}

fn sort_new_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.explanation
            .total_score
            .cmp(&a.explanation.total_score)
            .then_with(|| a.entry.path.cmp(&b.entry.path))
    });
}

/// Preserve identities already shown by the prior completed generation. New
/// identities follow in deterministic score/path order.
pub fn stabilize_hits(previous: &[FileIdentity], hits: &mut [SearchHit]) {
    let positions: HashMap<&FileIdentity, usize> = previous
        .iter()
        .enumerate()
        .map(|(index, identity)| (identity, index))
        .collect();
    hits.sort_by(|a, b| {
        let a_position = positions.get(&a.identity).copied();
        let b_position = positions.get(&b.identity).copied();
        match (a_position, b_position) {
            (Some(a_position), Some(b_position)) => a_position
                .cmp(&b_position)
                .then_with(|| a.entry.path.cmp(&b.entry.path)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => Reverse(a.explanation.total_score)
                .cmp(&Reverse(b.explanation.total_score))
                .then_with(|| a.entry.path.cmp(&b.entry.path)),
        }
    });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimePivot {
    #[default]
    None,
    Modified,
    Accessed,
}

impl TimePivot {
    pub const ALL: [Self; 3] = [Self::None, Self::Modified, Self::Accessed];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "List",
            Self::Modified => "Modified",
            Self::Accessed => "Accessed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeBucket {
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
    ThisYear,
    Older,
    Unknown,
}

impl TimeBucket {
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::ThisWeek => "This week",
            Self::ThisMonth => "This month",
            Self::ThisYear => "This year",
            Self::Older => "Older",
            Self::Unknown => "Unknown",
        }
    }
}

pub fn time_bucket(hit: &SearchHit, pivot: TimePivot, now: SystemTime) -> TimeBucket {
    let value = match pivot {
        TimePivot::None | TimePivot::Modified => hit.entry.modified,
        TimePivot::Accessed => hit.accessed,
    };
    let Some(value) = value else {
        return TimeBucket::Unknown;
    };
    let Ok(age) = now.duration_since(value) else {
        return TimeBucket::Today;
    };
    match age.as_secs() / 86_400 {
        0 => TimeBucket::Today,
        1 => TimeBucket::Yesterday,
        2..=6 => TimeBucket::ThisWeek,
        7..=30 => TimeBucket::ThisMonth,
        31..=365 => TimeBucket::ThisYear,
        _ => TimeBucket::Older,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub expression: String,
    pub mode: MatchMode,
    pub root: PathBuf,
    pub result_count: usize,
    pub scanned: usize,
    pub duration_ms: u64,
    pub last_run: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryHistory {
    pub entries: Vec<QueryHistoryEntry>,
    pub tick: u64,
}

impl QueryHistory {
    pub fn record(
        &mut self,
        expression: String,
        mode: MatchMode,
        root: PathBuf,
        summary: &SearchSummary,
    ) {
        self.tick = self.tick.saturating_add(1);
        self.entries.retain(|entry| {
            entry.expression != expression || entry.mode != mode || entry.root != root
        });
        self.entries.insert(
            0,
            QueryHistoryEntry {
                expression,
                mode,
                root,
                result_count: summary.matched,
                scanned: summary.scanned,
                duration_ms: summary.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                last_run: self.tick,
            },
        );
        self.entries.truncate(HISTORY_CAP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn file_entry(path: PathBuf, size: u64) -> FileEntry {
        FileEntry {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            name_lower: path.file_name().unwrap().to_string_lossy().to_lowercase(),
            extension: path
                .extension()
                .map(|extension| extension.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
            path,
            is_dir: false,
            size,
            modified: None,
            modified_str: "-".into(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn collect_run(run: &SearchRun) -> (Vec<SearchHit>, SearchSummary) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut hits = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "search timed out");
            match run.receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(SearchEvent::Batch { hits: batch, .. }) => hits.extend(batch),
                Ok(SearchEvent::Complete { summary, .. }) => return (hits, summary),
                Ok(SearchEvent::Progress { .. }) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("search channel closed: {error}"),
            }
        }
    }

    #[test]
    fn streaming_search_finds_metadata_and_content() {
        let tmp = TempDir::new();
        tmp.file("reports/annual.txt", "revenue grew");
        tmp.file("reports/notes.txt", "meeting notes");
        tmp.file("photo.jpg", "not really an image");
        let query =
            Query::parse("path:reports type:docs content:revenue", MatchMode::Exact).unwrap();
        let engine = SearchEngine::default();
        let run = engine
            .start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "annual.txt");
        assert!(hits[0].explanation.summary().contains("revenue"));
        assert_eq!(summary.matched, 1);
        assert!(!summary.cancelled);
    }

    #[test]
    fn first_batch_arrives_before_the_full_walk_completes() {
        let tmp = TempDir::new();
        for index in 0..80 {
            tmp.file(&format!("match-{index:03}.txt"), "x");
        }
        let engine = SearchEngine::default();
        let run = engine
            .start(
                tmp.path().to_path_buf(),
                Query::parse("match", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let first = run.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(first, SearchEvent::Batch { .. }));
        let (_, summary) = collect_run(&run);
        assert_eq!(summary.matched, 80);
    }

    #[test]
    fn stale_worker_reports_cancellation_before_scanning() {
        let tmp = TempDir::new();
        tmp.file("one.txt", "x");
        let active = Arc::new(AtomicU64::new(2));
        let (sender, receiver) = mpsc::channel();
        run_worker(
            1,
            active,
            tmp.path().to_path_buf(),
            CompiledQuery::new(Query::default()).unwrap(),
            100,
            sender,
            Arc::new(|| {}),
        );
        let event = receiver
            .into_iter()
            .find(|event| matches!(event, SearchEvent::Complete { .. }))
            .unwrap();
        let SearchEvent::Complete { summary, .. } = event else {
            unreachable!();
        };
        assert!(summary.cancelled);
        assert_eq!(summary.scanned, 0);
    }

    #[test]
    fn starting_a_generation_invalidates_the_previous_one() {
        let engine = SearchEngine::default();
        let root = TempDir::new();
        let first = engine
            .start(
                root.path().to_path_buf(),
                Query::default(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let second = engine
            .start(
                root.path().to_path_buf(),
                Query::default(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        assert!(second.generation > first.generation);
        assert_eq!(engine.active_generation(), second.generation);
    }

    #[test]
    fn stable_refresh_keeps_prior_identities_and_ranks_new_hits() {
        let mk = |name: &str, id: u64, score: i32| SearchHit {
            entry: file_entry(PathBuf::from(format!("/x/{name}")), 1),
            identity: FileIdentity::Native {
                volume: 1,
                file: id,
            },
            accessed: None,
            explanation: MatchExplanation {
                total_score: score,
                components: Vec::new(),
            },
        };
        let previous = vec![
            FileIdentity::Native { volume: 1, file: 2 },
            FileIdentity::Native { volume: 1, file: 1 },
        ];
        let mut hits = vec![mk("one", 1, 5), mk("new-low", 3, 1), mk("two", 2, 4)];
        stabilize_hits(&previous, &mut hits);
        let names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
        assert_eq!(names, vec!["two", "one", "new-low"]);
    }

    #[test]
    fn fuzzy_filename_ranking_has_a_golden_order() {
        let query = Query::parse("rpt", MatchMode::Fuzzy).unwrap();
        let compiled = CompiledQuery::new(query).unwrap();
        let now = SystemTime::now();
        let mut hits: Vec<_> = [
            "report.txt",
            "quarterly-report.txt",
            "rapid-test.txt",
            "project.txt",
        ]
        .into_iter()
        .filter_map(|name| {
            let entry = file_entry(PathBuf::from(format!("/fixture/{name}")), 1);
            let explanation = compiled.evaluate_metadata(&entry, now)?;
            Some(SearchHit {
                entry,
                identity: FileIdentity::Path(PathBuf::from(name)),
                accessed: None,
                explanation,
            })
        })
        .collect();
        sort_new_hits(&mut hits);
        let names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["rapid-test.txt", "report.txt", "quarterly-report.txt"]
        );
    }

    #[test]
    fn query_history_is_replayable_deduplicated_and_capped() {
        let mut history = QueryHistory::default();
        for index in 0..55 {
            history.record(
                format!("query-{index}"),
                MatchMode::Exact,
                PathBuf::from("/root"),
                &SearchSummary {
                    scanned: index,
                    matched: index / 2,
                    elapsed: Duration::from_millis(index as u64),
                    ..Default::default()
                },
            );
        }
        assert_eq!(history.entries.len(), HISTORY_CAP);
        history.record(
            "query-54".into(),
            MatchMode::Exact,
            PathBuf::from("/root"),
            &SearchSummary::default(),
        );
        assert_eq!(history.entries.len(), HISTORY_CAP);
        assert_eq!(history.entries[0].expression, "query-54");
    }

    #[test]
    fn time_pivot_has_stable_age_buckets() {
        let now = SystemTime::now();
        let mut hit = SearchHit {
            entry: file_entry(PathBuf::from("/x/a.txt"), 1),
            identity: FileIdentity::Path(PathBuf::from("/x/a.txt")),
            accessed: Some(now - Duration::from_secs(8 * 86_400)),
            explanation: MatchExplanation::default(),
        };
        hit.entry.modified = Some(now - Duration::from_secs(86_400));
        assert_eq!(
            time_bucket(&hit, TimePivot::Modified, now),
            TimeBucket::Yesterday
        );
        assert_eq!(
            time_bucket(&hit, TimePivot::Accessed, now),
            TimeBucket::ThisMonth
        );
    }

    #[test]
    #[ignore = "manual search benchmark harness"]
    fn search_benchmark_shallow_deep_many_small_huge_and_mixed() {
        let shapes = [
            ("shallow", 20_000usize, 1usize),
            ("deep", 20_000, 40),
            ("many-small", 50_000, 4),
            ("huge-file", 1, 1),
            ("mixed", 30_000, 8),
        ];
        let query = CompiledQuery::new(Query::parse("rpt", MatchMode::Fuzzy).unwrap()).unwrap();
        for (label, count, depth) in shapes {
            let started = Instant::now();
            let now = SystemTime::now();
            let mut matched = 0;
            for index in 0..count {
                let prefix = "nested/".repeat(depth);
                let name = if index % 17 == 0 {
                    format!("report-{index}.txt")
                } else {
                    format!("item-{index}.bin")
                };
                let entry = file_entry(
                    PathBuf::from(format!("/{prefix}{name}")),
                    if label == "huge-file" {
                        8_u64 << 30
                    } else {
                        64
                    },
                );
                matched += usize::from(query.evaluate_metadata(&entry, now).is_some());
            }
            eprintln!(
                "search-bench {label}: {count} entries, {matched} matches, {:?}",
                started.elapsed()
            );
        }
    }
}
