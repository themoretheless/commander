//! Optional, root-scoped content indexes built only while the app is idle.

use crate::panel::{FileEntry, format_size};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, ReadDir};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

const SCHEMA_VERSION: u32 = 1;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
pub const MAX_DOCUMENTS: usize = 200_000;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_CONTENT_BYTES: usize = 32 * 1024 * 1024;

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
        let modified = self
            .modified_secs
            .map(|seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds));
        let modified_str = modified.map_or_else(
            || "-".to_string(),
            |time| {
                let value: chrono::DateTime<chrono::Local> = time.into();
                value.format("%d %b %y  %H:%M").to_string()
            },
        );
        Some(FileEntry {
            name_lower: name.to_lowercase(),
            name,
            path: self.path.clone(),
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
        self.accessed_secs
            .map(|seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
    }
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
    progress: BuildProgress,
}

impl Drop for BuildRun {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub struct ContentIndex {
    settings: SettingsStore,
    snapshots: HashMap<PathBuf, Arc<RootIndex>>,
    load_errors: HashMap<PathBuf, String>,
    active: Option<BuildRun>,
    idle: Arc<AtomicBool>,
}

impl ContentIndex {
    pub fn load() -> Self {
        Self {
            settings: load_settings_from(&settings_path()),
            snapshots: HashMap::new(),
            load_errors: HashMap::new(),
            active: None,
            idle: Arc::new(AtomicBool::new(false)),
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

    pub fn set_enabled(&mut self, root: PathBuf, enabled: bool) -> bool {
        let previous = self.settings.clone();
        let settings = self.settings_for_mut(root.clone());
        settings.enabled = enabled;
        if !save_settings_to(&settings_path(), &self.settings) {
            self.settings = previous;
            return false;
        }
        if !enabled && self.active.as_ref().is_some_and(|run| run.root == root) {
            self.active = None;
        }
        true
    }

    pub fn set_exclusions(&mut self, root: &Path, values: Vec<PathBuf>) -> Result<(), String> {
        let exclusions = normalize_exclusions(root, values)?;
        let previous = self.settings.clone();
        self.settings_for_mut(root.to_path_buf()).excluded_roots = exclusions;
        if save_settings_to(&settings_path(), &self.settings) {
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
        if !self.is_enabled(&root) {
            return false;
        }
        if !crate::provider_runtime::activate_builtin(
            "content-index",
            &crate::provider_runtime::ActivationRequest {
                capability: crate::provider_runtime::ProviderCapability::IndexBuild,
                root: &root,
                extension: None,
                bytes: None,
            },
        ) {
            self.load_errors.insert(
                root,
                "Content index provider activation exceeded its startup budget".to_string(),
            );
            return false;
        }
        let exclusions = self.exclusions(&root);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let idle = Arc::clone(&self.idle);
        let (sender, receiver) = mpsc::channel();
        let worker_root = root.clone();
        std::thread::spawn(move || {
            let result = scan_index(
                worker_root.clone(),
                exclusions,
                idle,
                worker_cancelled,
                |progress| {
                    if sender.send(BuildEvent::Progress(progress.clone())).is_err() {
                        return false;
                    }
                    notify();
                    true
                },
            )
            .and_then(|index| {
                save_index_to(&index_path(&worker_root), &index)?;
                Ok(index)
            });
            let _ = sender.send(BuildEvent::Complete(result));
            notify();
        });
        self.active = Some(BuildRun {
            root: root.clone(),
            receiver,
            cancelled,
            progress: BuildProgress::default(),
        });
        self.load_errors.remove(&root);
        if let Some(settings) = self
            .settings
            .roots
            .iter_mut()
            .find(|settings| settings.root == root)
        {
            settings.last_error = None;
            let _ = save_settings_to(&settings_path(), &self.settings);
        }
        true
    }

    pub fn poll(&mut self) {
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
                    self.snapshots.insert(root.clone(), Arc::new(index));
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
        if let Some(snapshot) = self.snapshots.get(root) {
            return Some(Arc::clone(snapshot));
        }
        match load_index_from(&index_path(root), root) {
            Ok(Some(index)) => {
                let index = Arc::new(index);
                self.snapshots
                    .insert(root.to_path_buf(), Arc::clone(&index));
                self.load_errors.remove(root);
                Some(index)
            }
            Ok(None) => None,
            Err(error) => {
                self.load_errors.insert(root.to_path_buf(), error);
                None
            }
        }
    }

    pub fn status(&mut self, root: &Path) -> IndexStatus {
        let enabled = self.is_enabled(root);
        let excluded_roots = self.exclusions(root);
        let settings_error = self
            .settings
            .roots
            .iter()
            .find(|settings| settings.root == root)
            .and_then(|settings| settings.last_error.clone());
        let snapshot = self.snapshot(root);
        let active = self.active.as_ref().filter(|run| run.root == root);
        let last_error = self.load_errors.get(root).cloned().or(settings_error);
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
        let _ = save_settings_to(&settings_path(), &self.settings);
    }
}

fn scan_index(
    root: PathBuf,
    excluded_roots: Vec<PathBuf>,
    idle: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    mut report: impl FnMut(&BuildProgress) -> bool,
) -> Result<RootIndex, String> {
    wait_for_idle(&idle, &cancelled)?;
    let root_entries = fs::read_dir(&root)
        .map_err(|error| format!("Could not read index root {}: {error}", root.display()))?;
    let mut stack: Vec<ReadDir> = vec![root_entries];
    let mut documents = Vec::new();
    let mut progress = BuildProgress::default();
    let mut skipped_binary = 0usize;
    let mut skipped_large = 0usize;
    let mut skipped_unreadable = 0usize;
    let mut skipped_quota = 0usize;
    let mut truncated = false;
    let mut last_progress = Instant::now();

    while let Some(entries) = stack.last_mut() {
        wait_for_idle(&idle, &cancelled)?;
        if !crate::io_budget::background_checkpoint(|| cancelled.load(Ordering::Acquire)) {
            return Err("Index build cancelled".to_string());
        }
        let Some(next) = entries.next() else {
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
        let metadata = if file_type.is_symlink() {
            fs::symlink_metadata(&path)
        } else {
            dir_entry.metadata()
        };
        let Ok(metadata) = metadata else {
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
        if !is_dir && !file_type.is_symlink() {
            if metadata.len() > MAX_FILE_BYTES {
                skipped_large = skipped_large.saturating_add(1);
            } else if progress
                .content_bytes
                .saturating_add(metadata.len() as usize)
                > MAX_CONTENT_BYTES
            {
                skipped_quota = skipped_quota.saturating_add(1);
            } else {
                match read_indexable_text(&path, metadata.len()) {
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
            match fs::read_dir(&path) {
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

#[cfg(test)]
pub(crate) fn build_test_index(root: &Path) -> RootIndex {
    scan_index(
        root.to_path_buf(),
        Vec::new(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicBool::new(false)),
        |_| true,
    )
    .expect("test content index should build")
}

fn wait_for_idle(idle: &AtomicBool, cancelled: &AtomicBool) -> Result<(), String> {
    while !idle.load(Ordering::Acquire) {
        if cancelled.load(Ordering::Acquire) {
            return Err("Index build cancelled".to_string());
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    if cancelled.load(Ordering::Acquire) {
        Err("Index build cancelled".to_string())
    } else {
        Ok(())
    }
}

fn read_indexable_text(path: &Path, size: u64) -> std::io::Result<Option<String>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_FILE_BYTES).read_to_end(&mut bytes)?;
    if bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn system_time_secs(time: Option<SystemTime>) -> Option<u64> {
    time?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

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
        if seen.insert(path.clone()) {
            normalized.push(path);
        }
    }
    normalized.sort();
    Ok(normalized)
}

fn settings_path() -> PathBuf {
    crate::fs_util::config_dir().join("content_index_settings.json")
}

fn indexes_dir() -> PathBuf {
    crate::fs_util::config_dir().join("indexes")
}

fn index_path(root: &Path) -> PathBuf {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in root.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    indexes_dir().join(format!("content-v{SCHEMA_VERSION}-{hash:016x}.json"))
}

fn load_settings_from(path: &Path) -> SettingsStore {
    fs::File::open(path)
        .ok()
        .and_then(|file| serde_json::from_reader(BufReader::new(file)).ok())
        .unwrap_or_default()
}

fn save_settings_to(path: &Path, settings: &SettingsStore) -> bool {
    serde_json::to_string_pretty(settings)
        .ok()
        .is_some_and(|json| crate::fs_util::write_atomic(path, &json))
}

fn load_index_from(path: &Path, expected_root: &Path) -> Result<Option<RootIndex>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not open content index: {error}")),
    };
    let index: RootIndex = serde_json::from_reader(BufReader::new(file))
        .map_err(|error| format!("Could not read content index: {error}"))?;
    if index.schema != SCHEMA_VERSION || index.root != expected_root {
        return Err("Content index schema or root does not match".to_string());
    }
    Ok(Some(index))
}

fn save_index_to(path: &Path, index: &RootIndex) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err("Content index path has no parent".to_string());
    };
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create index folder: {error}"))?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let result = (|| {
        let file = fs::File::create(&temporary)
            .map_err(|error| format!("Could not create content index: {error}"))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, index)
            .map_err(|error| format!("Could not serialize content index: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("Could not flush content index: {error}"))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| format!("Could not sync content index: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("Could not install content index: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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
            scan_index(root, Vec::new(), worker_idle, worker_cancelled, |_| true)
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
}
