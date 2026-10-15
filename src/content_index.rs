//! Optional, root-scoped content indexes built only while the app is idle.

use crate::panel::{FileEntry, format_size};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, ReadDir};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const SCHEMA_VERSION: u32 = 1;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
pub const MAX_DOCUMENTS: usize = 200_000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_CONTENT_BYTES: usize = 32 * 1024 * 1024;
/// Loaded indexes kept in memory at once. Enough for every root the UI
/// asks about in one frame (Find dialog, developer panel), so they never
/// evict each other and reload from disk.
const MAX_CACHED_SNAPSHOTS: usize = 4;
/// Largest index file the loader accepts.
const MAX_INDEX_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Serialized-size budget for documents; the rest of the file cap covers the
/// envelope, root, counters and exclusion list. A build that would exceed it
/// is truncated so every index that saves can also be loaded back.
const MAX_DOCUMENT_JSON_BYTES: usize = MAX_INDEX_FILE_BYTES - 4 * 1024 * 1024;
/// Upper bound on one pretty-printed document's JSON apart from its path and
/// content strings: field names, indentation, and u64-sized numbers.
const DOCUMENT_JSON_OVERHEAD: usize = 384;

pub const fn schema_version() -> u32 {
    SCHEMA_VERSION
}

pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedDocument {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified_secs: Option<u64>,
    pub accessed_secs: Option<u64>,
    pub volume: Option<u64>,
    pub file_id: Option<u64>,
    pub content: Option<String>,
}

