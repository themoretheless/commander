//! Streaming recursive search with cancellation, stable identities and match
//! explanations. The UI owns only a [`SearchRun`] receiver; disk walking and
//! bounded content reads stay off the frame thread.

use crate::panel::FileEntry;
use crate::ports::SearchProvider;
use crate::query::{MatchMode, Predicate, Query, QueryError};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Folders and archives a provider could not read; reported with
    /// `content_skipped` so a partial walk does not look complete.
    unsearched: Cell<usize>,
    /// The per-search archive budget stopped archive scanning early.
    archives_capped: Cell<bool>,
}

impl ProviderRequest {
    fn cancelled(&self) -> bool {
        self.scheduler_cancel.is_cancelled()
            || self.active.load(Ordering::Acquire) != self.generation
    }

    fn note_unsearched(&self) {
        self.unsearched.set(self.unsearched.get().saturating_add(1));
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
    identity: FileIdentity,
    accessed: Option<SystemTime>,
    content: CandidateContent,
    location: SearchLocation,
}

pub type ProviderRecord = Option<SearchCandidate>;

struct FilesystemProvider {
    root: PathBuf,
}

impl FilesystemProvider {
    const fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl SearchProvider for FilesystemProvider {
    fn id(&self) -> &'static str {
        "live-search"
    }

    fn label(&self) -> &'static str {
        "Live + ZIP"
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut archive_budget = crate::archive::SearchBudget::default();
        for item in jwalk::WalkDir::new(&self.root).skip_hidden(false) {
            if request.cancelled() {
                break;
            }
            let item = match item {
                Ok(item) => item,
                // A folder the walk could not read: its children are missing
                // from the results, so count it instead of dropping it.
                Err(_) => {
                    request.note_unsearched();
                    if !emit(None) {
                        break;
                    }
                    continue;
                }
            };
            let path = item.path();
            if path == self.root {
                continue;
            }
            let Ok(metadata) = item.metadata() else {
                if !emit(None) {
                    break;
                }
                continue;
            };
            let identity = FileIdentity::from_metadata(&path, &metadata);
            let accessed = metadata.accessed().ok();
            let Some(entry) = FileEntry::from_meta(path, &metadata) else {
                if !emit(None) {
                    break;
                }
                continue;
            };
            let archive_path = (!entry.is_dir
                && crate::archive::is_supported(&entry.path)
                && crate::trust::allows_auto_archive_inspect(&entry.path))
            .then(|| entry.path.clone());
            if !emit(Some(SearchCandidate {
                location: SearchLocation::Native(entry.path.clone()),
                entry,
                identity,
                accessed,
                content: CandidateContent::OnDisk,
            })) {
                break;
            }
            if let Some(archive_path) = archive_path
                && !ArchiveProvider::visit_path(&archive_path, request, emit, &mut archive_budget)
            {
                break;
            }
        }
        request.note_archive_budget(&archive_budget);
    }
}

struct ArchiveProvider {
    archive: PathBuf,
}

