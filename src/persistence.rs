//! Shared JSON loading, atomic session writes and privacy-safe diagnostics.

use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const LEGACY_JSON_MAX_BYTES: usize = 8 * 1024 * 1024;
const ENVELOPE_FORMAT: &str = "commander.persist";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistenceHealth {
    pub issue_generation: u64,
    pub recovered_stores: u64,
    pub recovered_items: u64,
    pub rejected_items: u64,
    pub unreadable_stores: u64,
    pub save_failures: u64,
    pub durability_warnings: u64,
    pub last_issue: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedItems<T> {
    pub items: Vec<T>,
    pub rejected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStage {
    ValidatePath,
    CreateDirectory,
    CreateTemp,
    Write,
    Flush,
    SyncFile,
    Rename,
    SyncDirectory,
    Cleanup,
}

impl fmt::Display for SaveStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ValidatePath => "validate path",
            Self::CreateDirectory => "create parent directory",
            Self::CreateTemp => "create temporary file",
            Self::Write => "write temporary file",
            Self::Flush => "flush temporary file",
            Self::SyncFile => "sync temporary file",
            Self::Rename => "atomically replace target",
            Self::SyncDirectory => "sync parent directory",
            Self::Cleanup => "clean up temporary file",
        })
    }
}

#[derive(Debug)]
pub enum PreCommitError {
    Serialize(serde_json::Error),
    /// `cleanup` preserves a second failure without hiding the primary error.
    Io {
        stage: SaveStage,
        source: io::Error,
        cleanup: Option<io::Error>,
    },
}

impl PreCommitError {
    pub fn stage(&self) -> Option<SaveStage> {
        match self {
            Self::Serialize(_) => None,
            Self::Io { stage, .. } => Some(*stage),
        }
    }

    pub fn cleanup(&self) -> Option<&io::Error> {
        match self {
            Self::Serialize(_) => None,
            Self::Io { cleanup, .. } => cleanup.as_ref(),
        }
    }
}

/// A rename commits the new JSON before the parent directory is synced.
/// A directory-sync error therefore remains a successful write outcome with
/// weaker crash durability, never a signal to replace the value with defaults.
#[derive(Debug)]
pub enum AtomicWriteOutcome {
    Durable,
    CommittedButNotDurable(io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFailureKind {
    Io,
    Symlink,
    NotRegular,
    TooLarge,
}

#[derive(Debug)]
pub struct ReadFailure {
    pub kind: ReadFailureKind,
    pub source: io::Error,
}

impl fmt::Display for ReadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.source)
    }
}

impl std::error::Error for ReadFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub enum ReadOutcome {
    Missing,
    Present { bytes: Vec<u8> },
}

