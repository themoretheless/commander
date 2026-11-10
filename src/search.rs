//! Streaming recursive search with cancellation, stable identities and match
//! explanations. The UI owns only a [`SearchRun`] receiver; disk walking and
//! bounded content reads stay off the frame thread.

use crate::panel::FileEntry;
use crate::ports::SearchProvider;
use crate::query::{MatchMode, Predicate, Query, QueryError};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

mod providers;
use providers::*;
mod compiled_query;
#[cfg(test)]
pub use compiled_query::query_matches_metadata;
use compiled_query::*;
mod worker;
use worker::*;

const BATCH_SIZE: usize = 32;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
const MAX_CONTENT_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_RESULT_CAP: usize = 10_000;
pub const HISTORY_CAP: usize = 50;

pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileIdentity {
    Native {
        volume: u64,
        file: u64,
    },
    Archive {
        archive: PathBuf,
        member: PathBuf,
        index: usize,
    },
    Path(PathBuf),
}

fn entry_identity_parts(metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
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

    fn from_indexed(document: &crate::content_index::IndexedDocument) -> Self {
        match (document.volume, document.file_id) {
            (Some(volume), Some(file)) if file != 0 => Self::Native { volume, file },
            _ => Self::Path(document.path.clone()),
        }
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
    pub location: SearchLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SearchLocation {
    Native(PathBuf),
    ArchiveMember {
        archive: PathBuf,
        member: PathBuf,
        index: usize,
    },
}

impl SearchHit {
    pub fn relative_to<'a>(&'a self, root: &'a Path) -> &'a Path {
        self.entry
            .path
            .strip_prefix(root)
            .unwrap_or(&self.entry.path)
    }

    pub fn reveal_path(&self) -> &Path {
        match &self.location {
            SearchLocation::Native(path) => path,
            SearchLocation::ArchiveMember { archive, .. } => archive,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchSummary {
    pub scanned: usize,
    pub matched: usize,
    /// Entries that could not be searched: binary or unreadable files,
    /// folders the walk could not read, and archives that would not open.
    pub content_skipped: usize,
    /// Why the first archive that would not open was skipped (corrupt,
    /// encrypted, unreadable), so the count above has a reason to show.
    pub archive_error: Option<String>,
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
    pub provider: &'static str,
    pub snapshot: crate::workload::TaskSnapshot,
    receiver: Receiver<SearchEvent>,
    task: crate::workload::TaskHandle,
}

impl SearchRun {
    pub fn try_recv(&self) -> Result<SearchEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for SearchRun {
    fn drop(&mut self) {
        self.task.cancel();
    }
}

pub struct ProviderRequest {
    generation: u64,
    active: Arc<AtomicU64>,
    scheduler_cancel: crate::workload::CancellationToken,
    requires_content: bool,
    requires_empty: bool,
    requires_broken_symlink: bool,
    max_depth: Option<usize>,
    exclusions: Arc<Vec<regex::Regex>>,
    directory_exclusions: Arc<Vec<crate::query::DirectoryMask>>,
    search_archives: bool,
    archive_password: Option<zeroize::Zeroizing<String>>,
    hidden_mode: crate::query::HiddenMode,
    /// Folders and archives a provider could not read; reported with
    /// `content_skipped` so a partial walk does not look complete.
    unsearched: Cell<usize>,
    /// The per-search archive budget stopped archive scanning early.
    archives_capped: Cell<bool>,
    /// The first archive open error of this search.
    archive_error: RefCell<Option<String>>,
}

impl ProviderRequest {
    fn cancelled(&self) -> bool {
        self.scheduler_cancel.is_cancelled()
            || self.active.load(Ordering::Acquire) != self.generation
    }

    fn note_unsearched(&self) {
        self.unsearched.set(self.unsearched.get().saturating_add(1));
    }

    fn note_archive_error(&self, error: &str) {
        self.archive_error
            .borrow_mut()
            .get_or_insert_with(|| error.to_string());
    }

    fn note_archive_budget(&self, budget: &crate::archive::SearchBudget) {
        if budget.truncated {
            self.archives_capped.set(true);
        }
    }
}

enum CandidateContent {
    OnDisk,
    Indexed {
        index: Arc<crate::content_index::RootIndex>,
        document: usize,
    },
    Inline(Option<Arc<str>>),
}

pub struct SearchCandidate {
    entry: FileEntry,
    is_symlink: Option<bool>,
    is_executable: Option<bool>,
    is_regular: Option<bool>,
    unix_mode: Option<u32>,
    owner_uid: Option<u64>,
    owner_gid: Option<u64>,
    device_id: Option<u64>,
    inode: Option<u64>,
    file_type_bits: Option<u32>,
    is_empty: Option<bool>,
    is_broken_symlink: Option<bool>,
    identity: FileIdentity,
    accessed: Option<SystemTime>,
    content: CandidateContent,
    location: SearchLocation,
}

pub type ProviderRecord = Option<SearchCandidate>;

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
        self.start_with_archive_password(root, query, cap, notify, None)
    }

    pub fn start_indexed(
        &self,
        index: Arc<crate::content_index::RootIndex>,
        query: Query,
        cap: usize,
        notify: Notify,
    ) -> Result<SearchRun, QueryError> {
        self.start_indexed_with_archive_password(index, query, cap, notify, None)
    }

    pub fn start_with_archive_password(
        &self,
        root: PathBuf,
        query: Query,
        cap: usize,
        notify: Notify,
        password: Option<zeroize::Zeroizing<String>>,
    ) -> Result<SearchRun, QueryError> {
        self.start_provider(
            query,
            cap,
            notify,
            Box::new(FilesystemProvider::new(root)),
            password,
        )
    }
    pub fn start_indexed_with_archive_password(
        &self,
        index: Arc<crate::content_index::RootIndex>,
        query: Query,
        cap: usize,
        notify: Notify,
        password: Option<zeroize::Zeroizing<String>>,
    ) -> Result<SearchRun, QueryError> {
        self.start_provider(
            query,
            cap,
            notify,
            Box::new(IndexedProvider { index }),
            password,
        )
    }

    fn start_provider(
        &self,
        query: Query,
        cap: usize,
        notify: Notify,
        provider: Box<dyn SearchProvider>,
        archive_password: Option<zeroize::Zeroizing<String>>,
    ) -> Result<SearchRun, QueryError> {
        let compiled = CompiledQuery::new(query)?;
        let provider_id = provider.id();
        let provider_label = match (provider_id, compiled.search_archives) {
            ("live-search", false) => "Live (archives off)",
            ("indexed-search", false) => "Index (archives off)",
            _ => provider.label(),
        };
        let provider_root = provider.root().to_path_buf();
        let capability = if provider_id == "indexed-search" {
            crate::provider_runtime::ProviderCapability::SearchIndex
        } else {
            crate::provider_runtime::ProviderCapability::SearchLive
        };
        if let Err(reason) = crate::provider_runtime::activate_builtin_explained(
            provider_id,
            &crate::provider_runtime::ActivationRequest {
                capability,
                root: &provider_root,
                extension: None,
                bytes: None,
            },
        ) {
            return Err(QueryError {
                token: provider_id.to_string(),
                message: format!("{provider_label} search could not start: {reason}"),
            });
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.active_generation.store(generation, Ordering::Release);
        let active = Arc::clone(&self.active_generation);
        let (sender, receiver) = mpsc::channel();
        let sender = Arc::new(std::sync::Mutex::new(Some(sender)));
        let on_abandoned = abandonment_hook(Arc::clone(&sender), Arc::clone(&notify));
        let task = crate::workload::submit_with_abandonment(
            crate::workload::TaskSpec::new(
                crate::workload::TaskKind::Search,
                provider_root,
                generation,
            )
            .priority(crate::workload::Priority::Interactive)
            .estimated_bytes(MAX_CONTENT_BYTES)
            .replace_older_generation(),
            move |scheduler_cancel| {
                let Some(sender) = crate::lock_util::recover(&sender).take() else {
                    return;
                };
                run_worker(
                    ProviderRequest {
                        generation,
                        active,
                        scheduler_cancel,
                        requires_content: compiled.requires_content,
                        requires_empty: compiled.requires_empty,
                        requires_broken_symlink: compiled.requires_broken_symlink,
                        max_depth: compiled.max_depth,
                        exclusions: Arc::clone(&compiled.exclusions),
                        directory_exclusions: Arc::clone(&compiled.directory_exclusions),
                        search_archives: compiled.search_archives,
                        archive_password,
                        hidden_mode: compiled.hidden_mode,
                        unsearched: Cell::new(0),
                        archives_capped: Cell::new(false),
                        archive_error: RefCell::new(None),
                    },
                    compiled,
                    cap.max(1),
                    sender,
                    notify,
                    provider,
                );
            },
            on_abandoned,
        )
        .map_err(|error| QueryError {
            token: provider_id.to_string(),
            message: error.to_string(),
        })?;
        Ok(SearchRun {
            provider: provider_label,
            snapshot: task.snapshot().clone(),
            receiver,
            task,
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

type SharedSender = Arc<std::sync::Mutex<Option<mpsc::Sender<SearchEvent>>>>;

/// Runs when the scheduler drops a queued search without starting it (root
/// disconnected, worker spawn failed). Closing the channel first and then
/// waking the UI lets Find see the run end and say so, instead of showing
/// "Searching" until some unrelated input repaints it.
fn abandonment_hook(
    sender: SharedSender,
    notify: Notify,
) -> impl FnOnce(crate::workload::AbandonReason) + Send + 'static {
    move |reason| {
        drop(crate::lock_util::recover(&sender).take());
        log::info!(target: "commander::search", "search abandoned before it started: {reason}");
        notify();
    }
}

/// Direct children are depth one; archive members add their virtual path components.
fn search_depth(path: &Path, root: &Path) -> Option<usize> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.components().count());
    }
    let mut marker = root.as_os_str().to_owned();
    marker.push("!");
    path.strip_prefix(PathBuf::from(marker))
        .ok()
        .map(|relative| relative.components().count())
}

fn dot_name(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().first() == Some(&b'.')
}

/// Dot components below the explicitly chosen root, including virtual members.
fn dot_hidden_path(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative.components().any(
            |component| matches!(component, std::path::Component::Normal(name) if dot_name(name)),
        )
    })
}

/// Apply basename exclusions to the entry and every ancestor below the search root.
fn excluded_path(path: &Path, root: &Path, exclusions: &[regex::Regex]) -> bool {
    if exclusions.is_empty() {
        return false;
    }
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        exclusions.iter().any(|pattern| pattern.is_match(&name))
    })
}

/// Ancestors are directories in the provider's lexical scope; only a directory
/// leaf is excluded. Archive callers pass member paths, never virtual containers.
fn excluded_directory_path(
    path: &Path,
    root: &Path,
    is_dir: bool,
    patterns: &[crate::query::DirectoryMask],
) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut directory = relative.to_path_buf();
    if !is_dir {
        directory.pop();
    }
    while !directory.as_os_str().is_empty() {
        let full_path = root.join(&directory);
        if patterns
            .iter()
            .any(|pattern| pattern.matches_directory(&full_path, root))
        {
            return true;
        }
        directory.pop();
    }
    false
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
    Created,
    Changed,
}

