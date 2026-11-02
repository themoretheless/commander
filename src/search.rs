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
        "Live + archives"
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut archive_budget = crate::archive::SearchBudget::default();
        let mut walk = jwalk::WalkDir::new(&self.root).skip_hidden(false);
        if let Some(depth) = request.max_depth {
            walk = walk.max_depth(depth);
        }
        if !request.exclusions.is_empty()
            || !request.directory_exclusions.is_empty()
            || request.hidden_mode == crate::query::HiddenMode::Exclude
        {
            let exclusions = Arc::clone(&request.exclusions);
            let directory_exclusions = Arc::clone(&request.directory_exclusions);
            let root = self.root.clone();
            let exclude_hidden = request.hidden_mode == crate::query::HiddenMode::Exclude;
            walk = walk.process_read_dir(move |_, _, _, children| {
                children.retain(|result| {
                    result.as_ref().map_or(true, |entry| {
                        let name = entry.file_name.to_string_lossy();
                        entry.depth == 0
                            || ((!exclude_hidden || !dot_name(&entry.file_name))
                                && !exclusions.iter().any(|pattern| pattern.is_match(&name))
                                && !(entry.file_type.is_dir()
                                    && directory_exclusions.iter().any(|pattern| {
                                        pattern.matches_directory(&entry.path(), &root)
                                    })))
                    })
                });
            });
        }
        for item in walk {
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
            let archive_path = (request.search_archives
                && !entry.is_dir
                && request.max_depth.is_none_or(|max| {
                    search_depth(&entry.path, self.root()).is_some_and(|depth| depth < max)
                })
                && crate::archive::is_supported(&entry.path)
                && crate::trust::allows_auto_archive_inspect(&entry.path))
            .then(|| entry.path.clone());
            let is_empty = if request.requires_empty {
                crate::fs_util::observed_empty(&entry.path, &metadata)
            } else {
                None
            };
            let is_broken_symlink = request
                .requires_broken_symlink
                .then(|| crate::fs_util::observed_broken_symlink(&entry.path, &metadata))
                .flatten();
            if !emit(Some(SearchCandidate {
                location: SearchLocation::Native(entry.path.clone()),
                entry,
                is_symlink: Some(metadata.file_type().is_symlink()),
                is_executable: crate::query::executable_file(&metadata),
                is_regular: Some(metadata.is_file()),
                unix_mode: crate::query::unix_mode(&metadata.permissions()),
                owner_uid: crate::query::native_uid(&metadata),
                owner_gid: crate::query::native_gid(&metadata),
                device_id: entry_identity_parts(&metadata).map(|p| p.0),
                inode: entry_identity_parts(&metadata).map(|p| p.1),
                file_type_bits: crate::query::native_file_type_bits(&metadata),
                is_empty,
                is_broken_symlink,
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

/// Bounded archive topology proof for empty-directory queries. Known children
/// prove nonemptiness even if another part of the inspection is incomplete.
struct EmptyDirectoryTracker {
    occupied: std::collections::HashSet<PathBuf>,
    bytes: usize,
    limit: usize,
    complete: bool,
}

impl EmptyDirectoryTracker {
    fn new(limit: usize) -> Self {
        Self {
            occupied: Default::default(),
            bytes: 0,
            limit,
            complete: true,
        }
    }
    fn reserve(&mut self, bytes: usize) -> bool {
        if bytes > self.limit.saturating_sub(self.bytes) {
            self.complete = false;
            return false;
        }
        self.bytes += bytes;
        true
    }
    fn key(path: &Path) -> PathBuf {
        // The archive visitor already rejects escaping paths. Ignore a leading
        // ./ as well as redundant separators when comparing enclosed names.
        path.components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect()
    }
    fn observe(&mut self, path: &Path) {
        let path = Self::key(path);
        for ancestor in path
            .ancestors()
            .skip(1)
            .take_while(|path| !path.as_os_str().is_empty())
        {
            if self.occupied.contains(ancestor) {
                continue;
            }
            if !self.reserve(
                ancestor
                    .as_os_str()
                    .as_encoded_bytes()
                    .len()
                    .saturating_add(128),
            ) {
                break;
            }
            self.occupied.insert(ancestor.to_path_buf());
        }
    }
    fn reserve_directory(&mut self, path: &Path) -> bool {
        self.reserve(
            path.as_os_str()
                .as_encoded_bytes()
                .len()
                .saturating_mul(2)
                .saturating_add(512),
        )
    }
    fn empty(&self, path: &Path, inspection_complete: bool) -> Option<bool> {
        if self.occupied.contains(&Self::key(path)) {
            Some(false)
        } else if self.complete && inspection_complete {
            Some(true)
        } else {
            None
        }
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
        if !request.search_archives || !crate::trust::allows_auto_archive_inspect(archive_path) {
            return true;
        }
        let metadata = match std::fs::metadata(archive_path) {
            Ok(metadata) => metadata,
            Err(_) => return emit(None),
        };
        let archive_modified = metadata.modified().ok();
        let accessed = metadata.accessed().ok();
        // A member without its own time shows the archive's; format it once.
        let archive_modified_str = modified_label(archive_modified);
        let mut keep_going = true;
        let result = {
            let mut emit_member = |member: crate::archive::SearchMember, is_empty: Option<bool>| {
                if (request.hidden_mode == crate::query::HiddenMode::Exclude
                    && dot_hidden_path(&member.member.path, Path::new("")))
                    || excluded_path(&member.member.path, Path::new(""), &request.exclusions)
                    || excluded_directory_path(
                        &member.member.path,
                        Path::new(""),
                        member.member.is_dir,
                        &request.directory_exclusions,
                    )
                {
                    return true;
                }
                // The virtual path ends in the member's own name.
                if member.member.path.file_name().is_none() {
                    keep_going = emit(None);
                    return keep_going;
                }
                let virtual_path =
                    crate::archive::virtual_member_path(archive_path, &member.member.path);
                let (modified, modified_str) = match member.member.modified {
                    Some(own) => (Some(own), modified_label(Some(own))),
                    None => (archive_modified, archive_modified_str.clone()),
                };
                let Some(entry) = synthetic_entry(
                    virtual_path,
                    member.member.is_dir,
                    member.member.size,
                    modified,
                    modified_str,
                ) else {
                    keep_going = emit(None);
                    return keep_going;
                };
                keep_going = emit(Some(SearchCandidate {
                    is_symlink: Some(member.member.is_symlink),
                    is_executable: member.member.is_executable,
                    is_regular: Some(member.member.is_regular),
                    unix_mode: member.member.unix_mode,
                    owner_uid: member.member.owner_uid,
                    owner_gid: member.member.owner_gid,
                    device_id: None,
                    inode: None,
                    file_type_bits: Some(member.member.file_type_bits),
                    is_empty,
                    is_broken_symlink: (!member.member.is_symlink).then_some(false),
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
            };
            let mut directories = Vec::new();
            let mut topology = EmptyDirectoryTracker::new(8 * 1024 * 1024);
            let mut stopped = false;
            let result = crate::archive::visit_members(
                archive_path,
                request.requires_content,
                budget,
                || request.cancelled(),
                |member| {
                    if request.requires_empty {
                        // Count children before presentation/traversal filters: a hidden
                        // or excluded child still makes its real archive folder nonempty.
                        topology.observe(&member.member.path);
                        if member.member.is_dir {
                            if topology.reserve_directory(&member.member.path) {
                                directories.push(member);
                            } else {
                                // Other known OR branches may still prove this row.
                                let proceed = emit_member(member, None);
                                stopped |= !proceed;
                                return proceed;
                            }
                            return !request.cancelled();
                        }
                    }
                    let is_empty = Some(member.member.is_regular && member.member.size == 0);
                    let proceed = emit_member(member, is_empty);
                    stopped |= !proceed;
                    proceed
                },
            );
            let complete = result.as_ref().is_ok_and(|summary| {
                !summary.cancelled
                    && !summary.members_not_searched
                    && summary.unsafe_members == 0
                    && summary.unreadable_members == 0
            }) && !budget.truncated
                && !request.cancelled();
            if !topology.complete {
                request.archives_capped.set(true);
            }
            if !stopped {
                for member in directories {
                    if request.cancelled() {
                        break;
                    }
                    let is_empty = topology.empty(&member.member.path, complete);
                    if !emit_member(member, is_empty) {
                        break;
                    }
                }
            }
            result
        };
        match result {
            // Content search does not open this format: its members were
            // skipped, so count it rather than drop them silently.
            Ok(summary) if summary.members_not_searched => request.note_unsearched(),
            Ok(_) => {}
            Err(error) => {
                // A corrupt or encrypted archive: its members are not searched.
                request.note_unsearched();
                request.note_archive_error(&error);
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
        "Archives"
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
        "Index + archives"
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
            if request.max_depth.is_some_and(|max| {
                search_depth(&entry.path, self.root()).is_none_or(|depth| depth > max)
            }) {
                continue;
            }
            if (request.hidden_mode == crate::query::HiddenMode::Exclude
                && dot_hidden_path(&entry.path, self.root()))
                || excluded_path(&entry.path, self.root(), &request.exclusions)
                || excluded_directory_path(
                    &entry.path,
                    self.root(),
                    entry.is_dir,
                    &request.directory_exclusions,
                )
            {
                continue;
            }
            let archive_path = (request.search_archives
                && !entry.is_dir
                && request.max_depth.is_none_or(|max| {
                    search_depth(&entry.path, self.root()).is_some_and(|depth| depth < max)
                })
                && crate::archive::is_supported(&entry.path)
                && crate::trust::allows_auto_archive_inspect(&entry.path))
            .then(|| entry.path.clone());
            if !emit(Some(SearchCandidate {
                location: SearchLocation::Native(entry.path.clone()),
                entry,
                is_symlink: indexed.is_symlink,
                is_executable: indexed.is_executable,
                is_regular: indexed.is_regular,
                unix_mode: indexed.unix_mode,
                owner_uid: indexed.owner_uid,
                owner_gid: indexed.owner_gid,
                device_id: indexed.volume,
                inode: indexed.file_id,
                file_type_bits: indexed.file_type_bits(),
                is_empty: indexed.is_empty,
                is_broken_symlink: indexed.broken_symlink(),
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

#[derive(Clone, Copy, Default)]
struct ObservedMetadata {
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
}

impl ObservedMetadata {
    fn matches_types(
        self,
        entry: &FileEntry,
        types: &[crate::query::TypeCondition],
    ) -> Option<bool> {
        use crate::query::TypeCondition;
        let mut unknown = false;
        for kind in types {
            let matched = match kind {
                TypeCondition::File => Some(!entry.is_dir),
                TypeCondition::Empty => self.is_empty,
                TypeCondition::Regular => self.is_regular,
                TypeCondition::Symlink => self.is_symlink,
                TypeCondition::BrokenSymlink => self.is_broken_symlink,
                TypeCondition::Executable => self.is_executable,
                TypeCondition::Special(kind) => self.file_type_bits.map(|bits| bits == kind.bits()),
                TypeCondition::Kind(kind) => {
                    Some(crate::selection_summary::kind_of(entry) == *kind)
                }
            };
            match matched {
                Some(true) => return Some(true),
                None => unknown = true,
                Some(false) => {}
            }
        }
        if unknown { None } else { Some(false) }
    }
}

struct CompiledQuery {
    case_sensitive: bool,
    search_archives: bool,
    hidden_mode: crate::query::HiddenMode,
    exclusions: Arc<Vec<regex::Regex>>,
    directory_exclusions: Arc<Vec<crate::query::DirectoryMask>>,
    max_depth: Option<usize>,
    predicates: Vec<CompiledPredicate>,
    requires_content: bool,
    requires_empty: bool,
    requires_broken_symlink: bool,
    requires_complete_content: bool,
    requires_boundary_context: bool,
    requires_symlink_kind: bool,
    requires_executable_kind: bool,
    requires_regular_kind: bool,
    requires_unix_mode: bool,
    requires_file_type_bits: bool,
    requires_allocation: bool,
    requires_links: bool,
}

#[derive(Clone, Copy)]
enum TimestampField {
    Modified,
    Accessed,
    Created,
    Changed,
}
impl TimestampField {
    fn observed(self, entry: &FileEntry) -> Option<SystemTime> {
        match self {
            Self::Modified => entry.modified,
            Self::Accessed => entry.accessed,
            Self::Created => entry.created,
            Self::Changed => entry.changed,
        }
    }
}
enum CompiledPredicate {
    UnixPermissions {
        pattern: crate::query::PermissionPattern,
        detail: String,
    },
    Timestamp {
        field: TimestampField,
        after: bool,
        boundary: SystemTime,
        detail: String,
    },
    NameGlob {
        original: String,
        regex: regex::Regex,
    },
    ExcludeFileGlob {
        original: String,
        regex: regex::Regex,
    },
    NotText {
        path: bool,
        matcher: TextMatcher,
        detail: String,
    },
    Name(TextMatcher),
    AnyName(Vec<TextMatcher>),
    Path(TextMatcher),
    Content(TextMatcher),
    AnyContent(Vec<TextMatcher>),
    NotContent {
        matcher: TextMatcher,
        detail: String,
    },
    Metadata(Predicate),
}

enum TextMatcher {
    WholeName {
        original: String,
        regex: regex::Regex,
    },
    WholeLine {
        original: String,
        regex: regex::Regex,
    },
    WholeWord {
        original: String,
        regex: regex::Regex,
    },
    PanelFilter {
        value: String,
        case_sensitive: bool,
    },
    Exact {
        original: String,
        regex: regex::Regex,
    },
    Fuzzy {
        value: String,
        case_sensitive: bool,
    },
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
    #[cfg(test)]
    fn new(mode: MatchMode, value: String) -> Result<Self, QueryError> {
        Self::new_case(mode, value, false)
    }

    fn new_case(mode: MatchMode, value: String, case_sensitive: bool) -> Result<Self, QueryError> {
        match mode {
            MatchMode::Exact => {
                // An escaped literal always parses, but a huge pasted token
                // can still exceed the compiled-size limit.
                let regex = regex::RegexBuilder::new(&regex::escape(&value))
                    .case_insensitive(!case_sensitive)
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
            MatchMode::Fuzzy => Ok(Self::Fuzzy {
                value,
                case_sensitive,
            }),
            MatchMode::Regex => {
                let regex = regex::RegexBuilder::new(&value)
                    .case_insensitive(!case_sensitive)
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

    fn evaluate_bounded(&self, candidate: &str, boundary_unknown: bool) -> Option<TextMatch> {
        let (original, regex, whole_line) = match self {
            Self::WholeWord { original, regex } => (original, regex, false),
            Self::WholeLine { original, regex } => (original, regex, true),
            _ => return self.evaluate(candidate),
        };
        // A lossy replacement at a truncated UTF-8 tail is not a proven delimiter.
        let candidate = if boundary_unknown {
            candidate.trim_end_matches('\u{fffd}')
        } else {
            candidate
        };
        // An empty file has no lines; EOF after a terminator is no extra line.
        if whole_line && candidate.is_empty() {
            return None;
        }
        let found = regex.find_iter(candidate).find(|found| {
            (!boundary_unknown || found.end() < candidate.len())
                && (!whole_line || found.start() < candidate.len())
        })?;
        let range = byte_range_to_char_range(candidate, found.start(), found.end());
        Some(TextMatch {
            score: 120 - range.0.min(100) as i32,
            matched_ranges: vec![range],
            detail: format!(
                "whole {} literal {original:?}",
                if whole_line { "line" } else { "word" }
            ),
        })
    }

    fn evaluate(&self, candidate: &str) -> Option<TextMatch> {
        match self {
            Self::WholeWord { .. } | Self::WholeLine { .. } => {
                self.evaluate_bounded(candidate, false)
            }
            Self::PanelFilter {
                value,
                case_sensitive,
            } => {
                if crate::name_filter::is_fuzzy_query(value) {
                    return crate::fuzzy::score_case(value, candidate, *case_sensitive).map(
                        |matched| TextMatch {
                            score: matched.score,
                            matched_ranges: matched.matched_ranges,
                            detail: format!("pane filter {value:?}"),
                        },
                    );
                }
                if !crate::name_filter::is_match_case(value, candidate, *case_sensitive) {
                    return None;
                }
                let ranges =
                    crate::name_filter::matched_ranges_case(value, candidate, *case_sensitive);
                let position = ranges.first().map_or(0, |range| range.0.min(100)) as i32;
                Some(TextMatch {
                    score: 120 - position,
                    matched_ranges: ranges,
                    detail: format!("pane filter {value:?}"),
                })
            }
            Self::Exact { original, regex } | Self::WholeName { original, regex } => {
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
                    detail: if matches!(self, Self::WholeName { .. }) {
                        format!("whole name literal {original:?}")
                    } else {
                        format!("literal {original:?}")
                    },
                })
            }
            Self::Fuzzy {
                value,
                case_sensitive,
            } => crate::fuzzy::score_case(value, candidate, *case_sensitive).map(|matched| {
                TextMatch {
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                    detail: format!("fuzzy {value:?}"),
                }
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
            .evaluate_metadata(
                entry,
                Path::new(""),
                now,
                ObservedMetadata {
                    is_symlink: Some(entry.is_symlink()),
                    is_broken_symlink: entry.symlink.as_ref().and_then(|info| match info.state {
                        crate::panel::LinkTargetState::Available => Some(false),
                        crate::panel::LinkTargetState::Missing => Some(true),
                        crate::panel::LinkTargetState::Unavailable(_) => None,
                    }),
                    unix_mode: entry.permissions.as_ref().and_then(crate::query::unix_mode),
                    is_regular: entry.hard_link_count().map(|_| true),
                    owner_uid: entry.owner_uid,
                    owner_gid: entry.owner_gid,
                    device_id: entry.filesystem_id().map(|p| p.0),
                    inode: entry.filesystem_id().map(|p| p.1),
                    ..Default::default()
                },
            )
            .is_some()
    })
}

impl CompiledQuery {
    fn new(query: Query) -> Result<Self, QueryError> {
        query.validate()?;
        let requires_content = query.requires_content();
        let requires_empty = query.predicates.iter().any(|predicate| match predicate {
            Predicate::Empty => true,
            Predicate::TypesAny(types) => types.contains(&crate::query::TypeCondition::Empty),
            _ => false,
        });
        let requires_broken_symlink = query.predicates.iter().any(|p| match p {
            Predicate::BrokenSymlinks => true,
            Predicate::TypesAny(types) => {
                types.contains(&crate::query::TypeCondition::BrokenSymlink)
            }
            _ => false,
        });
        let requires_symlink_kind = query.predicates.contains(&Predicate::SymlinksOnly);
        let requires_executable_kind = query.predicates.contains(&Predicate::ExecutableFiles);
        let requires_regular_kind = query.predicates.contains(&Predicate::RegularFiles);
        let requires_file_type_bits = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::SpecialFileType(_)));
        let requires_unix_mode = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::UnixPermissions(_)));
        let requires_links = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::MinLinks(_) | Predicate::MaxLinks(_)));
        let requires_allocation = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::MinAllocated(_) | Predicate::MaxAllocated(_)));
        let case_sensitive = query.case_sensitive();
        let requires_boundary_context = query.predicates.iter().any(|predicate| {
            matches!(
                predicate,
                Predicate::ContentWord(_)
                    | Predicate::ContentWordExcludes(_)
                    | Predicate::ContentLine(_)
                    | Predicate::ContentLineExcludes(_)
            )
        });
        let requires_complete_content = query.predicates.iter().any(|predicate| {
            matches!(
                predicate,
                Predicate::ContentExcludes(_)
                    | Predicate::ContentWordExcludes(_)
                    | Predicate::ContentLineExcludes(_)
            )
        });
        let hidden_mode = query
            .predicates
            .iter()
            .find_map(|predicate| match predicate {
                Predicate::Hidden(mode) => Some(*mode),
                _ => None,
            })
            .unwrap_or_default();
        let search_archives = !query.predicates.contains(&Predicate::SearchArchives(false));
        let max_depth = query
            .predicates
            .iter()
            .filter_map(|predicate| match predicate {
                Predicate::MaxDepth(depth) => Some(*depth),
                _ => None,
            })
            .min();
        let exclusions = Arc::new(
            query
                .predicates
                .iter()
                .filter_map(|predicate| match predicate {
                    Predicate::ExcludeGlob(value) => {
                        Some(crate::query::compile_name_glob(value, case_sensitive))
                    }
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let directory_exclusions = Arc::new(
            query
                .predicates
                .iter()
                .filter_map(|predicate| match predicate {
                    Predicate::ExcludeDirGlob(value) => {
                        Some(crate::query::compile_directory_mask(value, case_sensitive))
                    }
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let mut predicates = Vec::with_capacity(query.predicates.len());
        for predicate in query.predicates {
            let compiled = match predicate {
                Predicate::UnixPermissions(value) => {
                    let pattern = crate::query::parse_permissions(&value)?;
                    CompiledPredicate::UnixPermissions {
                        pattern,
                        detail: format!("perm:{value}"),
                    }
                }
                Predicate::NameGlob(value) => CompiledPredicate::NameGlob {
                    regex: crate::query::compile_name_glob(&value, case_sensitive)?,
                    original: value,
                },
                Predicate::ExcludeFileGlob(value) => CompiledPredicate::ExcludeFileGlob {
                    regex: crate::query::compile_name_glob(&value, case_sensitive)?,
                    original: value,
                },
                Predicate::NameFilter(value) => CompiledPredicate::Name(TextMatcher::PanelFilter {
                    value,
                    case_sensitive,
                }),
                Predicate::NameExact(value) => CompiledPredicate::Name(TextMatcher::WholeName {
                    regex: crate::query::compile_exact_name(&value, case_sensitive)?,
                    original: value,
                }),
                Predicate::NameNotExact(value) => CompiledPredicate::NotText {
                    path: false,
                    detail: format!("no whole name literal match for {value:?}"),
                    matcher: TextMatcher::WholeName {
                        regex: crate::query::compile_exact_name(&value, case_sensitive)?,
                        original: value,
                    },
                },
                Predicate::NameExcludes(value) => CompiledPredicate::NotText {
                    path: false,
                    detail: format!("no {} name match for {value:?}", query.mode.label()),
                    matcher: TextMatcher::new_case(query.mode, value, case_sensitive)?,
                },
                Predicate::PathExcludes(value) => CompiledPredicate::NotText {
                    path: true,
                    detail: format!("no {} path match for {value:?}", query.mode.label()),
                    matcher: TextMatcher::new_case(query.mode, value, case_sensitive)?,
                },
                Predicate::NameContains(value) => CompiledPredicate::Name(TextMatcher::new_case(
                    query.mode,
                    value,
                    case_sensitive,
                )?),
                Predicate::PathContains(value) => CompiledPredicate::Path(TextMatcher::new_case(
                    query.mode,
                    value,
                    case_sensitive,
                )?),
                Predicate::NameAny(values) => CompiledPredicate::AnyName(
                    values
                        .into_iter()
                        .map(|value| TextMatcher::new_case(query.mode, value, case_sensitive))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Predicate::ContentAny(values) => CompiledPredicate::AnyContent(
                    values
                        .into_iter()
                        .map(|value| TextMatcher::new_case(query.mode, value, case_sensitive))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Predicate::ContentLine(value) => {
                    CompiledPredicate::Content(TextMatcher::WholeLine {
                        regex: crate::query::compile_content_line(&value, case_sensitive)?,
                        original: value,
                    })
                }
                Predicate::ContentLineExcludes(value) => CompiledPredicate::NotContent {
                    detail: format!("no whole line literal content match for {value:?}"),
                    matcher: TextMatcher::WholeLine {
                        regex: crate::query::compile_content_line(&value, case_sensitive)?,
                        original: value,
                    },
                },
                Predicate::ContentWord(value) => {
                    CompiledPredicate::Content(TextMatcher::WholeWord {
                        regex: crate::query::compile_content_word(&value, case_sensitive)?,
                        original: value,
                    })
                }
                Predicate::ContentWordExcludes(value) => CompiledPredicate::NotContent {
                    detail: format!("no whole word literal content match for {value:?}"),
                    matcher: TextMatcher::WholeWord {
                        regex: crate::query::compile_content_word(&value, case_sensitive)?,
                        original: value,
                    },
                },
                Predicate::ContentExcludes(value) => CompiledPredicate::NotContent {
                    detail: format!("no {} content match for {value:?}", query.mode.label()),
                    matcher: TextMatcher::new_case(query.mode, value, case_sensitive)?,
                },
                Predicate::ContentContains(value) => CompiledPredicate::Content(
                    TextMatcher::new_case(query.mode, value, case_sensitive)?,
                ),
                Predicate::Extension(value) => {
                    CompiledPredicate::Metadata(Predicate::Extension(if case_sensitive {
                        value
                    } else {
                        value.to_lowercase()
                    }))
                }
                Predicate::ExtensionsAny(values) => {
                    CompiledPredicate::Metadata(Predicate::ExtensionsAny(
                        values
                            .into_iter()
                            .map(|value| {
                                if case_sensitive {
                                    value
                                } else {
                                    value.to_lowercase()
                                }
                            })
                            .collect(),
                    ))
                }
                Predicate::ModifiedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Modified,
                    after: true,
                    detail: format!("after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::ModifiedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Modified,
                    after: false,
                    detail: format!("before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::AccessedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Accessed,
                    after: true,
                    detail: format!("accessed-after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::AccessedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Accessed,
                    after: false,
                    detail: format!("accessed-before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::CreatedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Created,
                    after: true,
                    detail: format!("created-after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::CreatedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Created,
                    after: false,
                    detail: format!("created-before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::ChangedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Changed,
                    after: true,
                    detail: format!("changed-after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::ChangedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Changed,
                    after: false,
                    detail: format!("changed-before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                metadata => CompiledPredicate::Metadata(metadata),
            };
            predicates.push(compiled);
        }
        Ok(Self {
            case_sensitive,
            search_archives,
            hidden_mode,
            exclusions,
            directory_exclusions,
            max_depth,
            predicates,
            requires_content,
            requires_empty,
            requires_broken_symlink,
            requires_complete_content,
            requires_boundary_context,
            requires_symlink_kind,
            requires_executable_kind,
            requires_regular_kind,
            requires_unix_mode,
            requires_file_type_bits,
            requires_allocation,
            requires_links,
        })
    }

    /// `root` is the search root: `path:` matches the path below it, as the
    /// Find list shows it, so terms in the root itself do not match every hit.
    fn evaluate_metadata(
        &self,
        entry: &FileEntry,
        root: &Path,
        now: SystemTime,
        observed: ObservedMetadata,
    ) -> Option<MatchExplanation> {
        if self.hidden_mode != crate::query::HiddenMode::Include {
            let hidden = dot_hidden_path(&entry.path, root);
            if (self.hidden_mode == crate::query::HiddenMode::Only) != hidden {
                return None;
            }
        }
        if excluded_path(&entry.path, root, &self.exclusions)
            || (entry.is_dir
                && self
                    .directory_exclusions
                    .iter()
                    .any(|pattern| pattern.matches_name(&entry.name)))
        {
            return None;
        }
        let mut explanation = MatchExplanation::default();
        for predicate in &self.predicates {
            match predicate {
                CompiledPredicate::UnixPermissions { pattern, detail } => {
                    let mode = observed.unix_mode?;
                    if !pattern.matches(mode) {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: detail.clone(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::NameGlob { original, regex } => {
                    if !regex.is_match(&entry.name) {
                        return None;
                    }
                    explanation.total_score += 100;
                    explanation.components.push(MatchComponent {
                        field: "name",
                        detail: format!("glob {original:?}"),
                        score: 100,
                        matched_ranges: vec![(0, entry.name.chars().count())],
                    });
                }
                CompiledPredicate::ExcludeFileGlob { original, regex } => {
                    if !entry.is_dir && regex.is_match(&entry.name) {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: format!("excludefile:{original}"),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::NotText {
                    path,
                    matcher,
                    detail,
                } => {
                    let candidate = if *path {
                        entry
                            .path
                            .strip_prefix(root)
                            .unwrap_or(&entry.path)
                            .to_string_lossy()
                    } else {
                        std::borrow::Cow::Borrowed(entry.name.as_str())
                    };
                    if matcher.evaluate(&candidate).is_some() {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: if *path { "path" } else { "name" },
                        detail: detail.clone(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::AnyName(matchers) => {
                    let matched = matchers
                        .iter()
                        .find_map(|matcher| matcher.evaluate(&entry.name))?;
                    explanation.total_score += matched.score;
                    explanation.components.push(MatchComponent {
                        field: "name",
                        detail: format!("any name: {}", matched.detail),
                        score: matched.score,
                        matched_ranges: matched.matched_ranges,
                    });
                }
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
                CompiledPredicate::Content(_)
                | CompiledPredicate::AnyContent(_)
                | CompiledPredicate::NotContent { .. } => {}
                CompiledPredicate::Timestamp {
                    field,
                    after,
                    boundary,
                    detail,
                } => {
                    let modified = field.observed(entry)?;
                    if !if *after {
                        modified > *boundary
                    } else {
                        modified < *boundary
                    } {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: detail.clone(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::Metadata(predicate) => {
                    let matches = match predicate {
                        Predicate::DeviceId { id, exclude } => observed
                            .device_id
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::Inode { id, exclude } => observed
                            .inode
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::OwnerId { id, exclude } => observed
                            .owner_uid
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::GroupId { id, exclude } => observed
                            .owner_gid
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::Empty => observed.is_empty == Some(true),
                        Predicate::TypesAny(types) => {
                            observed.matches_types(entry, types) == Some(true)
                        }
                        Predicate::SymlinksOnly => observed.is_symlink == Some(true),
                        Predicate::BrokenSymlinks => observed.is_broken_symlink == Some(true),
                        Predicate::ExecutableFiles => observed.is_executable == Some(true),
                        Predicate::RegularFiles => observed.is_regular == Some(true),
                        Predicate::SpecialFileType(kind) => {
                            observed.file_type_bits == Some(kind.bits())
                        }
                        Predicate::Extension(value) if self.case_sensitive => {
                            !entry.is_dir && crate::query::extension_matches(&entry.name, value)
                        }
                        Predicate::ExtensionsAny(values) if self.case_sensitive => {
                            !entry.is_dir
                                && values.iter().any(|value| {
                                    crate::query::extension_matches(&entry.name, value)
                                })
                        }
                        Predicate::MinDepth(min) => {
                            search_depth(&entry.path, root).is_some_and(|depth| depth >= *min)
                        }
                        Predicate::MaxDepth(max) => {
                            search_depth(&entry.path, root).is_some_and(|depth| depth <= *max)
                        }
                        Predicate::MinLinks(_) | Predicate::MaxLinks(_) => {
                            observed.is_regular == Some(true)
                                && observed.is_symlink == Some(false)
                                && !entry.is_dir
                                && predicate.matches_metadata(entry, now)
                        }
                        _ => predicate.matches_metadata(entry, now),
                    };
                    if !matches {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: match predicate {
                            Predicate::DeviceId { .. } => format!(
                                "{} (observed device {})",
                                predicate.chip(),
                                observed.device_id?
                            ),
                            Predicate::Inode { .. } => {
                                format!("{} (observed inode {})", predicate.chip(), observed.inode?)
                            }
                            Predicate::MinLinks(_) | Predicate::MaxLinks(_) => format!(
                                "{} (observed {} hard links)",
                                predicate.chip(),
                                entry.hard_link_count()?
                            ),
                            Predicate::MinAllocated(_) | Predicate::MaxAllocated(_) => format!(
                                "{} (observed {} allocated bytes)",
                                predicate.chip(),
                                entry.allocated_bytes()?
                            ),
                            _ => predicate.chip(),
                        },
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
                let Some((mut file, size)) = open_regular_file(&entry.path) else {
                    return ContentVerdict::Skipped;
                };
                let Some(bytes) = read_text_bytes(&mut file, size) else {
                    return ContentVerdict::Skipped;
                };
                let mut boundary_unknown = size > MAX_CONTENT_BYTES;
                if self.requires_complete_content || self.requires_boundary_context {
                    let mut extra = [0u8; 1];
                    let eof = file.read(&mut extra).ok() == Some(0);
                    if self.requires_complete_content && (size > MAX_CONTENT_BYTES || !eof) {
                        return ContentVerdict::Skipped;
                    }
                    boundary_unknown = !eof;
                }
                let content = crate::search_text::decode(&bytes);
                self.evaluate_content_text_with_boundaries(
                    &content,
                    size > MAX_CONTENT_BYTES,
                    boundary_unknown,
                    explanation,
                )
            }
            CandidateContent::Indexed { index, document } => {
                let Some(document) = index.documents.get(*document) else {
                    return ContentVerdict::Skipped;
                };
                let Some(content) = document.content.as_deref() else {
                    return ContentVerdict::Skipped;
                };
                if self.requires_complete_content && !document.content_complete {
                    return ContentVerdict::Skipped;
                }
                self.evaluate_content_text_with_boundaries(
                    content,
                    false,
                    !document.content_complete,
                    explanation,
                )
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
        self.evaluate_content_text_with_boundaries(content, partial, partial, explanation)
    }

    fn evaluate_content_text_with_boundaries(
        &self,
        content: &str,
        partial: bool,
        boundary_unknown: bool,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        if (partial || boundary_unknown) && self.requires_complete_content {
            return ContentVerdict::Skipped;
        }
        for predicate in &self.predicates {
            if let CompiledPredicate::NotContent { matcher, detail } = predicate {
                if matcher.evaluate(content).is_some() {
                    return ContentVerdict::NotMatched;
                }
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: detail.clone(),
                    score: 0,
                    matched_ranges: Vec::new(),
                });
            }
            if let CompiledPredicate::AnyContent(matchers) = predicate {
                let Some(matched) = matchers
                    .iter()
                    .find_map(|matcher| matcher.evaluate(content))
                else {
                    return ContentVerdict::NotMatched;
                };
                explanation.total_score += matched.score;
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: if partial {
                        format!("any content: {}, first 16MB", matched.detail)
                    } else {
                        format!("any content: {}", matched.detail)
                    },
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                });
            }
            if let CompiledPredicate::Content(matcher) = predicate {
                let Some(matched) = matcher.evaluate_bounded(content, boundary_unknown) else {
                    return if boundary_unknown
                        && matches!(
                            matcher,
                            TextMatcher::WholeWord { .. } | TextMatcher::WholeLine { .. }
                        ) {
                        ContentVerdict::Skipped
                    } else {
                        ContentVerdict::NotMatched
                    };
                };
                explanation.total_score += matched.score;
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: if partial {
                        format!("{}, first 16MB", matched.detail)
                    } else if boundary_unknown
                        && matches!(
                            matcher,
                            TextMatcher::WholeWord { .. } | TextMatcher::WholeLine { .. }
                        )
                    {
                        format!("{}, bounded cached text", matched.detail)
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
    if crate::search_text::binary_head(&bytes) {
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
        archive_error: request.archive_error.take(),
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
        is_symlink,
        is_executable,
        is_regular,
        unix_mode,
        owner_uid,
        owner_gid,
        device_id,
        inode,
        file_type_bits,
        is_empty,
        is_broken_symlink,
        identity,
        accessed,
        content,
        location,
    } = candidate;
    let mut entry = entry;
    // Archive pivots retain the container access time, but member predicates
    // must never borrow that observation as the member's own metadata.
    entry.accessed = if matches!(location, SearchLocation::Native(_)) {
        accessed
    } else {
        None
    };
    if matches!(location, SearchLocation::ArchiveMember { .. }) {
        entry.created = None;
        entry.changed = None;
    }
    let observed = ObservedMetadata {
        is_symlink,
        is_executable,
        is_regular,
        unix_mode,
        owner_uid,
        owner_gid,
        device_id,
        inode,
        file_type_bits,
        is_empty,
        is_broken_symlink,
    };
    if query.predicates.iter().any(|predicate| matches!(predicate, CompiledPredicate::Metadata(Predicate::DeviceId { .. }) if device_id.is_none()) || matches!(predicate, CompiledPredicate::Metadata(Predicate::Inode { .. }) if inode.is_none()))
        || query.predicates.iter().any(|predicate| matches!(predicate,
        CompiledPredicate::Metadata(Predicate::OwnerId { .. }) if owner_uid.is_none()) || matches!(predicate,
        CompiledPredicate::Metadata(Predicate::GroupId { .. }) if owner_gid.is_none()))
        || (query.requires_symlink_kind && is_symlink.is_none())
        || query.predicates.iter().any(|predicate| matches!(predicate,CompiledPredicate::Metadata(Predicate::BrokenSymlinks) if is_broken_symlink.is_none()))
        || (query.requires_executable_kind && is_executable.is_none())
        || (query.requires_regular_kind && is_regular.is_none())
        || (query.requires_unix_mode && unix_mode.is_none())
        || (query.requires_file_type_bits && file_type_bits.is_none())
        || (query.requires_links && entry.hard_link_count().is_none() && !entry.is_dir && is_regular!=Some(false) && is_symlink!=Some(true))
        || (query.requires_allocation && entry.allocated_bytes().is_none()
            && !entry.is_dir && is_regular!=Some(false) && is_symlink!=Some(true))
        || query.predicates.iter().any(|predicate| matches!(predicate,
            CompiledPredicate::Metadata(Predicate::Empty) if is_empty.is_none()))
        || query.predicates.iter().any(|predicate| matches!(predicate,
            CompiledPredicate::Metadata(Predicate::TypesAny(types)) if observed.matches_types(&entry, types).is_none()))
    {
        // Missing observed metadata cannot prove this filter. Keep partial coverage visible.
        *content_skipped = content_skipped.saturating_add(1);
        return None;
    }
    if query.predicates.iter().any(|p|matches!(p,CompiledPredicate::Timestamp{field,..} if !matches!(field,TimestampField::Modified) && field.observed(&entry).is_none())) {
        *content_skipped=content_skipped.saturating_add(1);return None;
    }
    let mut explanation = query.evaluate_metadata(&entry, root, now, observed)?;
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
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::io::Write;

    fn file_entry(path: PathBuf, size: u64) -> FileEntry {
        FileEntry {
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
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            name_lower: path.file_name().unwrap().to_string_lossy().to_lowercase(),
            sort_key: crate::panel::diacritic_sort_key(
                &path.file_name().unwrap().to_string_lossy().to_lowercase(),
            ),
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
    fn glob_query_filters_recursive_results_before_content_inspection() {
        let tmp = TempDir::new();
        tmp.file("reports/report-年.txt", "needle");
        tmp.file("reports/report-2.txt", "no match");
        tmp.file("reports/report-3.txt.bak", "needle");
        let query = Query::parse("glob:report-?.txt content:needle", MatchMode::Exact).unwrap();
        let run = SearchEngine::default()
            .start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert_eq!(summary.matched, 1);
        assert_eq!(hits[0].entry.name, "report-年.txt");
        let component = hits[0]
            .explanation
            .components
            .iter()
            .find(|component| component.field == "name")
            .unwrap();
        assert_eq!(
            component.matched_ranges,
            [(0, "report-年.txt".chars().count())]
        );
    }

    #[test]
    fn depth_query_agrees_across_live_indexed_and_archive_results() {
        let tmp = TempDir::new();
        tmp.file("root.txt", "needle");
        tmp.file("sub/child.txt", "needle");
        tmp.file("sub/deep/leaf.txt", "needle");
        write_zip(&tmp.path().join("bundle.zip"), "nested/arch.txt", b"needle");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for (expression, expected) in [
                ("depth:<=1", vec!["root.txt"]),
                ("depth:2", vec!["child.txt"]),
                ("depth:>=3", vec!["arch.txt", "leaf.txt"]),
                ("depth:1..2", vec!["child.txt", "root.txt"]),
                ("depth:0", vec![]),
                ("depth:<=1 depth:<=3", vec!["root.txt"]),
            ] {
                let query = Query::parse(
                    &format!("{expression} glob:*.txt content:needle"),
                    MatchMode::Exact,
                )
                .unwrap();
                let engine = SearchEngine::default();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                let (hits, summary) = collect_run(&run);
                let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                names.sort_unstable();
                assert_eq!(names, expected, "{indexed}: {expression}");
                assert_eq!(summary.content_skipped, 0);
                if !indexed && expression == "depth:<=1" {
                    assert_eq!(
                        summary.scanned, 3,
                        "walk and archive expansion must stop at the depth boundary"
                    );
                }
            }
        }
        assert_eq!(search_depth(Path::new("/outside/name"), tmp.path()), None);
        assert_eq!(
            search_depth(
                Path::new("/bundle.zip!/nested/file"),
                Path::new("/bundle.zip")
            ),
            Some(2)
        );
    }

    #[test]
    fn exclusion_query_prunes_live_indexed_and_archive_descendants() {
        let tmp = TempDir::new();
        // The root itself must not be excluded because of its basename.
        let root = tmp.path().join("node_modules");
        for path in [
            "keep.txt",
            "backup.BAK",
            "nested/node_modules/skipped.txt",
            "nested/keep2.txt",
            "build cache/no.txt",
            "nested/cafÉ.txt",
        ] {
            tmp.file(&format!("node_modules/{path}"), "needle");
        }
        tmp.file("node_modules/ignored.zip", "broken archive");
        tmp.file(
            "node_modules/nested/node_modules/broken.zip",
            "broken archive",
        );
        write_zip(&root.join("good.zip"), "node_modules/hidden.txt", b"needle");
        write_zip(&root.join("more.zip"), "docs/inside.txt", b"needle");
        let index = Arc::new(crate::content_index::build_test_index(&root));
        let expression = r#"exclude:node_modules exclude:*.bak exclude:"build cache" exclude:ignored.zip exclude:caf?.txt glob:*.txt content:needle"#;
        for indexed in [false, true] {
            let engine = SearchEngine::default();
            let query = Query::parse(expression, MatchMode::Regex).unwrap();
            let run = if indexed {
                engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
            } else {
                engine.start(root.clone(), query, 100, Arc::new(|| {}))
            }
            .unwrap();
            let (hits, summary) = collect_run(&run);
            let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(
                names,
                ["inside.txt", "keep.txt", "keep2.txt"],
                "indexed={indexed}"
            );
            assert_eq!(
                summary.content_skipped, 0,
                "excluded broken archives must not be opened"
            );
            assert!(summary.archive_error.is_none());
            if !indexed {
                assert_eq!(
                    summary.scanned, 6,
                    "excluded paths must not reach the candidate stream"
                );
            }
        }
    }

    #[test]
    fn exact_size_file_query_agrees_for_live_indexed_and_archive_entries() {
        let tmp = TempDir::new();
        tmp.file("empty.txt", "");
        tmp.file("nested/also-empty.txt", "");
        tmp.file("one.txt", "x");
        std::fs::create_dir_all(tmp.path().join("empty-folder")).unwrap();
        write_zip(&tmp.path().join("bundle.zip"), "inside/arch-empty.txt", b"");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            let query = Query::parse("type:file size:=0", MatchMode::Exact).unwrap();
            let engine = SearchEngine::default();
            let run = if indexed {
                engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
            } else {
                engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
            }
            .unwrap();
            let (hits, summary) = collect_run(&run);
            let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(
                names,
                ["also-empty.txt", "arch-empty.txt", "empty.txt"],
                "indexed={indexed}"
            );
            assert_eq!(summary.content_skipped, 0);
            assert!(
                hits.iter()
                    .all(|hit| !hit.entry.is_dir && hit.entry.size == 0)
            );
        }
    }

    #[test]
    fn archive_scope_query_controls_expansion_without_hiding_archive_files() {
        let tmp = TempDir::new();
        tmp.file("outside.txt", "needle");
        tmp.file("broken.zip", "broken archive");
        write_zip(&tmp.path().join("bundle.zip"), "inside.txt", b"needle");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for (control, expected, archive_error) in [
                ("archives:off", vec!["outside.txt"], false),
                ("archives:on", vec!["inside.txt", "outside.txt"], true),
                ("", vec!["inside.txt", "outside.txt"], true),
            ] {
                let engine = SearchEngine::default();
                let query = Query::parse(
                    &format!("{control} glob:*.txt content:needle"),
                    MatchMode::Exact,
                )
                .unwrap();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                assert_eq!(
                    run.provider.contains("archives off"),
                    control == "archives:off"
                );
                let (hits, summary) = collect_run(&run);
                let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                names.sort_unstable();
                assert_eq!(names, expected, "{indexed}: {control}");
                assert_eq!(summary.archive_error.is_some(), archive_error);
                if !archive_error {
                    assert_eq!(summary.content_skipped, 0);
                }
            }
            let engine = SearchEngine::default();
            let query = Query::parse("archives:off glob:*.zip", MatchMode::Exact).unwrap();
            let run = if indexed {
                engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
            } else {
                engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
            }
            .unwrap();
            let (hits, summary) = collect_run(&run);
            let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, ["broken.zip", "bundle.zip"]);
            assert!(summary.archive_error.is_none());
        }
    }

    #[test]
    fn exact_name_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for name in [
            "Report[1]*?.txt",
            "old-Report[1]*?.txt",
            "Report[1]*?.txt.bak",
            "Report1xx.txt",
        ] {
            std::fs::write(tmp.path().join(name), b"payload").unwrap();
        }
        std::fs::create_dir(tmp.path().join("folded")).unwrap();
        std::fs::write(tmp.path().join("folded/report[1]*?.txt"), b"payload").unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for name in ["nested/Report[1]*?.txt", "zip-other.txt"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"payload").unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for name in ["nested/Report[1]*?.txt", "tar-other.txt"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"payload"[..]).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected) in [
                    (
                        r#"nameexact:"Report[1]*?.txt""#,
                        vec![
                            "Report[1]*?.txt",
                            "Report[1]*?.txt",
                            "Report[1]*?.txt",
                            "report[1]*?.txt",
                        ],
                    ),
                    (
                        r#"case:sensitive nameexact:"Report[1]*?.txt""#,
                        vec!["Report[1]*?.txt"; 3],
                    ),
                    (
                        r#"case:smart nameexact:"Report[1]*?.txt""#,
                        vec!["Report[1]*?.txt"; 3],
                    ),
                    (
                        r#"notnameexact:"Report[1]*?.txt" glob:*.txt"#,
                        vec![
                            "Report1xx.txt",
                            "old-Report[1]*?.txt",
                            "tar-other.txt",
                            "zip-other.txt",
                        ],
                    ),
                    (
                        r#"name:Report notnameexact:"Report[1]*?.txt" glob:*.txt"#,
                        vec!["Report1xx.txt", "old-Report[1]*?.txt"],
                    ),
                    (
                        r#"nameexact:"Report[1]*?.txt" notnameexact:"Report[1]*?.txt""#,
                        vec![],
                    ),
                    (
                        r#"nameexact:"Report[1]*?.txt" content:payload"#,
                        vec![
                            "Report[1]*?.txt",
                            "Report[1]*?.txt",
                            "Report[1]*?.txt",
                            "report[1]*?.txt",
                        ],
                    ),
                ] {
                    let query = Query::parse(conditions, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, 0);
                    for hit in hits {
                        if !conditions.contains("notnameexact:") {
                            let component = hit
                                .explanation
                                .components
                                .iter()
                                .find(|component| component.field == "name")
                                .unwrap();
                            assert!(component.detail.starts_with("whole name literal"));
                            assert_eq!(component.score, 120);
                            assert_eq!(
                                component.matched_ranges,
                                vec![(0, hit.entry.name.chars().count())]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn any_name_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for name in [
            "invoice.txt",
            "receipt.txt",
            "invoice-receipt.txt",
            "other.txt",
            "invoice.bin",
        ] {
            std::fs::write(tmp.path().join(name), b"unrelated content").unwrap();
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for name in ["nested/zip-invoice.txt", "zip-receipt.txt", "zip-other.txt"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"unrelated content").unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for name in ["nested/tar-invoice.txt", "tar-receipt.txt", "tar-other.txt"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(17);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"unrelated content"[..])
                .unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected) in [
                    (
                        "anyname:invoice anyname:receipt",
                        vec![
                            "invoice-receipt.txt",
                            "invoice.txt",
                            "receipt.txt",
                            "tar-invoice.txt",
                            "tar-receipt.txt",
                            "zip-invoice.txt",
                            "zip-receipt.txt",
                        ],
                    ),
                    (
                        "anyname:invoice anyname:receipt notname:invoice",
                        vec!["receipt.txt", "tar-receipt.txt", "zip-receipt.txt"],
                    ),
                    ("name:invoice name:receipt", vec!["invoice-receipt.txt"]),
                    ("case:smart anyname:INVOICE anyname:RECEIPT", vec![]),
                    (
                        "anyname:invoice anyname:receipt content:unrelated",
                        vec![
                            "invoice-receipt.txt",
                            "invoice.txt",
                            "receipt.txt",
                            "tar-invoice.txt",
                            "tar-receipt.txt",
                            "zip-invoice.txt",
                            "zip-receipt.txt",
                        ],
                    ),
                    (
                        "anyname:invoice anyname:receipt anycontent:missing anycontent:unrelated",
                        vec![
                            "invoice-receipt.txt",
                            "invoice.txt",
                            "receipt.txt",
                            "tar-invoice.txt",
                            "tar-receipt.txt",
                            "zip-invoice.txt",
                            "zip-receipt.txt",
                        ],
                    ),
                ] {
                    let query = Query::parse(&format!("ext:txt {conditions}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, _) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    for hit in hits {
                        if conditions.starts_with("anyname:") {
                            let component = hit
                                .explanation
                                .components
                                .iter()
                                .find(|component| component.field == "name")
                                .unwrap();
                            assert!(component.detail.starts_with("any name:"));
                            assert!(!component.matched_ranges.is_empty());
                            if hit.entry.name == "invoice-receipt.txt" {
                                assert!(component.detail.contains("invoice"));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn any_content_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for (name, text) in [
            ("error.txt", "error ready"),
            ("warn.txt", "warning ready"),
            ("both.txt", "error warning"),
            ("other.txt", "ok"),
            ("binary.txt", "\0binary"),
        ] {
            tmp.file(name, text);
        }
        std::fs::write(
            tmp.path().join("encoded.txt"),
            crate::search_text::fixture("warning ready", 2, true),
        )
        .unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for (name, text) in [("zip-error.txt", "error"), ("zip-warn.txt", "warning")] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, text) in [("tar-error.txt", "error"), ("tar-warn.txt", "warning")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, text.as_bytes()).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected) in [
                    (
                        "anycontent:error anycontent:warn",
                        vec![
                            "both.txt",
                            "encoded.txt",
                            "error.txt",
                            "tar-error.txt",
                            "tar-warn.txt",
                            "warn.txt",
                            "zip-error.txt",
                            "zip-warn.txt",
                        ],
                    ),
                    (
                        "anycontent:error anycontent:warn content:ready",
                        vec!["encoded.txt", "error.txt", "warn.txt"],
                    ),
                    (
                        "anycontent:error anycontent:warn notcontent:warning",
                        vec!["error.txt", "tar-error.txt", "zip-error.txt"],
                    ),
                    ("content:error content:warn", vec!["both.txt"]),
                    ("case:smart anycontent:ERROR anycontent:WARN", vec![]),
                ] {
                    let query = Query::parse(&format!("ext:txt {conditions}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, 1);
                }
            }
        }
    }

    #[test]
    fn any_content_query_reports_first_matching_pattern_and_preserves_literal_delimiters() {
        for mode in MatchMode::ALL {
            let query = Query::parse("anycontent:error anycontent:warn", mode).unwrap();
            let compiled = CompiledQuery::new(query).unwrap();
            let mut explanation = MatchExplanation::default();
            assert!(matches!(
                compiled.evaluate_content_text("é error warning", false, &mut explanation),
                ContentVerdict::Matched
            ));
            assert_eq!(explanation.components.len(), 1);
            assert!(explanation.components[0].detail.contains("error"));
            assert_eq!(explanation.components[0].matched_ranges[0].0, 2);
        }
        let compiled = CompiledQuery::new(
            Query::parse(
                r#"anycontent:"error|warning" anycontent:"a,b""#,
                MatchMode::Exact,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            compiled.evaluate_content_text("error", false, &mut MatchExplanation::default()),
            ContentVerdict::NotMatched
        ));
        assert!(matches!(
            compiled.evaluate_content_text("a,b", false, &mut MatchExplanation::default()),
            ContentVerdict::Matched
        ));
        let compiled = CompiledQuery::new(
            Query::parse(
                "anycontent:foo anycontent:bar notcontent:missing",
                MatchMode::Exact,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            compiled.evaluate_content_text("foo", true, &mut MatchExplanation::default()),
            ContentVerdict::Skipped
        ));
    }

    #[test]
    fn bom_search_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        let text = "READY\r\nкот 🐱\r\n";
        let mut fixtures = vec![("plain.txt".to_string(), text.as_bytes().to_vec())];
        for width in [1, 2, 4] {
            for little in [false, true] {
                fixtures.push((
                    format!("utf{}-{}.txt", width * 8, if little { "le" } else { "be" }),
                    crate::search_text::fixture(text, width, little),
                ));
            }
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        let mut all = Vec::new();
        for (name, bytes) in &fixtures {
            std::fs::write(tmp.path().join(name), bytes).unwrap();
            all.push(name.clone());
            all.push(format!("zip-{name}"));
            all.push(format!("tar-{name}"));
            zip.start_file(
                format!("zip-{name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("tar-{name}"), &bytes[..])
                .unwrap();
        }
        zip.finish().unwrap();
        tar.finish().unwrap();
        all.sort_unstable();
        tmp.file("binary.txt", "\0binary");
        std::fs::write(tmp.path().join("unmarked.txt"), b"R\0E\0A\0D\0Y\0").unwrap();
        std::fs::write(
            tmp.path().join("nul.txt"),
            crate::search_text::fixture("a\0b", 2, true),
        )
        .unwrap();
        std::fs::write(tmp.path().join("only-bom.txt"), [0xff, 0xfe]).unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        assert!(
            built
                .documents
                .iter()
                .filter(|document| document
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("utf"))
                .all(|document| document.content.as_deref() == Some(text))
        );
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        let mut absent = all.clone();
        absent.push("only-bom.txt".into());
        absent.sort_unstable();
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    ("content:кот", all.clone()),
                    ("contentline:READY", all.clone()),
                    ("contentword:кот", all.clone()),
                    ("notcontent:READY", vec!["only-bom.txt".into()]),
                    ("notcontentword:missing", absent.clone()),
                ] {
                    let query = Query::parse(&format!("ext:txt {condition}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.clone()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 3);
                }
            }
        }
    }

    #[test]
    fn bom_search_utf16_surrogate_cut_cannot_manufacture_a_word_boundary() {
        let tmp = TempDir::new();
        let path = tmp.path().join("large.txt");
        let cap = MAX_CONTENT_BYTES as usize;
        let mut bytes = vec![0u8; cap + 2];
        for safe in [false, true] {
            for pair in bytes.as_chunks_mut::<2>().0 {
                *pair = [0x20, 0];
            }
            bytes[..2].copy_from_slice(&[0xff, 0xfe]);
            bytes[cap - 8..cap].copy_from_slice(&[b'c', 0, b'a', 0, b't', 0, 0, 0xd8]);
            bytes[cap..].copy_from_slice(&[0, 0xdc]);
            if safe {
                bytes[2..8].copy_from_slice(&[b'c', 0, b'a', 0, b't', 0]);
            }
            std::fs::write(&path, &bytes).unwrap();
            let engine = SearchEngine::default();
            let (hits, summary) = collect_run(
                &engine
                    .start(
                        tmp.path().to_path_buf(),
                        Query::parse("contentword:cat archives:off", MatchMode::Exact).unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            assert_eq!(!hits.is_empty(), safe);
            assert_eq!(summary.content_skipped, usize::from(!safe));
        }
    }

    #[test]
    fn hidden_search_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for name in [
            "visible.txt",
            ".dot.txt",
            ".cache/inside.txt",
            "visible/.nested/deep.txt",
            "visible/.leaf.txt",
        ] {
            tmp.file(name, "needle");
        }
        tmp.file(".cache/binary.txt", "\0binary");
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for name in [
            "zip-visible.txt",
            ".zip-hidden/zip-child.txt",
            ".zip-dot.txt",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.finish().unwrap();
        write_zip(
            &tmp.path().join(".hidden.zip"),
            "zip-container.txt",
            b"needle",
        );
        write_zip(
            &tmp.path().join(".cache/hidden-dir.zip"),
            "inside-archive.txt",
            b"needle",
        );
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for name in [
            "tar-visible.txt",
            ".tar-hidden/tar-child.txt",
            ".tar-dot.txt",
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"needle"[..]).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        let all = vec![
            ".dot.txt",
            ".leaf.txt",
            ".tar-dot.txt",
            ".zip-dot.txt",
            "deep.txt",
            "inside-archive.txt",
            "inside.txt",
            "tar-child.txt",
            "tar-visible.txt",
            "visible.txt",
            "zip-child.txt",
            "zip-container.txt",
            "zip-visible.txt",
        ];
        let hidden = vec![
            ".dot.txt",
            ".leaf.txt",
            ".tar-dot.txt",
            ".zip-dot.txt",
            "deep.txt",
            "inside-archive.txt",
            "inside.txt",
            "tar-child.txt",
            "zip-child.txt",
            "zip-container.txt",
        ];
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected, skipped) in [
                    ("", all.clone(), 1),
                    ("hidden:include", all.clone(), 1),
                    ("hidden:only", hidden.clone(), 1),
                    (
                        "hidden:exclude",
                        vec!["tar-visible.txt", "visible.txt", "zip-visible.txt"],
                        0,
                    ),
                ] {
                    let query =
                        Query::parse(&format!("ext:txt content:needle {condition}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, skipped);
                }
            }
        }
    }

    #[test]
    fn hidden_search_query_exempts_an_explicit_hidden_root_and_keeps_member_scope() {
        let tmp = TempDir::new();
        let root = tmp.path().join(".chosen");
        tmp.file(".chosen/visible.txt", "needle");
        tmp.file(".chosen/.leaf.txt", "needle");
        tmp.file(".chosen/.folder/child.txt", "needle");
        write_zip(&root.join("visible.zip"), "zip-visible.txt", b"needle");
        let index = Arc::new(crate::content_index::build_test_index(&root));
        for indexed in [false, true] {
            for (condition, expected) in [
                ("hidden:exclude", vec!["visible.txt", "zip-visible.txt"]),
                ("hidden:only", vec![".leaf.txt", "child.txt"]),
            ] {
                let query = Query::parse(
                    &format!("ext:txt content:needle {condition}"),
                    MatchMode::Exact,
                )
                .unwrap();
                let engine = SearchEngine::default();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(root.clone(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                let (hits, summary) = collect_run(&run);
                let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                names.sort_unstable();
                assert_eq!(names, expected);
                assert_eq!(summary.content_skipped, 0);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn hidden_search_query_dot_detection_preserves_non_utf8_name_bytes() {
        use std::os::unix::ffi::OsStrExt;
        assert!(dot_name(std::ffi::OsStr::from_bytes(b".\xff")));
        assert!(!dot_name(std::ffi::OsStr::from_bytes(b"x.\xff")));
        assert!(!dot_hidden_path(
            Path::new("/scope/.chosen/visible"),
            Path::new("/scope/.chosen")
        ));
        assert!(dot_hidden_path(
            Path::new("/scope/.chosen/.cache/file"),
            Path::new("/scope/.chosen")
        ));
        assert!(!dot_hidden_path(
            Path::new("/other/.cache/file"),
            Path::new("/scope")
        ));
    }

    #[test]
    fn smart_case_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for (name, text) in [
            ("a.txt", "READY"),
            ("b.txt", "ready"),
            ("caps.TXT", "ready"),
        ] {
            tmp.file(name, text);
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for (name, text) in [("zip-upper.txt", "READY"), ("zip-lower.txt", "ready")] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, text) in [("tar-upper.txt", "READY"), ("tar-lower.txt", "ready")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, text.as_bytes()).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "case:smart content:ready ext:txt",
                        vec![
                            "a.txt",
                            "b.txt",
                            "caps.TXT",
                            "tar-lower.txt",
                            "tar-upper.txt",
                            "zip-lower.txt",
                            "zip-upper.txt",
                        ],
                    ),
                    (
                        "case:smart content:READY ext:txt",
                        vec!["a.txt", "tar-upper.txt", "zip-upper.txt"],
                    ),
                    (
                        "case:smart contentword:READY ext:txt",
                        vec!["a.txt", "tar-upper.txt", "zip-upper.txt"],
                    ),
                    (
                        "case:smart contentline:ready ext:txt",
                        vec![
                            "a.txt",
                            "b.txt",
                            "caps.TXT",
                            "tar-lower.txt",
                            "tar-upper.txt",
                            "zip-lower.txt",
                            "zip-upper.txt",
                        ],
                    ),
                    ("case:smart ext:TXT archives:off", vec!["caps.TXT"]),
                    (
                        "case:smart notcontent:READY ext:txt",
                        vec!["b.txt", "tar-lower.txt", "zip-lower.txt"],
                    ),
                ] {
                    let query = Query::parse(condition, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
    }

    #[test]
    fn smart_case_query_regex_classes_escapes_and_inline_flags_keep_their_meaning() {
        for (pattern, text, expected) in [
            (r"ready\S*", "READY", true),
            (r"\p{Lu}+", "ready", false),
            (r"\p{Lu}+", "READY", true),
            (r"(?i:READY)", "ready", true),
            (r"\x{41}bc", "abc", false),
            (r"\x{41}bc", "Abc", true),
        ] {
            let query = Query {
                predicates: vec![
                    Predicate::SmartCase,
                    Predicate::ContentContains(pattern.into()),
                ],
                mode: MatchMode::Regex,
            };
            let compiled = CompiledQuery::new(query).unwrap();
            assert_eq!(
                matches!(
                    compiled.evaluate_content_text(text, false, &mut MatchExplanation::default()),
                    ContentVerdict::Matched
                ),
                expected,
                "{pattern}: {text}"
            );
        }
    }

    #[test]
    fn content_line_query_handles_terminators_spaces_empty_lines_and_prefixes() {
        for (needle, text, expected) in [
            ("cat", "cat", true),
            ("cat", "CAT\n", true),
            ("cat", "cat\r\n", true),
            ("cat", "other\rcat\rnext", true),
            ("cat", "catalog\n", false),
            ("cat", " cat\ncat ", false),
            (" cat ", " cat \r\n", true),
            ("кот", "кот\n", true),
            ("[cat]", "[cat]\n", true),
            ("", "", false),
            ("", "\n", true),
            ("", "\r\n", true),
            ("", "cat\n", false),
            ("", "cat\n\n", true),
            ("", " \n", false),
            ("cat", "cat\u{2028}dog", false),
        ] {
            let matcher = TextMatcher::WholeLine {
                original: needle.into(),
                regex: crate::query::compile_content_line(needle, false).unwrap(),
            };
            assert_eq!(
                matcher.evaluate(text).is_some(),
                expected,
                "{needle:?}: {text:?}"
            );
        }
        let compiled =
            CompiledQuery::new(Query::parse("contentline:cat", MatchMode::Regex).unwrap()).unwrap();
        for text in ["cat", "cat\u{fffd}", "none"] {
            assert!(matches!(
                compiled.evaluate_content_text(text, true, &mut MatchExplanation::default()),
                ContentVerdict::Skipped
            ));
        }
        let mut explanation = MatchExplanation::default();
        assert!(matches!(
            compiled.evaluate_content_text("é\ncat\r\nrest", true, &mut explanation),
            ContentVerdict::Matched
        ));
        assert_eq!(explanation.components[0].matched_ranges, vec![(2, 5)]);
        let negative =
            CompiledQuery::new(Query::parse("notcontentline:", MatchMode::Fuzzy).unwrap()).unwrap();
        assert!(matches!(
            negative.evaluate_content_text("", false, &mut MatchExplanation::default()),
            ContentVerdict::Matched
        ));
        assert!(matches!(
            negative.evaluate_content_text("no blank", true, &mut MatchExplanation::default()),
            ContentVerdict::Skipped
        ));
    }

    #[test]
    fn content_line_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for (name, text) in [
            ("line.txt", "cat\nother\n"),
            ("upper.txt", "CAT\r\n"),
            ("embedded.txt", "catalog\n cat\ncat \n"),
            ("blank.txt", "\r\n"),
            ("empty.txt", ""),
            ("binary.txt", "\0binary"),
        ] {
            tmp.file(name, text);
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for (name, text) in [("zip-line.txt", "cat\r\n"), ("zip-other.txt", "catx")] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, text) in [("tar-line.txt", "cat"), ("tar-other.txt", "catx")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, text.as_bytes()).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "contentline:cat",
                        vec!["line.txt", "tar-line.txt", "upper.txt", "zip-line.txt"],
                    ),
                    (
                        "notcontentline:cat",
                        vec![
                            "blank.txt",
                            "embedded.txt",
                            "empty.txt",
                            "tar-other.txt",
                            "zip-other.txt",
                        ],
                    ),
                    ("case:sensitive contentline:CAT", vec!["upper.txt"]),
                    ("contentline:", vec!["blank.txt"]),
                    (
                        "notcontentline:",
                        vec![
                            "embedded.txt",
                            "empty.txt",
                            "line.txt",
                            "tar-line.txt",
                            "tar-other.txt",
                            "upper.txt",
                            "zip-line.txt",
                            "zip-other.txt",
                        ],
                    ),
                ] {
                    let query = Query::parse(&format!("ext:txt {condition}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 1);
                }
            }
        }
    }

    #[test]
    fn content_line_query_proves_eof_or_known_line_terminators_in_bounded_text() {
        let tmp = TempDir::new();
        let path = tmp.path().join("large.txt");
        let mut bytes = vec![b'x'; MAX_CONTENT_BYTES as usize + 1];
        let cap = MAX_CONTENT_BYTES as usize;
        for (terminated, complete, expected) in [
            (false, false, false),
            (true, false, true),
            (false, true, true),
        ] {
            bytes.fill(b'x');
            if terminated {
                bytes[cap - 5..cap].copy_from_slice(b"\ncat\n");
            } else {
                bytes[cap - 4..cap].copy_from_slice(b"\ncat");
            }
            std::fs::write(&path, if complete { &bytes[..cap] } else { &bytes[..] }).unwrap();
            let engine = SearchEngine::default();
            let query = Query::parse("contentline:cat archives:off", MatchMode::Exact).unwrap();
            let (hits, summary) = collect_run(
                &engine
                    .start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    .unwrap(),
            );
            assert_eq!(!hits.is_empty(), expected, "{terminated}: {complete}");
            assert_eq!(summary.content_skipped, usize::from(!expected));
        }
        let small = TempDir::new();
        small.file("tail.txt", "cat");
        small.file("safe.txt", "cat\nrest");
        let mut index = crate::content_index::build_test_index(small.path());
        for document in &mut index.documents {
            document.content_complete = false;
        }
        let index = Arc::new(index);
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::clone(&index),
                    Query::parse("contentline:cat archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "safe.txt");
        assert_eq!(summary.content_skipped, 1);
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    index,
                    Query::parse("notcontentline:dog archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 2);
    }

    #[test]
    fn content_word_query_handles_unicode_punctuation_and_unknown_boundaries() {
        for (needle, text, expected) in [
            ("cat", "cat", true),
            ("cat", "catalog", false),
            ("cat", "cat_1", false),
            ("cat", "1cat", false),
            ("cat", "(CAT)", true),
            ("cat", "cat cat", true),
            ("кот", "кот!", true),
            ("кот", "котик", false),
            ("é", "é!", true),
            ("a", "a\u{301}", false),
            ("cat", "catβ", false),
            ("$", "$", true),
            ("$", "a$", false),
            ("$", "$a", false),
            ("cat.*", "cat.*", true),
            ("cat.*", "catalog", false),
        ] {
            let matcher = TextMatcher::WholeWord {
                original: needle.into(),
                regex: crate::query::compile_content_word(needle, false).unwrap(),
            };
            assert_eq!(
                matcher.evaluate(text).is_some(),
                expected,
                "{needle}: {text}"
            );
        }
        let compiled =
            CompiledQuery::new(Query::parse("contentword:cat", MatchMode::Fuzzy).unwrap()).unwrap();
        for text in ["cat", "cat\u{fffd}", "none"] {
            assert!(matches!(
                compiled.evaluate_content_text(text, true, &mut MatchExplanation::default()),
                ContentVerdict::Skipped
            ));
        }
        let mut explanation = MatchExplanation::default();
        assert!(matches!(
            compiled.evaluate_content_text("é cat cat", true, &mut explanation),
            ContentVerdict::Matched
        ));
        assert_eq!(explanation.components[0].matched_ranges, vec![(2, 5)]);
        assert!(explanation.components[0].detail.contains("first 16MB"));
        let negative =
            CompiledQuery::new(Query::parse("notcontentword:cat", MatchMode::Exact).unwrap())
                .unwrap();
        assert!(matches!(
            negative.evaluate_content_text("catalog", true, &mut MatchExplanation::default()),
            ContentVerdict::Skipped
        ));
    }

    #[test]
    fn content_word_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        for (name, text) in [
            ("word.txt", "cat"),
            ("upper.txt", "CAT"),
            ("embedded.txt", "catalog cat_1"),
            ("empty.txt", ""),
            ("binary.txt", "\0binary"),
        ] {
            tmp.file(name, text);
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for (name, text) in [("zip-word.txt", "cat"), ("zip-embedded.txt", "catalog")] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, text) in [("tar-word.txt", "cat"), ("tar-embedded.txt", "catalog")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, text.as_bytes()).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "contentword:cat",
                        vec!["tar-word.txt", "upper.txt", "word.txt", "zip-word.txt"],
                    ),
                    (
                        "notcontentword:cat",
                        vec![
                            "embedded.txt",
                            "empty.txt",
                            "tar-embedded.txt",
                            "zip-embedded.txt",
                        ],
                    ),
                    ("case:sensitive contentword:CAT", vec!["upper.txt"]),
                    (
                        "contentword:cat notcontentword:dog",
                        vec!["tar-word.txt", "upper.txt", "word.txt", "zip-word.txt"],
                    ),
                ] {
                    let query = Query::parse(&format!("ext:txt {condition}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 1);
                }
            }
        }
    }

    #[test]
    fn content_word_query_retains_safe_prefix_hits_but_requires_eof_for_tail() {
        let tmp = TempDir::new();
        let path = tmp.path().join("large.txt");
        let mut bytes = vec![b' '; MAX_CONTENT_BYTES as usize];
        let cap = bytes.len();
        bytes[cap - 3..].copy_from_slice(b"cat");
        bytes.push(b'a');
        for (cut_utf8, safe_prefix, complete, expected) in [
            (false, false, false, false),
            (true, false, false, false),
            (true, true, false, true),
            (false, false, true, true),
        ] {
            bytes.fill(b' ');
            if cut_utf8 {
                bytes[cap - 4..cap - 1].copy_from_slice(b"cat");
                bytes[cap - 1] = 0xce;
                bytes[cap] = 0xb2;
            } else {
                bytes[cap - 3..cap].copy_from_slice(b"cat");
                bytes[cap] = b'a';
            }
            if safe_prefix {
                bytes[..3].copy_from_slice(b"cat");
            }
            std::fs::write(&path, if complete { &bytes[..cap] } else { &bytes[..] }).unwrap();
            let query = Query::parse("contentword:cat archives:off", MatchMode::Exact).unwrap();
            let engine = SearchEngine::default();
            let (hits, summary) = collect_run(
                &engine
                    .start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    .unwrap(),
            );
            assert_eq!(
                !hits.is_empty(),
                expected,
                "{cut_utf8}: {safe_prefix}: {complete}"
            );
            assert_eq!(summary.content_skipped, usize::from(!expected));
        }
        let small = TempDir::new();
        small.file("end.txt", "cat");
        small.file("safe.txt", "cat rest");
        let mut index = crate::content_index::build_test_index(small.path());
        for document in &mut index.documents {
            document.content_complete = false;
        }
        let index = Arc::new(index);
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::clone(&index),
                    Query::parse("contentword:cat archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "safe.txt");
        assert_eq!(summary.content_skipped, 1);
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    index,
                    Query::parse("notcontentword:dog archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 2);
    }

    #[test]
    fn negative_content_query_does_not_infer_absence_from_partial_text() {
        for (mode, pattern) in [
            (MatchMode::Exact, "needle"),
            (MatchMode::Fuzzy, "ndl"),
            (MatchMode::Regex, "n..dle"),
        ] {
            let compiled =
                CompiledQuery::new(Query::parse(&format!("notcontent:{pattern}"), mode).unwrap())
                    .unwrap();
            for text in ["", "clean text", "needle"] {
                assert!(matches!(
                    compiled.evaluate_content_text(text, true, &mut MatchExplanation::default()),
                    ContentVerdict::Skipped
                ));
            }
            assert!(matches!(
                compiled.evaluate_content_text("", false, &mut MatchExplanation::default()),
                ContentVerdict::Matched
            ));
            assert!(matches!(
                compiled.evaluate_content_text(
                    "contains NEEDLE",
                    false,
                    &mut MatchExplanation::default()
                ),
                ContentVerdict::NotMatched
            ));
        }
    }

    #[test]
    fn negative_content_query_agrees_for_live_indexed_and_archived_text() {
        let tmp = TempDir::new();
        tmp.file("clean.txt", "allowed text");
        tmp.file("both.txt", "allowed forbidden text");
        tmp.file("empty.txt", "");
        tmp.file("binary.txt", "\0binary");
        write_zip(
            &tmp.path().join("bundle.zip"),
            "arch-clean.txt",
            b"allowed text",
        );
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for (positive, expected) in [
                ("", vec!["arch-clean.txt", "clean.txt", "empty.txt"]),
                ("content:allowed", vec!["arch-clean.txt", "clean.txt"]),
            ] {
                let query = Query::parse(
                    &format!("glob:*.txt notcontent:forbidden {positive}"),
                    MatchMode::Exact,
                )
                .unwrap();
                let engine = SearchEngine::default();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                let (hits, summary) = collect_run(&run);
                let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                names.sort_unstable();
                assert_eq!(names, expected, "{indexed}: {positive}");
                assert_eq!(
                    summary.content_skipped, 1,
                    "binary input is unverified, not an absence match"
                );
            }
        }
    }

    #[test]
    fn negative_content_query_skips_a_real_file_with_a_match_beyond_the_read_cap() {
        let tmp = TempDir::new();
        let mut bytes = vec![b'a'; MAX_CONTENT_BYTES as usize];
        bytes.extend_from_slice(b"needle");
        std::fs::write(tmp.path().join("large.txt"), bytes).unwrap();
        let engine = SearchEngine::default();
        let run = engine
            .start(
                tmp.path().to_path_buf(),
                Query::parse("notcontent:needle archives:off", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 1);
    }

    #[test]
    fn negative_content_query_does_not_assume_legacy_cached_text_is_complete() {
        let tmp = TempDir::new();
        tmp.file("clean.txt", "allowed text");
        let index = crate::content_index::build_test_index(tmp.path());
        assert!(
            index
                .documents
                .iter()
                .filter(|document| document.content.is_some())
                .all(|document| document.content_complete)
        );
        let mut legacy = serde_json::to_value(index).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("content_complete");
        }
        let index =
            Arc::new(serde_json::from_value::<crate::content_index::RootIndex>(legacy).unwrap());
        for (expression, matches, skipped) in
            [("notcontent:forbidden", 0, 1), ("content:allowed", 1, 0)]
        {
            let run = SearchEngine::default()
                .start_indexed(
                    Arc::clone(&index),
                    Query::parse(expression, MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap();
            let (hits, summary) = collect_run(&run);
            assert_eq!(hits.len(), matches);
            assert_eq!(summary.content_skipped, skipped);
        }
    }

    #[test]
    fn case_sensitive_query_agrees_across_live_indexed_and_archive_providers() {
        let tmp = TempDir::new();
        tmp.file("Upper.TXT", "Token draft");
        tmp.file("lower.txt", "token Draft");
        tmp.file("Build/keep.txt", "Token Draft");
        write_zip(&tmp.path().join("bundle.zip"), "Arch.TXT", b"Token Draft");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, expected) in [
                    ("case:sensitive ext:TXT", vec!["Arch.TXT", "Upper.TXT"]),
                    (
                        "case:insensitive ext:TXT",
                        vec!["Arch.TXT", "Upper.TXT", "keep.txt", "lower.txt"],
                    ),
                    ("case:sensitive glob:*.TXT", vec!["Arch.TXT", "Upper.TXT"]),
                    (
                        "case:sensitive content:Token notcontent:draft",
                        vec!["Arch.TXT", "keep.txt"],
                    ),
                    ("case:insensitive content:Token notcontent:draft", vec![]),
                    ("case:sensitive path:Build", vec!["Build", "keep.txt"]),
                    ("case:sensitive path:build", vec![]),
                    ("case:sensitive exclude:build name:keep", vec!["keep.txt"]),
                    ("case:insensitive exclude:build name:keep", vec![]),
                ] {
                    let engine = SearchEngine::default();
                    let query = Query::parse(expression, mode).unwrap();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, _) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {expression}");
                }
            }
        }
    }

    #[test]
    fn negative_metadata_text_queries_agree_across_live_indexed_and_archives() {
        let tmp = TempDir::new();
        let root = tmp.path().join("vendor");
        for path in [
            "src/main.rs",
            "src/draft.rs",
            "vendor/dep.rs",
            "Generated/auto.rs",
        ] {
            tmp.file(&format!("vendor/{path}"), "needle");
        }
        tmp.file("vendor/vendor/binary.rs", "\0binary");
        write_zip(&root.join("bundle.zip"), "vendor/arch.rs", b"needle");
        write_zip(&root.join("kept.zip"), "src/keep.rs", b"needle");
        let index = Arc::new(crate::content_index::build_test_index(&root));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected, skipped) in [
                    (
                        "notname:draft notpath:vendor",
                        vec!["auto.rs", "keep.rs", "main.rs"],
                        0,
                    ),
                    (
                        "case:insensitive notname:Draft notpath:VENDOR",
                        vec!["auto.rs", "keep.rs", "main.rs"],
                        0,
                    ),
                    (
                        "case:sensitive notname:Draft notpath:vendor",
                        vec!["auto.rs", "draft.rs", "keep.rs", "main.rs"],
                        0,
                    ),
                    (
                        "case:sensitive notname:draft notpath:VENDOR",
                        vec!["arch.rs", "auto.rs", "dep.rs", "keep.rs", "main.rs"],
                        1,
                    ),
                ] {
                    let query =
                        Query::parse(&format!("glob:*.rs content:needle {conditions}"), mode)
                            .unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(root.clone(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, skipped);
                    assert!(hits.iter().all(|hit| {
                        hit.explanation
                            .components
                            .iter()
                            .filter(|component| component.score == 0)
                            .all(|component| component.matched_ranges.is_empty())
                    }));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_identity_queries_find_recursive_aliases_live_and_cached_without_borrowing_archive_ids()
     {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new();
        let original = tmp.path().join("original.txt");
        std::fs::write(&original, "same").unwrap();
        std::fs::create_dir(tmp.path().join("nested")).unwrap();
        let alias = tmp.path().join("nested/alias.txt");
        std::fs::hard_link(&original, &alias).unwrap();
        tmp.file("copy.txt", "same");
        let link = tmp.path().join("link.txt");
        std::os::unix::fs::symlink("original.txt", &link).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("other.zip")).unwrap());
        zip.start_file("member.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"same").unwrap();
        zip.finish().unwrap();
        let raw = std::fs::symlink_metadata(&original).unwrap();
        let link_raw = std::fs::symlink_metadata(&link).unwrap();
        let pair = format!("device:{} inode:{} type:regular", raw.dev(), raw.ino());
        let index: crate::content_index::RootIndex = serde_json::from_str(
            &serde_json::to_string(&crate::content_index::build_test_index(tmp.path())).unwrap(),
        )
        .unwrap();
        let index = Arc::new(index);
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, expected, skipped) in [
                    (pair.clone(), vec![original.clone(), alias.clone()], 1),
                    (
                        format!("{pair} content:same"),
                        vec![original.clone(), alias.clone()],
                        1,
                    ),
                    (
                        format!("{pair} archives:off"),
                        vec![original.clone(), alias.clone()],
                        0,
                    ),
                    (
                        format!("device:{} inode:{}", link_raw.dev(), link_raw.ino()),
                        vec![link.clone()],
                        1,
                    ),
                    (
                        format!("device:!{} inode:{}", raw.dev(), raw.ino()),
                        vec![],
                        1,
                    ),
                ] {
                    let engine = SearchEngine::default();
                    let query = Query::parse(&expression, mode).unwrap();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut paths = hits
                        .iter()
                        .map(|h| h.entry.path.clone())
                        .collect::<Vec<_>>();
                    paths.sort();
                    let mut expected = expected;
                    expected.sort();
                    assert_eq!(paths, expected, "{indexed}:{mode:?}:{expression}");
                    assert_eq!(summary.content_skipped, skipped);
                    assert!(hits.iter().all(|h| {
                        h.explanation
                            .components
                            .iter()
                            .any(|c| c.detail.contains("observed inode"))
                    }));
                }
            }
        }
        std::fs::remove_file(&original).unwrap();
        std::fs::remove_file(&alias).unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::clone(&index),
                    Query::parse(&format!("{pair} archives:off"), MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(summary.content_skipped, 0);
        let original_doc = index
            .documents
            .iter()
            .find(|d| d.path == original)
            .unwrap()
            .clone();
        for (volume, inode) in [(0, u64::MAX), (u64::MAX, 0)] {
            let mut cached = (*index).clone();
            let mut document = original_doc.clone();
            document.volume = Some(volume);
            document.file_id = Some(inode);
            cached.documents = vec![document];
            let (hits, summary) = collect_run(
                &engine
                    .start_indexed(
                        Arc::new(cached),
                        Query::parse(
                            &format!("device:{volume} inode:{inode} archives:off"),
                            MatchMode::Exact,
                        )
                        .unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            assert_eq!(hits.len(), 1);
            assert_eq!(summary.content_skipped, 0);
        }
        let mut partial = (*index).clone();
        let mut document = original_doc;
        document.file_id = None;
        partial.documents = vec![document];
        let partial = Arc::new(partial);
        for (expression, count, skipped) in [
            (format!("device:{} archives:off", raw.dev()), 1, 0),
            (format!("inode:{} archives:off", raw.ino()), 0, 1),
        ] {
            let (hits, summary) = collect_run(
                &engine
                    .start_indexed(
                        Arc::clone(&partial),
                        Query::parse(&expression, MatchMode::Exact).unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            assert_eq!(hits.len(), count);
            assert_eq!(summary.content_skipped, skipped);
        }
        let mut old = serde_json::to_value(&*index).unwrap();
        for document in old["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("volume");
            document.as_object_mut().unwrap().remove("file_id");
        }
        let old: crate::content_index::RootIndex = serde_json::from_value(old).unwrap();
        let count = old.documents.len();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::new(old),
                    Query::parse("inode:!0 archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, count);
    }

    #[test]
    fn parent_query_matches_native_cached_and_virtual_immediate_children() {
        let tmp = TempDir::new();
        let folder = tmp.path().join("scope [literal]");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::create_dir_all(tmp.path().join("scope [literal]-other")).unwrap();
        std::fs::write(folder.join("child.txt"), "needle").unwrap();
        std::fs::write(folder.join("nested/child.txt"), "needle").unwrap();
        std::fs::write(tmp.path().join("scope [literal]-other/child.txt"), "needle").unwrap();
        let archive = tmp.path().join("bundle.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        for name in ["child.txt", "nested/child.txt", "nested/deep/child.txt"] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.finish().unwrap();
        let index: crate::content_index::RootIndex = serde_json::from_str(
            &serde_json::to_string(&crate::content_index::build_test_index(tmp.path())).unwrap(),
        )
        .unwrap();
        let index = Arc::new(index);
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (parent, suffix, expected) in [
                    (
                        folder.clone(),
                        "",
                        vec![folder.join("child.txt"), folder.join("nested")],
                    ),
                    (
                        folder.clone(),
                        " content:needle type:regular",
                        vec![folder.join("child.txt")],
                    ),
                    (
                        crate::archive::virtual_member_path(&archive, Path::new("nested")),
                        "",
                        vec![crate::archive::virtual_member_path(
                            &archive,
                            Path::new("nested/child.txt"),
                        )],
                    ),
                    (
                        crate::archive::virtual_member_path(&archive, Path::new("")),
                        "",
                        vec![crate::archive::virtual_member_path(
                            &archive,
                            Path::new("child.txt"),
                        )],
                    ),
                    (folder.join("absent"), "", vec![]),
                ] {
                    let parent_query = Query {
                        predicates: vec![Predicate::Parent(parent.to_string_lossy().into_owned())],
                        mode,
                    };
                    let expression = format!("{}{suffix}", parent_query.to_expression());
                    let query = Query::parse(&expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut paths: Vec<_> = hits.iter().map(|h| h.entry.path.clone()).collect();
                    paths.sort();
                    let mut expected = expected;
                    expected.sort();
                    assert_eq!(paths, expected, "{indexed}:{mode:?}:{expression}");
                    assert_eq!(summary.content_skipped, 0);
                    assert!(hits.iter().all(|h| {
                        h.explanation
                            .components
                            .iter()
                            .any(|c| c.detail.starts_with("parent:"))
                    }));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_queries_use_observed_counts_live_cached_and_never_archive_container_counts() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new();
        let original = tmp.file("link-original.txt", "needle");
        tmp.dir("nested");
        let alias = tmp.path().join("nested/link-alias.txt");
        std::fs::hard_link(&original, &alias).unwrap();
        tmp.file("link-copy.txt", "needle");
        std::os::unix::fs::symlink("link-original.txt", tmp.path().join("link-symlink.txt"))
            .unwrap();
        tmp.dir("link-folder");
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("other.zip")).unwrap());
        zip.start_file("link-member.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"needle").unwrap();
        zip.finish().unwrap();
        let raw = std::fs::symlink_metadata(&original).unwrap();
        assert_eq!(raw.nlink(), 2);
        let built = crate::content_index::build_test_index(tmp.path());
        let index: crate::content_index::RootIndex =
            serde_json::from_str(&serde_json::to_string(&built).unwrap()).unwrap();
        let index = Arc::new(index);
        let doc = index.documents.iter().find(|d| d.path == original).unwrap();
        assert_eq!(doc.hard_links, Some(raw.nlink()));
        assert_eq!(doc.entry().unwrap().hard_link_count(), Some(2));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, expected, skipped) in [
                    (
                        "links:>1 name:link-",
                        vec!["link-alias.txt", "link-original.txt"],
                        1,
                    ),
                    ("links:1 name:link-", vec!["link-copy.txt"], 1),
                    (
                        "links:2..2 name:link- content:needle",
                        vec!["link-alias.txt", "link-original.txt"],
                        1,
                    ),
                    ("links:>2 name:link-", vec![], 1),
                    ("links:=0 name:link-", vec![], 1),
                    (
                        "links:>1 archives:off name:link-",
                        vec!["link-alias.txt", "link-original.txt"],
                        0,
                    ),
                ] {
                    let engine = SearchEngine::default();
                    let query = Query::parse(expression, mode).unwrap();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names = hits
                        .iter()
                        .map(|h| h.entry.name.as_str())
                        .collect::<Vec<_>>();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}:{mode:?}:{expression}");
                    assert_eq!(summary.content_skipped, skipped, "{expression}");
                    assert!(hits.iter().all(|h| {
                        h.explanation.components.iter().any(|c| {
                            c.detail.starts_with("links:") && c.detail.contains("observed")
                        })
                    }));
                }
            }
        }
        // The count includes aliases outside the search root, not the number of local hits.
        let outside = TempDir::new();
        std::fs::hard_link(&original, outside.path().join("external-alias.txt")).unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start(
                    tmp.path().to_owned(),
                    Query::parse("links:3 archives:off name:link-", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(summary.content_skipped, 0);
        assert!(hits.iter().all(|h| h.entry.hard_link_count() == Some(3)));
        // Cached count survives source removal, without inventing an operation identity.
        std::fs::remove_file(&original).unwrap();
        std::fs::remove_file(alias).unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::clone(&index),
                    Query::parse("links:2 archives:off name:link-", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(summary.content_skipped, 0);
        assert!(
            hits.iter().all(
                |h| h.entry.hard_link_count() == Some(2) && h.entry.regular_file_id().is_none()
            )
        );
        let mut legacy = serde_json::to_value(&*index).unwrap();
        for d in legacy["documents"].as_array_mut().unwrap() {
            d.as_object_mut().unwrap().remove("hard_links");
        }
        let legacy = Arc::new(serde_json::from_value(legacy).unwrap());
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    legacy,
                    Query::parse("links:>=0 archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(
            summary.content_skipped,
            index
                .documents
                .iter()
                .filter(|d| d.is_regular == Some(true) && d.is_symlink == Some(false) && !d.is_dir)
                .count()
        );
        let mut example = doc.clone();
        for (count, expression) in [(0, "links:=0"), (u64::MAX, "links:18446744073709551615")] {
            example.hard_links = Some(count);
            let entry = example.entry().unwrap();
            assert_eq!(entry.hard_link_count(), Some(count));
            assert!(
                Query::parse(expression, MatchMode::Exact)
                    .unwrap()
                    .matches_metadata(&entry, SystemTime::now())
            );
        }
        example.hard_links = Some(999);
        example.is_symlink = Some(true);
        assert_eq!(example.entry().unwrap().hard_link_count(), None);
        example.is_symlink = Some(false);
        example.is_dir = true;
        assert_eq!(example.entry().unwrap().hard_link_count(), None);
        example.is_dir = false;
        example.is_regular = Some(false);
        assert_eq!(example.entry().unwrap().hard_link_count(), None);
        example.is_regular = None;
        assert_eq!(example.entry().unwrap().hard_link_count(), None);
    }

    #[cfg(unix)]
    #[test]
    fn allocated_query_distinguishes_sparse_logical_sizes_live_cached_and_archive_unknowns() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new();
        tmp.file("disk-empty", "");
        let sparse = std::fs::File::create(tmp.path().join("disk-sparse")).unwrap();
        sparse.set_len(8 * 1024 * 1024).unwrap();
        let dense = tmp.path().join("disk-dense");
        let mut bytes = vec![b'a'; 8192];
        bytes[..6].copy_from_slice(b"needle");
        std::fs::write(&dense, bytes).unwrap();
        std::fs::create_dir(tmp.path().join("disk-folder")).unwrap();
        std::os::unix::fs::symlink("disk-dense", tmp.path().join("disk-link")).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("other.zip")).unwrap());
        zip.start_file("disk-member", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"needle").unwrap();
        zip.finish().unwrap();
        assert_eq!(
            std::fs::metadata(tmp.path().join("disk-sparse"))
                .unwrap()
                .blocks(),
            0
        );
        assert!(std::fs::metadata(&dense).unwrap().blocks() > 0);
        let built = crate::content_index::build_test_index(tmp.path());
        let index: crate::content_index::RootIndex =
            serde_json::from_str(&serde_json::to_string(&built).unwrap()).unwrap();
        let index = Arc::new(index);
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, expected, skipped) in [
                    (
                        "allocated:=0 name:disk-",
                        vec!["disk-empty", "disk-sparse"],
                        1,
                    ),
                    ("allocated:=0 size:>1MB name:disk-", vec!["disk-sparse"], 1),
                    ("allocated:>0 name:disk-", vec!["disk-dense"], 1),
                    (
                        "allocated:>0 content:needle name:disk-",
                        vec!["disk-dense"],
                        1,
                    ),
                    ("allocated:>0 size:<1KB name:disk-", vec![], 1),
                    (
                        "allocated:=0 archives:off name:disk-",
                        vec!["disk-empty", "disk-sparse"],
                        0,
                    ),
                ] {
                    let query = Query::parse(expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> = hits.iter().map(|h| h.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}:{mode:?}:{expression}");
                    assert_eq!(summary.content_skipped, skipped, "{expression}");
                    assert!(hits.iter().all(|h| {
                        h.explanation
                            .components
                            .iter()
                            .any(|c| c.detail.starts_with("allocated:"))
                    }));
                }
            }
        }
        std::fs::remove_file(dense).unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::clone(&index),
                    Query::parse(
                        "allocated:>0 archives:off nameexact:disk-dense",
                        MatchMode::Exact,
                    )
                    .unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(summary.content_skipped, 0);
        let mut old = serde_json::to_value(&*index).unwrap();
        for row in old["documents"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("allocated_bytes");
        }
        let legacy = Arc::new(serde_json::from_value(old).unwrap());
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    legacy,
                    Query::parse("allocated:>=0 archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(
            summary.content_skipped,
            index
                .documents
                .iter()
                .filter(|d| d.is_regular == Some(true) && d.is_symlink == Some(false) && !d.is_dir)
                .count()
        );
        let mut link = index
            .documents
            .iter()
            .find(|d| d.path.ends_with("disk-link"))
            .unwrap()
            .clone();
        link.allocated_bytes = Some(999);
        link.is_regular = Some(true);
        assert_eq!(link.entry().unwrap().allocated_bytes(), None);
        link.is_symlink = Some(false);
        link.is_dir = true;
        assert_eq!(link.entry().unwrap().allocated_bytes(), None);
    }

    #[cfg(unix)]
    #[test]
    fn owner_id_search_agrees_across_native_cached_tar_and_unknown_zip() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new();
        let native = tmp.file("owned-native.txt", "needle");
        std::os::unix::fs::symlink("owned-native.txt", tmp.path().join("owned-native-link"))
            .unwrap();
        std::fs::hard_link(&native, tmp.path().join("owned-native-hard")).unwrap();
        let meta = std::fs::symlink_metadata(&native).unwrap();
        let uid = u64::from(meta.uid());
        let gid = u64::from(meta.gid());
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        zip.start_file("owned-unknown", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"needle").unwrap();
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, id, group) in [
            ("owned-alpha", i64::MAX as u64, 0),
            ("owned-beta", 0, i64::MAX as u64),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(0o644);
            header.set_uid(id);
            header.set_gid(group);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"needle"[..]).unwrap();
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_uid(i64::MAX as u64);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Symlink);
        tar.append_link(&mut header, "owned-link", "owned-beta")
            .unwrap();
        tar.finish().unwrap();
        drop(tar);
        let built = crate::content_index::build_test_index(tmp.path());
        let index: crate::content_index::RootIndex =
            serde_json::from_str(&serde_json::to_string(&built).unwrap()).unwrap();
        let index = Arc::new(index);
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, mut expected, skipped) in [
                    (
                        format!("uid:{uid} gid:{gid} archives:off name:owned-"),
                        vec!["owned-native-hard", "owned-native-link", "owned-native.txt"],
                        0,
                    ),
                    (format!("uid:!{uid} archives:off name:owned-"), vec![], 0),
                    (
                        "uid:9223372036854775807 gid:0 name:owned-".into(),
                        vec!["owned-alpha", "owned-link"],
                        1,
                    ),
                    (
                        "uid:!9223372036854775807 gid:9223372036854775807 name:owned-".into(),
                        vec!["owned-beta"],
                        1,
                    ),
                    (
                        "uid:9223372036854775807 type:regular content:needle name:owned-".into(),
                        vec!["owned-alpha"],
                        1,
                    ),
                    ("uid:0 uid:!0 name:owned-".into(), vec![], 1),
                ] {
                    let query = Query::parse(&expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> = hits.iter().map(|h| h.entry.name.as_str()).collect();
                    names.sort_unstable();
                    expected.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {expression}");
                    assert_eq!(summary.content_skipped, skipped, "{expression}");
                    assert!(hits.iter().all(|h| {
                        h.explanation
                            .components
                            .iter()
                            .any(|c| c.detail.starts_with("uid:"))
                    }));
                }
            }
        }
        // Cached observations survive removal; unknown legacy fields never match exclusion.
        std::fs::remove_file(native).unwrap();
        let query = Query::parse(
            &format!("uid:{uid} gid:{gid} archives:off nameexact:owned-native.txt"),
            MatchMode::Exact,
        )
        .unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(summary.content_skipped, 0);
        let mut old = serde_json::to_value(&*index).unwrap();
        for d in old["documents"].as_array_mut().unwrap() {
            d.as_object_mut().unwrap().remove("owner_uid");
            d.as_object_mut().unwrap().remove("owner_gid");
        }
        let legacy = Arc::new(serde_json::from_value(old).unwrap());
        for expression in [
            "uid:0 archives:off",
            "uid:!0 archives:off",
            "gid:0 archives:off",
            "gid:!0 archives:off",
        ] {
            let (hits, summary) = collect_run(
                &engine
                    .start_indexed(
                        Arc::clone(&legacy),
                        Query::parse(expression, MatchMode::Exact).unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            assert!(hits.is_empty());
            assert_eq!(summary.content_skipped, index.documents.len());
        }
    }

    #[test]
    fn owner_id_unknown_fields_do_not_match_even_when_excluded() {
        let entry = synthetic_entry(PathBuf::from("/gone"), false, 0, None, String::new()).unwrap();
        for expression in ["uid:0", "uid:!0", "gid:0", "gid:!0"] {
            let query =
                CompiledQuery::new(Query::parse(expression, MatchMode::Exact).unwrap()).unwrap();
            assert!(
                query
                    .evaluate_metadata(
                        &entry,
                        Path::new("/"),
                        SystemTime::now(),
                        ObservedMetadata::default()
                    )
                    .is_none()
            );
        }
        let observed = ObservedMetadata {
            owner_uid: Some(0),
            owner_gid: Some(u64::MAX),
            ..Default::default()
        };
        for expression in ["uid:0 gid:18446744073709551615", "uid:!1 gid:!0"] {
            let query =
                CompiledQuery::new(Query::parse(expression, MatchMode::Exact).unwrap()).unwrap();
            assert!(
                query
                    .evaluate_metadata(&entry, Path::new("/"), SystemTime::now(), observed)
                    .is_some()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn executable_query_agrees_across_live_indexed_zip_and_tar() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        for (name, mode, text) in [
            ("owner.sh", 0o700, "needle"),
            ("group.sh", 0o610, "needle"),
            ("other.sh", 0o601, "needle"),
            ("plain.sh", 0o644, "needle"),
            ("binary.sh", 0o755, "\0binary"),
        ] {
            tmp.file(name, text);
            std::fs::set_permissions(tmp.path().join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
        }
        tmp.dir("folder.sh");
        std::fs::hard_link(tmp.path().join("owner.sh"), tmp.path().join("hard.sh")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("owner.sh"), tmp.path().join("symbolic.sh"))
            .unwrap();
        let fifo = tmp.path().join("pipe.sh");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        std::fs::set_permissions(&fifo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for (name, mode) in [("zip-exec.sh", 0o755), ("zip-plain.sh", 0o644)] {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default().unix_permissions(mode),
            )
            .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.add_symlink(
            "zip-link.sh",
            "zip-exec.sh",
            zip::write::SimpleFileOptions::default().unix_permissions(0o777),
        )
        .unwrap();
        zip.add_directory(
            "zip-folder.sh/",
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, mode) in [("tar-exec.sh", 0o755), ("tar-plain.sh", 0o644)] {
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(mode);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"needle"[..]).unwrap();
        }
        for (name, kind) in [
            ("tar-link.sh", tar::EntryType::Symlink),
            ("tar-hard.sh", tar::EntryType::Link),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_entry_type(kind);
            tar.append_link(&mut header, name, "tar-exec.sh").unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected, skipped) in [
                    (
                        "type:executable",
                        vec![
                            "binary.sh",
                            "group.sh",
                            "hard.sh",
                            "other.sh",
                            "owner.sh",
                            "tar-exec.sh",
                            "zip-exec.sh",
                        ],
                        0,
                    ),
                    (
                        "type:exec archives:off",
                        vec!["binary.sh", "group.sh", "hard.sh", "other.sh", "owner.sh"],
                        0,
                    ),
                    (
                        "type:executable content:needle",
                        vec![
                            "group.sh",
                            "hard.sh",
                            "other.sh",
                            "owner.sh",
                            "tar-exec.sh",
                            "zip-exec.sh",
                        ],
                        1,
                    ),
                    ("type:executable type:symlink", vec![], 0),
                ] {
                    let query = Query::parse(condition, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(
                        summary.content_skipped, skipped,
                        "{indexed}: {mode:?}: {condition}"
                    );
                }
            }
        }
        std::fs::set_permissions(
            tmp.path().join("owner.sh"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let engine = SearchEngine::default();
        let query = Query::parse(
            "type:executable archives:off name:owner.sh",
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, _) = collect_run(
            &engine
                .start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(
            hits.len(),
            1,
            "index searches retain the cached mode observation"
        );
        let mut legacy = serde_json::to_value(&*index).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("is_executable");
        }
        let legacy = Arc::new(serde_json::from_value(legacy).unwrap());
        let query = Query::parse("type:executable archives:off", MatchMode::Exact).unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(legacy, query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, index.documents.len());
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_query_matches_live_and_cached_facts_across_modes() {
        let tmp = TempDir::new();
        tmp.file("regular.txt", "payload");
        tmp.file("dir/child.txt", "payload");
        tmp.file("plain.lnk", "payload");
        tmp.file(".hidden/keep", "x");
        for (name, target) in [
            ("missing.lnk", "no-such"),
            ("chain.lnk", "missing.lnk"),
            ("notdir.lnk", "regular.txt/child"),
            ("valid.lnk", "regular.txt"),
            ("directory.lnk", "dir"),
            ("loop.lnk", "loop.lnk"),
            (".hidden/missing.lnk", "../no-such"),
        ] {
            std::os::unix::fs::symlink(target, tmp.path().join(name)).unwrap();
        }
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        let missing = vec![
            ".hidden/missing.lnk",
            "chain.lnk",
            "missing.lnk",
            "notdir.lnk",
        ];
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, expected) in [
                    ("type:broken-link archives:off", missing.clone()),
                    (
                        "type:broken-symlink depth:<=1 archives:off",
                        vec!["chain.lnk", "missing.lnk", "notdir.lnk"],
                    ),
                    (
                        "type:broken-link hidden:exclude archives:off",
                        vec!["chain.lnk", "missing.lnk", "notdir.lnk"],
                    ),
                    (
                        "types:broken-link,regular glob:*.lnk archives:off",
                        vec![
                            ".hidden/missing.lnk",
                            "chain.lnk",
                            "missing.lnk",
                            "notdir.lnk",
                            "plain.lnk",
                        ],
                    ),
                    ("type:broken-link type:regular archives:off", vec![]),
                ] {
                    let query = Query::parse(expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut paths: Vec<_> = hits
                        .iter()
                        .map(|h| {
                            h.entry
                                .path
                                .strip_prefix(tmp.path())
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_owned()
                        })
                        .collect();
                    paths.sort();
                    assert_eq!(paths, expected, "{indexed} {mode:?} {expression}");
                    assert_eq!(
                        summary.content_skipped, 1,
                        "loop is unknown, not missing: {expression}"
                    );
                    assert!(!summary.truncated);
                }
            }
        }
        // Repair a target after indexing: cached observation must remain stable.
        tmp.file("no-such", "repaired");
        let engine = SearchEngine::default();
        let query = Query::parse("type:broken-link archives:off", MatchMode::Exact).unwrap();
        let (live, _) = collect_run(
            &engine
                .start(tmp.path().to_owned(), query.clone(), 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].entry.name, "notdir.lnk");
        let (cached, _) = collect_run(
            &engine
                .start_indexed(Arc::clone(&index), query.clone(), 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(cached.len(), 4);
        let mut legacy = serde_json::to_value(&*index).unwrap();
        for d in legacy["documents"].as_array_mut().unwrap() {
            d.as_object_mut().unwrap().remove("is_broken_symlink");
        }
        let legacy = Arc::new(serde_json::from_value(legacy).unwrap());
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(legacy, query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 7);
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_archive_targets_stay_unknown_but_known_or_branches_match() {
        let tmp = TempDir::new();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        zip.add_symlink(
            "zip-link",
            "missing",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(tar::EntryType::Symlink);
        tar.append_link(&mut header, "tar-link", "missing").unwrap();
        tar.finish().unwrap();
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, count, skipped) in [
                    ("type:broken-link", 0, 2),
                    ("types:broken-link,symlink", 2, 0),
                    ("type:broken-link archives:off", 0, 0),
                ] {
                    let query = Query::parse(expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    assert_eq!(hits.len(), count);
                    assert_eq!(summary.content_skipped, skipped);
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_query_agrees_across_live_indexed_zip_and_tar() {
        let tmp = TempDir::new();
        let target = tmp.file("regular.txt", "needle");
        tmp.file("real-dir/inside.txt", "needle");
        std::fs::hard_link(&target, tmp.path().join("hard-link")).unwrap();
        for (name, target) in [
            ("file-link", target),
            ("dir-link", tmp.path().join("real-dir")),
            ("broken-link", tmp.path().join("missing")),
            ("loop-link", tmp.path().to_path_buf()),
        ] {
            std::os::unix::fs::symlink(target, tmp.path().join(name)).unwrap();
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        zip.add_symlink(
            "zip-link",
            "missing",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.start_file("zip-regular", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"needle").unwrap();
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, kind) in [
            ("tar-link", tar::EntryType::Symlink),
            ("tar-hard", tar::EntryType::Link),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_entry_type(kind);
            tar.append_link(&mut header, name, "missing").unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index: crate::content_index::RootIndex =
            serde_json::from_str(&serde_json::to_string(&built).unwrap()).unwrap();
        let index = Arc::new(index);
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "type:symlink",
                        vec![
                            "broken-link",
                            "dir-link",
                            "file-link",
                            "loop-link",
                            "tar-link",
                            "zip-link",
                        ],
                    ),
                    (
                        "type:link archives:off",
                        vec!["broken-link", "dir-link", "file-link", "loop-link"],
                    ),
                ] {
                    let query = Query::parse(condition, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                    assert!(
                        summary.scanned < 30,
                        "must not descend through directory links"
                    );
                }
            }
        }
        // Cached link kind must not depend on observing the filesystem again.
        std::fs::remove_file(tmp.path().join("file-link")).unwrap();
        let engine = SearchEngine::default();
        let query =
            Query::parse("type:symlink archives:off name:file-link", MatchMode::Exact).unwrap();
        let (hits, _) = collect_run(
            &engine
                .start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(hits.len(), 1);
        // Old records have unknown kind, not guessed link metadata.
        let mut legacy = serde_json::to_value(&*index).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("is_symlink");
        }
        let legacy = Arc::new(serde_json::from_value(legacy).unwrap());
        let query = Query::parse("type:symlink archives:off", MatchMode::Exact).unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(legacy, query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, index.documents.len());
    }

    #[test]
    fn permission_query_matches_exact_bit_requirements_and_unknown_metadata() {
        let entry = file_entry(PathBuf::from("/fixture/file.txt"), 1);
        for (condition, required, forbidden) in [
            ("perm:ur+uw-ux+", 0o500, 0o200),
            ("perm:gs+sb-us-", 0o2000, 0o5000),
            ("perm:ow-ox-", 0, 0o003),
        ] {
            let compiled =
                CompiledQuery::new(Query::parse(condition, MatchMode::Exact).unwrap()).unwrap();
            assert!(
                compiled
                    .evaluate_metadata(
                        &entry,
                        Path::new("/fixture"),
                        SystemTime::UNIX_EPOCH,
                        ObservedMetadata::default()
                    )
                    .is_none()
            );
            for mode in 0..=0o7777 {
                let observed = ObservedMetadata {
                    unix_mode: Some(mode),
                    ..Default::default()
                };
                assert_eq!(
                    compiled
                        .evaluate_metadata(
                            &entry,
                            Path::new("/fixture"),
                            SystemTime::UNIX_EPOCH,
                            observed
                        )
                        .is_some(),
                    mode & required == required && mode & forbidden == 0,
                    "{condition}: {mode:o}"
                );
            }
        }
    }

    #[test]
    fn advanced_permission_query_unknown_modes_never_satisfy_negated_groups() {
        let entry = file_entry(PathBuf::from("/fixture/file.txt"), 1);
        for condition in [
            "perm:!700+",
            "perm:0+",
            "perm:!500+|us+",
            "perm:0+ perm:!0+",
            "perm:uw+|gw+|ow+ perm:ux-",
        ] {
            let compiled =
                CompiledQuery::new(Query::parse(condition, MatchMode::Exact).unwrap()).unwrap();
            assert!(
                compiled
                    .evaluate_metadata(
                        &entry,
                        Path::new("/fixture"),
                        SystemTime::UNIX_EPOCH,
                        ObservedMetadata::default()
                    )
                    .is_none()
            );
        }
        let compiled = CompiledQuery::new(
            Query::parse("perm:uw+|gw+|ow+ perm:ux-", MatchMode::Exact).unwrap(),
        )
        .unwrap();
        for mode in 0..=0o7777 {
            let observed = ObservedMetadata {
                unix_mode: Some(mode),
                ..Default::default()
            };
            assert_eq!(
                compiled
                    .evaluate_metadata(
                        &entry,
                        Path::new("/fixture"),
                        SystemTime::UNIX_EPOCH,
                        observed
                    )
                    .is_some(),
                mode & 0o222 != 0 && mode & 0o100 == 0,
                "{mode:o}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn permission_query_agrees_across_live_indexed_zip_and_tar() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        for (name, mode) in [
            ("owner.txt", 0o500),
            ("plain.txt", 0o644),
            ("suid.txt", 0o4500),
            ("sgid.txt", 0o2500),
        ] {
            tmp.file(name, "needle");
            std::fs::set_permissions(tmp.path().join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
            assert_eq!(
                std::fs::symlink_metadata(tmp.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                mode
            );
        }
        std::fs::hard_link(tmp.path().join("owner.txt"), tmp.path().join("hard.txt")).unwrap();
        std::os::unix::fs::symlink("owner.txt", tmp.path().join("link.txt")).unwrap();
        assert_ne!(
            std::fs::symlink_metadata(tmp.path().join("link.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o200,
            0
        );
        tmp.dir("sticky");
        std::fs::set_permissions(
            tmp.path().join("sticky"),
            std::fs::Permissions::from_mode(0o1777),
        )
        .unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for (name, mode) in [("zip-owner.txt", 0o500), ("zip-plain.txt", 0o644)] {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default().unix_permissions(mode),
            )
            .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, mode) in [
            ("tar-owner.txt", 0o500),
            ("tar-plain.txt", 0o644),
            ("tar-suid.txt", 0o4500),
            ("tar-sgid.txt", 0o2500),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(mode);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"needle"[..]).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        // Prove indexed permission matching uses its observation, not a live stat.
        std::fs::set_permissions(
            tmp.path().join("plain.txt"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected) in [
                    (
                        "type:regular ext:txt perm:ux+uw-",
                        vec![
                            "hard.txt",
                            "owner.txt",
                            "sgid.txt",
                            "suid.txt",
                            "tar-owner.txt",
                            "tar-sgid.txt",
                            "tar-suid.txt",
                            "zip-owner.txt",
                        ],
                    ),
                    (
                        "type:regular ext:txt perm:uw+|gw+|ow+",
                        vec!["plain.txt", "tar-plain.txt", "zip-plain.txt"],
                    ),
                    (
                        "type:regular ext:txt perm:us+|gs+ perm:uw-",
                        vec!["sgid.txt", "suid.txt", "tar-sgid.txt", "tar-suid.txt"],
                    ),
                    (
                        "type:regular ext:txt perm:2500+",
                        vec!["sgid.txt", "tar-sgid.txt"],
                    ),
                    (
                        "type:regular ext:txt perm:!500+",
                        vec!["plain.txt", "tar-plain.txt", "zip-plain.txt"],
                    ),
                    (
                        "type:regular ext:txt perm:!500+|us+",
                        vec![
                            "plain.txt",
                            "suid.txt",
                            "tar-plain.txt",
                            "tar-suid.txt",
                            "zip-plain.txt",
                        ],
                    ),
                    ("type:regular ext:txt perm:0777-", vec![]),
                    ("type:regular perm:us+", vec!["suid.txt", "tar-suid.txt"]),
                    ("type:regular perm:gs+", vec!["sgid.txt", "tar-sgid.txt"]),
                    ("type:folder perm:sb+ archives:off", vec!["sticky"]),
                    ("type:symlink perm:uw+ archives:off", vec!["link.txt"]),
                    (
                        "type:regular perm:or+ ext:txt",
                        if indexed {
                            vec!["plain.txt", "tar-plain.txt", "zip-plain.txt"]
                        } else {
                            vec!["tar-plain.txt", "zip-plain.txt"]
                        },
                    ),
                ] {
                    let query = Query::parse(conditions, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
        let mut legacy = serde_json::to_value(index.as_ref()).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("unix_mode");
        }
        let query = Query::parse("perm:!700+|uw+ archives:off", MatchMode::Exact).unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::new(serde_json::from_value(legacy).unwrap()),
                    query,
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, index.documents.len());
    }

    #[test]
    fn empty_query_observes_files_directories_and_unreadable_state_lexically() {
        let tmp = TempDir::new();
        let file = tmp.file("zero", "");
        assert_eq!(
            crate::fs_util::observed_empty(&file, &std::fs::symlink_metadata(&file).unwrap()),
            Some(true)
        );
        let file = tmp.file("data", "x");
        assert_eq!(
            crate::fs_util::observed_empty(&file, &std::fs::symlink_metadata(&file).unwrap()),
            Some(false)
        );
        let folder = tmp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let metadata = std::fs::symlink_metadata(&folder).unwrap();
        assert_eq!(
            crate::fs_util::observed_empty(&folder, &metadata),
            Some(true)
        );
        std::fs::write(folder.join(".hidden"), b"").unwrap();
        assert_eq!(
            crate::fs_util::observed_empty(&folder, &metadata),
            Some(false)
        );
        std::fs::remove_file(folder.join(".hidden")).unwrap();
        std::fs::remove_dir(&folder).unwrap();
        assert_eq!(crate::fs_util::observed_empty(&folder, &metadata), None);
    }

    #[test]
    fn empty_query_archive_topology_requires_complete_bounded_proof() {
        let mut tracker = EmptyDirectoryTracker::new(4096);
        tracker.observe(Path::new("./parent/.hidden"));
        tracker.observe(Path::new("parent/child/grandchild"));
        assert_eq!(tracker.empty(Path::new("parent"), true), Some(false));
        assert_eq!(
            tracker.empty(Path::new("./parent/child"), false),
            Some(false)
        );
        assert_eq!(tracker.empty(Path::new("vacant"), true), Some(true));
        assert_eq!(tracker.empty(Path::new("vacant"), false), None);
        let mut bounded = EmptyDirectoryTracker::new(140);
        bounded.observe(Path::new("p/child"));
        bounded.observe(Path::new("another/child"));
        assert!(!bounded.complete);
        assert_eq!(bounded.empty(Path::new("p"), false), Some(false));
        assert_eq!(bounded.empty(Path::new("vacant"), true), None);
        assert!(!bounded.reserve_directory(Path::new("vacant")));
        assert!(bounded.bytes <= bounded.limit);
    }

    #[cfg(unix)]
    #[test]
    fn empty_query_agrees_across_native_index_zip_tar_and_filtered_children() {
        let tmp = TempDir::new();
        for directory in [
            "native-empty",
            "native-hidden",
            "native-parent/native-nested-empty",
        ] {
            std::fs::create_dir_all(tmp.path().join(directory)).unwrap();
        }
        tmp.file("native-hidden/.hidden", "x");
        let zero = tmp.file("native-zero.txt", "");
        std::fs::hard_link(&zero, tmp.path().join("native-hardzero")).unwrap();
        tmp.file("native-data.txt", "needle");
        std::os::unix::fs::symlink("native-empty", tmp.path().join("empty-link")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(tmp.path().join("native-pipe"))
                .status()
                .unwrap()
                .success()
        );
        let _socket =
            std::os::unix::net::UnixListener::bind(tmp.path().join("native-socket")).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for directory in [
            "zip-empty/",
            "zip-hidden/",
            "zip-parent/",
            "zip-parent/zip-nested-empty/",
            "zip-late/",
        ] {
            zip.add_directory(directory, zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        for (name, data) in [
            ("zip-hidden/.hidden", "x"),
            ("./zip-late/data", "x"),
            ("zip-zero.txt", ""),
            ("zip-data.txt", "needle"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.add_symlink(
            "zip-link",
            "zip-zero.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for directory in [
            "tar-empty",
            "tar-hidden",
            "tar-parent",
            "tar-parent/tar-nested-empty",
            "tar-late",
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_cksum();
            tar.append_data(&mut header, directory, std::io::empty())
                .unwrap();
        }
        for (name, data) in [
            ("tar-hidden/.hidden", "x"),
            ("./tar-late/data", "x"),
            ("tar-zero.txt", ""),
            ("tar-data.txt", "needle"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, data.as_bytes()).unwrap();
        }
        for (name, kind) in [
            ("tar-hardzero", tar::EntryType::Link),
            ("tar-linkzero", tar::EntryType::Symlink),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_entry_type(kind);
            tar.append_link(&mut header, name, "tar-zero.txt").unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        let all = vec![
            "native-empty",
            "native-hardzero",
            "native-nested-empty",
            "native-zero.txt",
            "tar-empty",
            "tar-nested-empty",
            "tar-zero.txt",
            "zip-empty",
            "zip-nested-empty",
            "zip-zero.txt",
        ];
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected) in [
                    ("type:empty", all.clone()),
                    (
                        "type:empty type:folder",
                        vec![
                            "native-empty",
                            "native-nested-empty",
                            "tar-empty",
                            "tar-nested-empty",
                            "zip-empty",
                            "zip-nested-empty",
                        ],
                    ),
                    (
                        "type:empty type:regular",
                        vec![
                            "native-hardzero",
                            "native-zero.txt",
                            "tar-zero.txt",
                            "zip-zero.txt",
                        ],
                    ),
                    ("type:empty hidden:exclude exclude:.hidden", all.clone()),
                    (
                        "type:empty excludedir:*-parent",
                        vec![
                            "native-empty",
                            "native-hardzero",
                            "native-zero.txt",
                            "tar-empty",
                            "tar-zero.txt",
                            "zip-empty",
                            "zip-zero.txt",
                        ],
                    ),
                    (
                        "types:empty,socket archives:off",
                        vec![
                            "native-empty",
                            "native-hardzero",
                            "native-nested-empty",
                            "native-socket",
                            "native-zero.txt",
                        ],
                    ),
                    (
                        "type:empty depth:<=1",
                        vec!["native-empty", "native-hardzero", "native-zero.txt"],
                    ),
                    ("type:empty content:needle", vec![]),
                ] {
                    let engine = SearchEngine::default();
                    let query = Query::parse(conditions, mode).unwrap();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, _) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                }
            }
        }
        let mut legacy = serde_json::to_value(&*index).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("is_empty");
        }
        let legacy =
            Arc::new(serde_json::from_value::<crate::content_index::RootIndex>(legacy).unwrap());
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::clone(&legacy),
                    Query::parse("type:empty archives:off", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, index.documents.len());
        for (condition, directories) in [
            ("types:empty,folders archives:off", true),
            ("types:empty,file archives:off", false),
        ] {
            let (hits, summary) = collect_run(
                &engine
                    .start_indexed(
                        Arc::clone(&legacy),
                        Query::parse(condition, MatchMode::Exact).unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            let count = index
                .documents
                .iter()
                .filter(|document| document.is_dir == directories)
                .count();
            assert_eq!(hits.len(), count);
            assert_eq!(summary.content_skipped, index.documents.len() - count);
        }
        // Indexed emptiness is an observation, not a claim about later filesystem changes.
        tmp.file("native-empty/new-child", "x");
        for indexed in [false, true] {
            let query = Query::parse(
                "type:empty nameexact:native-empty archives:off",
                MatchMode::Exact,
            )
            .unwrap();
            let run = if indexed {
                engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
            } else {
                engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
            }
            .unwrap();
            assert_eq!(collect_run(&run).0.len(), usize::from(indexed));
        }
    }

    #[test]
    fn empty_query_does_not_prove_archive_directories_from_partial_inspection() {
        let tmp = TempDir::new();
        let path = tmp.path().join("unsafe.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.add_directory("candidate/", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"x").unwrap();
        zip.start_file("safe-zero", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start(
                    tmp.path().to_path_buf(),
                    Query::parse("type:empty", MatchMode::Exact).unwrap(),
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "safe-zero");
        assert!(summary.content_skipped >= 1);
        let path = tmp.path().join("capped.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.add_directory("candidate/", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.start_file(
            "candidate/late-child",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        let request = ProviderRequest {
            generation: 1,
            active: Arc::new(AtomicU64::new(1)),
            scheduler_cancel: crate::workload::CancellationToken::new(),
            requires_content: false,
            requires_empty: true,
            requires_broken_symlink: false,
            max_depth: None,
            exclusions: Arc::new(Vec::new()),
            directory_exclusions: Arc::new(Vec::new()),
            search_archives: true,
            hidden_mode: crate::query::HiddenMode::Include,
            unsearched: Cell::new(0),
            archives_capped: Cell::new(false),
            archive_error: RefCell::new(None),
        };
        let mut budget = crate::archive::SearchBudget {
            members: crate::archive::MAX_MEMBERS_PER_RUN - 1,
            ..Default::default()
        };
        let mut directories = Vec::new();
        ArchiveProvider::visit_path(
            &path,
            &request,
            &mut |candidate| {
                if let Some(candidate) = candidate
                    && candidate.entry.is_dir
                {
                    directories.push(candidate.is_empty);
                }
                true
            },
            &mut budget,
        );
        assert!(budget.truncated);
        assert_eq!(directories, vec![None]);
    }

    #[test]
    fn type_alternatives_use_proven_matches_before_unknown_branches() {
        use crate::query::{SpecialFileType, TypeCondition};
        let mut entry = file_entry(PathBuf::from("/fixture/image.png"), 0);
        let unknown = ObservedMetadata::default();
        let fifo = TypeCondition::Special(SpecialFileType::Fifo);
        assert_eq!(
            unknown.matches_types(&entry, &[fifo, TypeCondition::File]),
            Some(true)
        );
        assert_eq!(
            unknown.matches_types(&entry, &[TypeCondition::File, fifo]),
            Some(true)
        );
        assert_eq!(
            unknown.matches_types(&entry, &[fifo, TypeCondition::Symlink]),
            None
        );
        assert_eq!(
            unknown.matches_types(
                &entry,
                &[
                    fifo,
                    TypeCondition::Kind(crate::selection_summary::Kind::Folder)
                ]
            ),
            None
        );
        assert_eq!(
            unknown.matches_types(
                &entry,
                &[
                    TypeCondition::Kind(crate::selection_summary::Kind::Image),
                    fifo
                ]
            ),
            Some(true)
        );
        let observed = ObservedMetadata {
            file_type_bits: Some(0o010000),
            is_regular: Some(false),
            is_symlink: Some(false),
            is_executable: Some(false),
            ..Default::default()
        };
        assert_eq!(
            observed.matches_types(&entry, &[TypeCondition::Regular, fifo]),
            Some(true)
        );
        assert_eq!(
            observed.matches_types(&entry, &[TypeCondition::Regular, TypeCondition::Symlink]),
            Some(false)
        );
        entry.is_dir = true;
        assert_eq!(
            unknown.matches_types(&entry, &[TypeCondition::File, fifo]),
            None
        );
        assert_eq!(
            unknown.matches_types(
                &entry,
                &[
                    TypeCondition::Kind(crate::selection_summary::Kind::Folder),
                    fifo
                ]
            ),
            Some(true)
        );
    }

    #[test]
    fn special_file_query_compares_observed_bits_without_guessing_names() {
        use crate::query::SpecialFileType;
        let entry = file_entry(PathBuf::from("/fixture/fake.pipe.sock"), 0);
        for kind in [
            SpecialFileType::Fifo,
            SpecialFileType::Socket,
            SpecialFileType::BlockDevice,
            SpecialFileType::CharDevice,
        ] {
            let query = CompiledQuery::new(
                Query::parse(&format!("type:{}", kind.label()), MatchMode::Exact).unwrap(),
            )
            .unwrap();
            for bits in [
                0, 0o010000, 0o020000, 0o040000, 0o060000, 0o100000, 0o120000, 0o140000,
            ] {
                assert_eq!(
                    query
                        .evaluate_metadata(
                            &entry,
                            Path::new("/fixture"),
                            SystemTime::UNIX_EPOCH,
                            ObservedMetadata {
                                file_type_bits: Some(bits),
                                ..Default::default()
                            }
                        )
                        .is_some(),
                    bits == kind.bits()
                );
            }
            assert!(
                query
                    .evaluate_metadata(
                        &entry,
                        Path::new("/fixture"),
                        SystemTime::UNIX_EPOCH,
                        ObservedMetadata::default()
                    )
                    .is_none()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn special_file_query_agrees_across_native_index_zip_and_tar() {
        let tmp = TempDir::new();
        tmp.file("ordinary.sock", "needle");
        tmp.dir("directory.fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(tmp.path().join("native-pipe"))
                .status()
                .unwrap()
                .success()
        );
        let _socket =
            std::os::unix::net::UnixListener::bind(tmp.path().join("native-socket")).unwrap();
        std::os::unix::fs::symlink("native-pipe", tmp.path().join("pipe-link")).unwrap();
        std::os::unix::fs::symlink("native-socket", tmp.path().join("socket-link")).unwrap();
        std::os::unix::fs::symlink("/dev/null", tmp.path().join("device-link")).unwrap();
        let zip_path = tmp.path().join("bundle.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        for name in [
            "zip-pipe",
            "zip-socket",
            "zip-block",
            "nested/zip-char",
            "zip-regular",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.finish().unwrap();
        // ZipWriter masks off file-type bits. Set the central-directory Unix
        // external attributes explicitly to create stored special-file fixtures.
        let mut bytes = std::fs::read(&zip_path).unwrap();
        let offsets: Vec<_> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(index, magic)| (magic == b"PK\x01\x02").then_some(index))
            .collect();
        assert_eq!(offsets.len(), 5);
        for offset in offsets {
            let length =
                u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
            let name = std::str::from_utf8(&bytes[offset + 46..offset + 46 + length]).unwrap();
            let bits = match name {
                "zip-pipe" => 0o010000,
                "zip-socket" => 0o140000,
                "zip-block" => 0o060000,
                "nested/zip-char" => 0o020000,
                _ => continue,
            };
            bytes[offset + 5] = 3; // Unix creator system.
            bytes[offset + 38..offset + 42]
                .copy_from_slice(&((bits | 0o644u32) << 16).to_le_bytes());
        }
        std::fs::write(&zip_path, bytes).unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, kind) in [
            ("tar-pipe", tar::EntryType::Fifo),
            ("tar-block", tar::EntryType::Block),
            ("nested/tar-char", tar::EntryType::Char),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_entry_type(kind);
            header.set_cksum();
            tar.append_data(&mut header, name, std::io::empty())
                .unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    ("type:fifo", vec!["native-pipe", "tar-pipe", "zip-pipe"]),
                    (
                        "types:fifo,socket",
                        vec![
                            "native-pipe",
                            "native-socket",
                            "tar-pipe",
                            "zip-pipe",
                            "zip-socket",
                        ],
                    ),
                    (
                        "types:block,char",
                        vec!["tar-block", "tar-char", "zip-block", "zip-char"],
                    ),
                    (
                        "types:fifo,socket archives:off",
                        vec!["native-pipe", "native-socket"],
                    ),
                    (
                        "types:fifo,socket type:fifo",
                        vec!["native-pipe", "tar-pipe", "zip-pipe"],
                    ),
                    ("types:fifo,socket types:regular,symlink", vec![]),
                    (
                        "types:regular,symlink archives:off",
                        vec![
                            "bundle.tar",
                            "bundle.zip",
                            "device-link",
                            "ordinary.sock",
                            "pipe-link",
                            "socket-link",
                        ],
                    ),
                    (
                        "types:folders,socket archives:off",
                        vec!["directory.fifo", "native-socket"],
                    ),
                    ("types:fifo,socket content:needle", vec![]),
                    ("type:socket", vec!["native-socket", "zip-socket"]),
                    ("type:block-device", vec!["tar-block", "zip-block"]),
                    ("type:char-device", vec!["tar-char", "zip-char"]),
                    ("type:fifo archives:off", vec!["native-pipe"]),
                    ("type:socket name:native", vec!["native-socket"]),
                    ("type:fifo type:socket", vec![]),
                    ("type:fifo content:needle", vec![]),
                    ("type:socket notcontent:missing", vec![]),
                ] {
                    let query = Query::parse(condition, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    if condition.contains("content:") {
                        assert!(summary.content_skipped > 0);
                    } else {
                        assert_eq!(summary.content_skipped, 0);
                    }
                    assert!(hits.iter().all(|hit| {
                        hit.explanation.components.iter().any(|component| {
                            component.field == "filter"
                                && (component.detail.starts_with("type:")
                                    || component.detail.starts_with("types:"))
                        })
                    }));
                }
            }
        }
        let mut legacy = serde_json::to_value(&*index).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("file_type_bits");
            document.as_object_mut().unwrap().remove("is_regular");
            document.as_object_mut().unwrap().remove("is_symlink");
        }
        // A known matching OR branch proves a result even when another branch's
        // facts are absent. Nonmatching known branches cannot erase uncertainty.
        for (condition, expected, skipped) in [
            (
                "types:folders,fifo archives:off",
                vec!["directory.fifo"],
                index.documents.len() - 1,
            ),
            (
                "types:file,fifo archives:off",
                vec![
                    "bundle.tar",
                    "bundle.zip",
                    "device-link",
                    "native-pipe",
                    "native-socket",
                    "ordinary.sock",
                    "pipe-link",
                    "socket-link",
                ],
                1,
            ),
            (
                "types:regular,symlink archives:off",
                vec![],
                index.documents.len(),
            ),
        ] {
            let engine = SearchEngine::default();
            let (hits, summary) = collect_run(
                &engine
                    .start_indexed(
                        Arc::new(serde_json::from_value(legacy.clone()).unwrap()),
                        Query::parse(condition, MatchMode::Exact).unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            let mut names: Vec<_> = hits.iter().map(|hit| hit.entry.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, expected, "{condition}");
            assert_eq!(summary.content_skipped, skipped, "{condition}");
        }
        for malformed in [false, true] {
            let mut data = legacy.clone();
            if malformed {
                for document in data["documents"].as_array_mut().unwrap() {
                    document["file_type_bits"] = serde_json::json!(u32::MAX);
                }
            }
            let engine = SearchEngine::default();
            let (hits, summary) = collect_run(
                &engine
                    .start_indexed(
                        Arc::new(serde_json::from_value(data).unwrap()),
                        Query::parse("type:fifo archives:off", MatchMode::Exact).unwrap(),
                        100,
                        Arc::new(|| {}),
                    )
                    .unwrap(),
            );
            assert!(hits.is_empty());
            assert_eq!(summary.content_skipped, index.documents.len());
        }
        let metadata = std::fs::symlink_metadata("/dev/null").unwrap();
        assert_eq!(
            crate::query::native_file_type_bits(&metadata),
            Some(crate::query::SpecialFileType::CharDevice.bits())
        );
        assert!(open_regular_file(Path::new("/dev/null")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn regular_file_query_agrees_across_live_indexed_zip_and_tar() {
        let tmp = TempDir::new();
        let regular = tmp.file("regular.txt", "needle");
        std::fs::hard_link(&regular, tmp.path().join("hard.txt")).unwrap();
        tmp.dir("folder.txt");
        std::os::unix::fs::symlink(&regular, tmp.path().join("symbolic.txt")).unwrap();
        std::os::unix::fs::symlink("missing", tmp.path().join("broken.txt")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(tmp.path().join("pipe.txt"))
                .status()
                .unwrap()
                .success()
        );
        let _socket =
            std::os::unix::net::UnixListener::bind(tmp.path().join("socket.txt")).unwrap();
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        zip.start_file("zip-regular.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"needle").unwrap();
        zip.add_symlink(
            "zip-link.txt",
            "zip-regular.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "tar-regular.txt", &b"needle"[..])
            .unwrap();
        for (name, kind) in [
            ("tar-link.txt", tar::EntryType::Symlink),
            ("tar-hard.txt", tar::EntryType::Link),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_entry_type(kind);
            tar.append_link(&mut header, name, "tar-regular.txt")
                .unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "type:regular ext:txt",
                        vec![
                            "hard.txt",
                            "regular.txt",
                            "tar-regular.txt",
                            "zip-regular.txt",
                        ],
                    ),
                    (
                        "type:file ext:txt archives:off",
                        vec![
                            "broken.txt",
                            "hard.txt",
                            "pipe.txt",
                            "regular.txt",
                            "socket.txt",
                            "symbolic.txt",
                        ],
                    ),
                ] {
                    let query = Query::parse(condition, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
        let mut legacy = serde_json::to_value(index.as_ref()).unwrap();
        for document in legacy["documents"].as_array_mut().unwrap() {
            document.as_object_mut().unwrap().remove("is_regular");
        }
        let query = Query::parse("type:regular archives:off", MatchMode::Exact).unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(
                    Arc::new(serde_json::from_value(legacy).unwrap()),
                    query,
                    100,
                    Arc::new(|| {}),
                )
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, index.documents.len());
    }

    #[test]
    fn strict_size_decimal_bounds_agree_across_live_indexed_and_zip_results() {
        let tmp = TempDir::new();
        tmp.file("three.txt", "123");
        tmp.file("four.txt", "1234");
        tmp.file("five.txt", "12345");
        write_zip(&tmp.path().join("bundle.zip"), "arch.txt", b"1234");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "size:>3.499999999999999999999999B",
                        vec!["arch.txt", "five.txt", "four.txt"],
                    ),
                    ("size:<4.499999999999999999999999B", vec!["three.txt"]),
                    ("size:=3.499999999999999999999999B", vec!["three.txt"]),
                ] {
                    let query = Query::parse(&format!("ext:txt {condition}"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
    }

    #[test]
    fn precise_age_query_agrees_across_live_indexed_and_archive_results() {
        let tmp = TempDir::new();
        let now = SystemTime::now();
        for (name, modified) in [
            ("recent.txt", now - Duration::from_secs(600)),
            ("old.txt", now - Duration::from_secs(7200)),
            ("future.txt", now + Duration::from_secs(7200)),
        ] {
            tmp.file(name, "needle");
            std::fs::File::options()
                .write(true)
                .open(tmp.path().join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
        }
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o644);
        header.set_mtime(
            (now - Duration::from_secs(600))
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        header.set_cksum();
        tar.append_data(&mut header, "arch.txt", &b"needle"[..])
            .unwrap();
        tar.finish().unwrap();
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    ("age:<=30min", vec!["arch.txt", "future.txt", "recent.txt"]),
                    ("age:>1h age:<3h", vec!["old.txt"]),
                    ("age:=0s", vec!["future.txt"]),
                ] {
                    let query =
                        Query::parse(&format!("{condition} ext:txt content:needle"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
    }

    #[test]
    fn precise_indexed_modification_times_match_live_search() {
        let tmp = TempDir::new();
        for (name, date) in [
            ("fractional.txt", "2026-01-01T00:00:00.5Z"),
            ("pre-epoch.txt", "1969-12-31T23:59:59.5Z"),
        ] {
            tmp.file(name, "needle");
            let expected = crate::query::parse_absolute_time(date).unwrap();
            std::fs::File::options()
                .write(true)
                .open(tmp.path().join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(expected))
                .unwrap();
            assert_eq!(
                std::fs::metadata(tmp.path().join(name))
                    .unwrap()
                    .modified()
                    .unwrap(),
                expected
            );
        }
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        "after:2026-01-01T00:00:00.25Z before:2026-01-01T00:00:00.75Z",
                        vec!["fractional.txt"],
                    ),
                    ("before:1970-01-01", vec!["pre-epoch.txt"]),
                ] {
                    let query = Query::parse(&format!("{condition} content:needle"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, _) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                }
            }
        }
    }

    #[test]
    fn absolute_modified_dates_agree_across_live_indexed_and_archives() {
        let tmp = TempDir::new();
        for (name, date) in [("old.txt", "2020-01-01"), ("new.txt", "2026-01-01")] {
            tmp.file(name, "needle");
            std::fs::File::options()
                .write(true)
                .open(tmp.path().join(name))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(crate::query::parse_absolute_time(date).unwrap()),
                )
                .unwrap();
        }
        let mut writer =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .last_modified_time(zip::DateTime::from_date_and_time(2020, 1, 1, 0, 0, 0).unwrap());
        writer.start_file("arch.txt", options).unwrap();
        writer.write_all(b"needle").unwrap();
        writer.finish().unwrap();
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    ("after:2025-01-01", vec!["new.txt"]),
                    ("before:2025-01-01", vec!["arch.txt", "old.txt"]),
                    ("after:2027-01-01 before:2025-01-01", vec![]),
                ] {
                    let query =
                        Query::parse(&format!("{condition} ext:txt content:needle"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                    assert!(hits.iter().all(|hit| hit.explanation.components.iter().any(
                        |component| component.field == "filter"
                            && (component.detail.starts_with("after:")
                                || component.detail.starts_with("before:"))
                    )));
                }
            }
        }
    }

    #[test]
    fn created_search_uses_birth_time_live_and_cached_without_archive_or_mtime_fallback() {
        let tmp = TempDir::new();
        let lower =
            chrono::DateTime::<chrono::Utc>::from(SystemTime::now() - Duration::from_secs(86400))
                .to_rfc3339();
        let upper =
            chrono::DateTime::<chrono::Utc>::from(SystemTime::now() + Duration::from_secs(86400))
                .to_rfc3339();
        let expression = format!("name:born- created-after:{lower} created-before:{upper}");
        for name in ["born-first.txt", "born-middle.txt", "born-last.txt"] {
            tmp.file(name, "needle");
            std::fs::File::open(tmp.path().join(name))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(SystemTime::now() + Duration::from_secs(7 * 86400)),
                )
                .unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                let query = Query::parse(&expression, mode).unwrap();
                let engine = SearchEngine::default();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                let (hits, summary) = collect_run(&run);
                let expected = index
                    .documents
                    .iter()
                    .filter(|d| {
                        d.path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("born-")
                            && d.entry().unwrap().created.is_some_and(|t| {
                                t > crate::query::parse_absolute_time(&lower).unwrap()
                                    && t < crate::query::parse_absolute_time(&upper).unwrap()
                            })
                    })
                    .count();
                assert_eq!(hits.len(), expected);
                assert!(hits.iter().all(|h| {
                    h.entry.created.is_some()
                        && h.entry.created != h.entry.modified
                        && h.explanation
                            .components
                            .iter()
                            .any(|c| c.detail.starts_with("created-"))
                }));
                assert_eq!(
                    summary.content_skipped,
                    index
                        .documents
                        .iter()
                        .filter(|d| d.entry().unwrap().created.is_none())
                        .count()
                );
            }
        }
        write_zip(&tmp.path().join("bundle.zip"), "born-member.txt", b"needle");
        let engine = SearchEngine::default();
        let query = Query::parse(
            &format!("name:born- created-after:{lower} archives:on"),
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(
            hits.iter()
                .all(|h| matches!(h.location, SearchLocation::Native(_)))
        );
        assert!(summary.content_skipped >= 1);
        let mut old = (*index).clone();
        for d in &mut old.documents {
            d.created_time = None;
        }
        let engine = SearchEngine::default();
        let query = Query::parse(
            &format!("name:born- created-after:{lower}"),
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(Arc::new(old), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 3);
        std::fs::remove_file(tmp.path().join("born-middle.txt")).unwrap();
        let engine = SearchEngine::default();
        let query = Query::parse(
            &format!("name:born-middle created-after:{lower}"),
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, _) = collect_run(
            &engine
                .start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(
            hits.len(),
            usize::from(
                index
                    .documents
                    .iter()
                    .find(|d| d.path.ends_with("born-middle.txt"))
                    .unwrap()
                    .entry()
                    .unwrap()
                    .created
                    .is_some()
            )
        );
    }

    #[test]
    fn changed_search_uses_status_time_live_and_cached_without_archive_or_mtime_fallback() {
        let tmp = TempDir::new();
        let lower =
            chrono::DateTime::<chrono::Utc>::from(SystemTime::now() - Duration::from_secs(86400))
                .to_rfc3339();
        let upper =
            chrono::DateTime::<chrono::Utc>::from(SystemTime::now() + Duration::from_secs(86400))
                .to_rfc3339();
        let expression = format!("name:status- changed-after:{lower} changed-before:{upper}");
        for name in ["status-first.txt", "status-middle.txt", "status-last.txt"] {
            tmp.file(name, "needle");
            std::fs::File::open(tmp.path().join(name))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(SystemTime::now() + Duration::from_secs(7 * 86400)),
                )
                .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    tmp.path().join(name),
                    std::fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                let query = Query::parse(&expression, mode).unwrap();
                let engine = SearchEngine::default();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                let (hits, summary) = collect_run(&run);
                let expected = index
                    .documents
                    .iter()
                    .filter(|d| {
                        d.path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("status-")
                            && d.entry().unwrap().changed.is_some_and(|t| {
                                t > crate::query::parse_absolute_time(&lower).unwrap()
                                    && t < crate::query::parse_absolute_time(&upper).unwrap()
                            })
                    })
                    .count();
                assert_eq!(hits.len(), expected);
                assert!(hits.iter().all(|h| {
                    h.entry.changed.is_some()
                        && h.entry.changed != h.entry.modified
                        && h.explanation
                            .components
                            .iter()
                            .any(|c| c.detail.starts_with("changed-"))
                }));
                assert_eq!(
                    summary.content_skipped,
                    index
                        .documents
                        .iter()
                        .filter(|d| d.entry().unwrap().changed.is_none())
                        .count()
                );
            }
        }
        write_zip(
            &tmp.path().join("bundle.zip"),
            "status-member.txt",
            b"needle",
        );
        let engine = SearchEngine::default();
        let query = Query::parse(
            &format!("name:status- changed-after:{lower} archives:on"),
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(
            hits.iter()
                .all(|h| matches!(h.location, SearchLocation::Native(_)))
        );
        assert!(summary.content_skipped >= 1);
        let mut old = (*index).clone();
        for d in &mut old.documents {
            d.changed_time = None;
        }
        let engine = SearchEngine::default();
        let query = Query::parse(
            &format!("name:status- changed-after:{lower}"),
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(Arc::new(old), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 3);
        std::fs::remove_file(tmp.path().join("status-middle.txt")).unwrap();
        let engine = SearchEngine::default();
        let query = Query::parse(
            &format!("name:status-middle changed-after:{lower}"),
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, _) = collect_run(
            &engine
                .start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(
            hits.len(),
            usize::from(
                index
                    .documents
                    .iter()
                    .find(|d| d.path.ends_with("status-middle.txt"))
                    .unwrap()
                    .entry()
                    .unwrap()
                    .changed
                    .is_some()
            )
        );
    }

    #[test]
    fn accessed_search_uses_precise_live_cached_observations_and_skips_archive_unknowns() {
        let tmp = TempDir::new();
        let samples = [
            ("early.txt", "2026-01-01T00:00:00.1Z"),
            ("middle.txt", "2026-01-01T00:00:00.5Z"),
            ("late.txt", "2026-01-01T00:00:00.9Z"),
            ("pre.txt", "1969-12-31T23:59:59.5Z"),
        ];
        for (name, _) in samples {
            tmp.file(name, "needle");
        }
        let reset = || {
            for (name, t) in samples {
                std::fs::File::open(tmp.path().join(name))
                    .unwrap()
                    .set_times(
                        std::fs::FileTimes::new()
                            .set_accessed(crate::query::parse_absolute_time(t).unwrap()),
                    )
                    .unwrap();
            }
        };
        reset();
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (expression, expected) in [
                    (
                        "accessed-after:2026-01-01T00:00:00.25Z accessed-before:2026-01-01T00:00:00.75Z",
                        vec!["middle.txt"],
                    ),
                    ("accessed-before:1970-01-01", vec!["pre.txt"]),
                ] {
                    reset();
                    let query = Query::parse(expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, _) = collect_run(&run);
                    assert_eq!(
                        hits.iter()
                            .map(|h| h.entry.name.as_str())
                            .collect::<Vec<_>>(),
                        expected
                    );
                    assert!(hits.iter().all(|h| {
                        h.accessed == h.entry.accessed
                            && h.explanation
                                .components
                                .iter()
                                .any(|c| c.detail.starts_with("accessed-"))
                    }));
                }
            }
        }
        write_zip(&tmp.path().join("bundle.zip"), "arch.txt", b"needle");
        reset();
        let query = Query::parse(
            "ext:txt accessed-before:2027-01-01 archives:on",
            MatchMode::Exact,
        )
        .unwrap();
        let engine = SearchEngine::default();
        let (hits, summary) = collect_run(
            &engine
                .start(tmp.path().to_owned(), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(hits.len(), 4);
        assert_eq!(summary.content_skipped, 1);
        let mut missing = (*index).clone();
        let document = missing
            .documents
            .iter_mut()
            .find(|d| d.path.ends_with("middle.txt"))
            .unwrap();
        document.accessed_time = None;
        document.accessed_secs = None;
        let engine = SearchEngine::default();
        let query = Query::parse(
            "accessed-after:2026-01-01T00:00:00.25Z accessed-before:2026-01-01T00:00:00.75Z",
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, summary) = collect_run(
            &engine
                .start_indexed(Arc::new(missing), query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert!(hits.is_empty());
        assert_eq!(summary.content_skipped, 1);
        std::fs::remove_file(tmp.path().join("middle.txt")).unwrap();
        let engine = SearchEngine::default();
        let query = Query::parse(
            "accessed-after:2026-01-01T00:00:00.25Z accessed-before:2026-01-01T00:00:00.75Z",
            MatchMode::Exact,
        )
        .unwrap();
        let (hits, _) = collect_run(
            &engine
                .start_indexed(index, query, 100, Arc::new(|| {}))
                .unwrap(),
        );
        assert_eq!(hits[0].entry.name, "middle.txt");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_directory_query_filters_native_branches_without_inventing_archive_paths() {
        let tmp = TempDir::new();
        for name in [
            "vendor/cache/drop.txt",
            "vendor/deep/cache/deep.txt",
            "other/cache/keep.txt",
        ] {
            tmp.file(name, "needle");
        }
        write_zip(
            &tmp.path().join("bundle.zip"),
            "vendor/cache/zip.txt",
            b"needle",
        );
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "vendor/cache/tar.txt", &b"needle"[..])
            .unwrap();
        tar.finish().unwrap();
        // Real archives under an excluded native directory must also be skipped.
        write_zip(
            &tmp.path().join("vendor/cache/blocked.zip"),
            "hidden.txt",
            b"needle",
        );
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (suffix, expected) in [
                    (
                        "vendor/cache",
                        vec!["deep.txt", "keep.txt", "tar.txt", "zip.txt"],
                    ),
                    ("vendor/**/cache", vec!["keep.txt", "tar.txt", "zip.txt"]),
                    (
                        "bundle.zip!/**",
                        vec![
                            "deep.txt",
                            "drop.txt",
                            "hidden.txt",
                            "keep.txt",
                            "tar.txt",
                            "zip.txt",
                        ],
                    ),
                    (
                        "bundle.zip",
                        vec![
                            "deep.txt",
                            "drop.txt",
                            "hidden.txt",
                            "keep.txt",
                            "tar.txt",
                            "zip.txt",
                        ],
                    ),
                    (
                        "other/cache/keep.txt",
                        vec![
                            "deep.txt",
                            "drop.txt",
                            "hidden.txt",
                            "keep.txt",
                            "tar.txt",
                            "zip.txt",
                        ],
                    ),
                ] {
                    let expression = format!(
                        r#"ext:txt content:needle excludedir:"{}""#,
                        tmp.path().join(suffix).display()
                    );
                    let query = Query::parse(&expression, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {suffix}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
    }

    #[test]
    fn relative_directory_query_scopes_native_index_zip_and_tar_separately() {
        let tmp = TempDir::new();
        let members = [
            "vendor/cache/drop.txt",
            "vendor/deep/cache/deep.txt",
            "other/cache/keep.txt",
            "other/vendor/cache/nested.txt",
            "vendor/CACHE-ONLY/upper.txt",
            "vendor/file.txt",
        ];
        for name in members {
            tmp.file(name, "needle");
        }
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        for name in members {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for name in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"needle"[..]).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, names) in [
                    (
                        "excludedir:vendor/cache*",
                        vec!["deep.txt", "file.txt", "keep.txt", "nested.txt"],
                    ),
                    (
                        "excludedir:vendor/**/cache*",
                        vec!["file.txt", "keep.txt", "nested.txt"],
                    ),
                    ("excludedir:**/cache*", vec!["file.txt"]),
                    ("excludedir:vendor/**", vec!["keep.txt", "nested.txt"]),
                    (
                        "case:sensitive excludedir:vendor/cache*",
                        vec![
                            "deep.txt",
                            "file.txt",
                            "keep.txt",
                            "nested.txt",
                            "upper.txt",
                        ],
                    ),
                    (
                        "excludedir:vendor/file.txt",
                        vec![
                            "deep.txt",
                            "drop.txt",
                            "file.txt",
                            "keep.txt",
                            "nested.txt",
                            "upper.txt",
                        ],
                    ),
                    // A virtual container prefix is not part of a member's scope.
                    (
                        "excludedir:bundle.zip!/**",
                        vec![
                            "deep.txt",
                            "drop.txt",
                            "file.txt",
                            "keep.txt",
                            "nested.txt",
                            "upper.txt",
                        ],
                    ),
                ] {
                    let query = Query::parse(&format!("ext:txt content:needle {conditions}"), mode)
                        .unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut actual: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    actual.sort_unstable();
                    let mut expected: Vec<_> =
                        names.into_iter().flat_map(|name| [name; 3]).collect();
                    expected.sort_unstable();
                    assert_eq!(actual, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
    }

    #[test]
    fn directory_exclusion_query_prunes_live_indexed_zip_and_tar_directories_only() {
        let tmp = TempDir::new();
        for name in [
            "keep.txt",
            "cache.tmp/drop.txt",
            "cache.tmp/nested/deep.txt",
            "other/cache.tmp",
            "Upper.TMP/upper.txt",
            "build cache/space.txt",
        ] {
            tmp.file(name, "needle");
        }
        tmp.file("cache.tmp/binary.bin", "\0binary");
        let mut zip =
            zip::ZipWriter::new(std::fs::File::create(tmp.path().join("bundle.zip")).unwrap());
        zip.add_directory("cache.tmp/", zip::write::SimpleFileOptions::default())
            .unwrap();
        for name in [
            "cache.tmp/zip-drop.txt",
            "other/cache.tmp",
            "keep/zip-keep.txt",
            "Upper.TMP/zip-upper.txt",
            "build cache/zip-space.txt",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"needle").unwrap();
        }
        zip.finish().unwrap();
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for name in [
            "cache.tmp/tar-drop.txt",
            "other/cache.tmp",
            "keep/tar-keep.txt",
            "Upper.TMP/tar-upper.txt",
            "build cache/tar-space.txt",
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(6);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"needle"[..]).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        let root_query = format!(
            "excludedir:{} glob:keep.txt archives:off",
            tmp.path().file_name().unwrap().to_string_lossy()
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (condition, expected) in [
                    (
                        r#"glob:*.txt excludedir:*.tmp excludedir:"build cache" content:needle"#,
                        vec!["keep.txt", "tar-keep.txt", "zip-keep.txt"],
                    ),
                    (
                        r#"glob:cache.tmp excludedir:*.tmp excludedir:"build cache""#,
                        vec!["cache.tmp", "cache.tmp", "cache.tmp"],
                    ),
                    (
                        r#"case:sensitive glob:*.txt excludedir:*.tmp excludedir:"build cache" content:needle"#,
                        vec![
                            "keep.txt",
                            "tar-keep.txt",
                            "tar-upper.txt",
                            "upper.txt",
                            "zip-keep.txt",
                            "zip-upper.txt",
                        ],
                    ),
                    (root_query.as_str(), vec!["keep.txt"]),
                ] {
                    let query = Query::parse(condition, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {condition}");
                    assert_eq!(summary.content_skipped, 0);
                }
            }
        }
    }

    #[test]
    fn directory_exclusion_query_keeps_archive_containers_and_virtual_scope() {
        let tmp = TempDir::new();
        tmp.file("blocked.zip/drop.txt", "needle");
        tmp.file("blocked.zip!/drop.txt", "needle");
        write_zip(&tmp.path().join("bundle.zip"), "keep.txt", b"needle");
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                let query = Query::parse(
                    "excludedir:*.zip excludedir:*.zip! ext:txt content:needle",
                    mode,
                )
                .unwrap();
                let engine = SearchEngine::default();
                let run = if indexed {
                    engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                } else {
                    engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                }
                .unwrap();
                let (hits, summary) = collect_run(&run);
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].entry.name, "keep.txt");
                assert_eq!(summary.content_skipped, 0);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn directory_exclusion_query_does_not_treat_symlinks_as_directories() {
        let tmp = TempDir::new();
        tmp.file("actual/inside.txt", "needle");
        std::os::unix::fs::symlink("actual", tmp.path().join("cache.tmp")).unwrap();
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            let query = Query::parse(
                "glob:cache.tmp excludedir:*.tmp archives:off",
                MatchMode::Exact,
            )
            .unwrap();
            let engine = SearchEngine::default();
            let run = if indexed {
                engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
            } else {
                engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
            }
            .unwrap();
            let (hits, summary) = collect_run(&run);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].entry.name, "cache.tmp");
            assert_eq!(summary.content_skipped, 0);
        }
    }

    #[test]
    fn exclude_file_query_keeps_directories_and_archive_descendants() {
        let tmp = TempDir::new();
        for name in ["keep.txt", "cache.tmp/child.txt", "Case.TMP"] {
            tmp.file(name, "needle");
        }
        for name in ["top.tmp", "cache.tmp/nested.tmp", "ignore.bak"] {
            tmp.file(name, "\0binary");
        }
        write_zip(
            &tmp.path().join("bundle.zip"),
            "folder.tmp/zip.txt",
            b"needle",
        );
        let mut tar =
            tar::Builder::new(std::fs::File::create(tmp.path().join("bundle.tar")).unwrap());
        for (name, data) in [
            ("folder.tmp/tar.txt", &b"needle"[..]),
            ("drop.tmp", &b"\0binary"[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, data).unwrap();
        }
        tar.finish().unwrap();
        let built = crate::content_index::build_test_index(tmp.path());
        let index = Arc::new(
            serde_json::from_str::<crate::content_index::RootIndex>(
                &serde_json::to_string(&built).unwrap(),
            )
            .unwrap(),
        );
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected, skipped) in [
                    (
                        "type:file excludefile:*.tmp excludefile:*.bak content:needle",
                        vec!["child.txt", "keep.txt", "tar.txt", "zip.txt"],
                        2,
                    ),
                    (
                        "type:file excludefile:*.tmp excludefile:*.bak excludefile:*.zip excludefile:*.tar content:needle",
                        vec!["child.txt", "keep.txt", "tar.txt", "zip.txt"],
                        0,
                    ),
                    (
                        "type:file case:sensitive excludefile:*.tmp excludefile:*.bak excludefile:*.zip excludefile:*.tar content:needle",
                        vec!["Case.TMP", "child.txt", "keep.txt", "tar.txt", "zip.txt"],
                        0,
                    ),
                    (
                        "glob:*.tmp excludefile:*.tmp archives:off",
                        vec!["cache.tmp"],
                        0,
                    ),
                    (
                        "type:file exclude:*.tmp excludefile:*.bak content:needle",
                        vec!["keep.txt"],
                        2,
                    ),
                ] {
                    let query = Query::parse(conditions, mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, skipped, "{indexed}: {conditions}");
                    for hit in hits {
                        if conditions.contains("excludefile:") {
                            assert!(
                                hit.explanation
                                    .components
                                    .iter()
                                    .any(|component| component.detail.starts_with("excludefile:"))
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn extension_alternatives_agree_across_live_indexed_and_archives() {
        let tmp = TempDir::new();
        for name in ["main.rs", "Upper.RS", "Cargo.toml", "README.md"] {
            tmp.file(name, "needle");
        }
        tmp.file("binary.rs", "\0binary");
        tmp.dir("folder.rs");
        write_zip(
            &tmp.path().join("bundle.zip"),
            "nested/arch.toml",
            b"needle",
        );
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        for indexed in [false, true] {
            for mode in MatchMode::ALL {
                for (conditions, expected, skipped) in [
                    (
                        "exts:rs,toml",
                        vec!["Cargo.toml", "Upper.RS", "arch.toml", "main.rs"],
                        1,
                    ),
                    ("case:sensitive exts:RS,TOML", vec!["Upper.RS"], 0),
                    ("exts:rs,toml ext:toml", vec!["Cargo.toml", "arch.toml"], 0),
                    (
                        "exts:rs,toml exts:toml,md",
                        vec!["Cargo.toml", "arch.toml"],
                        0,
                    ),
                ] {
                    let query =
                        Query::parse(&format!("{conditions} content:needle"), mode).unwrap();
                    let engine = SearchEngine::default();
                    let run = if indexed {
                        engine.start_indexed(Arc::clone(&index), query, 100, Arc::new(|| {}))
                    } else {
                        engine.start(tmp.path().to_path_buf(), query, 100, Arc::new(|| {}))
                    }
                    .unwrap();
                    let (hits, summary) = collect_run(&run);
                    let mut names: Vec<_> =
                        hits.iter().map(|hit| hit.entry.name.as_str()).collect();
                    names.sort_unstable();
                    assert_eq!(names, expected, "{indexed}: {mode:?}: {conditions}");
                    assert_eq!(summary.content_skipped, skipped);
                }
            }
        }
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
        let error = summary.archive_error.expect("the open error is reported");
        assert!(error.contains("broken.zip"), "{error}");
    }

    #[test]
    fn filtered_content_search_reports_corrupt_tar_gz_archives() {
        let tmp = TempDir::new();
        tmp.file("server.log", "no match here");
        // The archive fails the name filter, so it is never read as a binary
        // file; failing to read its members must still be counted.
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
                requires_empty: false,
                requires_broken_symlink: false,
                max_depth: None,
                exclusions: Arc::new(Vec::new()),
                directory_exclusions: Arc::new(Vec::new()),
                search_archives: true,
                hidden_mode: crate::query::HiddenMode::Include,
                unsearched: Cell::new(0),
                archives_capped: Cell::new(false),
                archive_error: RefCell::new(None),
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

        assert_eq!(run.provider, "Index + archives");
        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.name, "notes.txt");
        assert_eq!(summary.content_skipped, 0);
    }

    #[test]
    fn live_provider_searches_text_inside_plain_and_compressed_tar() {
        let tmp = TempDir::new();
        for name in ["logs.tar", "logs.tgz", "logs.tbz2", "logs.txz", "logs.tzst"] {
            let mut builder = tar::Builder::new(Vec::new());
            let content = b"Unicode archive needle";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(37);
            header.set_cksum();
            builder
                .append_data(&mut header, "docs/inside.txt", &content[..])
                .unwrap();
            let bytes = builder.into_inner().unwrap();
            let bytes = crate::archive::encode_tar_fixture(
                crate::archive::kind_of(Path::new(name)).unwrap(),
                &bytes,
            )
            .unwrap();
            std::fs::write(tmp.path().join(name), bytes).unwrap();
        }
        let engine = SearchEngine::default();
        let run = engine
            .start(
                tmp.path().into(),
                Query::parse("path:inside.txt content:needle", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert_eq!(summary.matched, 5);
        assert_eq!(summary.content_skipped, 0);
        assert!(!summary.truncated);
        assert_eq!(hits.len(), 5);
        for hit in hits {
            assert!(
                matches!(&hit.location, SearchLocation::ArchiveMember { member, .. } if member == Path::new("docs/inside.txt"))
            );
            assert!(hit.reveal_path().is_file());
        }
    }

    #[test]
    fn indexed_provider_searches_new_compressed_tar_containers_with_content_filters() {
        let tmp = TempDir::new();
        for name in ["bundle.tbz2", "bundle.txz", "bundle.tzst"] {
            let mut builder = tar::Builder::new(Vec::new());
            for (member, content) in [
                ("docs/inside.txt", "Unicode needle ёж"),
                ("docs/other.txt", "unrelated"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, member, content.as_bytes())
                    .unwrap();
            }
            let bytes = builder.into_inner().unwrap();
            std::fs::write(
                tmp.path().join(name),
                crate::archive::encode_tar_fixture(
                    crate::archive::kind_of(Path::new(name)).unwrap(),
                    &bytes,
                )
                .unwrap(),
            )
            .unwrap();
        }
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        let run = SearchEngine::default()
            .start_indexed(
                index,
                Query::parse("path:inside.txt content:ёж", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let (hits, summary) = collect_run(&run);
        assert_eq!(hits.len(), 3);
        assert_eq!(summary.matched, 3);
        assert!(!summary.truncated);
        assert_eq!(summary.content_skipped, 0);
        assert!(hits.iter().all(|hit| matches!(&hit.location, SearchLocation::ArchiveMember { member, .. } if member == Path::new("docs/inside.txt"))));
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

        assert_eq!(run.provider, "Live + archives");
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
    fn archive_members_report_their_own_modified_time() {
        let tmp = TempDir::new();
        let archive = tmp.path().join("dated.zip");
        let file = std::fs::File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let stamp = zip::DateTime::from_date_and_time(2001, 2, 3, 4, 5, 6).unwrap();
        let options = zip::write::SimpleFileOptions::default().last_modified_time(stamp);
        writer.start_file("old.txt", options).unwrap();
        writer.write_all(b"old").unwrap();
        writer.finish().unwrap();
        let engine = SearchEngine::default();
        // The archive itself was written just now; only the member is old.
        let run = engine
            .start(
                tmp.path().to_path_buf(),
                Query::parse("path:old.txt date:>365d", MatchMode::Exact).unwrap(),
                100,
                Arc::new(|| {}),
            )
            .unwrap();
        let (hits, _) = collect_run(&run);
        assert_eq!(hits.len(), 1, "{hits:?}");
        let year = chrono::DateTime::<chrono::Local>::from(hits[0].entry.modified.unwrap())
            .format("%Y")
            .to_string();
        assert_eq!(year, "2001");
    }

    #[test]
    fn deflate64_content_search_agrees_for_live_and_indexed_zipx() {
        let tmp = TempDir::new();
        for name in ["one.zip", "two.ZIPX"] {
            std::fs::write(
                tmp.path().join(name),
                include_bytes!(
                    "../docs/qualification/zip-deflate64-2026-11-03/fixtures/extended.zipx"
                ),
            )
            .unwrap();
        }
        let engine = SearchEngine::default();
        let query =
            Query::parse("path:ёж.txt content:needle archives:on", MatchMode::Exact).unwrap();
        let live = engine
            .start(tmp.path().into(), query.clone(), 100, Arc::new(|| {}))
            .unwrap();
        let (live_hits, live_summary) = collect_run(&live);
        let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
        let indexed = engine
            .start_indexed(index, query, 100, Arc::new(|| {}))
            .unwrap();
        let (indexed_hits, indexed_summary) = collect_run(&indexed);
        for (hits, summary) in [(live_hits, live_summary), (indexed_hits, indexed_summary)] {
            assert_eq!(summary.matched, 2);
            assert_eq!(summary.content_skipped, 0);
            assert!(!summary.truncated);
            assert_eq!(hits.len(), 2);
            for hit in hits {
                assert!(
                    matches!(hit.location,SearchLocation::ArchiveMember{member,..} if member==Path::new("nested/ёж.txt"))
                );
            }
        }
    }

    #[test]
    fn bzip2_zip_and_zipx_content_agree_for_live_and_indexed_search() {
        let tmp = TempDir::new();
        for name in ["one.zip", "two.ZIPX"] {
            std::fs::write(
                tmp.path().join(name),
                include_bytes!("../docs/qualification/zip-bzip2-2026-11-03/fixtures/mixed.zipx"),
            )
            .unwrap();
        }
        let engine = SearchEngine::default();
        for (expression, skipped) in [
            ("content:needle archives:on", 2),
            ("path:ёж.txt content:needle archives:on", 0),
        ] {
            let query = Query::parse(expression, MatchMode::Exact).unwrap();
            let live = engine
                .start(tmp.path().into(), query.clone(), 100, Arc::new(|| {}))
                .unwrap();
            let (live_hits, live_summary) = collect_run(&live);
            let index = Arc::new(crate::content_index::build_test_index(tmp.path()));
            let indexed = engine
                .start_indexed(index, query, 100, Arc::new(|| {}))
                .unwrap();
            let (indexed_hits, indexed_summary) = collect_run(&indexed);
            for (hits, summary) in [(live_hits, live_summary), (indexed_hits, indexed_summary)] {
                assert_eq!(summary.matched, 2);
                assert_eq!(summary.content_skipped, skipped, "{expression}");
                assert!(!summary.truncated);
                assert_eq!(hits.len(), 2);
                for hit in hits {
                    assert!(
                        matches!(hit.location,SearchLocation::ArchiveMember {member,..} if member==Path::new("nested/ёж.txt"))
                    );
                }
            }
        }
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
            let explanation = compiled.evaluate_metadata(
                &entry,
                Path::new("/fixture"),
                now,
                ObservedMetadata::default(),
            )?;
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
    fn birth_and_status_pivots_use_independent_metadata_and_unknown_archive_members() {
        use chrono::TimeZone;
        let now: SystemTime = chrono::Local
            .with_ymd_and_hms(2026, 1, 20, 12, 0, 0)
            .single()
            .unwrap()
            .into();
        let mut hit = SearchHit {
            entry: file_entry(PathBuf::from("/x/a.txt"), 1),
            identity: FileIdentity::Path("/x/a.txt".into()),
            accessed: Some(now),
            explanation: MatchExplanation::default(),
            location: SearchLocation::Native("/x/a.txt".into()),
        };
        hit.entry.modified = Some(now);
        hit.entry.created = Some(now - Duration::from_secs(400 * 86400));
        hit.entry.changed = Some(now - Duration::from_secs(86400));
        assert_eq!(
            time_bucket(&hit, TimePivot::Created, now),
            TimeBucket::Older
        );
        assert_eq!(
            time_bucket(&hit, TimePivot::Changed, now),
            TimeBucket::Yesterday
        );
        for pivot in [TimePivot::Created, TimePivot::Changed] {
            assert_eq!(
                serde_json::from_str::<TimePivot>(&serde_json::to_string(&pivot).unwrap()).unwrap(),
                pivot
            );
        }
        hit.entry.created = None;
        hit.entry.changed = None;
        assert_eq!(
            time_bucket(&hit, TimePivot::Created, now),
            TimeBucket::Unknown
        );
        assert_eq!(
            time_bucket(&hit, TimePivot::Changed, now),
            TimeBucket::Unknown
        );
        hit.entry.created = Some(now);
        hit.entry.changed = Some(now);
        hit.location = SearchLocation::ArchiveMember {
            archive: "/x/a.zip".into(),
            member: "a.txt".into(),
            index: 0,
        };
        assert_eq!(
            time_bucket(&hit, TimePivot::Created, now),
            TimeBucket::Unknown
        );
        assert_eq!(
            time_bucket(&hit, TimePivot::Changed, now),
            TimeBucket::Unknown
        );
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
                        .evaluate_metadata(&entry, Path::new("/"), now, ObservedMetadata::default())
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
        assert!(
            compiled
                .evaluate_metadata(&outside, root, now, ObservedMetadata::default())
                .is_none()
        );
        let explanation = compiled
            .evaluate_metadata(&inside, root, now, ObservedMetadata::default())
            .unwrap();
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