/// Byte-level persistence boundary. Serialization, schema migration, and
/// recovery policy deliberately live above this object-safe port.
pub trait Persist: Send + Sync {
    fn read(&self, path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure>;

    fn commit(&self, path: &Path, bytes: &[u8]) -> Result<AtomicWriteOutcome, PreCommitError>;
}

#[derive(Default)]
pub struct FsPersist {
    path_locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl FsPersist {
    fn path_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        crate::lock_util::recover(&self.path_locks)
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

impl Persist for FsPersist {
    fn read(&self, path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure> {
        read_file_bounded(path, max_bytes)
    }

    fn commit(&self, path: &Path, bytes: &[u8]) -> Result<AtomicWriteOutcome, PreCommitError> {
        let path_lock = self.path_lock(path);
        let _guard = crate::lock_util::recover(&path_lock);
        write_bytes_atomic_with(path, bytes, &StdFsOps)
    }
}

pub fn fs_persist() -> Arc<dyn Persist> {
    Arc::new(FsPersist::default())
}

fn default_fs_persist() -> &'static FsPersist {
    static PERSIST: OnceLock<FsPersist> = OnceLock::new();
    PERSIST.get_or_init(FsPersist::default)
}

fn read_failure(kind: ReadFailureKind, source: io::Error) -> ReadFailure {
    ReadFailure { kind, source }
}

fn read_file_bounded(path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ReadOutcome::Missing),
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(read_failure(ReadFailureKind::Symlink, error));
        }
        Err(error) => return Err(read_failure(ReadFailureKind::Io, error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| read_failure(ReadFailureKind::Io, error))?;
    if !metadata.is_file() {
        return Err(read_failure(
            ReadFailureKind::NotRegular,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "persistence target is not a regular file",
            ),
        ));
    }
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(read_failure(
            ReadFailureKind::TooLarge,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "persistence target exceeds its byte limit",
            ),
        ));
    }
    let read_limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(max_bytes)
            .min(max_bytes),
    );
    Read::by_ref(&mut file)
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| read_failure(ReadFailureKind::Io, error))?;
    if bytes.len() > max_bytes {
        return Err(read_failure(
            ReadFailureKind::TooLarge,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "persistence target grew beyond its byte limit while reading",
            ),
        ));
    }
    Ok(ReadOutcome::Present { bytes })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    Missing,
    Legacy,
    Current,
    Recovered,
    Corrupt,
    FutureVersion,
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveIntent {
    Automatic,
    Explicit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreGate {
    status: LoadStatus,
    generation: u64,
}

impl StoreGate {
    pub fn missing() -> Self {
        Self {
            status: LoadStatus::Missing,
            generation: 0,
        }
    }

    pub fn status(&self) -> LoadStatus {
        self.status
    }

    pub(crate) fn block(&mut self, status: LoadStatus) {
        debug_assert!(matches!(
            status,
            LoadStatus::Corrupt | LoadStatus::FutureVersion | LoadStatus::Unreadable
        ));
        self.status = status;
    }

    fn allows(&self, intent: SaveIntent) -> bool {
        match self.status {
            LoadStatus::Missing | LoadStatus::Legacy | LoadStatus::Current => true,
            LoadStatus::Recovered => intent == SaveIntent::Explicit,
            LoadStatus::Corrupt | LoadStatus::FutureVersion | LoadStatus::Unreadable => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StoreSpec {
    pub format: &'static str,
    pub store: &'static str,
    pub schema: u32,
    pub max_bytes: usize,
}

impl StoreSpec {
    pub const fn new(store: &'static str, schema: u32, max_bytes: usize) -> Self {
        Self {
            format: ENVELOPE_FORMAT,
            store,
            schema,
            max_bytes,
        }
    }
}

#[derive(Debug)]
pub struct LoadedJson<T> {
    pub value: Option<T>,
    pub gate: StoreGate,
}

#[derive(Debug)]
pub enum JsonSaveError {
    Blocked(LoadStatus),
    PreCommit(PreCommitError),
}

#[derive(Serialize)]
struct Envelope<'a, T> {
    format: &'static str,
    store: &'static str,
    schema: u32,
    generation: u64,
    payload: &'a T,
}

fn loaded_without_value(status: LoadStatus) -> LoadedJson<serde_json::Value> {
    LoadedJson {
        value: None,
        gate: if status == LoadStatus::Missing {
            StoreGate::missing()
        } else {
            StoreGate {
                status,
                generation: 0,
            }
        },
    }
}

fn load_raw_envelope(
    persist: &dyn Persist,
    path: &Path,
    spec: StoreSpec,
) -> LoadedJson<serde_json::Value> {
    let bytes = match persist.read(path, spec.max_bytes) {
        Ok(ReadOutcome::Missing) => return loaded_without_value(LoadStatus::Missing),
        Ok(ReadOutcome::Present { bytes }) => bytes,
        Err(_) => return loaded_without_value(LoadStatus::Unreadable),
    };
    let root = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(root) => root,
        Err(_) => return loaded_without_value(LoadStatus::Corrupt),
    };
    let Some(object) = root.as_object() else {
        return loaded_without_value(LoadStatus::Corrupt);
    };
    let envelope_candidate = object.contains_key("format")
        || object.contains_key("store")
        || object.contains_key("payload")
        || object.contains_key("generation");
    if !envelope_candidate {
        return LoadedJson {
            value: Some(root),
            gate: StoreGate {
                status: LoadStatus::Legacy,
                generation: 0,
            },
        };
    }

    let format = object.get("format").and_then(serde_json::Value::as_str);
    let store = object.get("store").and_then(serde_json::Value::as_str);
    let schema = object
        .get("schema")
        .and_then(serde_json::Value::as_u64)
        .and_then(|schema| u32::try_from(schema).ok());
    let generation = object.get("generation").and_then(serde_json::Value::as_u64);
    if format != Some(spec.format) || store != Some(spec.store) {
        return loaded_without_value(LoadStatus::Corrupt);
    }
    let Some(schema) = schema else {
        return loaded_without_value(LoadStatus::Corrupt);
    };
    if schema > spec.schema {
        return loaded_without_value(LoadStatus::FutureVersion);
    }
    if schema != spec.schema {
        return loaded_without_value(LoadStatus::Corrupt);
    }
    let (Some(generation), Some(payload)) = (generation, object.get("payload")) else {
        return loaded_without_value(LoadStatus::Corrupt);
    };
    LoadedJson {
        value: Some(payload.clone()),
        gate: StoreGate {
            status: LoadStatus::Current,
            generation,
        },
    }
}

pub fn load_enveloped<T: DeserializeOwned>(
    persist: &dyn Persist,
    path: &Path,
    spec: StoreSpec,
) -> LoadedJson<T> {
    let raw = load_raw_envelope(persist, path, spec);
    let Some(value) = raw.value else {
        return LoadedJson {
            value: None,
            gate: raw.gate,
        };
    };
    match serde_json::from_value(value) {
        Ok(value) => LoadedJson {
            value: Some(value),
            gate: raw.gate,
        },
        Err(_) => loaded_without_value(LoadStatus::Corrupt).map_value(),
    }
}

impl LoadedJson<serde_json::Value> {
    fn map_value<T>(self) -> LoadedJson<T> {
        LoadedJson {
            value: None,
            gate: self.gate,
        }
    }
}

pub fn load_enveloped_items<T: DeserializeOwned>(
    persist: &dyn Persist,
    path: &Path,
    spec: StoreSpec,
) -> LoadedJson<DecodedItems<T>> {
    let raw = load_raw_envelope(persist, path, spec);
    let Some(value) = raw.value else {
        return LoadedJson {
            value: None,
            gate: raw.gate,
        };
    };
    match decode_item_value(value) {
        Ok(decoded) => {
            let mut gate = raw.gate;
            if decoded.rejected > 0 {
                gate.status = LoadStatus::Recovered;
            }
            LoadedJson {
                value: Some(decoded),
                gate,
            }
        }
        Err(_) => loaded_without_value(LoadStatus::Corrupt).map_value(),
    }
}

pub fn save_enveloped<T: Serialize>(
    persist: &dyn Persist,
    path: &Path,
    spec: StoreSpec,
    value: &T,
    gate: &mut StoreGate,
    intent: SaveIntent,
) -> Result<AtomicWriteOutcome, JsonSaveError> {
    if !gate.allows(intent) {
        return Err(JsonSaveError::Blocked(gate.status));
    }
    let generation = gate.generation.saturating_add(1);
    let envelope = Envelope {
        format: spec.format,
        store: spec.store,
        schema: spec.schema,
        generation,
        payload: value,
    };
    let bytes = serde_json::to_vec_pretty(&envelope)
        .map_err(PreCommitError::Serialize)
        .map_err(JsonSaveError::PreCommit)?;
    let outcome = persist
        .commit(path, &bytes)
        .map_err(JsonSaveError::PreCommit)?;
    gate.status = LoadStatus::Current;
    gate.generation = generation;
    Ok(outcome)
}

trait FsOps {
    fn before(&self, _stage: SaveStage) -> io::Result<()> {
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.before(SaveStage::CreateDirectory)?;
        std::fs::create_dir_all(path)
    }

    fn create_new(&self, path: &Path) -> io::Result<File> {
        self.before(SaveStage::CreateTemp)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    }

    fn write_all(&self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        self.before(SaveStage::Write)?;
        file.write_all(bytes)
    }

    fn flush(&self, file: &mut File) -> io::Result<()> {
        self.before(SaveStage::Flush)?;
        file.flush()
    }

    fn sync_file(&self, file: &File) -> io::Result<()> {
        self.before(SaveStage::SyncFile)?;
        file.sync_all()
    }

    fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
        self.before(SaveStage::Rename)?;
        std::fs::rename(source, target)
    }

    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        self.before(SaveStage::SyncDirectory)?;
        File::open(path)?.sync_all()
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.before(SaveStage::Cleanup)?;
        std::fs::remove_file(path)
    }
}