impl IndexedDocument {
    pub fn entry(&self) -> Option<FileEntry> {
        let name = self.path.file_name()?.to_string_lossy().to_string();
        let extension = self
            .path
            .extension()
            .map(|value| value.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = self.modified_secs.and_then(epoch_secs);
        let modified_str = modified.map_or_else(|| "-".to_string(), crate::search::format_modified);
        Some(FileEntry {
            name_lower: name.to_lowercase(),
            name,
            path: self.path.clone(),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: self.is_dir,
            size: if self.is_dir { 0 } else { self.size },
            extension,
            modified,
            modified_str,
            size_str: if self.is_dir {
                "...".to_string()
            } else {
                format_size(self.size)
            },
        })
    }

    pub fn accessed(&self) -> Option<SystemTime> {
        self.accessed_secs.and_then(epoch_secs)
    }
}

/// `UNIX_EPOCH + seconds`, or `None` when a corrupt or hand-edited index
/// stores a value `SystemTime` cannot hold (the plain `+` panics there).
fn epoch_secs(seconds: u64) -> Option<SystemTime> {
    std::time::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootIndex {
    schema: u32,
    pub root: PathBuf,
    pub built_at_secs: u64,
    pub documents: Vec<IndexedDocument>,
    pub scanned: usize,
    pub files_seen: usize,
    pub directories_seen: usize,
    pub content_indexed: usize,
    pub content_bytes: usize,
    pub skipped_binary: usize,
    pub skipped_large: usize,
    pub skipped_unreadable: usize,
    pub skipped_quota: usize,
    pub excluded_roots: Vec<PathBuf>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildProgress {
    pub scanned: usize,
    pub indexed: usize,
    pub files_seen: usize,
    pub directories_seen: usize,
    pub content_indexed: usize,
    pub content_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexPhase {
    Disabled,
    Missing,
    WaitingForIdle,
    Building,
    Ready,
    Error,
}

#[derive(Clone, Debug)]
pub struct IndexStatus {
    pub enabled: bool,
    pub phase: IndexPhase,
    pub progress: BuildProgress,
    pub built_at_secs: Option<u64>,
    pub excluded_roots: Vec<PathBuf>,
    pub skipped_binary: usize,
    pub skipped_large: usize,
    pub skipped_unreadable: usize,
    pub skipped_quota: usize,
    pub truncated: bool,
    pub last_error: Option<String>,
}

impl IndexStatus {
    pub fn coverage_percent(&self) -> f32 {
        if self.progress.files_seen == 0 {
            return 0.0;
        }
        self.progress.content_indexed as f32 * 100.0 / self.progress.files_seen as f32
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SettingsStore {
    roots: Vec<RootSettings>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RootSettings {
    root: PathBuf,
    enabled: bool,
    excluded_roots: Vec<PathBuf>,
    last_error: Option<String>,
}

enum BuildEvent {
    Progress(BuildProgress),
    Complete(Result<RootIndex, String>),
}

struct BuildRun {
    root: PathBuf,
    receiver: Receiver<BuildEvent>,
    cancelled: Arc<AtomicBool>,
    /// Held by the worker while it checks for cancellation and saves the
    /// index file, so a disable can wait out an in-flight save before it
    /// deletes the file.
    save_gate: Arc<Mutex<()>>,
    progress: BuildProgress,
    task: crate::workload::TaskHandle,
}

impl Drop for BuildRun {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.task.cancel();
    }
}

pub struct ContentIndex {
    settings: SettingsStore,
    snapshots: HashMap<PathBuf, Arc<RootIndex>>,
    /// Cached roots, least recently used first.
    snapshot_order: Vec<PathBuf>,
    /// Roots whose index file failed to load; not retried until a build or
    /// settings change, since `status()` asks for the snapshot every frame.
    unloadable: HashSet<PathBuf>,
    load_errors: HashMap<PathBuf, String>,
    /// Why the settings file could not be loaded; shown for every root until
    /// a settings change is saved again.
    settings_error: Option<String>,
    /// The settings file exists but could not be read. Saving now would
    /// replace the user's roots with the defaults, so saves are refused.
    settings_unreadable: bool,
    active: Option<BuildRun>,
    workload: crate::workload::WorkloadHandle,
    idle: Arc<AtomicBool>,
    next_generation: u64,
}

impl ContentIndex {
    pub fn load(workload: crate::workload::WorkloadHandle) -> Self {
        let loaded = load_settings_from(&settings_path());
        let mut index = Self::from_settings(loaded.settings, workload);
        index.settings_error = loaded.problem;
        index.settings_unreadable = loaded.unreadable;
        index
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn empty(workload: crate::workload::WorkloadHandle) -> Self {
        Self::from_settings(SettingsStore::default(), workload)
    }

    fn from_settings(settings: SettingsStore, workload: crate::workload::WorkloadHandle) -> Self {
        Self {
            settings,
            snapshots: HashMap::new(),
            snapshot_order: Vec::new(),
            unloadable: HashSet::new(),
            load_errors: HashMap::new(),
            settings_error: None,
            settings_unreadable: false,
            active: None,
            workload,
            idle: Arc::new(AtomicBool::new(false)),
            next_generation: 0,
        }
    }

    pub fn set_idle(&self, idle: bool) {
        self.idle.store(idle, Ordering::Release);
    }

    pub fn is_enabled(&self, root: &Path) -> bool {
        self.settings
            .roots
            .iter()
            .find(|settings| settings.root == root)
            .is_some_and(|settings| settings.enabled)
    }

    /// Why settings must not be saved right now: the file is there but was
    /// unreadable at load, so a save would overwrite it with the defaults.
    fn settings_save_blocked(&self) -> Option<String> {
        settings_save_blocked_reason(self.settings_unreadable, &settings_path())
    }

    pub fn set_enabled(&mut self, root: PathBuf, enabled: bool) -> bool {
        if let Some(reason) = self.settings_save_blocked() {
            log::warn!(target: "commander::content_index", "{reason}");
            self.settings_error = Some(reason);
            return false;
        }
        let previous = self.settings.clone();
        let settings = self.settings_for_mut(root.clone());
        settings.enabled = enabled;
        if !save_settings_to(&settings_path(), &self.settings) {
            self.settings = previous;
            return false;
        }
        if !enabled
            && self.active.as_ref().is_some_and(|run| run.root == root)
            && let Some(run) = self.active.take()
        {
            let save_gate = Arc::clone(&run.save_gate);
            // Dropping cancels the build; taking the gate then waits out a
            // save that had already started, so the file removal below wins.
            drop(run);
            drop(save_gate.lock());
        }
        if !enabled {
            self.forget_root_index(&root, &index_path(&root));
        }
        self.unloadable.remove(&root);
        self.settings_error = None;
        self.settings_unreadable = false;
        true
    }

    pub fn set_exclusions(&mut self, root: &Path, values: Vec<PathBuf>) -> Result<(), String> {
        let exclusions = normalize_exclusions(root, values)?;
        if let Some(reason) = self.settings_save_blocked() {
            self.settings_error = Some(reason.clone());
            return Err(reason);
        }
        let previous = self.settings.clone();
        self.settings_for_mut(root.to_path_buf()).excluded_roots = exclusions;
        if save_settings_to(&settings_path(), &self.settings) {
            self.settings_error = None;
            self.settings_unreadable = false;
            Ok(())
        } else {
            self.settings = previous;
            Err("Could not save index settings".to_string())
        }
    }

    pub fn exclusions(&self, root: &Path) -> Vec<PathBuf> {
        self.settings
            .roots
            .iter()
            .find(|settings| settings.root == root)
            .map(|settings| settings.excluded_roots.clone())
            .unwrap_or_default()
    }

    pub fn start_build(&mut self, root: PathBuf, notify: Notify) -> bool {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ContentIndex) {
            self.load_errors.insert(
                root,
                "Content index disabled by runtime control".to_string(),
            );
            return false;
        }
        if !crate::machine_pressure::snapshot().allows_background_index() {
            self.load_errors.insert(
                root,
                "Content index deferred under battery/thermal pressure".to_string(),
            );
            return false;
        }
        if !self.is_enabled(&root) {
            return false;
        }
        if let Err(reason) = crate::provider_runtime::activate_builtin_explained(
            "content-index",
            &crate::provider_runtime::ActivationRequest {
                capability: crate::provider_runtime::ProviderCapability::IndexBuild,
                root: &root,
                extension: None,
                bytes: None,
            },
        ) {
            self.load_errors
                .insert(root, format!("Content index could not start: {reason}"));
            return false;
        }
        let exclusions = self.exclusions(&root);
        if let Err(error) = self.enqueue_build(root.clone(), exclusions, notify) {
            self.load_errors
                .insert(root, format!("Content index queue refused build: {error}"));
            return false;
        }
        self.load_errors.remove(&root);
        self.unloadable.remove(&root);
        if let Some(settings) = self
            .settings
            .roots
            .iter_mut()
            .find(|settings| settings.root == root)
        {
            settings.last_error = None;
            if !save_settings_to(&settings_path(), &self.settings) {
                log::warn!(
                    target: "commander::content_index",
                    "failed to persist content-index settings after enqueueing build for {}",
                    root.display()
                );
            }
        }
        true
    }

    fn enqueue_build(
        &mut self,
        root: PathBuf,
        exclusions: Vec<PathBuf>,
        notify: Notify,
    ) -> Result<(), crate::workload::AdmissionError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let save_cancelled = Arc::clone(&cancelled);
        let save_gate = Arc::new(Mutex::new(()));
        let worker_save_gate = Arc::clone(&save_gate);
        let idle = Arc::clone(&self.idle);
        let (sender, receiver) = mpsc::channel();
        let worker_root = root.clone();
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        let task = self.workload.submit(
            crate::workload::TaskSpec::new(
                crate::workload::TaskKind::Index,
                root.clone(),
                generation,
            )
            .priority(crate::workload::Priority::Background)
            .estimated_bytes(MAX_CONTENT_BYTES as u64)
            .replace_older_generation(),
            move |scheduler_cancel| {
                let save_scheduler_cancel = scheduler_cancel.clone();
                let result = scan_index(
                    worker_root.clone(),
                    exclusions,
                    idle,
                    worker_cancelled,
                    Some(scheduler_cancel),
                    |progress| {
                        if sender.send(BuildEvent::Progress(progress.clone())).is_err() {
                            return false;
                        }
                        notify();
                        true
                    },
                )
                .and_then(|index| {
                    save_unless_cancelled(
                        &index_path(&worker_root),
                        &index,
                        &worker_save_gate,
                        &save_cancelled,
                        Some(&save_scheduler_cancel),
                    )?;
                    Ok(index)
                });
                let _ = sender.send(BuildEvent::Complete(result));
                notify();
            },
        )?;
        self.active = Some(BuildRun {
            root,
            receiver,
            cancelled,
            save_gate,
            progress: BuildProgress::default(),
            task,
        });
        Ok(())
    }

    pub fn poll(&mut self) {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ContentIndex) {
            self.active = None;
            return;
        }
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(run) = &self.active {
            loop {
                match run.receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        let root = self.active.as_ref().map(|run| run.root.clone());
        let mut complete = None;
        if let Some(run) = self.active.as_mut() {
            for event in events {
                match event {
                    BuildEvent::Progress(progress) => run.progress = progress,
                    BuildEvent::Complete(result) => complete = Some(result),
                }
            }
        }
        if disconnected && complete.is_none() {
            complete = Some(Err("Index worker stopped before completion".to_string()));
        }
        if let (Some(root), Some(result)) = (root, complete) {
            match result {
                Ok(index) => {
                    self.cache_snapshot(root.clone(), Arc::new(index));
                    self.unloadable.remove(&root);
                    self.load_errors.remove(&root);
                    self.set_last_error(&root, None);
                }
                Err(error) => {
                    self.load_errors.insert(root.clone(), error.clone());
                    self.set_last_error(&root, Some(error));
                }
            }
            self.active = None;
        }
    }

    pub fn snapshot(&mut self, root: &Path) -> Option<Arc<RootIndex>> {
        self.snapshot_from(&index_path(root), root)
    }

    fn snapshot_from(&mut self, path: &Path, root: &Path) -> Option<Arc<RootIndex>> {
        if let Some(snapshot) = self.snapshots.get(root).map(Arc::clone) {
            self.touch_snapshot(root);
            return Some(snapshot);
        }
        if self.unloadable.contains(root) {
            return None;
        }
        match load_index_from(path, root) {
            Ok(Some(index)) => {
                let index = Arc::new(index);
                self.cache_snapshot(root.to_path_buf(), Arc::clone(&index));
                self.load_errors.remove(root);
                Some(index)
            }
            Ok(None) => None,
            Err(error) => {
                self.unloadable.insert(root.to_path_buf());
                self.load_errors.insert(root.to_path_buf(), error);
                None
            }
        }
    }

    /// Keep at most `MAX_CACHED_SNAPSHOTS` indexes (each up to
    /// `MAX_CONTENT_BYTES` of text) alive, dropping the least recently used.
    fn cache_snapshot(&mut self, root: PathBuf, index: Arc<RootIndex>) {
        self.touch_snapshot(&root);
        if self.snapshots.insert(root.clone(), index).is_none() {
            self.snapshot_order.push(root);
        }
        while self.snapshot_order.len() > MAX_CACHED_SNAPSHOTS {
            let evicted = self.snapshot_order.remove(0);
            self.snapshots.remove(&evicted);
        }
    }

    fn touch_snapshot(&mut self, root: &Path) {
        if self.snapshot_order.last().is_some_and(|last| last == root) {
            return;
        }
        if let Some(position) = self.snapshot_order.iter().position(|cached| cached == root) {
            let cached = self.snapshot_order.remove(position);
            self.snapshot_order.push(cached);
        }
    }

    /// Drop a disabled root's cached index, its load error and its index
    /// file, which holds up to `MAX_CONTENT_BYTES` of the folder's text.
    /// Enabling always rebuilds, so nothing reusable is lost. Best effort: a
    /// file that cannot be removed is only logged.
    fn forget_root_index(&mut self, root: &Path, index_file: &Path) {
        self.evict_snapshot(root);
        self.load_errors.remove(root);
        if let Err(error) = fs::remove_file(index_file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                target: "commander::content_index",
                "Could not remove disabled index {}: {error}",
                index_file.display()
            );
        }
    }

    fn evict_snapshot(&mut self, root: &Path) {
        self.snapshots.remove(root);
        self.snapshot_order.retain(|cached| cached != root);
    }

    pub fn status(&mut self, root: &Path) -> IndexStatus {
        let runtime_enabled =
            crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ContentIndex);
        let enabled = self.is_enabled(root) && runtime_enabled;
        let excluded_roots = self.exclusions(root);
        let settings_error = self
            .settings
            .roots
            .iter()
            .find(|settings| settings.root == root)
            .and_then(|settings| settings.last_error.clone());
        let snapshot = enabled.then(|| self.snapshot(root)).flatten();
        let active = self.active.as_ref().filter(|run| run.root == root);
        let last_error = self
            .load_errors
            .get(root)
            .cloned()
            .or(settings_error)
            .or_else(|| self.settings_error.clone());
        let phase = if !enabled {
            IndexPhase::Disabled
        } else if active.is_some() && !self.idle.load(Ordering::Acquire) {
            IndexPhase::WaitingForIdle
        } else if active.is_some() {
            IndexPhase::Building
        } else if snapshot.is_some() {
            IndexPhase::Ready
        } else if last_error.is_some() {
            IndexPhase::Error
        } else {
            IndexPhase::Missing
        };
        let progress = active
            .map(|run| run.progress.clone())
            .or_else(|| {
                snapshot.as_ref().map(|index| BuildProgress {
                    scanned: index.scanned,
                    indexed: index.documents.len(),
                    files_seen: index.files_seen,
                    directories_seen: index.directories_seen,
                    content_indexed: index.content_indexed,
                    content_bytes: index.content_bytes,
                })
            })
            .unwrap_or_default();
        IndexStatus {
            enabled,
            phase,
            progress,
            built_at_secs: snapshot.as_ref().map(|index| index.built_at_secs),
            excluded_roots,
            skipped_binary: snapshot.as_ref().map_or(0, |index| index.skipped_binary),
            skipped_large: snapshot.as_ref().map_or(0, |index| index.skipped_large),
            skipped_unreadable: snapshot
                .as_ref()
                .map_or(0, |index| index.skipped_unreadable),
            skipped_quota: snapshot.as_ref().map_or(0, |index| index.skipped_quota),
            truncated: snapshot.as_ref().is_some_and(|index| index.truncated),
            last_error,
        }
    }

    fn settings_for_mut(&mut self, root: PathBuf) -> &mut RootSettings {
        if let Some(index) = self
            .settings
            .roots
            .iter()
            .position(|settings| settings.root == root)
        {
            return &mut self.settings.roots[index];
        }
        self.settings.roots.push(RootSettings {
            root,
            enabled: false,
            excluded_roots: Vec::new(),
            last_error: None,
        });
        self.settings.roots.last_mut().expect("just inserted")
    }

    fn set_last_error(&mut self, root: &Path, error: Option<String>) {
        self.settings_for_mut(root.to_path_buf()).last_error = error;
        if !save_settings_to(&settings_path(), &self.settings) {
            log::warn!(
                target: "commander::content_index",
                "failed to persist content-index settings for {}",
                root.display()
            );
        }
    }
}

impl Drop for ContentIndex {
    fn drop(&mut self) {
        drop(self.active.take());
    }
}

fn scan_index(
    root: PathBuf,
    excluded_roots: Vec<PathBuf>,
    idle: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    scheduler_cancel: Option<crate::workload::CancellationToken>,
    report: impl FnMut(&BuildProgress) -> bool,
) -> Result<RootIndex, String> {
    scan_index_within(
        root,
        excluded_roots,
        idle,
        cancelled,
        scheduler_cancel,
        ScanBudget {
            json_bytes: MAX_DOCUMENT_JSON_BYTES,
            content_bytes: MAX_CONTENT_BYTES,
        },
        report,
    )
}

/// Size caps for one build; tests shrink them to reach the limits cheaply.
#[derive(Clone, Copy)]
struct ScanBudget {
    /// Serialized size of all documents (see `MAX_DOCUMENT_JSON_BYTES`).
    json_bytes: usize,
    /// Decoded text kept across all documents (see `MAX_CONTENT_BYTES`).
    content_bytes: usize,
}

fn scan_index_within(
    root: PathBuf,
    excluded_roots: Vec<PathBuf>,
    idle: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    scheduler_cancel: Option<crate::workload::CancellationToken>,
    budget: ScanBudget,
    mut report: impl FnMut(&BuildProgress) -> bool,
) -> Result<RootIndex, String> {
    wait_for_idle(&idle, &cancelled, scheduler_cancel.as_ref())?;
    let root_entries = fs::read_dir(&root)
        .map_err(|error| format!("Could not read index root {}: {error}", root.display()))?;
    let mut stack = vec![DirCursor::Open(Box::new(root_entries))];
    let mut documents = Vec::new();
    let mut progress = BuildProgress::default();
    let mut skipped_binary = 0usize;
    let mut skipped_large = 0usize;
    let mut skipped_unreadable = 0usize;
    let mut skipped_quota = 0usize;
    let mut truncated = false;
    let mut json_bytes = 0usize;
    let mut last_progress = Instant::now();

    while let Some(entries) = stack.last_mut() {
        wait_for_idle(&idle, &cancelled, scheduler_cancel.as_ref())?;
        if !crate::io_budget::background_checkpoint(|| {
            build_cancelled(&cancelled, scheduler_cancel.as_ref())
        }) {
            return Err("Index build cancelled".to_string());
        }
        let Some(next) = entries.next_entry() else {
            stack.pop();
            continue;
        };
        let Ok(dir_entry) = next else {
            skipped_unreadable = skipped_unreadable.saturating_add(1);
            continue;
        };
        let path = dir_entry.path();
        progress.scanned = progress.scanned.saturating_add(1);
        if excluded_roots
            .iter()
            .any(|excluded| path.starts_with(excluded))
        {
            continue;
        }
        let Ok(file_type) = dir_entry.file_type() else {
            skipped_unreadable = skipped_unreadable.saturating_add(1);
            continue;
        };
        let Ok(metadata) = dir_entry.metadata(file_type) else {
            skipped_unreadable = skipped_unreadable.saturating_add(1);
            continue;
        };
        if documents.len() >= MAX_DOCUMENTS {
            truncated = true;
            break;
        }

        let is_dir = file_type.is_dir();
        if is_dir {
            progress.directories_seen = progress.directories_seen.saturating_add(1);
        } else {
            progress.files_seen = progress.files_seen.saturating_add(1);
        }
        let mut content = None;
        // Only regular files are read: opening a FIFO blocks the build thread
        // forever, and devices or sockets have no indexable text.
        if file_type.is_file() {
            if metadata.len() > MAX_FILE_BYTES {
                skipped_large = skipped_large.saturating_add(1);
            } else if progress
                .content_bytes
                .saturating_add(usize::try_from(metadata.len()).unwrap_or(usize::MAX))
                > budget.content_bytes
            {
                skipped_quota = skipped_quota.saturating_add(1);
            } else {
                match read_indexable_text(&path, metadata.len()) {
                    // Lossy decoding turns each invalid byte into a 3-byte
                    // U+FFFD, so the text can outgrow the on-disk size that
                    // passed the check above.
                    Ok(Some(text))
                        if progress.content_bytes.saturating_add(text.len())
                            > budget.content_bytes =>
                    {
                        skipped_quota = skipped_quota.saturating_add(1);
                    }
                    Ok(Some(text)) => {
                        progress.content_bytes = progress.content_bytes.saturating_add(text.len());
                        progress.content_indexed = progress.content_indexed.saturating_add(1);
                        content = Some(text);
                    }
                    Ok(None) => skipped_binary = skipped_binary.saturating_add(1),
                    Err(_) => skipped_unreadable = skipped_unreadable.saturating_add(1),
                }
            }
        }
        let document_bytes =
            DOCUMENT_JSON_OVERHEAD.saturating_add(json_string_len(&path.to_string_lossy()));
        if json_bytes.saturating_add(document_bytes) > budget.json_bytes {
            truncated = true;
            break;
        }
        let content_json_bytes = content.as_deref().map_or(0, json_string_len);
        if json_bytes
            .saturating_add(document_bytes)
            .saturating_add(content_json_bytes)
            > budget.json_bytes
            && let Some(text) = content.take()
        {
            // Keep the document's metadata; its text no longer fits the file.
            progress.content_bytes = progress.content_bytes.saturating_sub(text.len());
            progress.content_indexed = progress.content_indexed.saturating_sub(1);
            skipped_quota = skipped_quota.saturating_add(1);
        }
        json_bytes = json_bytes.saturating_add(document_bytes);
        if content.is_some() {
            json_bytes = json_bytes.saturating_add(content_json_bytes);
        }
        let (volume, file_id) = native_identity(&metadata);
        documents.push(IndexedDocument {
            path: path.clone(),
            is_dir,
            size: metadata.len(),
            modified_secs: system_time_secs(metadata.modified().ok()),
            accessed_secs: system_time_secs(metadata.accessed().ok()),
            volume,
            file_id,
            content,
        });
        progress.indexed = documents.len();

        if is_dir {
            match DirCursor::open(&path, stack.len()) {
                Ok(entries) => stack.push(entries),
                Err(_) => skipped_unreadable = skipped_unreadable.saturating_add(1),
            }
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            if !report(&progress) {
                return Err("Index receiver closed".to_string());
            }
            last_progress = Instant::now();
        }
    }

    report(&progress);
    Ok(RootIndex {
        schema: SCHEMA_VERSION,
        root,
        built_at_secs: system_time_secs(Some(SystemTime::now())).unwrap_or(0),
        documents,
        scanned: progress.scanned,
        files_seen: progress.files_seen,
        directories_seen: progress.directories_seen,
        content_indexed: progress.content_indexed,
        content_bytes: progress.content_bytes,
        skipped_binary,
        skipped_large,
        skipped_unreadable,
        skipped_quota,
        excluded_roots,
        truncated,
    })
}

/// Open directory levels the scan keeps as live `ReadDir` handles. Deeper
/// levels are read fully and closed before descending, so a pathologically
/// deep tree cannot exhaust file descriptors (one per open level) and turn
/// every directory below into "unreadable".
const MAX_OPEN_DIRS: usize = 64;

/// One directory level of the scan stack.
enum DirCursor {
    // Boxed: Windows' `ReadDir` embeds a whole `WIN32_FIND_DATAW`.
    Open(Box<ReadDir>),
    Buffered(std::vec::IntoIter<std::io::Result<CursorEntry>>),
}

impl DirCursor {
    fn open(path: &Path, depth: usize) -> std::io::Result<Self> {
        let entries = fs::read_dir(path)?;
        if depth < MAX_OPEN_DIRS {
            return Ok(Self::Open(Box::new(entries)));
        }
        // `DirEntry` keeps its directory handle alive, so only owned
        // values are buffered.
        let buffered: Vec<_> = entries
            .map(|entry| {
                entry.map(|entry| CursorEntry::Buffered(entry.path(), entry.file_type().ok()))
            })
            .collect();
        Ok(Self::Buffered(buffered.into_iter()))
    }

    fn next_entry(&mut self) -> Option<std::io::Result<CursorEntry>> {
        match self {
            Self::Open(entries) => entries.next().map(|entry| entry.map(CursorEntry::Open)),
            Self::Buffered(entries) => entries.next(),
        }
    }
}

// Not boxed: one entry is live at a time, and boxing the large Windows
// `DirEntry` would cost an allocation per scanned entry.
#[allow(clippy::large_enum_variant)]
enum CursorEntry {
    Open(fs::DirEntry),
    Buffered(PathBuf, Option<fs::FileType>),
}

impl CursorEntry {
    fn path(&self) -> PathBuf {
        match self {
            Self::Open(entry) => entry.path(),
            Self::Buffered(path, _) => path.clone(),
        }
    }

    fn file_type(&self) -> std::io::Result<fs::FileType> {
        match self {
            Self::Open(entry) => entry.file_type(),
            Self::Buffered(_, file_type) => {
                file_type.ok_or_else(|| std::io::Error::other("file type unavailable"))
            }
        }
    }

    /// Metadata of the entry itself, never of a symlink's target.
    fn metadata(&self, file_type: fs::FileType) -> std::io::Result<fs::Metadata> {
        match self {
            Self::Open(entry) if !file_type.is_symlink() => entry.metadata(),
            _ => fs::symlink_metadata(self.path()),
        }
    }
}

#[cfg(test)]
pub fn build_test_index(root: &Path) -> RootIndex {
    scan_index(
        root.to_path_buf(),
        Vec::new(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
        None,
        |_| true,
    )
    .expect("test content index should build")
}

/// Save a finished build's index unless the build was cancelled after its
/// scan: a root disabled meanwhile must not get back the file the disable
/// deleted. The check and the write happen under `gate`, which the disable
/// path takes before it removes the file.
fn save_unless_cancelled(
    path: &Path,
    index: &RootIndex,
    gate: &Mutex<()>,
    cancelled: &AtomicBool,
    scheduler_cancel: Option<&crate::workload::CancellationToken>,
) -> Result<(), String> {
    let _gate = gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if build_cancelled(cancelled, scheduler_cancel) {
        return Err("Index build cancelled".to_string());
    }
    save_index_to(path, index)
}

fn build_cancelled(
    cancelled: &AtomicBool,
    scheduler_cancel: Option<&crate::workload::CancellationToken>,
) -> bool {
    cancelled.load(Ordering::Acquire)
        || scheduler_cancel.is_some_and(crate::workload::CancellationToken::is_cancelled)
}

fn wait_for_idle(
    idle: &AtomicBool,
    cancelled: &AtomicBool,
    scheduler_cancel: Option<&crate::workload::CancellationToken>,
) -> Result<(), String> {
    while !idle.load(Ordering::Acquire) {
        if build_cancelled(cancelled, scheduler_cancel) {
            return Err("Index build cancelled".to_string());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    if build_cancelled(cancelled, scheduler_cancel) {
        Err("Index build cancelled".to_string())
    } else {
        Ok(())
    }
}

/// Leading bytes checked for NULs to tell binary files from text.
const BINARY_SNIFF_BYTES: u64 = 8_192;

fn read_indexable_text(path: &Path, size: u64) -> std::io::Result<Option<String>> {
    read_indexable_from(fs::File::open(path)?, size)
}

/// Sniff the head before reading the rest, so a binary file costs one
/// small read instead of up to `MAX_FILE_BYTES`.
fn read_indexable_from(reader: impl Read, size: u64) -> std::io::Result<Option<String>> {
    let mut reader = reader.take(MAX_FILE_BYTES);
    let mut bytes = Vec::with_capacity(usize::try_from(size.min(BINARY_SNIFF_BYTES)).unwrap_or(0));
    Read::by_ref(&mut reader)
        .take(BINARY_SNIFF_BYTES)
        .read_to_end(&mut bytes)?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    bytes.reserve(
        usize::try_from(size.min(MAX_FILE_BYTES))
            .unwrap_or(0)
            .saturating_sub(bytes.len()),
    );
    reader.read_to_end(&mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Upper bound on a string's length once JSON-escaped and quoted.
fn json_string_len(value: &str) -> usize {
    value
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' => 2,
            0x00..=0x1f => 6,
            _ => 1,
        })
        .sum::<usize>()
        .saturating_add(2)
}

fn system_time_secs(time: Option<SystemTime>) -> Option<u64> {
    time?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

// The body is cfg-dependent: const on non-unix, but `MetadataExt` is not const.
#[allow(clippy::missing_const_for_fn)]
fn native_identity(metadata: &fs::Metadata) -> (Option<u64>, Option<u64>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return (Some(metadata.dev()), Some(metadata.ino()));
    }
    #[allow(unreachable_code)]
    (None, None)
}

fn normalize_exclusions(root: &Path, values: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for value in values {
        if value
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(format!(
                "Excluded path cannot contain '..': {}",
                value.display()
            ));
        }
        let path = if value.is_absolute() {
            value
        } else {
            root.join(value)
        };
        if path == root || !path.starts_with(root) {
            return Err(format!(
                "Excluded path must be below the index root: {}",
                path.display()
            ));
        }
        let path = on_disk_case(root, path);
        if seen.insert(path.clone()) {
            normalized.push(path);
        }
    }
    normalized.sort();
    Ok(normalized)
}

/// Rebuild each existing component of an exclusion from its on-disk name.
/// Excluded subtrees are matched byte-exactly against scanned paths, so on a
/// case-insensitive volume a typed `Target` must become the real `target`.
/// Components that do not exist are kept as typed.
fn on_disk_case(root: &Path, path: PathBuf) -> PathBuf {
    let Ok(relative) = path.strip_prefix(root) else {
        return path;
    };
    let mut resolved = root.to_path_buf();
    let mut exists = true;
    for component in relative.components() {
        let typed = component.as_os_str();
        exists = exists && fs::symlink_metadata(resolved.join(typed)).is_ok();
        let name = exists
            .then(|| fs::read_dir(&resolved).ok())
            .flatten()
            .and_then(|entries| {
                matching_entry_name(
                    typed,
                    entries.filter_map(|entry| entry.ok().map(|entry| entry.file_name())),
                )
            })
            .unwrap_or_else(|| typed.to_os_string());
        resolved.push(name);
    }
    resolved
}

/// The directory entry a typed name refers to: an exact match first, else
/// the first entry equal ignoring case.
fn matching_entry_name(
    typed: &std::ffi::OsStr,
    names: impl Iterator<Item = std::ffi::OsString>,
) -> Option<std::ffi::OsString> {
    let typed_lower = typed.to_string_lossy().to_lowercase();
    let mut folded = None;
    for name in names {
        if name == typed {
            return Some(name);
        }
        if folded.is_none() && name.to_string_lossy().to_lowercase() == typed_lower {
            folded = Some(name);
        }
    }
    folded
}

fn settings_path() -> PathBuf {
    crate::fs_util::config_dir().join("content_index_settings.json")
}

fn indexes_dir() -> PathBuf {
    crate::fs_util::config_dir().join("indexes")
}

fn index_path(root: &Path) -> PathBuf {
    let mut hash = 0xcbf29ce484222325u64;
    // Raw bytes, not a lossy string, so roots differing only in non-UTF-8
    // bytes get distinct files. UTF-8 roots hash exactly as before.
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    indexes_dir().join(format!("content-v{SCHEMA_VERSION}-{hash:016x}.json"))
}

/// Settings as loaded, plus a problem to show in the index panel.
struct LoadedSettings {
    settings: SettingsStore,
    problem: Option<String>,
    /// The file exists but could not be read; saves must not replace it.
    unreadable: bool,
}

impl LoadedSettings {
    fn ok(settings: SettingsStore) -> Self {
        Self {
            settings,
            problem: None,
            unreadable: false,
        }
    }
}

/// The saved settings, plus a problem to show in the index panel when they
/// could not be loaded. Every save writes the whole store, so a file that
/// does not parse is moved aside first: otherwise the next settings change
/// would replace the user's roots and exclusions with defaults. A file that
/// cannot be read is left in place and marked so saves are refused.
fn load_settings_from(path: &Path) -> LoadedSettings {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LoadedSettings::ok(SettingsStore::default());
        }
        Err(error) => {
            let message = format!(
                "Could not read index settings {}: {error}. Defaults are active and changes are not saved until the file is readable.",
                path.display()
            );
            log::warn!(target: "commander::content_index", "{message}");
            return LoadedSettings {
                settings: SettingsStore::default(),
                problem: Some(message),
                unreadable: true,
            };
        }
    };
    let error = match serde_json::from_reader(BufReader::new(file)) {
        Ok(settings) => return LoadedSettings::ok(settings),
        Err(error) => error,
    };
    let backup = unused_backup_path(path);
    let message = match fs::rename(path, &backup) {
        Ok(()) => format!(
            "Index settings {} were damaged ({error}); defaults are active and the old file was kept as {}.",
            path.display(),
            backup.display()
        ),
        Err(rename_error) => format!(
            "Index settings {} were damaged ({error}) and could not be moved aside ({rename_error}); the next settings change will replace them.",
            path.display()
        ),
    };
    log::warn!(target: "commander::content_index", "{message}");
    LoadedSettings {
        settings: SettingsStore::default(),
        problem: Some(message),
        unreadable: false,
    }
}

/// Why a settings save to `path` must be refused: it was unreadable at load
/// and is still there, so saving would replace it with the defaults.
fn settings_save_blocked_reason(unreadable: bool, path: &Path) -> Option<String> {
    if !unreadable || fs::symlink_metadata(path).is_err() {
        return None;
    }
    Some(format!(
        "Index settings {} could not be read, so changes are not saved over it. Fix its permissions and restart Commander.",
        path.display()
    ))
}

/// `<path>.bak`, or `<path>.bak.N` when earlier backups exist, so a second
/// damaged file never replaces the first one's backup.
fn unused_backup_path(path: &Path) -> PathBuf {
    let with_suffix = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let first = with_suffix(".bak");
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    (1..1000)
        .map(|n| with_suffix(&format!(".bak.{n}")))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .unwrap_or_else(|| with_suffix(&format!(".bak.{}", std::process::id())))
}

fn save_settings_to(path: &Path, settings: &SettingsStore) -> bool {
    serde_json::to_string_pretty(settings)
        .ok()
        .is_some_and(|json| crate::fs_util::write_atomic(path, &json))
}

const INDEX_STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.content_index", 1, MAX_INDEX_FILE_BYTES)
        .allow_legacy_schema_marker();

fn load_index_from(path: &Path, expected_root: &Path) -> Result<Option<RootIndex>, String> {
    let persist = crate::persistence::FsPersist::default();
    let (index, _) =
        crate::persistence::load_strict::<RootIndex>(&persist, path, INDEX_STORE, INDEX_NOUN)?;
    match index {
        Some(index) if index.schema != SCHEMA_VERSION || index.root != expected_root => {
            Err("Content index schema or root does not match".to_string())
        }
        index => Ok(index),
    }
}

/// Sentence-case name used in this store's error messages.
const INDEX_NOUN: &str = "Content index";

fn save_index_to(path: &Path, index: &RootIndex) -> Result<(), String> {
    let persist = crate::persistence::FsPersist::default();
    // Only the gate (revision and status) is needed: skip decoding the old
    // payload into a `RootIndex`, which can be tens of MB of documents.
    let mut gate =
        crate::persistence::load_enveloped::<serde::de::IgnoredAny>(&persist, path, INDEX_STORE)
            .gate;
    if !matches!(
        gate.status(),
        crate::persistence::LoadStatus::Missing
            | crate::persistence::LoadStatus::Legacy
            | crate::persistence::LoadStatus::Current
    ) {
        // The index is a derived cache: a file this build cannot read must
        // not block every rebuild forever, so drop it and write a fresh one.
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Could not replace unreadable content index {}: {error}",
                    path.display()
                ));
            }
        }
        gate = crate::persistence::load_enveloped::<serde::de::IgnoredAny>(
            &persist,
            path,
            INDEX_STORE,
        )
        .gate;
    }
    match crate::persistence::save_enveloped_streaming(
        &persist,
        path,
        INDEX_STORE,
        index,
        &mut gate,
        crate::persistence::SaveIntent::Automatic,
    ) {
        Ok(outcome) => crate::persistence::require_durable(INDEX_NOUN, outcome),
        Err(error) => Err(describe_index_save_error(path, &error)),
    }
}

