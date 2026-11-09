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
            let result = crate::archive::visit_members_with_password(
                archive_path,
                request.requires_content,
                budget,
                request.archive_password.as_ref().map(|p| p.as_bytes()),
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
    drop(request.archive_password);
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
mod tests;