struct StdFsOps;

impl FsOps for StdFsOps {}

/// Atomically replace one JSON file on Commander's supported macOS/POSIX
/// target. Concurrent writers in this process are serialized per pathname.
/// The port remains last-writer-wins and does not promise a multi-file
/// transaction or cross-process compare-and-swap.
pub fn save_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(PreCommitError::Serialize)?;
    default_fs_persist().commit(path, &bytes)
}

#[cfg(test)]
fn save_json_atomic_with<T: Serialize>(
    path: &Path,
    value: &T,
    fs: &impl FsOps,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    // Serialization must finish before the first filesystem operation.
    let bytes = serde_json::to_vec_pretty(value).map_err(PreCommitError::Serialize)?;
    write_bytes_atomic_with(path, &bytes, fs)
}

fn write_bytes_atomic_with(
    path: &Path,
    bytes: &[u8],
    fs: &impl FsOps,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    let (parent, prefix) = store_location(path)?;
    fs.create_dir_all(parent)
        .map_err(|source| precommit_io(SaveStage::CreateDirectory, source, None))?;
    let (temp_path, mut temp) = create_unique_temp(fs, parent, &prefix)?;

    let prepare = fs
        .write_all(&mut temp, bytes)
        .map_err(|source| (SaveStage::Write, source))
        .and_then(|()| {
            fs.flush(&mut temp)
                .map_err(|source| (SaveStage::Flush, source))
        })
        .and_then(|()| {
            fs.sync_file(&temp)
                .map_err(|source| (SaveStage::SyncFile, source))
        });
    if let Err((stage, source)) = prepare {
        drop(temp);
        return Err(precommit_with_cleanup(fs, &temp_path, stage, source));
    }
    drop(temp);

    if let Err(source) = fs.rename(&temp_path, path) {
        return Err(precommit_with_cleanup(
            fs,
            &temp_path,
            SaveStage::Rename,
            source,
        ));
    }
    match fs.sync_directory(parent) {
        Ok(()) => Ok(AtomicWriteOutcome::Durable),
        Err(source) => Ok(AtomicWriteOutcome::CommittedButNotDurable(source)),
    }
}