/// A sentence for the Find index panel; the error's `Debug` form
/// (`Blocked(Corrupt)`, `PreCommit(Io { .. })`) means nothing to a user.
fn describe_index_save_error(path: &Path, error: &crate::persistence::JsonSaveError) -> String {
    use crate::persistence::{JsonSaveError, PreCommitError};
    let reason = match error {
        JsonSaveError::Blocked(status) => format!(
            "the existing index file is {}",
            format!("{status:?}").to_lowercase()
        ),
        JsonSaveError::GenerationExhausted => "its revision counter is exhausted".to_string(),
        JsonSaveError::RecoveryPreservation => {
            "the previous index file could not be preserved".to_string()
        }
        JsonSaveError::PreCommit(PreCommitError::Conflict) => {
            "another writer changed it during the build".to_string()
        }
        JsonSaveError::PreCommit(PreCommitError::Serialize(source)) => {
            format!("it could not be serialized ({source})")
        }
        JsonSaveError::PreCommit(PreCommitError::Io { stage, source, .. }) => {
            format!("could not {stage} ({source})")
        }
    };
    format!(
        "Could not save content index {}: {reason}. Rebuild the index to try again.",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn scan(root: &Path, exclusions: Vec<PathBuf>) -> RootIndex {
        scan_index(
            root.to_path_buf(),
            exclusions,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
            None,
            |_| true,
        )
        .unwrap()
    }

    #[test]
    fn scan_indexes_text_metadata_and_honors_excluded_subtrees() {
        let temp = TempDir::new();
        temp.file("notes/readme.txt", "searchable words");
        temp.file("binary.dat", "a\0b");
        let excluded = temp.dir("vendor");
        temp.file("vendor/secret.txt", "do not index");

        let index = scan(temp.path(), vec![excluded.clone()]);

        assert!(index.documents.iter().any(|document| {
            document.path.ends_with("readme.txt")
                && document.content.as_deref() == Some("searchable words")
        }));
        assert!(index.documents.iter().any(|document| {
            document.path.ends_with("binary.dat") && document.content.is_none()
        }));
        assert!(
            !index
                .documents
                .iter()
                .any(|document| document.path.starts_with(&excluded))
        );
        assert_eq!(index.skipped_binary, 1);
    }

    #[test]
    fn worker_waits_for_idle_and_observes_cancellation() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let idle = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_idle = Arc::clone(&idle);
        let worker_cancelled = Arc::clone(&cancelled);
        let root = temp.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            scan_index(
                root,
                Vec::new(),
                worker_idle,
                worker_cancelled,
                None,
                |_| true,
            )
        });

        std::thread::sleep(Duration::from_millis(80));
        assert!(!handle.is_finished());
        cancelled.store(true, Ordering::Release);
        assert!(handle.join().unwrap().unwrap_err().contains("cancelled"));
    }

    #[test]
    fn index_round_trips_through_streaming_atomic_store() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let index = scan(temp.path(), Vec::new());
        let path = temp.path().join("index.json");

        save_index_to(&path, &index).unwrap();
        let loaded = load_index_from(&path, temp.path()).unwrap().unwrap();

        assert_eq!(loaded.root, index.root);
        assert_eq!(loaded.documents.len(), index.documents.len());
        assert_eq!(loaded.content_indexed, 1);
    }

    #[test]
    fn exclusions_are_root_scoped_deduplicated_and_reject_parent_escape() {
        let root = PathBuf::from("/workspace");
        let values = vec![PathBuf::from("target"), PathBuf::from("target")];
        assert_eq!(
            normalize_exclusions(&root, values).unwrap(),
            vec![PathBuf::from("/workspace/target")]
        );
        assert!(normalize_exclusions(&root, vec![PathBuf::from("../outside")]).is_err());
        assert!(normalize_exclusions(&root, vec![root.clone()]).is_err());
    }

    #[test]
    fn coverage_is_content_documents_over_files_seen() {
        let status = IndexStatus {
            enabled: true,
            phase: IndexPhase::Ready,
            progress: BuildProgress {
                files_seen: 8,
                content_indexed: 6,
                ..Default::default()
            },
            built_at_secs: None,
            excluded_roots: Vec::new(),
            skipped_binary: 0,
            skipped_large: 0,
            skipped_unreadable: 0,
            skipped_quota: 0,
            truncated: false,
            last_error: None,
        };
        assert_eq!(status.coverage_percent(), 75.0);
    }

    #[test]
    fn injected_workload_owns_and_cancels_the_active_build() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let root = temp.path().to_path_buf();
        let runtime = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        let settings = SettingsStore {
            roots: vec![RootSettings {
                root: root.clone(),
                enabled: true,
                excluded_roots: Vec::new(),
                last_error: None,
            }],
        };
        let mut index = ContentIndex::from_settings(settings, runtime.handle());
        index.set_idle(true);

        index
            .enqueue_build(root, Vec::new(), Arc::new(|| {}))
            .unwrap();
        assert_eq!(runtime.stats().queued, 1);

        drop(index);

        let stats = runtime.stats();
        assert_eq!(stats.queued, 0);
        assert_eq!(stats.cancelled, 1);
        assert!(!runtime.run_next());
    }

    #[test]
    fn rebuild_replaces_a_corrupt_or_oversized_index_file() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let index = scan(temp.path(), Vec::new());
        let path = temp.path().join("index.json");

        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load_index_from(&path, temp.path()).is_err());
        save_index_to(&path, &index).unwrap();
        assert!(load_index_from(&path, temp.path()).unwrap().is_some());

        let oversized = std::fs::File::create(&path).unwrap();
        oversized.set_len(MAX_INDEX_FILE_BYTES as u64 + 1).unwrap();
        drop(oversized);
        assert!(load_index_from(&path, temp.path()).is_err());
        save_index_to(&path, &index).unwrap();
        assert!(load_index_from(&path, temp.path()).unwrap().is_some());
    }

    #[test]
    fn build_truncates_before_the_serialized_index_exceeds_its_budget() {
        let temp = TempDir::new();
        for number in 0..40 {
            temp.file(&format!("file-{number:02}.txt"), &"\"quoted\" ".repeat(20));
        }
        let budget = 8 * 1024;
        let index = scan_index_within(
            temp.path().to_path_buf(),
            Vec::new(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
            None,
            ScanBudget {
                json_bytes: budget,
                content_bytes: MAX_CONTENT_BYTES,
            },
            |_| true,
        )
        .unwrap();

        assert!(index.truncated);
        assert!(!index.documents.is_empty());
        let documents_json = serde_json::to_vec_pretty(&index.documents).unwrap();
        assert!(documents_json.len() <= budget);
    }

    #[test]
    fn content_quota_counts_decoded_text_not_file_size() {
        let temp = TempDir::new();
        // 10 invalid bytes decode to 10 three-byte U+FFFD characters.
        std::fs::write(temp.path().join("latin1.txt"), [0xffu8; 10]).unwrap();
        let index = scan_index_within(
            temp.path().to_path_buf(),
            Vec::new(),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
            None,
            ScanBudget {
                json_bytes: MAX_DOCUMENT_JSON_BYTES,
                content_bytes: 20,
            },
            |_| true,
        )
        .unwrap();

        assert_eq!(index.skipped_quota, 1);
        assert_eq!(index.content_indexed, 0);
        assert_eq!(index.content_bytes, 0);
        assert!(index.documents[0].content.is_none());
    }

    #[test]
    fn scan_descends_below_the_open_directory_limit() {
        // Covers correctness of the buffered (handle-closed) levels below
        // MAX_OPEN_DIRS only. The tree is too shallow to exhaust any fd
        // limit, and fd counts are not stable while other tests run in
        // parallel threads, so fd release itself is not asserted here.
        let temp = TempDir::new();
        let mut deep = PathBuf::new();
        for level in 0..MAX_OPEN_DIRS + 6 {
            deep.push(format!("d{level}"));
        }
        temp.file(&deep.join("leaf.txt").to_string_lossy(), "deep words");
        temp.file(
            &deep.parent().unwrap().join("side.txt").to_string_lossy(),
            "side",
        );

        let index = scan(temp.path(), Vec::new());

        assert_eq!(index.skipped_unreadable, 0);
        assert_eq!(index.directories_seen, MAX_OPEN_DIRS + 6);
        assert_eq!(index.content_indexed, 2);
        assert!(index.documents.iter().any(|document| {
            document.path == temp.path().join(&deep).join("leaf.txt")
                && document.content.as_deref() == Some("deep words")
        }));
    }

    #[test]
    fn corrupt_index_times_do_not_panic() {
        let document = IndexedDocument {
            path: PathBuf::from("/data/file.txt"),
            is_dir: false,
            size: 1,
            modified_secs: Some(u64::MAX),
            accessed_secs: Some(u64::MAX),
            volume: None,
            file_id: None,
            content: None,
        };
        let entry = document.entry().unwrap();
        assert_eq!(entry.modified, None);
        assert_eq!(entry.modified_str, "-");
        assert_eq!(document.accessed(), None);

        // Past chrono's ~year 262k limit (still a valid SystemTime on Unix).
        let far = IndexedDocument {
            modified_secs: Some(1 << 43),
            ..document.clone()
        };
        assert_eq!(far.entry().unwrap().modified_str, "-");

        let normal = IndexedDocument {
            modified_secs: Some(0),
            accessed_secs: Some(60),
            ..document
        };
        assert_eq!(
            normal.entry().unwrap().modified,
            Some(std::time::UNIX_EPOCH)
        );
        assert_eq!(
            normal.accessed(),
            Some(std::time::UNIX_EPOCH + Duration::from_secs(60))
        );
    }

    fn test_content_index() -> ContentIndex {
        let runtime = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        ContentIndex::from_settings(SettingsStore::default(), runtime.handle())
    }

    #[test]
    fn failed_index_load_is_not_retried_every_frame() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let path = temp.path().join("index.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let mut index = test_content_index();

        assert!(index.snapshot_from(&path, temp.path()).is_none());
        assert!(index.load_errors.contains_key(temp.path()));
        // A now-valid file is not re-read until a build or settings change.
        save_index_to(&path, &scan(temp.path(), Vec::new())).unwrap();
        assert!(index.snapshot_from(&path, temp.path()).is_none());

        index.unloadable.clear();
        assert!(index.snapshot_from(&path, temp.path()).is_some());
    }

    #[test]
    fn disabling_a_root_forgets_its_index_file_and_errors() {
        let temp = TempDir::new();
        temp.file("file.txt", "private text");
        let path = temp.path().join("index.json");
        save_index_to(&path, &scan(temp.path(), Vec::new())).unwrap();
        let mut index = test_content_index();
        assert!(index.snapshot_from(&path, temp.path()).is_some());
        index
            .load_errors
            .insert(temp.path().to_path_buf(), "old".to_string());

        index.forget_root_index(temp.path(), &path);

        assert!(!path.exists(), "disabled index kept its file contents");
        assert!(!index.snapshots.contains_key(temp.path()));
        assert!(!index.load_errors.contains_key(temp.path()));
        // Already gone: forgetting again is a quiet no-op.
        index.forget_root_index(temp.path(), &path);
    }

    #[test]
    fn a_build_cancelled_after_its_scan_does_not_write_the_index() {
        let temp = TempDir::new();
        let path = temp.path().join("index.bin");
        let index = scan(temp.path(), Vec::new());
        let gate = Mutex::new(());
        let cancelled = AtomicBool::new(true);

        let result = save_unless_cancelled(&path, &index, &gate, &cancelled, None);

        assert!(result.is_err());
        assert!(!path.exists(), "a disabled root's index was written back");
        cancelled.store(false, Ordering::Release);
        save_unless_cancelled(&path, &index, &gate, &cancelled, None).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn snapshot_cache_evicts_the_least_recently_used_root() {
        let temp = TempDir::new();
        let built = Arc::new(scan(temp.path(), Vec::new()));
        let mut index = test_content_index();
        let roots: Vec<PathBuf> = (0..=MAX_CACHED_SNAPSHOTS)
            .map(|number| PathBuf::from(format!("/root-{number}")))
            .collect();
        for root in &roots[..MAX_CACHED_SNAPSHOTS] {
            index.cache_snapshot(root.clone(), Arc::clone(&built));
        }
        // Reading the oldest root makes the second-oldest the eviction pick.
        let missing = temp.path().join("missing.json");
        assert!(index.snapshot_from(&missing, &roots[0]).is_some());

        index.cache_snapshot(roots[MAX_CACHED_SNAPSHOTS].clone(), built);

        assert_eq!(index.snapshots.len(), MAX_CACHED_SNAPSHOTS);
        assert!(index.snapshots.contains_key(&roots[0]));
        assert!(!index.snapshots.contains_key(&roots[1]));
        index.evict_snapshot(&roots[0]);
        assert!(!index.snapshots.contains_key(&roots[0]));
        assert_eq!(index.snapshot_order.len(), MAX_CACHED_SNAPSHOTS - 1);
    }

    #[test]
    fn binary_files_are_rejected_after_reading_only_the_head() {
        struct Counting<'a>(&'a [u8], &'a std::cell::Cell<usize>);
        impl Read for Counting<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let read = self.0.read(buf)?;
                self.1.set(self.1.get() + read);
                Ok(read)
            }
        }
        let read = std::cell::Cell::new(0);
        let mut binary = vec![b'a'; MAX_FILE_BYTES as usize];
        binary[10] = 0;
        let size = binary.len() as u64;
        assert!(
            read_indexable_from(Counting(&binary, &read), size)
                .unwrap()
                .is_none()
        );
        assert!(read.get() as u64 <= BINARY_SNIFF_BYTES);

        // A NUL past the sniffed head is still text, read in full.
        let mut late_nul = vec![b'a'; 20_000];
        late_nul[10_000] = 0;
        let text = read_indexable_from(&late_nul[..], 20_000).unwrap().unwrap();
        assert_eq!(text.len(), 20_000);
        let capped = vec![b'b'; MAX_FILE_BYTES as usize + 10];
        let text = read_indexable_from(&capped[..], capped.len() as u64)
            .unwrap()
            .unwrap();
        assert_eq!(text.len() as u64, MAX_FILE_BYTES);
    }

    #[test]
    fn save_replaces_an_index_whose_payload_no_longer_decodes() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let index = scan(temp.path(), Vec::new());
        let path = temp.path().join("index.json");
        save_index_to(&path, &index).unwrap();
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        envelope["payload"] = serde_json::json!({ "unexpected": true });
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(load_index_from(&path, temp.path()).is_err());
        // The save gate skips the payload, so a payload that is not a
        // `RootIndex` still reads as a current file instead of unreadable.
        let gate = crate::persistence::load_enveloped::<serde::de::IgnoredAny>(
            &crate::persistence::FsPersist::default(),
            &path,
            INDEX_STORE,
        )
        .gate;
        assert!(matches!(
            gate.status(),
            crate::persistence::LoadStatus::Current
        ));

        save_index_to(&path, &index).unwrap();

        let loaded = load_index_from(&path, temp.path()).unwrap().unwrap();
        assert_eq!(loaded.documents.len(), index.documents.len());
    }

    #[test]
    fn document_overhead_bounds_the_largest_metadata() {
        let document = IndexedDocument {
            path: PathBuf::new(),
            is_dir: false,
            size: u64::MAX,
            modified_secs: Some(u64::MAX),
            accessed_secs: Some(u64::MAX),
            volume: Some(u64::MAX),
            file_id: Some(u64::MAX),
            content: Some(String::new()),
        };
        // Nested as the envelope nests it: { payload: { documents: [doc] } }.
        let nested = serde_json::json!({ "payload": { "documents": [document] } });
        let bare = serde_json::json!({ "payload": { "documents": [] } });
        let document_bytes = serde_json::to_vec_pretty(&nested).unwrap().len()
            - serde_json::to_vec_pretty(&bare).unwrap().len();
        assert!(
            document_bytes <= DOCUMENT_JSON_OVERHEAD + 2 * json_string_len(""),
            "{document_bytes} bytes"
        );
        assert_eq!(json_string_len("a\"b\\c\n"), 2 + 3 + 2 + 2 + 6);
    }

    #[cfg(unix)]
    #[test]
    fn build_does_not_open_fifos() {
        let temp = TempDir::new();
        temp.file("file.txt", "text");
        let fifo = temp.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        let root = temp.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(scan(&root, Vec::new()));
        });

        let index = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("index build blocked on a FIFO");
        let pipe = index
            .documents
            .iter()
            .find(|document| document.path == fifo)
            .unwrap();
        assert!(pipe.content.is_none());
        assert_eq!(index.content_indexed, 1);
    }

    #[cfg(unix)]
    #[test]
    fn index_file_name_keeps_non_utf8_roots_apart() {
        use std::os::unix::ffi::OsStrExt;
        let first = index_path(Path::new(std::ffi::OsStr::from_bytes(b"/data/\xff")));
        let second = index_path(Path::new(std::ffi::OsStr::from_bytes(b"/data/\xfe")));
        assert_ne!(first, second);

        // UTF-8 roots keep the file name earlier builds wrote.
        let mut hash = 0xcbf29ce484222325u64;
        for byte in "/data/ü".as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        assert!(
            index_path(Path::new("/data/ü"))
                .ends_with(format!("content-v{SCHEMA_VERSION}-{hash:016x}.json"))
        );
    }

    #[test]
    fn exclusions_take_the_on_disk_name_case() {
        let names = || {
            ["Target", "target", "other"]
                .into_iter()
                .map(std::ffi::OsString::from)
        };
        assert_eq!(
            matching_entry_name(std::ffi::OsStr::new("TARGET"), names()),
            Some("Target".into())
        );
        assert_eq!(
            matching_entry_name(std::ffi::OsStr::new("target"), names()),
            Some("target".into())
        );
        assert_eq!(
            matching_entry_name(std::ffi::OsStr::new("missing"), names()),
            None
        );

        let temp = TempDir::new();
        temp.dir("Build/Out");
        let normalized =
            normalize_exclusions(temp.path(), vec![PathBuf::from("Build/Out/new")]).unwrap();
        assert_eq!(normalized, vec![temp.path().join("Build/Out/new")]);
    }

    #[test]
    fn damaged_settings_are_moved_aside_and_reported() {
        let temp = TempDir::new();
        let path = temp.path().join("content_index_settings.json");
        std::fs::write(&path, b"{ not json").unwrap();

        let loaded = load_settings_from(&path);

        assert!(loaded.settings.roots.is_empty());
        assert!(!loaded.unreadable);
        let error = loaded.problem.expect("a damaged file is reported");
        assert!(error.contains("damaged"), "{error}");
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(temp.path().join("content_index_settings.json.bak")).unwrap(),
            b"{ not json"
        );
        // A missing file is the normal first run, not an error.
        assert!(load_settings_from(&path).problem.is_none());

        // A second damaged file keeps the first backup.
        std::fs::write(&path, b"{ also not json").unwrap();
        assert!(load_settings_from(&path).problem.is_some());
        assert_eq!(
            std::fs::read(temp.path().join("content_index_settings.json.bak")).unwrap(),
            b"{ not json"
        );
        assert_eq!(
            std::fs::read(temp.path().join("content_index_settings.json.bak.1")).unwrap(),
            b"{ also not json"
        );
    }

    #[test]
    fn saves_are_refused_while_the_settings_file_is_unreadable() {
        let temp = TempDir::new();
        let path = temp.file("content_index_settings.json", "{}");

        let reason = settings_save_blocked_reason(true, &path).expect("save is refused");
        assert!(reason.contains(&path.display().to_string()), "{reason}");
        assert!(settings_save_blocked_reason(false, &path).is_none());
        // Once the unreadable file is gone, saving cannot destroy it.
        std::fs::remove_file(&path).unwrap();
        assert!(settings_save_blocked_reason(true, &path).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_settings_are_left_in_place_and_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new();
        let path = temp.file("content_index_settings.json", "{\"roots\": []}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads the file anyway; there is no read error to test then.
        if std::fs::File::open(&path).is_ok() {
            return;
        }

        let loaded = load_settings_from(&path);

        assert!(loaded.unreadable);
        assert!(loaded.problem.is_some());
        assert!(path.exists());
        assert!(!temp.path().join("content_index_settings.json.bak").exists());
    }

    #[test]
    fn index_save_errors_read_as_sentences() {
        let path = Path::new("/indexes/content.json");
        let blocked = describe_index_save_error(
            path,
            &crate::persistence::JsonSaveError::Blocked(crate::persistence::LoadStatus::Corrupt),
        );
        assert!(!blocked.contains("Blocked("), "{blocked}");
        assert!(blocked.contains("is corrupt"), "{blocked}");
        assert!(blocked.contains("/indexes/content.json"), "{blocked}");

        let io = describe_index_save_error(
            path,
            &crate::persistence::JsonSaveError::PreCommit(crate::persistence::PreCommitError::Io {
                stage: crate::persistence::SaveStage::Write,
                source: std::io::Error::other("disk full"),
                cleanup: None,
            }),
        );
        assert!(
            io.contains("could not write temporary file (disk full)"),
            "{io}"
        );
    }
}
