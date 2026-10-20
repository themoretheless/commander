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
    /// Data a store no longer references that could not be removed. Kept
    /// apart from `save_failures` because the save itself succeeded.
    pub cleanup_failures: u64,
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
    pub const fn stage(&self) -> Option<SaveStage> {
        match self {
            Self::Serialize(_) | Self::Conflict => None,
            Self::Io { stage, .. } => Some(*stage),
        }
    }

    pub const fn cleanup(&self) -> Option<&io::Error> {
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

const fn process_lock_io(error: io::Error) -> PreCommitError {
    PreCommitError::Io {
        stage: SaveStage::ValidatePath,
        source: error,
        cleanup: None,
    }
}

/// Hold both store locks (cross-process flock, then in-process path lock)
/// for the duration of `body`.
fn with_store_locks<T, E>(
    path: &Path,
    lock_error: impl FnOnce(io::Error) -> E,
    body: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let _process_lock = acquire_process_store_lock(path).map_err(lock_error)?;
    let path_lock = path_lock(path);
    let _guard = crate::lock_util::recover(&path_lock);
    body()
}

impl Persist for FsPersist {
    fn read(&self, path: &Path, max_bytes: usize) -> Result<ReadOutcome, ReadFailure> {
        // A read must not create the store's directory, and a store that
        // does not exist is missing even where no lock file can be created
        // (an absent or read-only config directory). Saves still take the
        // lock and re-check the expected revision, so skipping it here
        // cannot let a stale snapshot through.
        let store_absent = || {
            std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && std::fs::symlink_metadata(parent)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        {
            return Ok(ReadOutcome::Missing);
        }
        let _process_lock = match acquire_process_store_lock(path) {
            Ok(lock) => lock,
            Err(_) if store_absent() => return Ok(ReadOutcome::Missing),
            Err(error) => return Err(read_failure(ReadFailureKind::Io, error)),
        };
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
        with_store_locks(path, process_lock_io, || {
            sweep_stale_temps(path);
            verify_expected_revision(path, &expected)?;
            write_bytes_atomic_with(path, bytes, &StdFsOps)
        })
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
        with_store_locks(path, process_lock_io, || {
            sweep_stale_temps(path);
            verify_expected_revision(path, &expected)?;
            write_stream_atomic_with(path, write, &StdFsOps)
        })
    }
}

/// Remove temp files a crashed or killed writer left beside `path`, once per
/// store per process. Must run under the store's flock: every writer of this
/// store holds it while its temp exists, so a temp named for another PID is
/// orphaned. A temp is never authoritative (the rename is the commit), so
/// removal cannot lose data. Without flock (non-Unix) another process may be
/// mid-write, so nothing is swept there.
fn sweep_stale_temps(path: &Path) {
    #[cfg(unix)]
    {
        static SWEPT: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
        if !crate::lock_util::recover(SWEPT.get_or_init(Mutex::default))
            .insert(normalized_lock_path(path))
        {
            return;
        }
        let Ok((parent, prefix)) = store_location(path) else {
            return;
        };
        let (Some(prefix), Ok(entries)) = (prefix.to_str(), std::fs::read_dir(parent)) else {
            return;
        };
        let own_pid = std::process::id().to_string();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some((pid, sequence)) = name
                .to_str()
                .and_then(|name| name.strip_prefix(prefix))
                .and_then(|rest| rest.strip_suffix(".tmp"))
                .and_then(|rest| rest.split_once('-'))
            else {
                continue;
            };
            let numeric = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
            if numeric(pid)
                && numeric(sequence)
                && pid != own_pid
                && entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
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

const fn read_failure(kind: ReadFailureKind, source: io::Error) -> ReadFailure {
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

impl LoadStatus {
    /// Statuses that must block writes instead of replacing unread data.
    pub const fn fails_closed(self) -> bool {
        matches!(self, Self::Corrupt | Self::FutureVersion | Self::Unreadable)
    }
}

/// What a reported save left on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Committed {
    Durable,
    NotDurable,
}

/// Record a store save in the health counters and classify it; `None` when
/// nothing was committed.
pub fn report_save(
    store: &'static str,
    result: Result<AtomicWriteOutcome, JsonSaveError>,
) -> Option<Committed> {
    match result {
        Ok(AtomicWriteOutcome::Durable) => Some(Committed::Durable),
        Ok(AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            record_durability_warning(store, &error);
            Some(Committed::NotDurable)
        }
        Err(error) => {
            record_json_save_failure(store, &error);
            None
        }
    }
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
    /// Why a fail-closed load was rejected (read failure or decode error),
    /// kept so strict stores can say what to fix instead of only "corrupt".
    detail: Option<String>,
}

impl StoreGate {
    pub const fn missing() -> Self {
        Self {
            status: LoadStatus::Missing,
            generation: 0,
            expected: ExpectedRevision::Missing,
            recovered_source: None,
            detail: None,
        }
    }

    pub const fn status(&self) -> LoadStatus {
        self.status
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    pub(crate) fn block(&mut self, status: LoadStatus) {
        debug_assert!(status.fails_closed());
        self.status = status;
    }

    pub(crate) const fn mark_recovered(&mut self) {
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

fn loaded_without_value(
    status: LoadStatus,
    expected: ExpectedRevision,
    detail: impl Into<String>,
) -> LoadedRaw {
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
                detail: Some(detail.into()),
            }
        },
        source_bytes: None,
    }
}

fn load_raw_envelope(persist: &dyn Persist, path: &Path, spec: StoreSpec) -> LoadedRaw {
    let (bytes, revision) = match persist.read(path, spec.max_bytes) {
        Ok(ReadOutcome::Missing) => {
            return loaded_without_value(LoadStatus::Missing, ExpectedRevision::Missing, "");
        }
        Ok(ReadOutcome::Present { bytes, revision }) => (bytes, revision),
        Err(failure) => {
            return loaded_without_value(
                LoadStatus::Unreadable,
                ExpectedRevision::Any,
                failure.to_string(),
            );
        }
    };
    let expected = ExpectedRevision::Exact(revision);
    let corrupt =
        |expected, detail: &str| loaded_without_value(LoadStatus::Corrupt, expected, detail);
    let mut root = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(root) => root,
        Err(error) => return corrupt(expected, &error.to_string()),
    };
    let Some(object) = root.as_object_mut() else {
        return corrupt(expected, "top-level value is not an object");
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
                detail: None,
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
        return corrupt(expected, "envelope names a different format or store");
    }
    let Some(schema) = schema else {
        return corrupt(expected, "envelope has no valid schema");
    };
    if schema > spec.schema {
        return loaded_without_value(
            LoadStatus::FutureVersion,
            expected,
            format!("envelope schema {schema}, supported {}", spec.schema),
        );
    }
    if schema != spec.schema {
        return corrupt(expected, &format!("unsupported envelope schema {schema}"));
    }
    // Take the payload instead of cloning it: the journal can be tens of MB
    // and is reloaded on every mutation.
    let (Some(generation), Some(payload)) = (generation, object.remove("payload")) else {
        return corrupt(expected, "envelope has no generation or payload");
    };
    LoadedRaw {
        value: Some(payload),
        gate: StoreGate {
            status: LoadStatus::Current,
            generation,
            expected,
            recovered_source: None,
            detail: None,
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
        Err(error) => LoadedJson {
            value: None,
            gate: StoreGate {
                status: LoadStatus::Corrupt,
                detail: Some(format!("payload did not decode: {error}")),
                ..raw.gate
            },
        },
    }
}

/// Load a store that must never fall back to defaults (operation journal,
/// content index). `Ok(None)` only when the store is missing; every
/// fail-closed status becomes an error naming the store. `noun` is the
/// store's sentence-case name, e.g. "Operation journal". Errors name the
/// file (after a `(file ` marker) so the user knows what to inspect or move
/// aside, followed by the read or decode failure.
pub fn load_strict<T: DeserializeOwned>(
    persist: &dyn Persist,
    path: &Path,
    spec: StoreSpec,
    noun: &str,
) -> Result<(Option<T>, StoreGate), String> {
    let loaded = load_enveloped::<T>(persist, path, spec);
    let file = path.display();
    let detail = loaded
        .gate
        .detail()
        .unwrap_or("envelope or payload rejected");
    match loaded.gate.status() {
        LoadStatus::Missing => Ok((None, loaded.gate)),
        LoadStatus::Legacy | LoadStatus::Current | LoadStatus::Recovered => match loaded.value {
            Some(value) => Ok((Some(value), loaded.gate)),
            None => Err(format!(
                "{noun} is corrupt (file {file}): payload did not decode"
            )),
        },
        LoadStatus::Corrupt => Err(format!("{noun} is corrupt (file {file}): {detail}")),
        LoadStatus::FutureVersion => Err(format!(
            "{noun} uses a newer persist envelope than this build supports (file {file}): {detail}"
        )),
        LoadStatus::Unreadable => Err(format!(
            "Could not open {}: unreadable (file {file}): {detail}",
            noun.to_lowercase()
        )),
    }
}

/// A strict store's commit must be durable: a replaced target whose
/// directory was not synced is ambiguous across a crash.
pub fn require_durable(noun: &str, outcome: AtomicWriteOutcome) -> Result<(), String> {
    match outcome {
        AtomicWriteOutcome::Durable => Ok(()),
        AtomicWriteOutcome::CommittedButNotDurable(error) => Err(format!(
            "{noun} commit is ambiguous across a crash: target was replaced but its directory could not be synced: {error}"
        )),
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
    // A store the loader would reject as too large must never be committed:
    // it would replace readable data with a file that fails closed forever.
    if bytes.len() > spec.max_bytes {
        return Err(JsonSaveError::PreCommit(too_large_error(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            spec.max_bytes,
        )));
    }
    preserve_recovered_source(persist, path, spec.store, spec.max_bytes, gate)?;
    let outcome = persist
        .commit(path, &bytes, gate.expected.clone())
        .map_err(JsonSaveError::PreCommit)?;
    gate.status = LoadStatus::Current;
    gate.generation = generation;
    gate.expected = ExpectedRevision::Exact(Revision::from_bytes(&bytes));
    Ok(outcome)
}

fn too_large_error(size: u64, max_bytes: usize) -> PreCommitError {
    PreCommitError::Io {
        stage: SaveStage::Write,
        source: io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("serialized store reached {size} bytes; the load limit is {max_bytes}"),
        ),
        cleanup: None,
    }
}

/// Save an enveloped document through a streaming writer so large payloads
/// (content index) never require a full in-memory `to_vec` of the envelope.
/// The output is compact JSON (the store is machine-only) and is aborted
/// before the rename once it exceeds `spec.max_bytes`.
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
    preserve_recovered_source(persist, path, spec.store, spec.max_bytes, gate)?;
    let mut written = 0u64;
    let mut hasher = blake3::Hasher::new();
    let expected = gate.expected.clone();
    let outcome = persist
        .commit_with_writer(path, expected, |writer| {
            // serde_json emits many tiny writes; batch them before the file.
            let mut buffered = io::BufWriter::with_capacity(256 * 1024, writer);
            let mut hashing = HashingWriter {
                inner: &mut buffered,
                hasher: &mut hasher,
                written: &mut written,
                limit: u64::try_from(spec.max_bytes).unwrap_or(u64::MAX),
            };
            let envelope = Envelope {
                format: spec.format,
                store: spec.store,
                schema: spec.schema,
                generation,
                payload: value,
            };
            serde_json::to_writer(&mut hashing, &envelope).map_err(|error| {
                if error.is_io() {
                    PreCommitError::Io {
                        stage: SaveStage::Write,
                        source: error.into(),
                        cleanup: None,
                    }
                } else {
                    PreCommitError::Serialize(error)
                }
            })?;
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
    limit: u64,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let size = (*self.written).saturating_add(buf.len() as u64);
        if size > self.limit {
            let limit = usize::try_from(self.limit).unwrap_or(usize::MAX);
            let PreCommitError::Io { source, .. } = too_large_error(size, limit) else {
                unreachable!("too_large_error is an I/O error");
            };
            return Err(source);
        }
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
    store: &'static str,
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
        AtomicWriteOutcome::CommittedButNotDurable(error) => {
            let verified = matches!(
                persist.read(&quarantine, max_bytes),
                Ok(ReadOutcome::Present { bytes, .. }) if bytes == *source
            );
            if !verified {
                return Err(JsonSaveError::RecoveryPreservation);
            }
            record_durability_warning(store, &error);
            gate.detail = Some(format!(
                "Recovered source was preserved at {}, but its directory sync was not durable: {error}",
                quarantine.display()
            ));
            gate.recovered_source = None;
            Ok(())
        }
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
///
/// A lenient load turns an unreadable or partly invalid file into an empty or
/// trimmed store, so before replacing a file that does not fully decode as `T`
/// it is moved aside to a `.<name>.unreadable-<ms>.json` sibling; the save is
/// refused when that move fails, so the original is never silently lost.
pub fn save_item_store<T: Serialize + DeserializeOwned>(
    path: &Path,
    store: &'static str,
    value: &T,
) -> bool {
    if let Err(source) = set_aside_undecodable::<T>(path) {
        record_save_failure(
            store,
            &PreCommitError::Io {
                stage: SaveStage::ValidatePath,
                source,
                cleanup: None,
            },
        );
        return false;
    }
    match save_json_atomic(path, value) {
        Ok(_) => true,
        Err(error) => {
            record_save_failure(store, &error);
            false
        }
    }
}

fn lossy_loads() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static LOSSY: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    LOSSY.get_or_init(Mutex::default)
}

/// Remember that the item store at `path` loaded lossily (unreadable, or with
/// items dropped while decoding or normalizing), so the next
/// [`save_item_store`] keeps the original file aside even if it would decode
/// cleanly by then, e.g. after a transient read error.
pub fn mark_lossy_load(path: &Path) {
    crate::lock_util::recover(lossy_loads()).insert(path.to_path_buf());
}

fn set_aside_undecodable<T: DeserializeOwned>(path: &Path) -> io::Result<()> {
    let lossy = crate::lock_util::recover(lossy_loads()).contains(path);
    let intact = match read_target(path) {
        Ok(None) => {
            crate::lock_util::recover(lossy_loads()).remove(path);
            return Ok(());
        }
        Ok(Some(json)) => !lossy && serde_json::from_str::<T>(&json).is_ok(),
        Err(_) => false,
    };
    if intact {
        return Ok(());
    }
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "store path has no file name")
    })?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let mut backup = OsString::from(".");
    backup.push(file_name);
    backup.push(format!(".unreadable-{millis}.json"));
    std::fs::rename(path, path.with_file_name(backup))?;
    crate::lock_util::recover(lossy_loads()).remove(path);
    Ok(())
}

pub fn commit_bytes_atomic(
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
    commit_temp_with(path, fs, |temp| {
        fs.write_all(temp, bytes)
            .map_err(|source| FillError::Stage(SaveStage::Write, source))
    })
}

fn write_stream_atomic_with<F>(
    path: &Path,
    write: F,
    fs: &impl FsOps,
) -> Result<AtomicWriteOutcome, PreCommitError>
where
    F: FnOnce(&mut dyn Write) -> Result<(), PreCommitError>,
{
    commit_temp_with(path, fs, |temp| write(temp).map_err(FillError::Caller))
}

/// How filling the temp file failed: an I/O stage of ours (cleanup is
/// recorded on the error) or the caller's own error (temp removed quietly).
enum FillError {
    Stage(SaveStage, io::Error),
    Caller(PreCommitError),
}

/// The one atomic commit pipeline: unique temp beside `path`, fill, flush,
/// fsync, rename over `path`, then fsync the directory.
fn commit_temp_with(
    path: &Path,
    fs: &impl FsOps,
    fill: impl FnOnce(&mut File) -> Result<(), FillError>,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    let (parent, prefix) = store_location(path)?;
    fs.create_dir_all(parent)
        .map_err(|source| precommit_io(SaveStage::CreateDirectory, source, None))?;
    let (temp_path, mut temp) = create_unique_temp(fs, parent, &prefix)?;

    let prepare = fill(&mut temp)
        .and_then(|()| {
            fs.flush(&mut temp)
                .map_err(|source| FillError::Stage(SaveStage::Flush, source))
        })
        .and_then(|()| {
            fs.sync_file(&temp)
                .map_err(|source| FillError::Stage(SaveStage::SyncFile, source))
        });
    drop(temp);
    match prepare {
        Ok(()) => {}
        Err(FillError::Stage(stage, source)) => {
            return Err(precommit_with_cleanup(fs, &temp_path, stage, source));
        }
        Err(FillError::Caller(error)) => {
            let _ = fs.remove_file(&temp_path);
            return Err(error);
        }
    }

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

const fn precommit_io(
    stage: SaveStage,
    source: io::Error,
    cleanup: Option<io::Error>,
) -> PreCommitError {
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
        #[cfg(test)]
        THREAD_ISSUES.with(|issues| issues.borrow_mut().push(issue.clone()));
    }
}

// Issues published by the current thread, so a test can see what its own
// call reported even while parallel tests overwrite the shared `last_issue`.
#[cfg(test)]
thread_local! {
    static THREAD_ISSUES: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn take_thread_issues() -> Vec<String> {
    THREAD_ISSUES.with(|issues| std::mem::take(&mut *issues.borrow_mut()))
}

pub fn record_recovery(store: &'static str, recovered: usize, rejected: usize) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadIssue {
    /// The file could not be read; the store runs on defaults.
    Unreadable,
    /// The file was read but is malformed; the store runs on defaults.
    Malformed,
    /// The store fails closed: nothing is replaced until the file is fixed.
    Blocked,
}

fn load_issue_text(store: &str, issue: LoadIssue) -> String {
    match issue {
        LoadIssue::Unreadable => {
            format!("{store}: settings could not be read; defaults are active")
        }
        LoadIssue::Malformed => format!("{store}: file is malformed; defaults are active"),
        LoadIssue::Blocked => {
            format!("{store}: could not be read; changes are blocked to protect the file")
        }
    }
}

fn record_load_issue(store: &'static str, issue: LoadIssue) {
    let mut health = crate::lock_util::recover(health_state());
    health.unreadable_stores = health.unreadable_stores.saturating_add(1);
    health.last_issue = Some(load_issue_text(store, issue));
    publish_issue(&mut health);
}

pub fn record_unreadable(store: &'static str) {
    record_load_issue(store, LoadIssue::Unreadable);
}

/// The store's file was read but did not decode; defaults are active.
pub fn record_corrupt(store: &'static str) {
    record_load_issue(store, LoadIssue::Malformed);
}

/// A fail-closed store could not be loaded, so saves are refused rather
/// than defaults being used.
pub fn record_blocked(store: &'static str) {
    record_load_issue(store, LoadIssue::Blocked);
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

/// Data a store no longer references could not be removed; it stays on
/// disk (outside quota accounting) until removed by hand.
pub fn record_cleanup_failure(store: &'static str, source: &io::Error) {
    let mut health = crate::lock_util::recover(health_state());
    health.cleanup_failures = health.cleanup_failures.saturating_add(1);
    health.last_issue = Some(format!(
        "{store}: could not remove expired data ({:?}); it remains on disk",
        source.kind()
    ));
    publish_issue(&mut health);
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
            mark_lossy_load(path);
            return Vec::new();
        }
    };
    match decode_item_store(&json) {
        Ok(decoded) => {
            if decoded.rejected > 0 {
                record_recovery(store, decoded.items.len(), decoded.rejected);
                mark_lossy_load(path);
            }
            decoded.items
        }
        Err(_) => {
            record_corrupt(store);
            mark_lossy_load(path);
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
    fn load_issues_describe_unreadable_malformed_and_blocked_stores() {
        assert_eq!(
            load_issue_text("Smart folders", LoadIssue::Unreadable),
            "Smart folders: settings could not be read; defaults are active"
        );
        assert_eq!(
            load_issue_text("Smart folders", LoadIssue::Malformed),
            "Smart folders: file is malformed; defaults are active"
        );
        let blocked = load_issue_text("Version manifest", LoadIssue::Blocked);
        assert!(blocked.contains("changes are blocked"), "{blocked}");
        assert!(!blocked.contains("defaults are active"), "{blocked}");
    }

    #[test]
    fn item_store_load_reports_malformed_and_unreadable_files_distinctly() {
        let temp = TempDir::new();
        let malformed = temp.file("items.json", "not json");
        take_thread_issues();
        assert!(load_item_store::<Item>(&malformed, "Smart folders").is_empty());
        assert_eq!(
            take_thread_issues(),
            vec!["Smart folders: file is malformed; defaults are active".to_string()]
        );

        // Bytes that are not UTF-8 cannot be read as text.
        let unreadable = temp.path().join("binary.json");
        std::fs::write(&unreadable, [0xff, 0xfe, 0x00]).unwrap();
        assert!(load_item_store::<Item>(&unreadable, "Smart folders").is_empty());
        assert_eq!(
            take_thread_issues(),
            vec!["Smart folders: settings could not be read; defaults are active".to_string()]
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
    fn load_strict_maps_every_fail_closed_status_to_a_named_error() {
        let path = Path::new("strict.json");
        let current = br#"{"format":"commander.persist","store":"commander.test","schema":1,"generation":3,"payload":{"name":"x","count":1}}"#;
        let (value, gate) = load_strict::<Item>(
            &MemoryPersist::new(Some(current.to_vec())),
            path,
            TEST_STORE,
            "Test store",
        )
        .unwrap();
        assert_eq!(value.map(|item| item.name), Some("x".to_string()));
        assert_eq!(gate.status(), LoadStatus::Current);

        let (missing, gate) =
            load_strict::<Item>(&MemoryPersist::new(None), path, TEST_STORE, "Test store").unwrap();
        assert!(missing.is_none());
        assert_eq!(gate.status(), LoadStatus::Missing);

        let cases: [(Vec<u8>, &str, &str); 4] = [
            (
                b"not json".to_vec(),
                "Test store is corrupt",
                "expected ident at line 1",
            ),
            (
                br#"{"format":"commander.persist","store":"commander.test","schema":1,"generation":1,"payload":{"name":"x","count":"many"}}"#
                    .to_vec(),
                "Test store is corrupt",
                "payload did not decode: invalid type: string",
            ),
            (
                br#"{"format":"commander.persist","store":"commander.test","schema":2,"generation":1,"payload":{}}"#
                    .to_vec(),
                "Test store uses a newer persist envelope",
                "envelope schema 2, supported 1",
            ),
            (
                vec![b' '; TEST_STORE.max_bytes + 1],
                "Could not open test store",
                "TooLarge",
            ),
        ];
        for (bytes, expected, detail) in cases {
            let error = load_strict::<Item>(
                &MemoryPersist::new(Some(bytes)),
                path,
                TEST_STORE,
                "Test store",
            )
            .unwrap_err();
            assert!(error.starts_with(expected), "{error}");
            assert!(error.contains("(file strict.json)"), "{error}");
            assert!(error.contains(detail), "{error}");
        }
    }

    #[test]
    fn oversized_envelope_is_refused_before_the_port_is_called() {
        let persist = MemoryPersist::new(None);
        let mut gate = StoreGate::missing();
        let result = save_enveloped(
            &persist,
            Path::new("memory.json"),
            TEST_STORE,
            &Item {
                name: "x".repeat(TEST_STORE.max_bytes),
                count: 1,
            },
            &mut gate,
            SaveIntent::Explicit,
        );
        let Err(JsonSaveError::PreCommit(PreCommitError::Io { stage, source, .. })) = result else {
            panic!("an oversized store must not be committed: {result:?}");
        };
        assert_eq!(stage, SaveStage::Write);
        assert_eq!(source.kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(persist.commits.load(Ordering::Relaxed), 0);
        assert_eq!(gate.status(), LoadStatus::Missing);
    }

    #[test]
    fn streaming_save_round_trips_compact_and_refuses_an_oversized_store() {
        let temp = TempDir::new();
        let path = temp.path().join("index.json");
        let persist = FsPersist::default();
        let small = Item {
            name: "small".to_string(),
            count: 1,
        };
        let mut gate = StoreGate::missing();
        save_enveloped_streaming(
            &persist,
            &path,
            TEST_STORE,
            &small,
            &mut gate,
            SaveIntent::Explicit,
        )
        .unwrap();
        let committed = std::fs::read(&path).unwrap();
        assert!(!committed.contains(&b'\n'), "machine store is compact");
        let loaded = load_enveloped::<Item>(&persist, &path, TEST_STORE);
        assert_eq!(loaded.value, Some(small));
        assert_eq!(loaded.gate.expected, gate.expected);

        let result = save_enveloped_streaming(
            &persist,
            &path,
            TEST_STORE,
            &Item {
                name: "x".repeat(TEST_STORE.max_bytes),
                count: 2,
            },
            &mut gate,
            SaveIntent::Explicit,
        );
        let Err(JsonSaveError::PreCommit(PreCommitError::Io { stage, source, .. })) = result else {
            panic!("an oversized stream must abort before the rename: {result:?}");
        };
        assert_eq!(stage, SaveStage::Write);
        assert_eq!(source.kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        assert_eq!(non_lock_entry_count(temp.path()), 1, "temp file removed");
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
            detail: None,
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

        assert!(matches!(
            save_enveloped(
                &persist,
                &path,
                TEST_STORE,
                &decoded.items,
                &mut loaded.gate,
                SaveIntent::Automatic,
            ),
            Err(JsonSaveError::Blocked(LoadStatus::Recovered))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), original);

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
                candidate.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.contains(".recovered-") && !name.contains(".persist.lock")
                })
            })
            .unwrap();
        assert_eq!(std::fs::read(quarantine).unwrap(), original);
        assert_eq!(loaded.gate.status(), LoadStatus::Current);
    }

    #[test]
    fn identical_recovered_quarantine_survives_a_directory_sync_warning() {
        let source = br#"{"damaged":true}"#.to_vec();
        let persist = MemoryPersist::not_durable(None);
        let mut gate = StoreGate {
            status: LoadStatus::Recovered,
            generation: 0,
            expected: ExpectedRevision::Missing,
            recovered_source: Some(source.clone()),
            detail: None,
        };

        preserve_recovered_source(
            &persist,
            Path::new("state.json"),
            TEST_STORE.store,
            TEST_STORE.max_bytes,
            &mut gate,
        )
        .unwrap();

        assert_eq!(persist.bytes(), Some(source));
        assert_eq!(persist.commits.load(Ordering::Relaxed), 1);
        assert_eq!(persist.reads.load(Ordering::Relaxed), 2);
        assert!(gate.recovered_source.is_none());
        assert!(
            gate.detail()
                .is_some_and(|detail| detail.contains("directory sync was not durable"))
        );
    }

    #[test]
    fn global_path_lock_registry_reclaims_unused_entries() {
        // Other tests share this process-global registry, so assert on this
        // test's own keys rather than on the total size.
        let transient = |index: usize| format!("registry-reclaim-transient-{index}");
        for index in 0..512 {
            drop(path_lock(Path::new(&transient(index))));
        }
        let survivor = path_lock(Path::new("registry-reclaim-survivor"));
        let locks = crate::lock_util::recover(path_locks());
        assert!(
            (0..512)
                .all(|index| !locks
                    .contains_key(&normalized_lock_path(Path::new(&transient(index))))),
            "dropped locks must be reclaimed"
        );
        assert!(locks.contains_key(&normalized_lock_path(Path::new(
            "registry-reclaim-survivor"
        ))));
        drop(locks);
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
        assert_eq!(non_lock_entry_count(temp.path()), 1);
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
        assert_eq!(non_lock_entry_count(temp.path()), 1);

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
    fn first_commit_removes_temps_orphaned_by_another_process() {
        let temp = TempDir::new();
        let path = temp.path().join("state.json");
        let orphan = temp.file(
            ".state.json.commander-json-4194305-00000000000000000007.tmp",
            "partial",
        );
        let own = temp.file(
            &format!(
                ".state.json.commander-json-{}-99999999999999999999.tmp",
                std::process::id()
            ),
            "ours",
        );
        let unrelated = temp.file(".state.json.commander-json-notes.tmp", "keep");
        let other_store = temp.file(
            ".other.json.commander-json-4194305-00000000000000000007.tmp",
            "keep",
        );

        FsPersist::default()
            .commit(&path, b"{}", ExpectedRevision::Any)
            .unwrap();

        assert!(!orphan.exists());
        assert!(own.exists());
        assert!(unrelated.exists());
        assert!(other_store.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
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
    fn reading_a_store_in_an_absent_directory_is_missing_and_creates_nothing() {
        let temp = TempDir::new();
        let parent = temp.path().join("absent").join("config");
        let outcome = FsPersist::default()
            .read(&parent.join("state.json"), 4096)
            .unwrap();
        assert!(matches!(outcome, ReadOutcome::Missing));
        assert!(!temp.path().join("absent").exists());
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
        assert_eq!(non_lock_entry_count(temp.path()), 1);
    }

    fn non_lock_entry_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| {
                !entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".persist.lock")
            })
            .count()
    }
}