fn store_location(path: &Path) -> Result<(&Path, OsString), PreCommitError> {
    let Some(file_name) = path.file_name() else {
        return Err(precommit_io(
            SaveStage::ValidatePath,
            io::Error::new(io::ErrorKind::InvalidInput, "store path has no file name"),
            None,
        ));
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut prefix = OsString::from(".");
    prefix.push(file_name);
    prefix.push(".commander-json-");
    Ok((parent, prefix))
}

fn create_unique_temp(
    fs: &impl FsOps,
    parent: &Path,
    prefix: &OsString,
) -> Result<(PathBuf, File), PreCommitError> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    for _ in 0..128 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = prefix.clone();
        name.push(format!("{}-{sequence:020}.tmp", std::process::id()));
        let path = parent.join(name);
        match fs.create_new(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(precommit_io(SaveStage::CreateTemp, source, None));
            }
        }
    }
    Err(precommit_io(
        SaveStage::CreateTemp,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary file",
        ),
        None,
    ))
}

fn precommit_io(stage: SaveStage, source: io::Error, cleanup: Option<io::Error>) -> PreCommitError {
    PreCommitError::Io {
        stage,
        source,
        cleanup,
    }
}

fn precommit_with_cleanup(
    fs: &impl FsOps,
    temp_path: &Path,
    stage: SaveStage,
    source: io::Error,
) -> PreCommitError {
    let cleanup = match fs.remove_file(temp_path) {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => Some(source),
    };
    PreCommitError::Io {
        stage,
        source,
        cleanup,
    }
}

enum LoadFailure {
    Io,
    Symlink,
}

fn read_target(path: &Path) -> Result<Option<String>, LoadFailure> {
    match default_fs_persist().read(path, LEGACY_JSON_MAX_BYTES) {
        Ok(ReadOutcome::Missing) => Ok(None),
        Ok(ReadOutcome::Present { bytes }) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| LoadFailure::Io),
        Err(failure) if failure.kind == ReadFailureKind::Symlink => Err(LoadFailure::Symlink),
        Err(_) => Err(LoadFailure::Io),
    }
}

static HEALTH: OnceLock<Mutex<PersistenceHealth>> = OnceLock::new();
static ISSUE_GENERATION: AtomicU64 = AtomicU64::new(0);

fn health_state() -> &'static Mutex<PersistenceHealth> {
    HEALTH.get_or_init(|| Mutex::new(PersistenceHealth::default()))
}

fn publish_issue(health: &mut PersistenceHealth) {
    let previous = ISSUE_GENERATION
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
            Some(generation.saturating_add(1))
        })
        .unwrap_or_else(|generation| generation);
    health.issue_generation = previous.saturating_add(1);
}

