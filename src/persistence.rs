//! Shared JSON loading, atomic session writes and privacy-safe diagnostics.

use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

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
    Conflict,
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
            Self::Serialize(_) | Self::Conflict => None,
            Self::Io { stage, .. } => Some(*stage),
        }
    }

    pub fn cleanup(&self) -> Option<&io::Error> {
        match self {
            Self::Serialize(_) | Self::Conflict => None,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    len: u64,
    digest: [u8; 32],
}

impl Revision {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            digest: *blake3::hash(bytes).as_bytes(),
        }
    }

    pub(crate) fn from_hasher(len: u64, hasher: blake3::Hasher) -> Self {
        Self {
            len,
            digest: *hasher.finalize().as_bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpectedRevision {
    Missing,
    Exact(Revision),
    /// Compatibility mode for stores whose owner explicitly accepts
    /// serialized last-writer-wins updates.
    Any,
}

#[derive(Debug)]
pub enum ReadOutcome {
    Missing,
    Present { bytes: Vec<u8>, revision: Revision },
}

/// Byte-level persistence boundary. Serialization, schema migration, and
/// recovery policy deliberately live above this object-safe port.
pub trait Persist: Send + Sync {
    fn read(&self, path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure>;

    fn commit(
        &self,
        path: &Path,
        bytes: &[u8],
        expected: ExpectedRevision,
    ) -> Result<AtomicWriteOutcome, PreCommitError>;
}

#[derive(Default)]
pub struct FsPersist {
    _instance: (),
}

fn normalized_lock_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => {
                normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            std::path::Component::Normal(component) => normalized.push(component),
        }
    }
    if let (Some(parent), Some(file_name)) = (normalized.parent(), normalized.file_name())
        && let Ok(canonical_parent) = std::fs::canonicalize(parent)
    {
        return canonical_parent.join(file_name);
    }
    normalized
}

fn path_locks() -> &'static Mutex<HashMap<PathBuf, Weak<Mutex<()>>>> {
    static PATH_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    PATH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn path_lock(path: &Path) -> Arc<Mutex<()>> {
    let mut locks = crate::lock_util::recover(path_locks());
    locks.retain(|_, lock| lock.strong_count() > 0);
    let key = normalized_lock_path(path);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

/// Exclusive cross-process lock for one Persist path.
///
/// Held across revision verification and the atomic replace so two processes
/// cannot both observe the same expected revision and race the rename.
struct ProcessStoreLock {
    _file: File,
}

fn store_lock_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("store"));
    let mut name = std::ffi::OsString::from(".");
    name.push(file_name);
    name.push(".persist.lock");
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

fn acquire_process_store_lock(path: &Path) -> io::Result<ProcessStoreLock> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = store_lock_path(path);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let file = options.open(&lock_path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ProcessStoreLock { _file: file })
    }
    #[cfg(not(unix))]
    {
        let file = options.open(&lock_path)?;
        Ok(ProcessStoreLock { _file: file })
    }
}

fn process_lock_io(error: io::Error) -> PreCommitError {
    PreCommitError::Io {
        stage: SaveStage::ValidatePath,
        source: error,
        cleanup: None,
    }
}

