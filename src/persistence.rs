//! Shared JSON loading, atomic session writes and privacy-safe diagnostics.

use serde::{Serialize, de::DeserializeOwned};
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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
/// target. This is deliberately single-process and last-writer-wins: it has no
/// lock, compare-and-swap or multi-process merge semantics.
pub fn save_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    save_json_atomic_with(path, value, &StdFsOps)
}

fn save_json_atomic_with<T: Serialize>(
    path: &Path,
    value: &T,
    fs: &impl FsOps,
) -> Result<AtomicWriteOutcome, PreCommitError> {
    // Serialization must finish before the first filesystem operation.
    let bytes = serde_json::to_vec_pretty(value).map_err(PreCommitError::Serialize)?;
    let (parent, prefix) = store_location(path)?;
    fs.create_dir_all(parent)
        .map_err(|source| precommit_io(SaveStage::CreateDirectory, source, None))?;
    let (temp_path, mut temp) = create_unique_temp(fs, parent, &prefix)?;

    let prepare = fs
        .write_all(&mut temp, &bytes)
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
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LoadFailure::Io),
    };
    if metadata.file_type().is_symlink() {
        return Err(LoadFailure::Symlink);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|_| LoadFailure::Io)
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

fn record_recovery(store: &'static str, recovered: usize, rejected: usize) {
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

fn record_unreadable(store: &'static str) {
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

        assert!(load_json::<Item>(&link, "Test").is_none());
    }
}