impl TimePivot {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Modified,
        Self::Accessed,
        Self::Created,
        Self::Changed,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "List",
            Self::Modified => "Modified",
            Self::Accessed => "Accessed",
            Self::Created => "Created",
            Self::Changed => "Metadata changed",
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
    pub const fn label(self) -> &'static str {
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
        TimePivot::Created if matches!(hit.location, SearchLocation::Native(_)) => {
            hit.entry.created
        }
        TimePivot::Changed if matches!(hit.location, SearchLocation::Native(_)) => {
            hit.entry.changed
        }
        TimePivot::Created | TimePivot::Changed => None,
    };
    let Some(value) = value else {
        return TimeBucket::Unknown;
    };
    if value > now {
        return TimeBucket::Today;
    }
    // Count local calendar days, not 24-hour windows, so a file changed at
    // 23:00 yesterday is "Yesterday" at 08:00 today.
    // chrono's `From<SystemTime>` panics outside its date range, so convert
    // through the checked constructor; out-of-range times are simply old.
    let (Some(now_date), Some(value_date)) = (local_date(now), local_date(value)) else {
        return TimeBucket::Older;
    };
    let days = now_date.signed_duration_since(value_date).num_days();
    match days {
        ..=0 => TimeBucket::Today,
        1 => TimeBucket::Yesterday,
        2..=6 => TimeBucket::ThisWeek,
        7..=30 => TimeBucket::ThisMonth,
        31..=365 => TimeBucket::ThisYear,
        _ => TimeBucket::Older,
    }
}