impl ArchiveProvider {
    fn visit_path(
        archive_path: &Path,
        request: &ProviderRequest,
        emit: &mut dyn FnMut(ProviderRecord) -> bool,
        budget: &mut crate::archive::SearchBudget,
    ) -> bool {
        if !crate::trust::allows_auto_archive_inspect(archive_path) {
            return true;
        }
        let metadata = match std::fs::metadata(archive_path) {
            Ok(metadata) => metadata,
            Err(_) => return emit(None),
        };
        let modified = metadata.modified().ok();
        let accessed = metadata.accessed().ok();
        // Every member shows the archive's own mtime; format it once.
        let modified_str = modified_label(modified);
        let mut keep_going = true;
        let result = crate::archive::visit_members(
            archive_path,
            request.requires_content,
            budget,
            || request.cancelled(),
            |member| {
                // The virtual path ends in the member's own name.
                if member.member.path.file_name().is_none() {
                    keep_going = emit(None);
                    return keep_going;
                }
                let virtual_path =
                    crate::archive::virtual_member_path(archive_path, &member.member.path);
                let Some(entry) = synthetic_entry(
                    virtual_path,
                    member.member.is_dir,
                    member.member.size,
                    modified,
                    modified_str.clone(),
                ) else {
                    keep_going = emit(None);
                    return keep_going;
                };
                keep_going = emit(Some(SearchCandidate {
                    entry,
                    identity: FileIdentity::Archive {
                        archive: archive_path.to_path_buf(),
                        member: member.member.path.clone(),
                        index: member.member.index,
                    },
                    accessed,
                    content: CandidateContent::Inline(member.content),
                    location: SearchLocation::ArchiveMember {
                        archive: archive_path.to_path_buf(),
                        member: member.member.path,
                        index: member.member.index,
                    },
                }));
                keep_going
            },
        );
        match result {
            // Content search does not open this format: its members were
            // skipped, so count it rather than drop them silently.
            Ok(summary) if summary.members_not_searched => request.note_unsearched(),
            Ok(_) => {}
            Err(error) => {
                // A corrupt or encrypted archive: its members are not searched.
                request.note_unsearched();
                log::info!(target: "commander::search", "archive not searched: {error}");
                if keep_going {
                    keep_going = emit(None);
                }
            }
        }
        keep_going && !request.cancelled()
    }
}

impl SearchProvider for ArchiveProvider {
    fn id(&self) -> &'static str {
        "archive-search"
    }

    fn label(&self) -> &'static str {
        "ZIP"
    }

    fn root(&self) -> &Path {
        &self.archive
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut budget = crate::archive::SearchBudget::default();
        Self::visit_path(&self.archive, request, emit, &mut budget);
        request.note_archive_budget(&budget);
    }
}

struct IndexedProvider {
    index: Arc<crate::content_index::RootIndex>,
}

impl SearchProvider for IndexedProvider {
    fn id(&self) -> &'static str {
        "indexed-search"
    }

    fn label(&self) -> &'static str {
        "Index + ZIP"
    }

    fn root(&self) -> &Path {
        &self.index.root
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut archive_budget = crate::archive::SearchBudget::default();
        for (document, indexed) in self.index.documents.iter().enumerate() {
            if request.cancelled() {
                break;
            }
            let Some(entry) = indexed.entry() else {
                if !emit(None) {
                    break;
                }
                continue;
            };
            let archive_path = (!entry.is_dir
                && crate::archive::is_supported(&entry.path)
                && crate::trust::allows_auto_archive_inspect(&entry.path))
            .then(|| entry.path.clone());
            if !emit(Some(SearchCandidate {
                location: SearchLocation::Native(entry.path.clone()),
                entry,
                identity: FileIdentity::from_indexed(indexed),
                accessed: indexed.accessed(),
                content: CandidateContent::Indexed {
                    index: Arc::clone(&self.index),
                    document,
                },
            })) {
                break;
            }
            if let Some(archive_path) = archive_path
                && !ArchiveProvider::visit_path(&archive_path, request, emit, &mut archive_budget)
            {
                break;
            }
        }
        request.note_archive_budget(&archive_budget);
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
        self.start_provider(query, cap, notify, Box::new(FilesystemProvider::new(root)))
    }

    pub fn start_indexed(
        &self,
        index: Arc<crate::content_index::RootIndex>,
        query: Query,
        cap: usize,
        notify: Notify,
    ) -> Result<SearchRun, QueryError> {
        self.start_provider(query, cap, notify, Box::new(IndexedProvider { index }))
    }

    fn start_provider(
        &self,
        query: Query,
        cap: usize,
        notify: Notify,
        provider: Box<dyn SearchProvider>,
    ) -> Result<SearchRun, QueryError> {
        let compiled = CompiledQuery::new(query)?;
        let provider_id = provider.id();
        let provider_label = provider.label();
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
                        unsearched: Cell::new(0),
                        archives_capped: Cell::new(false),
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
                // An escaped literal always parses, but a huge pasted token
                // can still exceed the compiled-size limit.
                let regex = regex::RegexBuilder::new(&regex::escape(&value))
                    .case_insensitive(true)
                    .build()
                    .map_err(|error| QueryError {
                        token: value.clone(),
                        message: error.to_string(),
                    })?;
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
            Self::Exact { original, regex } => {
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
            Self::Fuzzy(value) => crate::fuzzy::score(value, candidate).map(|matched| TextMatch {
                score: matched.score,
                matched_ranges: matched.matched_ranges,
                detail: format!("fuzzy {value:?}"),
            }),
            Self::Regex { original, regex } => regex.find(candidate).map(|found| TextMatch {
                score: 100,
                matched_ranges: vec![byte_range_to_char_range(
                    candidate,
                    found.start(),
                    found.end(),
                )],
                detail: format!("regex {original:?}"),
            }),
        }
    }
}