impl Persist for FsPersist {
    fn read(&self, path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure> {
        let _process_lock = acquire_process_store_lock(path).map_err(|error| ReadFailure {
            kind: ReadFailureKind::Io,
            source: error,
        })?;
        let path_lock = path_lock(path);
        let _guard = crate::lock_util::recover(&path_lock);
        read_file_bounded(path, max_bytes)
    }

    fn commit(
        &self,
        path: &Path,
        bytes: &[u8],
        expected: ExpectedRevision,
    ) -> Result<AtomicWriteOutcome, PreCommitError> {
        let _process_lock = acquire_process_store_lock(path).map_err(process_lock_io)?;
        let path_lock = path_lock(path);
        let _guard = crate::lock_util::recover(&path_lock);
        verify_expected_revision(path, &expected)?;
        write_bytes_atomic_with(path, bytes, &StdFsOps)
    }
}

impl FsPersist {
    /// Stream bytes into an atomic commit without buffering the full payload
    /// in memory. Used by large stores such as the content index.
    pub fn commit_with_writer<F>(
        &self,
        path: &Path,
        expected: ExpectedRevision,
        write: F,
    ) -> Result<AtomicWriteOutcome, PreCommitError>
    where
        F: FnOnce(&mut dyn Write) -> Result<(), PreCommitError>,
    {
        let _process_lock = acquire_process_store_lock(path).map_err(process_lock_io)?;
        let path_lock = path_lock(path);
        let _guard = crate::lock_util::recover(&path_lock);
        verify_expected_revision(path, &expected)?;
        write_stream_atomic_with(path, write, &StdFsOps)
    }
}

pub fn fs_persist() -> Arc<dyn Persist> {
    Arc::new(FsPersist::default())
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Default)]
pub struct EphemeralPersist {
    files: Mutex<HashMap<PathBuf, Vec<u8>>>,
}

#[cfg(any(test, feature = "visual-qa"))]
impl Persist for EphemeralPersist {
    fn read(&self, path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure> {
        let files = crate::lock_util::recover(&self.files);
        let Some(bytes) = files.get(path) else {
            return Ok(ReadOutcome::Missing);
        };
        if bytes.len() > max_bytes {
            return Err(read_failure(
                ReadFailureKind::TooLarge,
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ephemeral persistence target exceeds its byte limit",
                ),
            ));
        }
        Ok(ReadOutcome::Present {
            bytes: bytes.clone(),
            revision: Revision::from_bytes(bytes),
        })
    }

    fn commit(
        &self,
        path: &Path,
        bytes: &[u8],
        expected: ExpectedRevision,
    ) -> Result<AtomicWriteOutcome, PreCommitError> {
        let mut files = crate::lock_util::recover(&self.files);
        let current = files.get(path).map(|bytes| Revision::from_bytes(bytes));
        let matches = match expected {
            ExpectedRevision::Missing => current.is_none(),
            ExpectedRevision::Exact(expected) => current.as_ref() == Some(&expected),
            ExpectedRevision::Any => true,
        };
        if !matches {
            return Err(PreCommitError::Conflict);
        }
        files.insert(path.to_path_buf(), bytes.to_vec());
        Ok(AtomicWriteOutcome::Durable)
    }
}

#[cfg(any(test, feature = "visual-qa"))]
pub fn ephemeral_persist() -> Arc<dyn Persist> {
    Arc::new(EphemeralPersist::default())
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
    let revision = Revision::from_bytes(&bytes);
    Ok(ReadOutcome::Present { bytes, revision })
}

fn verify_expected_revision(
    path: &Path,
    expected: &ExpectedRevision,
) -> Result<(), PreCommitError> {
    match expected {
        ExpectedRevision::Any => Ok(()),
        ExpectedRevision::Missing => match read_file_bounded(path, 0) {
            Ok(ReadOutcome::Missing) => Ok(()),
            Ok(ReadOutcome::Present { .. }) | Err(_) => Err(PreCommitError::Conflict),
        },
        ExpectedRevision::Exact(expected) => {
            let Ok(max_bytes) = usize::try_from(expected.len) else {
                return Err(PreCommitError::Conflict);
            };
            match read_file_bounded(path, max_bytes) {
                Ok(ReadOutcome::Present { revision, .. }) if revision == *expected => Ok(()),
                _ => Err(PreCommitError::Conflict),
            }
        }
    }
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
    expected: ExpectedRevision,
    recovered_source: Option<Vec<u8>>,
}