pub(crate) fn record_recovery(store: &'static str, recovered: usize, rejected: usize) {
    let mut health = crate::lock_util::recover(health_state());
    health.recovered_stores = health.recovered_stores.saturating_add(1);
    health.recovered_items = health
        .recovered_items
        .saturating_add(u64::try_from(recovered).unwrap_or(u64::MAX));
    health.rejected_items = health
        .rejected_items
        .saturating_add(u64::try_from(rejected).unwrap_or(u64::MAX));
    health.last_issue = Some(format!(
        "{store}: recovered {recovered} item(s), skipped {rejected} invalid item(s)"
    ));
    publish_issue(&mut health);
}

pub(crate) fn record_unreadable(store: &'static str) {
    let mut health = crate::lock_util::recover(health_state());
    health.unreadable_stores = health.unreadable_stores.saturating_add(1);
    health.last_issue = Some(format!(
        "{store}: settings could not be read; defaults are active"
    ));
    publish_issue(&mut health);
}

pub fn record_save_failure(store: &'static str, error: &PreCommitError) {
    let stage = error
        .stage()
        .map_or("serialize value".to_string(), |stage| stage.to_string());
    let reason = match error {
        PreCommitError::Serialize(source) => format!("{:?}", source.classify()),
        PreCommitError::Io { source, .. } => format!("{:?}", source.kind()),
    };
    let cleanup = error
        .cleanup()
        .map_or("", |_| "; temporary-file cleanup also failed");
    let mut health = crate::lock_util::recover(health_state());
    health.save_failures = health.save_failures.saturating_add(1);
    health.last_issue = Some(format!(
        "{store}: save failed before commit while trying to {stage} ({reason}){cleanup}; previous data remains authoritative"
    ));
    publish_issue(&mut health);
}

pub fn record_json_save_failure(store: &'static str, error: &JsonSaveError) {
    match error {
        JsonSaveError::PreCommit(error) => record_save_failure(store, error),
        JsonSaveError::Blocked(status) => {
            let mut health = crate::lock_util::recover(health_state());
            health.save_failures = health.save_failures.saturating_add(1);
            health.last_issue = Some(format!(
                "{store}: save blocked because the loaded store is {status:?}; original data remains authoritative"
            ));
            publish_issue(&mut health);
        }
    }
}

pub fn record_durability_warning(store: &'static str, source: &io::Error) {
    let mut health = crate::lock_util::recover(health_state());
    health.durability_warnings = health.durability_warnings.saturating_add(1);
    health.last_issue = Some(format!(
        "{store}: save committed, but could not sync parent directory ({:?}); the new data remains active",
        source.kind()
    ));
    publish_issue(&mut health);
}

pub fn issue_generation() -> u64 {
    ISSUE_GENERATION.load(Ordering::Acquire)
}

pub fn health_snapshot() -> PersistenceHealth {
    crate::lock_util::recover(health_state()).clone()
}

/// Decode an object containing an `items` array one element at a time. A bad
/// element cannot erase valid siblings; malformed roots still fail closed.
pub fn decode_item_store<T: DeserializeOwned>(json: &str) -> Result<DecodedItems<T>, String> {
    let root: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    decode_item_value(root)
}

fn decode_item_value<T: DeserializeOwned>(
    root: serde_json::Value,
) -> Result<DecodedItems<T>, String> {
    let values = root
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "settings root does not contain an items array".to_string())?;
    let mut items = Vec::with_capacity(values.len());
    let mut rejected = 0usize;
    for value in values {
        match serde_json::from_value(value.clone()) {
            Ok(item) => items.push(item),
            Err(_) => rejected = rejected.saturating_add(1),
        }
    }
    Ok(DecodedItems { items, rejected })
}

pub fn load_item_store<T: DeserializeOwned>(path: &Path, store: &'static str) -> Vec<T> {
    let json = match read_target(path) {
        Ok(Some(json)) => json,
        Ok(None) => return Vec::new(),
        Err(_) => {
            record_unreadable(store);
            return Vec::new();
        }
    };
    match decode_item_store(&json) {
        Ok(decoded) => {
            if decoded.rejected > 0 {
                record_recovery(store, decoded.items.len(), decoded.rejected);
            }
            decoded.items
        }
        Err(_) => {
            record_unreadable(store);
            Vec::new()
        }
    }
}