/// Whether `entry` passes `query`'s metadata stage (everything but content),
/// matching `path:` against the full path.
#[cfg(test)]
pub fn query_matches_metadata(query: &Query, entry: &FileEntry, now: SystemTime) -> bool {
    CompiledQuery::new(query.clone()).is_ok_and(|compiled| {
        compiled
            .evaluate_metadata(entry, Path::new(""), now)
            .is_some()
    })
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

    /// `root` is the search root: `path:` matches the path below it, as the
    /// Find list shows it, so terms in the root itself do not match every hit.
    fn evaluate_metadata(
        &self,
        entry: &FileEntry,
        root: &Path,
        now: SystemTime,
    ) -> Option<MatchExplanation> {
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
                    let path = entry
                        .path
                        .strip_prefix(root)
                        .unwrap_or(&entry.path)
                        .to_string_lossy();
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
                    if !predicate.matches_metadata(entry, now) {
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
        source: &CandidateContent,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        if !self.requires_content {
            return ContentVerdict::Matched;
        }
        if entry.is_dir {
            return ContentVerdict::NotMatched;
        }
        match source {
            CandidateContent::OnDisk => {
                let Some((file, size)) = open_regular_file(&entry.path) else {
                    return ContentVerdict::Skipped;
                };
                let Some(bytes) = read_text_bytes(file, size) else {
                    return ContentVerdict::Skipped;
                };
                let content = String::from_utf8_lossy(&bytes);
                // The opened file's size, not the listing's: for a symlink
                // the listing holds the link's own (lstat) length.
                self.evaluate_content_text(&content, size > MAX_CONTENT_BYTES, explanation)
            }
            CandidateContent::Indexed { index, document } => {
                let Some(content) = index
                    .documents
                    .get(*document)
                    .and_then(|document| document.content.as_deref())
                else {
                    return ContentVerdict::Skipped;
                };
                self.evaluate_content_text(content, false, explanation)
            }
            CandidateContent::Inline(content) => {
                let Some(content) = content.as_deref() else {
                    return ContentVerdict::Skipped;
                };
                self.evaluate_content_text(content, false, explanation)
            }
        }
    }

    fn evaluate_content_text(
        &self,
        content: &str,
        partial: bool,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        for predicate in &self.predicates {
            if let CompiledPredicate::Content(matcher) = predicate {
                let Some(matched) = matcher.evaluate(content) else {
                    return ContentVerdict::NotMatched;
                };
                explanation.total_score += matched.score;
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: if partial {
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

/// Leading bytes checked for NULs to tell binary files from text.
const BINARY_SNIFF_BYTES: u64 = 8_192;

/// Up to `MAX_CONTENT_BYTES` of a text file, or `None` for a binary file or
/// a read error. The head is sniffed before the rest is read, so a binary
/// file costs one small read instead of up to 16 MB.
fn read_text_bytes(reader: impl Read, size: u64) -> Option<Vec<u8>> {
    let mut reader = reader.take(MAX_CONTENT_BYTES);
    let mut bytes = Vec::with_capacity(usize::try_from(size.min(BINARY_SNIFF_BYTES)).ok()?);
    Read::by_ref(&mut reader)
        .take(BINARY_SNIFF_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    bytes.reserve(
        usize::try_from(size.min(MAX_CONTENT_BYTES))
            .unwrap_or(0)
            .saturating_sub(bytes.len()),
    );
    reader.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Open `path` for a content read only if it is a regular file, returning the
/// handle and its size. FIFOs, sockets and devices are refused: opening a
/// FIFO for reading blocks until a writer appears, which would hang the
/// search worker. The path is stat-ed first so devices are never opened;
/// unix then opens non-blocking (and without acquiring a controlling tty) and
/// re-checks the type on the handle itself.
fn open_regular_file(path: &Path) -> Option<(std::fs::File, u64)> {
    // Refuse devices, FIFOs and sockets before opening: opening a tty, serial
    // or tape device can have side effects. The fstat below covers the race
    // where the path is swapped after this check.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    metadata.is_file().then_some((file, metadata.len()))
}

enum ContentVerdict {
    Matched,
    NotMatched,
    Skipped,
}

fn run_worker(
    request: ProviderRequest,
    query: CompiledQuery,
    cap: usize,
    sender: mpsc::Sender<SearchEvent>,
    notify: Notify,
    mut provider: Box<dyn SearchProvider>,
) {
    let generation = request.generation;
    let started = Instant::now();
    let now = SystemTime::now();
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut scanned = 0usize;
    let mut matched = 0usize;
    let mut content_skipped = 0usize;
    let mut truncated = false;
    let mut cancelled = false;
    let mut channel_open = true;
    let mut last_progress = Instant::now();
    let root = provider.root().to_path_buf();

    provider.visit(&request, &mut |record| {
        if request.cancelled() {
            cancelled = true;
            return false;
        }
        scanned += 1;
        let hit = record.and_then(|candidate| {
            evaluate_candidate(&query, candidate, &root, now, &mut content_skipped)
        });
        if let Some(hit) = hit {
            // Only a match beyond the cap proves the results are truncated; a
            // walk that ends with exactly `cap` matches is complete.
            if matched >= cap {
                truncated = true;
                return false;
            }
            batch.push(hit);
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
                    channel_open = false;
                    return false;
                }
                notify();
            }
        }
        channel_open = maybe_send_progress(
            generation,
            scanned,
            matched,
            &mut last_progress,
            &sender,
            &notify,
        );
        channel_open
    });

    if !channel_open {
        return;
    }
    if request.cancelled() {
        cancelled = true;
    }
    content_skipped = content_skipped.saturating_add(request.unsearched.get());
    // Archives left unscanned mean the results are incomplete, like a hit cap.
    truncated |= request.archives_capped.get();

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

/// The hit for one provider record, or `None` when its metadata or content
/// does not match. A candidate whose content could not be searched counts
/// in `content_skipped`.
fn evaluate_candidate(
    query: &CompiledQuery,
    candidate: SearchCandidate,
    root: &Path,
    now: SystemTime,
    content_skipped: &mut usize,
) -> Option<SearchHit> {
    let SearchCandidate {
        entry,
        identity,
        accessed,
        content,
        location,
    } = candidate;
    let mut explanation = query.evaluate_metadata(&entry, root, now)?;
    match query.evaluate_content(&entry, &content, &mut explanation) {
        ContentVerdict::NotMatched => None,
        ContentVerdict::Skipped => {
            *content_skipped += 1;
            None
        }
        ContentVerdict::Matched => Some(SearchHit {
            entry,
            identity,
            accessed,
            explanation,
            location,
        }),
    }
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

    pub const fn label(self) -> &'static str {
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
        permissions: None,
        name_lower: name.to_lowercase(),
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
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::io::Write;

    fn file_entry(path: PathBuf, size: u64) -> FileEntry {
        FileEntry {
            permissions: None,
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            name_lower: path.file_name().unwrap().to_string_lossy().to_lowercase(),
            extension: path
                .extension()
                .map(|extension| extension.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
            path,
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size,
            modified: None,
            modified_str: "-".into(),
            size_str: crate::panel::format_size(size),
        }
    }

    #[test]
    fn content_read_stops_after_the_head_of_a_binary_file() {
        struct Counting<'a>(&'a [u8], &'a std::cell::Cell<usize>);
        impl Read for Counting<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let read = self.0.read(buf)?;
                self.1.set(self.1.get() + read);
                Ok(read)
            }
        }
        let read = std::cell::Cell::new(0);
        let mut binary = vec![b'a'; 1 << 20];
        binary[100] = 0;
        assert!(read_text_bytes(Counting(&binary, &read), binary.len() as u64).is_none());
        assert!(read.get() as u64 <= BINARY_SNIFF_BYTES);

        let mut late_nul = vec![b'a'; 20_000];
        late_nul[9_000] = 0;
        assert_eq!(read_text_bytes(&late_nul[..], 20_000).unwrap(), late_nul);
    }

    #[test]
    fn oversized_literal_query_is_an_error_not_a_panic() {
        let huge = "ab".repeat(1 << 17);
        let error = TextMatcher::new(MatchMode::Exact, huge.clone())
            .err()
            .expect("compiled-size limit");
        assert_eq!(error.token, huge);
        assert!(TextMatcher::new(MatchMode::Exact, "a.b".to_string()).is_ok());
    }

    #[test]
    fn modified_text_falls_back_for_times_chrono_cannot_hold() {
        let epoch = format_modified(std::time::UNIX_EPOCH);
        assert_ne!(epoch, "\u{2013}");
        // ~278k years: past chrono's range. Skipped where SystemTime itself
        // cannot hold it (Windows FILETIME).
        if let Some(far) = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(1 << 43)) {
            assert_eq!(format_modified(far), "\u{2013}");
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

    fn write_zip(path: &Path, name: &str, content: &[u8]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(name, options).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap();
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

    fn name_search(root: &Path, expression: &str) -> SearchSummary {
        let engine = SearchEngine::default();
        let run = engine
            .start(
                root.to_path_buf(),
                Query::parse(expression, MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        collect_run(&run).1
    }

    #[test]
    fn unreadable_folders_are_counted_not_dropped() {
        let tmp = TempDir::new();
        // The walk cannot read a root that does not exist; it reports an
        // error item, which used to vanish in `.flatten()`.
        let summary = name_search(&tmp.path().join("missing"), "anything");
        assert_eq!(summary.content_skipped, 1);
    }

    #[test]
    fn archives_that_will_not_open_are_counted() {
        let tmp = TempDir::new();
        tmp.file("broken.zip", "not a zip archive");
        let summary = name_search(tmp.path(), "nothing-matches-this");
        assert_eq!(summary.content_skipped, 1);
    }

    #[test]
    fn filtered_content_search_counts_unsearched_tar_gz_archives() {
        let tmp = TempDir::new();
        tmp.file("server.log", "no match here");
        // The archive fails the name filter, so it is never read as a binary
        // file; its members are still skipped and must be counted.
        tmp.file("logs.tgz", "members would hold server.log with ERROR");
        let summary = name_search(tmp.path(), "path:server.log content:ERROR");
        assert!(summary.content_skipped >= 1, "{summary:?}");
    }

    #[test]
    fn exhausted_archive_budget_marks_the_results_capped() {
        let tmp = TempDir::new();
        for index in 0..=crate::archive::MAX_ARCHIVES_PER_SEARCH {
            tmp.file(&format!("archive-{index:04}.zip"), "x");
        }
        let summary = name_search(tmp.path(), "nothing-matches-this");
        assert!(summary.truncated);
        assert_eq!(summary.matched, 0);
    }

    #[test]
    fn abandoned_search_closes_its_channel_and_wakes_the_ui() {
        let (sender, receiver) = mpsc::channel::<SearchEvent>();
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let hook = abandonment_hook(
            Arc::new(std::sync::Mutex::new(Some(sender))),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        );

        hook(crate::workload::AbandonReason::SpawnFailed);

        assert_eq!(woken.load(Ordering::SeqCst), 1);
        assert_eq!(receiver.try_recv().unwrap_err(), TryRecvError::Disconnected);
    }

    #[test]
    fn stale_worker_reports_cancellation_before_scanning() {
        let tmp = TempDir::new();
        tmp.file("one.txt", "x");
        let active = Arc::new(AtomicU64::new(2));
        let (sender, receiver) = mpsc::channel();
        run_worker(
            ProviderRequest {
                generation: 1,
                active,
                scheduler_cancel: crate::workload::CancellationToken::new(),
                requires_content: false,
                unsearched: Cell::new(0),
                archives_capped: Cell::new(false),
            },
            CompiledQuery::new(Query::default()).unwrap(),
            100,
            sender,
            Arc::new(|| {}),
            Box::new(FilesystemProvider::new(tmp.path().to_path_buf())),
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
    fn indexed_provider_searches_the_snapshot_without_disk_reads() {
        let tmp = TempDir::new();
        let source = tmp.file("notes.txt", "durable indexed needle");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        std::fs::remove_file(source).unwrap();
        let engine = SearchEngine::default();
        let run = engine
            .start_indexed(
                index,
                Query::parse("content:needle", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();

        assert_eq!(run.provider, "Index + ZIP");
        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "notes.txt");
        assert_eq!(summary.content_skipped, 0);
    }

    #[test]
    fn live_provider_streams_archive_members_and_reveals_the_archive() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("bundle.zip");
        write_zip(&archive, "docs/inside.txt", b"archive provider needle");
        let engine = SearchEngine::default();
        let run = engine
            .start(
                tmp.path().to_path_buf(),
                Query::parse("path:inside.txt content:needle", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();

        assert_eq!(run.provider, "Live + ZIP");
        let (hits, summary) = collect_run(&run);
        assert_eq!(summary.matched, 1);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].entry.path.to_string_lossy().contains("bundle.zip!"));
        assert_eq!(hits[0].reveal_path(), archive);
        assert!(matches!(
            &hits[0].location,
            SearchLocation::ArchiveMember { member, .. }
                if member == Path::new("docs/inside.txt")
        ));
    }

    #[test]
    fn indexed_provider_delegates_zip_paths_to_the_archive_provider() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("indexed-bundle.zip");
        write_zip(&archive, "nested/report.txt", b"indexed archive phrase");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        let engine = SearchEngine::default();
        let run = engine
            .start_indexed(
                index,
                Query::parse("path:report.txt content:phrase", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();

        let (hits, summary) = collect_run(&run);
        assert_eq!(summary.matched, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].reveal_path(), archive);
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
        assert!(second.snapshot.generation > first.snapshot.generation);
        assert_eq!(engine.active_generation(), second.snapshot.generation);
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
            location: SearchLocation::Native(PathBuf::from(format!("/x/{name}"))),
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
            let explanation = compiled.evaluate_metadata(&entry, Path::new("/fixture"), now)?;
            Some(SearchHit {
                entry,
                identity: FileIdentity::Path(PathBuf::from(name)),
                accessed: None,
                explanation,
                location: SearchLocation::Native(PathBuf::from(format!("/fixture/{name}"))),
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
        use chrono::TimeZone;
        // A fixed local noon away from DST transitions keeps the day count
        // independent of the time zone the test runs in.
        let now: SystemTime = chrono::Local
            .with_ymd_and_hms(2026, 1, 20, 12, 0, 0)
            .single()
            .unwrap()
            .into();
        let mut hit = SearchHit {
            entry: file_entry(PathBuf::from("/x/a.txt"), 1),
            identity: FileIdentity::Path(PathBuf::from("/x/a.txt")),
            accessed: Some(now - Duration::from_secs(8 * 86_400)),
            explanation: MatchExplanation::default(),
            location: SearchLocation::Native(PathBuf::from("/x/a.txt")),
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
        // Clock skew: a time ahead of now is still today, not "older".
        hit.entry.modified = Some(now + Duration::from_secs(3_600));
        assert_eq!(
            time_bucket(&hit, TimePivot::Modified, now),
            TimeBucket::Today
        );
        hit.accessed = None;
        assert_eq!(
            time_bucket(&hit, TimePivot::Accessed, now),
            TimeBucket::Unknown
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
                matched += usize::from(
                    query
                        .evaluate_metadata(&entry, Path::new("/"), now)
                        .is_some(),
                );
            }
            eprintln!(
                "search-bench {label}: {count} entries, {matched} matches, {:?}",
                started.elapsed()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn content_search_skips_fifos_without_blocking() {
        let tmp = TempDir::new();
        tmp.file("notes.txt", "needle");
        let status = std::process::Command::new("mkfifo")
            .arg(tmp.path().join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        let engine = SearchEngine::default();
        let run = engine
            .start(
                tmp.path().to_path_buf(),
                Query::parse("content:needle", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();

        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "notes.txt");
        assert_eq!(summary.content_skipped, 1);
    }

    #[cfg(unix)]
    #[test]
    fn content_partial_note_uses_the_symlink_target_size() {
        let tmp = TempDir::new();
        let target = tmp.path().join("big.txt");
        let mut text = vec![b'a'; MAX_CONTENT_BYTES as usize + 1];
        text[..6].copy_from_slice(b"needle");
        std::fs::write(&target, text).unwrap();
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let lexical = std::fs::symlink_metadata(&link).unwrap();
        let entry = FileEntry::from_meta(link, &lexical).unwrap();
        assert!(entry.size < MAX_CONTENT_BYTES);

        let query =
            CompiledQuery::new(Query::parse("content:needle", MatchMode::Exact).unwrap()).unwrap();
        let mut explanation = MatchExplanation::default();
        assert!(matches!(
            query.evaluate_content(&entry, &CandidateContent::OnDisk, &mut explanation),
            ContentVerdict::Matched
        ));
        assert!(explanation.summary().contains("first 16MB"));
    }

    #[test]
    fn path_predicate_ignores_the_search_root() {
        let compiled =
            CompiledQuery::new(Query::parse("path:home", MatchMode::Exact).unwrap()).unwrap();
        let now = SystemTime::now();
        let root = Path::new("/home/user");
        let outside = file_entry(PathBuf::from("/home/user/docs/a.txt"), 1);
        let inside = file_entry(PathBuf::from("/home/user/home/b.txt"), 1);
        assert!(compiled.evaluate_metadata(&outside, root, now).is_none());
        let explanation = compiled.evaluate_metadata(&inside, root, now).unwrap();
        assert_eq!(explanation.components[0].matched_ranges, vec![(0, 4)]);
    }

    #[test]
    fn exactly_cap_matches_is_not_truncated() {
        let tmp = TempDir::new();
        tmp.file("match-1.txt", "x");
        tmp.file("match-2.txt", "x");
        let engine = SearchEngine::default();
        let query = || Query::parse("match", MatchMode::Exact).unwrap();

        let run = engine
            .start(tmp.path().to_path_buf(), query(), 2, Arc::new(|| {}))
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 2);
        assert!(!summary.truncated);

        tmp.file("match-3.txt", "x");
        let run = engine
            .start(tmp.path().to_path_buf(), query(), 2, Arc::new(|| {}))
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 2);
        assert_eq!(summary.matched, 2);
        assert!(summary.truncated);
    }

    #[test]
    fn time_buckets_follow_local_calendar_days() {
        use chrono::TimeZone;
        let at = |y, m, d, h| -> SystemTime {
            chrono::Local
                .with_ymd_and_hms(y, m, d, h, 0, 0)
                .single()
                .unwrap()
                .into()
        };
        let now = at(2026, 6, 10, 8);
        let mut hit = SearchHit {
            entry: file_entry(PathBuf::from("/x/a.txt"), 1),
            identity: FileIdentity::Path(PathBuf::from("/x/a.txt")),
            accessed: None,
            explanation: MatchExplanation::default(),
            location: SearchLocation::Native(PathBuf::from("/x/a.txt")),
        };
        let mut bucket = |value| {
            hit.entry.modified = Some(value);
            time_bucket(&hit, TimePivot::Modified, now)
        };
        assert_eq!(bucket(at(2026, 6, 10, 1)), TimeBucket::Today);
        assert_eq!(bucket(at(2026, 6, 9, 23)), TimeBucket::Yesterday);
        assert_eq!(bucket(at(2026, 6, 8, 9)), TimeBucket::ThisWeek);
    }

    #[cfg(unix)]
    #[test]
    fn open_regular_file_refuses_devices_and_directories() {
        assert!(open_regular_file(Path::new("/dev/null")).is_none());
        assert!(open_regular_file(Path::new("/")).is_none());
    }

    #[test]
    fn time_bucket_handles_times_outside_chrono_range() {
        let now = SystemTime::now();
        let mut hit = SearchHit {
            entry: file_entry(PathBuf::from("/x/a.txt"), 1),
            identity: FileIdentity::Path(PathBuf::from("/x/a.txt")),
            accessed: None,
            explanation: MatchExplanation::default(),
            location: SearchLocation::Native(PathBuf::from("/x/a.txt")),
        };
        hit.entry.modified = std::time::UNIX_EPOCH.checked_sub(Duration::from_millis(1_500));
        assert_eq!(
            time_bucket(&hit, TimePivot::Modified, now),
            TimeBucket::Older
        );
        // Windows cannot represent times this far back; unix (64-bit
        // time_t) can, and chrono cannot.
        if let Some(ancient) = std::time::UNIX_EPOCH.checked_sub(Duration::from_secs(1 << 62)) {
            hit.entry.modified = Some(ancient);
            assert_eq!(
                time_bucket(&hit, TimePivot::Modified, now),
                TimeBucket::Older
            );
        }
    }

    #[test]
    fn content_search_skips_binary_files_and_counts_them() {
        let tmp = TempDir::new();
        tmp.file("text.txt", "needle here");
        std::fs::write(tmp.path().join("blob.bin"), b"needle\0here").unwrap();

        let engine = SearchEngine::default();
        let run = engine
            .start(
                tmp.path().to_path_buf(),
                Query::parse("content:needle", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let (hits, summary) = collect_run(&run);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "text.txt");
        assert_eq!(summary.content_skipped, 1);
    }

    #[test]
    fn highlight_ranges_count_characters_not_bytes() {
        // Two-byte, three-byte (CJK) and four-byte (emoji) characters, plus
        // a decomposed e + combining acute that is two chars.
        let name = "r\u{e9}sum\u{e9} \u{65e5}\u{672c} \u{1f600} e\u{301}Report.txt";
        let report_at = name.chars().position(|c| c == 'R').unwrap();
        let expected = vec![(report_at, report_at + 6)];

        let exact = TextMatcher::new(MatchMode::Exact, "report".to_string()).unwrap();
        assert_eq!(exact.evaluate(name).unwrap().matched_ranges, expected);
        let regex = TextMatcher::new(MatchMode::Regex, "rep.rt".to_string()).unwrap();
        assert_eq!(regex.evaluate(name).unwrap().matched_ranges, expected);

        // A match that itself spans multibyte characters.
        let cjk = TextMatcher::new(MatchMode::Exact, "\u{65e5}\u{672c}".to_string()).unwrap();
        assert_eq!(cjk.evaluate(name).unwrap().matched_ranges, vec![(7, 9)]);
        assert_eq!(byte_range_to_char_range(name, 0, 0), (0, 0));
        assert_eq!(
            byte_range_to_char_range(name, name.len(), name.len()),
            (24, 24)
        );
    }

    #[test]
    fn synthetic_entries_use_the_panel_placeholders() {
        let dir = synthetic_entry(
            PathBuf::from("/a.zip!/Docs"),
            true,
            4096,
            None,
            modified_label(None),
        )
        .unwrap();
        assert_eq!(
            (dir.name.as_str(), dir.name_lower.as_str()),
            ("Docs", "docs")
        );
        assert_eq!((dir.size, dir.size_str.as_str()), (0, "\u{2026}"));
        assert_eq!(dir.modified_str, "\u{2013}");
        assert!(matches!(
            dir.identity,
            crate::panel::ListingIdentity::Unavailable
        ));

        let file = synthetic_entry(
            PathBuf::from("/data/Report.PDF"),
            false,
            2048,
            None,
            String::new(),
        )
        .unwrap();
        assert_eq!(file.extension, "pdf");
        assert_eq!(file.size, 2048);
        assert_eq!(file.size_str, crate::panel::format_size(2048));

        assert!(synthetic_entry(PathBuf::from("/"), true, 0, None, String::new()).is_none());
    }
}