impl StoreGate {
    pub fn missing() -> Self {
        Self {
            status: LoadStatus::Missing,
            generation: 0,
            expected: ExpectedRevision::Missing,
            recovered_source: None,
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

    pub(crate) fn mark_recovered(&mut self) {
        if matches!(
            self.status,
            LoadStatus::Legacy | LoadStatus::Current | LoadStatus::Recovered
        ) {
            self.status = LoadStatus::Recovered;
        }
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
    legacy_schema_marker: bool,
}

impl StoreSpec {
    pub const fn new(store: &'static str, schema: u32, max_bytes: usize) -> Self {
        Self {
            format: ENVELOPE_FORMAT,
            store,
            schema,
            max_bytes,
            legacy_schema_marker: false,
        }
    }

    pub const fn allow_legacy_schema_marker(mut self) -> Self {
        self.legacy_schema_marker = true;
        self
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
    GenerationExhausted,
    RecoveryPreservation,
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

struct LoadedRaw {
    value: Option<serde_json::Value>,
    gate: StoreGate,
    source_bytes: Option<Vec<u8>>,
}

fn loaded_without_value(status: LoadStatus, expected: ExpectedRevision) -> LoadedRaw {
    LoadedRaw {
        value: None,
        gate: if status == LoadStatus::Missing {
            StoreGate::missing()
        } else {
            StoreGate {
                status,
                generation: 0,
                expected,
                recovered_source: None,
            }
        },
        source_bytes: None,
    }
}

fn load_raw_envelope(persist: &dyn Persist, path: &Path, spec: StoreSpec) -> LoadedRaw {
    let (bytes, revision) = match persist.read(path, spec.max_bytes) {
        Ok(ReadOutcome::Missing) => {
            return loaded_without_value(LoadStatus::Missing, ExpectedRevision::Missing);
        }
        Ok(ReadOutcome::Present { bytes, revision }) => (bytes, revision),
        Err(_) => {
            return loaded_without_value(LoadStatus::Unreadable, ExpectedRevision::Any);
        }
    };
    let expected = ExpectedRevision::Exact(revision);
    let root = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(root) => root,
        Err(_) => return loaded_without_value(LoadStatus::Corrupt, expected),
    };
    let Some(object) = root.as_object() else {
        return loaded_without_value(LoadStatus::Corrupt, expected);
    };
    let envelope_candidate = object.contains_key("format")
        || object.contains_key("store")
        || (object.contains_key("schema") && !spec.legacy_schema_marker)
        || object.contains_key("payload")
        || object.contains_key("generation");
    if !envelope_candidate {
        return LoadedRaw {
            value: Some(root),
            gate: StoreGate {
                status: LoadStatus::Legacy,
                generation: 0,
                expected,
                recovered_source: None,
            },
            source_bytes: Some(bytes),
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
        return loaded_without_value(LoadStatus::Corrupt, expected);
    }
    let Some(schema) = schema else {
        return loaded_without_value(LoadStatus::Corrupt, expected);
    };
    if schema > spec.schema {
        return loaded_without_value(LoadStatus::FutureVersion, expected);
    }
    if schema != spec.schema {
        return loaded_without_value(LoadStatus::Corrupt, expected);
    }
    let (Some(generation), Some(payload)) = (generation, object.get("payload")) else {
        return loaded_without_value(LoadStatus::Corrupt, expected);
    };
    LoadedRaw {
        value: Some(payload.clone()),
        gate: StoreGate {
            status: LoadStatus::Current,
            generation,
            expected,
            recovered_source: None,
        },
        source_bytes: Some(bytes),
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
        Err(_) => LoadedJson {
            value: None,
            gate: StoreGate {
                status: LoadStatus::Corrupt,
                ..raw.gate
            },
        },
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
                gate.recovered_source = raw.source_bytes;
            }
            LoadedJson {
                value: Some(decoded),
                gate,
            }
        }
        Err(_) => LoadedJson {
            value: None,
            gate: StoreGate {
                status: LoadStatus::Corrupt,
                ..raw.gate
            },
        },
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
    let generation = gate
        .generation
        .checked_add(1)
        .ok_or(JsonSaveError::GenerationExhausted)?;
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
    preserve_recovered_source(persist, path, spec.max_bytes, gate)?;
    let outcome = persist
        .commit(path, &bytes, gate.expected.clone())
        .map_err(JsonSaveError::PreCommit)?;
    gate.status = LoadStatus::Current;
    gate.generation = generation;
    gate.expected = ExpectedRevision::Exact(Revision::from_bytes(&bytes));
    Ok(outcome)
}

/// Save an enveloped document through a streaming writer so large payloads
/// (content index) never require a full in-memory `to_vec` of the envelope.
pub fn save_enveloped_streaming<T: Serialize>(
    persist: &FsPersist,
    path: &Path,
    spec: StoreSpec,
    value: &T,
    gate: &mut StoreGate,
    intent: SaveIntent,
) -> Result<AtomicWriteOutcome, JsonSaveError> {
    if !gate.allows(intent) {
        return Err(JsonSaveError::Blocked(gate.status));
    }
    let generation = gate
        .generation
        .checked_add(1)
        .ok_or(JsonSaveError::GenerationExhausted)?;
    preserve_recovered_source(persist, path, spec.max_bytes, gate)?;
    let mut written = 0u64;
    let mut hasher = blake3::Hasher::new();
    let expected = gate.expected.clone();
    let outcome = persist
        .commit_with_writer(path, expected, |writer| {
            let mut hashing = HashingWriter {
                inner: writer,
                hasher: &mut hasher,
                written: &mut written,
            };
            let envelope = Envelope {
                format: spec.format,
                store: spec.store,
                schema: spec.schema,
                generation,
                payload: value,
            };
            serde_json::to_writer_pretty(&mut hashing, &envelope)
                .map_err(PreCommitError::Serialize)?;
            hashing.flush().map_err(|source| PreCommitError::Io {
                stage: SaveStage::Write,
                source,
                cleanup: None,
            })?;
            Ok(())
        })
        .map_err(JsonSaveError::PreCommit)?;
    gate.status = LoadStatus::Current;
    gate.generation = generation;
    gate.expected = ExpectedRevision::Exact(Revision::from_hasher(written, hasher));
    Ok(outcome)
}

struct HashingWriter<'a> {
    inner: &'a mut dyn Write,
    hasher: &'a mut blake3::Hasher,
    written: &'a mut u64,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        *self.written = (*self.written).saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn preserve_recovered_source(
    persist: &dyn Persist,
    path: &Path,
    max_bytes: usize,
    gate: &mut StoreGate,
) -> Result<(), JsonSaveError> {
    let Some(source) = gate.recovered_source.as_ref() else {
        return Ok(());
    };
    let file_name = path
        .file_name()
        .ok_or(JsonSaveError::RecoveryPreservation)?;
    let digest = blake3::hash(source);
    let mut quarantine_name = OsString::from(".");
    quarantine_name.push(file_name);
    quarantine_name.push(".recovered-");
    quarantine_name.push(&digest.to_hex()[..16]);
    quarantine_name.push(".json");
    let quarantine = path.with_file_name(quarantine_name);
    let expected = match persist.read(&quarantine, max_bytes) {
        Ok(ReadOutcome::Missing) => ExpectedRevision::Missing,
        Ok(ReadOutcome::Present { bytes, revision }) if bytes == *source => {
            ExpectedRevision::Exact(revision)
        }
        Ok(ReadOutcome::Present { .. }) | Err(_) => {
            return Err(JsonSaveError::RecoveryPreservation);
        }
    };
    match persist
        .commit(&quarantine, source, expected)
        .map_err(JsonSaveError::PreCommit)?
    {
        AtomicWriteOutcome::Durable => {
            gate.recovered_source = None;
            Ok(())
        }
        AtomicWriteOutcome::CommittedButNotDurable(_) => Err(JsonSaveError::RecoveryPreservation),
    }
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
/// Concurrent writers are excluded with a per-store flock plus the
/// revision token checked under that lock (cross-process CAS). The port
/// still does not promise a multi-file transaction.
/// Compatibility facade for stores that keep their schema and concurrency
/// policy outside [`Persist`]. Versioned stores should use `save_enveloped`
/// so stale snapshots are rejected.
pub fn save_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(PreCommitError::Serialize)?;
    commit_bytes_atomic(path, &bytes)
}

/// Save a lenient item store through the shared durable JSON boundary while
/// retaining the simple boolean contract used by dialog callers. Failures are
/// also published to persistence health diagnostics under `store`.
pub fn save_item_store<T: Serialize>(path: &Path, store: &'static str, value: &T) -> bool {
    match save_json_atomic(path, value) {
        Ok(_) => true,
        Err(error) => {
            record_save_failure(store, &error);
            false
        }
    }
}

pub(crate) fn commit_bytes_atomic(
    path: &Path,
    bytes: &[u8],
) -> Result<AtomicWriteOutcome, PreCommitError> {
    default_fs_persist().commit(path, bytes, ExpectedRevision::Any)
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

fn write_stream_atomic_with<F>(
    path: &Path,
    write: F,
    fs: &impl FsOps,
) -> Result<AtomicWriteOutcome, PreCommitError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), PreCommitError>,
{
    let (parent, prefix) = store_location(path)?;
    fs.create_dir_all(parent)
        .map_err(|source| precommit_io(SaveStage::CreateDirectory, source, None))?;
    let (temp_path, mut temp) = create_unique_temp(fs, parent, &prefix)?;
    if let Err(error) = write(&mut temp) {
        drop(temp);
        let _ = fs.remove_file(&temp_path);
        return Err(error);
    }
    let prepare = fs
        .flush(&mut temp)
        .map_err(|source| (SaveStage::Flush, source))
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
        Ok(ReadOutcome::Present { bytes, .. }) => String::from_utf8(bytes)
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
        .try_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
            Some(generation.saturating_add(1))
        })
        .unwrap_or_else(|generation| generation);
    health.issue_generation = previous.saturating_add(1);
    if let Some(issue) = &health.last_issue {
        log::warn!(target: "commander::persistence", "{issue}");
    }
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
    let stage = match error {
        PreCommitError::Serialize(_) => "serialize value".to_string(),
        PreCommitError::Conflict => "validate the loaded revision".to_string(),
        PreCommitError::Io { stage, .. } => stage.to_string(),
    };
    let reason = match error {
        PreCommitError::Serialize(source) => format!("{:?}", source.classify()),
        PreCommitError::Conflict => "stale snapshot".to_string(),
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
        JsonSaveError::GenerationExhausted | JsonSaveError::RecoveryPreservation => {
            let mut health = crate::lock_util::recover(health_state());
            health.save_failures = health.save_failures.saturating_add(1);
            health.last_issue = Some(format!(
                "{store}: save blocked to preserve the authoritative source data"
            ));
            publish_issue(&mut health);
        }
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
            let revision = Revision::from_bytes(&bytes);
            Ok(ReadOutcome::Present { bytes, revision })
        }

        fn commit(
            &self,
            _path: &Path,
            bytes: &[u8],
            expected: ExpectedRevision,
        ) -> Result<AtomicWriteOutcome, PreCommitError> {
            self.commits.fetch_add(1, Ordering::Relaxed);
            let mut current = crate::lock_util::recover(&self.bytes);
            let revision = current.as_deref().map(Revision::from_bytes);
            let matches = match expected {
                ExpectedRevision::Missing => current.is_none(),
                ExpectedRevision::Exact(expected) => revision.as_ref() == Some(&expected),
                ExpectedRevision::Any => true,
            };
            if !matches {
                return Err(PreCommitError::Conflict);
            }
            *current = Some(bytes.to_vec());
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
            (
                br#"{"schema":1,"name":"legacy-looking","count":1}"#.as_slice(),
                LoadStatus::Corrupt,
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
    fn stale_store_gate_cannot_overwrite_a_newer_snapshot() {
        let temp = TempDir::new();
        let path = temp.path().join("state.json");
        let persist = FsPersist::default();
        let mut initial_gate = StoreGate::missing();
        save_enveloped(
            &persist,
            &path,
            TEST_STORE,
            &Item {
                name: "initial".to_string(),
                count: 1,
            },
            &mut initial_gate,
            SaveIntent::Explicit,
        )
        .unwrap();
        let mut first = load_enveloped::<Item>(&persist, &path, TEST_STORE).gate;
        let mut stale = load_enveloped::<Item>(&persist, &path, TEST_STORE).gate;

        save_enveloped(
            &persist,
            &path,
            TEST_STORE,
            &Item {
                name: "newer".to_string(),
                count: 2,
            },
            &mut first,
            SaveIntent::Explicit,
        )
        .unwrap();
        let result = save_enveloped(
            &FsPersist::default(),
            &path,
            TEST_STORE,
            &Item {
                name: "stale".to_string(),
                count: 3,
            },
            &mut stale,
            SaveIntent::Explicit,
        );

        assert!(matches!(
            result,
            Err(JsonSaveError::PreCommit(PreCommitError::Conflict))
        ));
        assert_eq!(
            load_enveloped::<Item>(&persist, &path, TEST_STORE)
                .value
                .unwrap()
                .name,
            "newer"
        );
    }

    #[test]
    fn generation_overflow_fails_before_port_io() {
        let persist = MemoryPersist::new(None);
        let mut gate = StoreGate {
            status: LoadStatus::Current,
            generation: u64::MAX,
            expected: ExpectedRevision::Any,
            recovered_source: None,
        };
        let result = save_enveloped(
            &persist,
            Path::new("memory.json"),
            TEST_STORE,
            &Item {
                name: "overflow".to_string(),
                count: 1,
            },
            &mut gate,
            SaveIntent::Explicit,
        );
        assert!(matches!(result, Err(JsonSaveError::GenerationExhausted)));
        assert_eq!(persist.reads.load(Ordering::Relaxed), 0);
        assert_eq!(persist.commits.load(Ordering::Relaxed), 0);
        assert_eq!(gate.generation, u64::MAX);
    }

    #[test]
    fn recovered_item_save_preserves_the_original_raw_store() {
        let temp = TempDir::new();
        let path = temp.file(
            "items.json",
            r#"{"items":[{"name":"valid","count":1},{"name":"rejected","count":"many"}]}"#,
        );
        let original = std::fs::read(&path).unwrap();
        let persist = FsPersist::default();
        let mut loaded = load_enveloped_items::<Item>(&persist, &path, TEST_STORE);
        assert_eq!(loaded.gate.status(), LoadStatus::Recovered);
        let decoded = loaded.value.take().unwrap();

        save_enveloped(
            &persist,
            &path,
            TEST_STORE,
            &decoded.items,
            &mut loaded.gate,
            SaveIntent::Explicit,
        )
        .unwrap();

        let quarantine = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|candidate| {
                candidate
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains(".recovered-"))
            })
            .unwrap();
        assert_eq!(std::fs::read(quarantine).unwrap(), original);
        assert_eq!(loaded.gate.status(), LoadStatus::Current);
    }

    #[test]
    fn global_path_lock_registry_reclaims_unused_entries() {
        let baseline_lock = path_lock(Path::new("baseline"));
        let baseline = crate::lock_util::recover(path_locks()).len();
        drop(baseline_lock);
        for index in 0..512 {
            drop(path_lock(Path::new(&format!("transient-{index}"))));
        }
        let survivor = path_lock(Path::new("survivor"));
        let retained = crate::lock_util::recover(path_locks()).len();
        assert!(retained <= baseline.saturating_add(1));
        drop(survivor);
    }

    #[cfg(unix)]
    #[test]
    fn parent_symlink_aliases_share_the_same_in_process_lock() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let real_parent = temp.dir("real");
        let alias_parent = temp.path().join("alias");
        symlink(&real_parent, &alias_parent).unwrap();
        let real = real_parent.join("state.json");
        let alias = alias_parent.join("state.json");

        let real_lock = path_lock(&real);
        let alias_lock = path_lock(&alias);

        assert!(Arc::ptr_eq(&real_lock, &alias_lock));
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
                persist
                    .commit(&path, bytes.as_bytes(), ExpectedRevision::Any)
                    .unwrap();
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