#[cfg(test)]
pub fn load_json<T: DeserializeOwned>(path: &Path, store: &'static str) -> Option<T> {
    let json = match read_target(path) {
        Ok(Some(json)) => json,
        Ok(None) => return None,
        Err(_) => {
            record_unreadable(store);
            return None;
        }
    };
    match serde_json::from_str(&json) {
        Ok(value) => Some(value),
        Err(_) => {
            record_unreadable(store);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use serde::{Deserialize, Serializer};
    use std::sync::Barrier;

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
    struct Item {
        name: String,
        count: u8,
    }

    struct EncodeFailure;

    impl Serialize for EncodeFailure {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom("injected encode failure"))
        }
    }

    struct MemoryPersist {
        bytes: Mutex<Option<Vec<u8>>>,
        reads: AtomicU64,
        commits: AtomicU64,
        committed_not_durable: bool,
    }

    impl MemoryPersist {
        fn new(bytes: Option<Vec<u8>>) -> Self {
            Self {
                bytes: Mutex::new(bytes),
                reads: AtomicU64::new(0),
                commits: AtomicU64::new(0),
                committed_not_durable: false,
            }
        }

        fn not_durable(bytes: Option<Vec<u8>>) -> Self {
            Self {
                committed_not_durable: true,
                ..Self::new(bytes)
            }
        }

        fn bytes(&self) -> Option<Vec<u8>> {
            crate::lock_util::recover(&self.bytes).clone()
        }
    }

    impl Persist for MemoryPersist {
        fn read(&self, _path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let Some(bytes) = self.bytes() else {
                return Ok(ReadOutcome::Missing);
            };
            if bytes.len() > max_bytes {
                return Err(read_failure(
                    ReadFailureKind::TooLarge,
                    io::Error::new(io::ErrorKind::InvalidData, "fake value is oversized"),
                ));
            }
            Ok(ReadOutcome::Present { bytes })
        }

        fn commit(&self, _path: &Path, bytes: &[u8]) -> Result<AtomicWriteOutcome, PreCommitError> {
            self.commits.fetch_add(1, Ordering::Relaxed);
            *crate::lock_util::recover(&self.bytes) = Some(bytes.to_vec());
            if self.committed_not_durable {
                Ok(AtomicWriteOutcome::CommittedButNotDurable(
                    io::Error::other("injected directory sync failure"),
                ))
            } else {
                Ok(AtomicWriteOutcome::Durable)
            }
        }
    }

    const TEST_STORE: StoreSpec = StoreSpec::new("commander.test", 1, 4096);

    struct FailFs {
        failures: Vec<SaveStage>,
    }

    impl FailFs {
        fn new(failures: &[SaveStage]) -> Self {
            Self {
                failures: failures.to_vec(),
            }
        }
    }

    impl FsOps for FailFs {
        fn before(&self, stage: SaveStage) -> io::Result<()> {
            if self.failures.contains(&stage) {
                Err(io::Error::other(format!("injected {stage:?} failure")))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn item_decoder_preserves_valid_siblings_around_a_bad_item() {
        let decoded = decode_item_store::<Item>(
            r#"{"items":[{"name":"first","count":1},{"name":"bad","count":"many"},{"name":"last","count":3}]}"#,
        )
        .unwrap();
        assert_eq!(decoded.rejected, 1);
        assert_eq!(
            decoded.items,
            vec![
                Item {
                    name: "first".to_string(),
                    count: 1,
                },
                Item {
                    name: "last".to_string(),
                    count: 3,
                },
            ]
        );
    }

    #[test]
    fn item_decoder_rejects_a_malformed_or_wrong_shaped_root() {
        assert!(decode_item_store::<Item>("not json").is_err());
        assert!(decode_item_store::<Item>(r#"{"entries": []}"#).is_err());
        assert!(decode_item_store::<Item>(r#"{"items": {}}"#).is_err());
    }

    #[test]
    fn injected_port_loads_legacy_without_writing_and_explicit_save_upgrades() {
        let original = Item {
            name: "legacy".to_string(),
            count: 4,
        };
        let persist = MemoryPersist::new(Some(serde_json::to_vec(&original).unwrap()));
        let path = Path::new("memory.json");

        let mut loaded = load_enveloped::<Item>(&persist, path, TEST_STORE);

        assert_eq!(loaded.value, Some(original.clone()));
        assert_eq!(loaded.gate.status(), LoadStatus::Legacy);
        assert_eq!(persist.reads.load(Ordering::Relaxed), 1);
        assert_eq!(persist.commits.load(Ordering::Relaxed), 0);

        let updated = Item {
            name: "current".to_string(),
            count: 5,
        };
        assert!(matches!(
            save_enveloped(
                &persist,
                path,
                TEST_STORE,
                &updated,
                &mut loaded.gate,
                SaveIntent::Explicit,
            ),
            Ok(AtomicWriteOutcome::Durable)
        ));
        assert_eq!(persist.commits.load(Ordering::Relaxed), 1);
        let current = load_enveloped::<Item>(&persist, path, TEST_STORE);
        assert_eq!(current.gate.status(), LoadStatus::Current);
        assert_eq!(current.value, Some(updated));
    }

    #[test]
    fn malformed_wrong_store_and_future_envelopes_never_fall_back_or_save() {
        let cases = [
            (
                br#"{"format":"commander.persist","schema":1,"payload":{"name":"x","count":1}}"#
                    .as_slice(),
                LoadStatus::Corrupt,
            ),
            (
                br#"{"format":"commander.persist","store":"other","schema":1,"generation":1,"payload":{"name":"x","count":1}}"#
                    .as_slice(),
                LoadStatus::Corrupt,
            ),
            (
                br#"{"format":"commander.persist","store":"commander.test","schema":2,"generation":1,"payload":{"name":"x","count":1}}"#
                    .as_slice(),
                LoadStatus::FutureVersion,
            ),
        ];
        for (bytes, expected) in cases {
            let persist = MemoryPersist::new(Some(bytes.to_vec()));
            let mut loaded = load_enveloped::<Item>(&persist, Path::new("memory.json"), TEST_STORE);
            assert!(loaded.value.is_none());
            assert_eq!(loaded.gate.status(), expected);
            let result = save_enveloped(
                &persist,
                Path::new("memory.json"),
                TEST_STORE,
                &Item {
                    name: "replacement".to_string(),
                    count: 9,
                },
                &mut loaded.gate,
                SaveIntent::Explicit,
            );
            assert!(matches!(result, Err(JsonSaveError::Blocked(status)) if status == expected));
            assert_eq!(persist.commits.load(Ordering::Relaxed), 0);
            assert_eq!(persist.bytes().as_deref(), Some(bytes));
        }
    }

    #[test]
    fn envelope_serialization_finishes_before_the_port_is_called() {
        let persist = MemoryPersist::new(None);
        let mut gate = StoreGate::missing();
        let result = save_enveloped(
            &persist,
            Path::new("memory.json"),
            TEST_STORE,
            &EncodeFailure,
            &mut gate,
            SaveIntent::Explicit,
        );
        assert!(matches!(
            result,
            Err(JsonSaveError::PreCommit(PreCommitError::Serialize(_)))
        ));
        assert_eq!(persist.commits.load(Ordering::Relaxed), 0);
        assert_eq!(gate.status(), LoadStatus::Missing);
    }

    #[test]
    fn committed_not_durable_remains_committed_and_advances_the_gate() {
        let persist = MemoryPersist::not_durable(None);
        let mut gate = StoreGate::missing();
        let value = Item {
            name: "committed".to_string(),
            count: 7,
        };
        let result = save_enveloped(
            &persist,
            Path::new("memory.json"),
            TEST_STORE,
            &value,
            &mut gate,
            SaveIntent::Explicit,
        );
        assert!(matches!(
            result,
            Ok(AtomicWriteOutcome::CommittedButNotDurable(_))
        ));
        assert_eq!(gate.status(), LoadStatus::Current);
        assert_eq!(
            load_enveloped::<Item>(&persist, Path::new("memory.json"), TEST_STORE).value,
            Some(value)
        );
        assert_eq!(persist.commits.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn encode_failure_happens_before_filesystem_io() {
        let temp = TempDir::new();
        let path = temp.file("state.json", "old");

        let error = save_json_atomic(&path, &EncodeFailure).unwrap_err();

        assert!(matches!(error, PreCommitError::Serialize(_)));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "old");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn successful_save_replaces_target_and_leaves_no_temp() {
        let temp = TempDir::new();
        let path = temp.file("state.json", r#"{"name":"old","count":1}"#);
        let expected = Item {
            name: "new".to_string(),
            count: 2,
        };

        assert!(matches!(
            save_json_atomic(&path, &expected).unwrap(),
            AtomicWriteOutcome::Durable
        ));
        assert_eq!(load_json::<Item>(&path, "Test"), Some(expected));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failpoint_matrix_preserves_commit_boundary_and_both_cleanup_errors() {
        let cases: &[(&str, &[SaveStage], SaveStage, bool, bool)] = &[
            ("write", &[SaveStage::Write], SaveStage::Write, false, false),
            ("flush", &[SaveStage::Flush], SaveStage::Flush, false, false),
            (
                "file-sync",
                &[SaveStage::SyncFile],
                SaveStage::SyncFile,
                false,
                false,
            ),
            (
                "rename",
                &[SaveStage::Rename],
                SaveStage::Rename,
                false,
                false,
            ),
            (
                "directory-sync",
                &[SaveStage::SyncDirectory],
                SaveStage::SyncDirectory,
                true,
                false,
            ),
            (
                "cleanup",
                &[SaveStage::Write, SaveStage::Cleanup],
                SaveStage::Write,
                false,
                true,
            ),
        ];
        let temp = TempDir::new();
        let new_value = Item {
            name: "new".to_string(),
            count: 2,
        };

        for &(name, failures, stage, committed, cleanup_failed) in cases {
            let root = temp.dir(name);
            let path = root.join("state.json");
            std::fs::write(&path, r#"{"name":"old","count":1}"#).unwrap();
            let result = save_json_atomic_with(&path, &new_value, &FailFs::new(failures));

            if committed {
                let AtomicWriteOutcome::CommittedButNotDurable(_) = result.unwrap() else {
                    panic!("{name} should be committed with a warning");
                };
                assert_eq!(stage, SaveStage::SyncDirectory, "{name}");
                assert_eq!(load_json::<Item>(&path, "Test"), Some(new_value.clone()));
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.stage(), Some(stage), "{name}");
                assert_eq!(error.cleanup().is_some(), cleanup_failed, "{name}");
                if let Some(cleanup) = error.cleanup() {
                    assert!(cleanup.to_string().contains("Cleanup"), "{name}");
                }
                if cleanup_failed {
                    let PreCommitError::Io { source, .. } = &error else {
                        unreachable!();
                    };
                    assert!(source.to_string().contains("Write"), "{name}");
                }
                assert_eq!(
                    std::fs::read_to_string(&path).unwrap(),
                    r#"{"name":"old","count":1}"#,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn corrupt_target_is_not_replaced_by_a_temp_candidate() {
        let temp = TempDir::new();
        let path = temp.file("state.json", "not json");
        temp.file(
            ".state.json.commander-json-999-00000000000000000001.tmp",
            r#"{"name":"stale","count":9}"#,
        );

        assert!(load_json::<Item>(&path, "Test").is_none());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not json");
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_a_symlink_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let real = temp.file("real.json", r#"{"name":"real","count":1}"#);
        let link = temp.path().join("state.json");
        symlink(real, &link).unwrap();

        let error = FsPersist::default().read(&link, 4096).unwrap_err();
        assert_eq!(error.kind, ReadFailureKind::Symlink);
        assert!(load_json::<Item>(&link, "Test").is_none());
    }

    #[test]
    fn bounded_read_rejects_oversized_files_before_json_decode() {
        let temp = TempDir::new();
        let path = temp.file("large.json", "0123456789");
        let error = FsPersist::default().read(&path, 4).unwrap_err();
        assert_eq!(error.kind, ReadFailureKind::TooLarge);
    }

    #[cfg(unix)]
    #[test]
    fn bounded_read_rejects_fifo_without_blocking() {
        let temp = TempDir::new();
        let path = temp.path().join("state.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let error = FsPersist::default().read(&path, 64).unwrap_err();
        assert_eq!(error.kind, ReadFailureKind::NotRegular);
    }

    #[test]
    fn concurrent_same_path_commits_publish_only_complete_values() {
        let temp = TempDir::new();
        let path = temp.path().join("state.json");
        let persist = Arc::new(FsPersist::default());
        let barrier = Arc::new(Barrier::new(9));
        let mut workers = Vec::new();
        for index in 0..8 {
            let persist = persist.clone();
            let barrier = barrier.clone();
            let path = path.clone();
            workers.push(std::thread::spawn(move || {
                let bytes = format!(r#"{{"writer":{index},"payload":"complete"}}"#);
                barrier.wait();
                persist.commit(&path, bytes.as_bytes()).unwrap();
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["payload"], "complete");
        assert!(value["writer"].as_u64().is_some_and(|writer| writer < 8));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