fn local_date(time: SystemTime) -> Option<chrono::NaiveDate> {
    local_datetime(time).map(|local| local.date_naive())
}

/// Placeholder for a date the listing cannot show, as in the live panel.
const UNKNOWN_MODIFIED: &str = "\u{2013}";

/// Listing-style "modified" text for `time`, or a dash when chrono cannot
/// represent it (an mtime past ~year 262k): `From<SystemTime>` panics there.
pub fn format_modified(time: SystemTime) -> String {
    local_datetime(time).map_or_else(
        || UNKNOWN_MODIFIED.to_string(),
        |local| local.format("%d %b %y  %H:%M").to_string(),
    )
}

/// `format_modified` for an optional time; a missing time shows the dash.
pub fn modified_label(time: Option<SystemTime>) -> String {
    time.map_or_else(|| UNKNOWN_MODIFIED.to_string(), format_modified)
}

/// A listing row for something that is not stat-ed on disk (an index
/// document or an archive member), with the live panel's placeholders.
/// `None` when `path` has no file name.
pub fn synthetic_entry(
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    modified_str: String,
) -> Option<FileEntry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    Some(FileEntry {
        symlink: None,
        permissions: None,
        allocated_size: None,
        hard_links: None,
        owner_uid: None,
        owner_gid: None,
        accessed: None,
        created: None,
        accessed_str: "Unavailable".into(),
        created_str: "Unavailable".into(),
        changed: None,
        changed_str: "Unavailable".into(),
        name_lower: name.to_lowercase(),
        sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
        name,
        path,
        identity: crate::panel::ListingIdentity::Unavailable,
        is_dir,
        size: if is_dir { 0 } else { size },
        extension,
        modified,
        modified_str,
        size_str: if is_dir {
            "\u{2026}".to_string()
        } else {
            crate::panel::format_size(size)
        },
    })
}

fn local_datetime(time: SystemTime) -> Option<chrono::DateTime<chrono::Local>> {
    let (secs, nanos) = match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => (i64::try_from(after.as_secs()).ok()?, after.subsec_nanos()),
        Err(error) => {
            let before = error.duration();
            let secs = i64::try_from(before.as_secs()).ok()?;
            if before.subsec_nanos() == 0 {
                (secs.checked_neg()?, 0)
            } else {
                (
                    secs.checked_neg()?.checked_sub(1)?,
                    1_000_000_000 - before.subsec_nanos(),
                )
            }
        }
    };
    let utc = chrono::DateTime::from_timestamp(secs, nanos)?;
    Some(utc.with_timezone(&chrono::Local))
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
mod tests;
