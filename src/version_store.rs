//! Local verified versions for the `Versioned` durability profile.

use crate::operation::{IdempotencyKey, OperationId, VersionRetentionPolicy};
use crate::path_identity::PathIdentity;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
thread_local! {
    static TEST_VERSIONS_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct TestVersionsDirGuard {
    previous: Option<PathBuf>,
}

#[cfg(test)]
impl Drop for TestVersionsDirGuard {
    fn drop(&mut self) {
        TEST_VERSIONS_DIR.with(|directory| {
            *directory.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn use_test_versions_dir(path: PathBuf) -> TestVersionsDirGuard {
    let previous = TEST_VERSIONS_DIR.with(|directory| directory.borrow_mut().replace(path));
    TestVersionsDirGuard { previous }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRecord {
    pub operation_id: OperationId,
    pub key: IdempotencyKey,
    pub original: PathBuf,
    pub stored: PathBuf,
    pub created_at_secs: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct VersionManifest {
    records: Vec<VersionRecord>,
}

#[cfg(test)]
pub fn preserve_with_policy(
    path: &Path,
    operation_id: &OperationId,
    key: IdempotencyKey,
    retention: VersionRetentionPolicy,
) -> Result<Option<VersionRecord>, String> {
    preserve_at(&versions_dir(), path, operation_id, key, retention)
}

pub fn preserve_expected_with_policy(
    path: &Path,
    expected: &PathIdentity,
    operation_id: &OperationId,
    key: IdempotencyKey,
    retention: VersionRetentionPolicy,
) -> Result<Option<VersionRecord>, crate::ports::NativeFailure> {
    preserve_expected_at(
        &versions_dir(),
        path,
        expected,
        operation_id,
        key,
        retention,
    )
}

#[cfg(test)]
fn preserve_at(
    root: &Path,
    path: &Path,
    operation_id: &OperationId,
    key: IdempotencyKey,
    retention: VersionRetentionPolicy,
) -> Result<Option<VersionRecord>, String> {
    preserve_at_inner(root, path, None, operation_id, key, retention)
        .map_err(|failure| failure.message)
}

pub(crate) fn preserve_expected_at(
    root: &Path,
    path: &Path,
    expected: &PathIdentity,
    operation_id: &OperationId,
    key: IdempotencyKey,
    retention: VersionRetentionPolicy,
) -> Result<Option<VersionRecord>, crate::ports::NativeFailure> {
    preserve_at_inner(root, path, Some(expected), operation_id, key, retention)
}

fn preserve_at_inner(
    root: &Path,
    path: &Path,
    expected: Option<&PathIdentity>,
    operation_id: &OperationId,
    key: IdempotencyKey,
    retention: VersionRetentionPolicy,
) -> Result<Option<VersionRecord>, crate::ports::NativeFailure> {
    let before = PathIdentity::observe(path).map_err(|error| {
        let mut failure = crate::ports::NativeFailure::from_io(&error);
        failure.message = format!(
            "Could not inspect {} for versioning: {}",
            path.display(),
            failure.message
        );
        failure
    })?;
    if expected.is_some_and(|expected| !expected.same_shallow_binding(&before)) {
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Stale,
            message: format!(
                "{} changed before its version could be preserved",
                path.display()
            ),
        });
    }
    if !before.exists {
        return Ok(None);
    }
    let stored = record_path(root, operation_id, &key, path);
    if let Some(parent) = stored.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            version_failure(format!(
                "Could not create version directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    if let Err(error) = copy_path(path, &stored) {
        let _ = remove_path(&stored);
        return Err(version_failure(format!(
            "Could not preserve {} at {}: {error}",
            path.display(),
            stored.display()
        )));
    }
    let after = PathIdentity::observe(path).map_err(|error| {
        let mut failure = crate::ports::NativeFailure::from_io(&error);
        failure.message = format!(
            "Could not recheck {} after versioning: {}",
            path.display(),
            failure.message
        );
        failure
    })?;
    if !before.same_version(&after) || !paths_equal(path, &stored) {
        let _ = remove_path(&stored);
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Stale,
            message: format!(
                "Source changed or version verification failed for {}",
                path.display()
            ),
        });
    }

    let record = VersionRecord {
        operation_id: operation_id.clone(),
        key,
        original: path.to_path_buf(),
        stored: stored.clone(),
        created_at_secs: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
    };
    let mut manifest = load_manifest(root);
    manifest
        .records
        .retain(|existing| existing.key != record.key);
    manifest.records.push(record.clone());
    if !save_manifest(root, &manifest) {
        let _ = remove_path(&stored);
        return Err(version_failure("Could not save version manifest"));
    }
    prune_manifest(root, &manifest, retention, record.created_at_secs);
    Ok(Some(record))
}

fn version_failure(message: impl Into<String>) -> crate::ports::NativeFailure {
    crate::ports::NativeFailure {
        kind: crate::ports::NativeFailureKind::Unknown,
        message: message.into(),
    }
}

#[cfg_attr(test, allow(dead_code))]
pub fn discard_record(record: &VersionRecord) -> Result<(), String> {
    discard_record_at(&versions_dir(), record)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn discard_record_at(root: &Path, record: &VersionRecord) -> Result<(), String> {
    let mut manifest = load_manifest(root);
    let previous_len = manifest.records.len();
    manifest
        .records
        .retain(|candidate| candidate.key != record.key);
    if manifest.records.len() == previous_len {
        return Ok(());
    }
    if !save_manifest(root, &manifest) {
        return Err("Could not remove rejected version from the manifest".to_string());
    }
    remove_path(&record.stored).map_err(|error| {
        format!(
            "Could not remove rejected version {}: {error}",
            record.stored.display()
        )
    })
}

fn retained_records(
    records: &[VersionRecord],
    policy: VersionRetentionPolicy,
    now_secs: u64,
) -> (Vec<VersionRecord>, Vec<VersionRecord>) {
    if policy == VersionRetentionPolicy::Forever {
        return (records.to_vec(), Vec::new());
    }

    let mut by_path = std::collections::HashMap::<&Path, Vec<(usize, &VersionRecord)>>::new();
    for (index, record) in records.iter().enumerate() {
        by_path
            .entry(record.original.as_path())
            .or_default()
            .push((index, record));
    }
    let mut keep = vec![false; records.len()];
    for versions in by_path.values_mut() {
        versions.sort_by(|(left_index, left), (right_index, right)| {
            right
                .created_at_secs
                .cmp(&left.created_at_secs)
                .then(right_index.cmp(left_index))
        });
        for (rank, (index, record)) in versions.iter().enumerate() {
            let newest = rank == 0;
            let within_count = policy.max_per_path().is_none_or(|max| rank < max);
            let within_age = policy
                .max_age_secs()
                .is_none_or(|max_age| now_secs.saturating_sub(record.created_at_secs) <= max_age);
            keep[*index] = newest || (within_count && within_age);
        }
    }
    let (retained, expired) = records
        .iter()
        .cloned()
        .enumerate()
        .partition::<Vec<_>, _>(|(index, _)| keep[*index]);
    let retained = retained.into_iter().map(|(_, record)| record).collect();
    let expired = expired.into_iter().map(|(_, record)| record).collect();
    (retained, expired)
}

fn prune_manifest(
    root: &Path,
    manifest: &VersionManifest,
    policy: VersionRetentionPolicy,
    now_secs: u64,
) {
    let (retained, expired) = retained_records(&manifest.records, policy, now_secs);
    if expired.is_empty() {
        return;
    }
    let pruned = VersionManifest { records: retained };
    // Publish the manifest before deleting data. A failed policy update keeps
    // extra recovery data; it never leaves a manifest pointing at a deleted
    // version.
    if !save_manifest(root, &pruned) {
        return;
    }
    for record in expired {
        let _ = remove_path(&record.stored);
        remove_empty_version_dirs(record.stored.parent(), root);
    }
}

fn remove_empty_version_dirs(mut directory: Option<&Path>, root: &Path) {
    while let Some(path) = directory {
        if path == root || !path.starts_with(root) || fs::remove_dir(path).is_err() {
            break;
        }
        directory = path.parent();
    }
}

pub fn restore(record: &VersionRecord) -> Result<(), String> {
    if crate::fs_util::path_is_taken(&record.original) {
        return Err(format!(
            "Restore destination is occupied: {}",
            record.original.display()
        ));
    }
    copy_path(&record.stored, &record.original)
        .map_err(|error| format!("Could not restore {}: {error}", record.original.display()))?;
    if !paths_equal(&record.stored, &record.original) {
        let _ = remove_path(&record.original);
        return Err(format!(
            "Restored version failed verification: {}",
            record.original.display()
        ));
    }
    Ok(())
}

pub fn records_for(operation_id: &OperationId) -> Vec<VersionRecord> {
    load_manifest(&versions_dir())
        .records
        .into_iter()
        .filter(|record| &record.operation_id == operation_id)
        .collect()
}

pub fn records() -> Vec<VersionRecord> {
    load_manifest(&versions_dir()).records
}

pub fn record_for_key(key: &IdempotencyKey) -> Option<VersionRecord> {
    load_manifest(&versions_dir())
        .records
        .into_iter()
        .find(|record| &record.key == key)
}

fn versions_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_VERSIONS_DIR.with(|directory| directory.borrow().clone()) {
        return path;
    }
    crate::fs_util::config_dir().join("versions")
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join("manifest.json")
}

fn record_path(
    root: &Path,
    operation_id: &OperationId,
    key: &IdempotencyKey,
    original: &Path,
) -> PathBuf {
    let name = original
        .file_name()
        .map_or_else(|| "root".into(), |name| name.to_os_string());
    root.join(&operation_id.0).join(&key.0).join(name)
}

fn load_manifest(root: &Path) -> VersionManifest {
    fs::File::open(manifest_path(root))
        .ok()
        .and_then(|file| serde_json::from_reader(file).ok())
        .unwrap_or_default()
}

fn save_manifest(root: &Path, manifest: &VersionManifest) -> bool {
    let path = manifest_path(root);
    let _ = fs::create_dir_all(root);
    serde_json::to_string_pretty(manifest)
        .ok()
        .is_some_and(|json| crate::fs_util::write_atomic(&path, &json))
}

fn copy_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    enum Task {
        Copy(PathBuf, PathBuf),
        SetPermissions(PathBuf, fs::Permissions),
    }

    let mut tasks = vec![Task::Copy(source.to_path_buf(), destination.to_path_buf())];
    while let Some(task) = tasks.pop() {
        match task {
            Task::SetPermissions(path, permissions) => {
                fs::set_permissions(path, permissions)?;
            }
            Task::Copy(source, destination) => {
                let metadata = fs::symlink_metadata(&source)?;
                if metadata.file_type().is_symlink() {
                    let target = fs::read_link(&source)?;
                    create_symlink(&target, &destination, source.is_dir())?;
                    continue;
                }
                if metadata.is_dir() {
                    fs::create_dir(&destination)?;
                    tasks.push(Task::SetPermissions(
                        destination.clone(),
                        metadata.permissions(),
                    ));
                    let mut children = fs::read_dir(&source)?.collect::<Result<Vec<_>, _>>()?;
                    children.sort_by_key(|entry| entry.file_name());
                    for entry in children.into_iter().rev() {
                        tasks.push(Task::Copy(
                            entry.path(),
                            destination.join(entry.file_name()),
                        ));
                    }
                    continue;
                }
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                let mut input = fs::File::open(&source)?;
                let mut output = options.open(&destination)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                fs::set_permissions(destination, metadata.permissions())?;
            }
        }
    }
    Ok(())
}

pub fn paths_equal(left: &Path, right: &Path) -> bool {
    let mut tasks = vec![(left.to_path_buf(), right.to_path_buf())];
    while let Some((left, right)) = tasks.pop() {
        let Ok(left_meta) = fs::symlink_metadata(&left) else {
            return false;
        };
        let Ok(right_meta) = fs::symlink_metadata(&right) else {
            return false;
        };
        if left_meta.file_type().is_symlink() || right_meta.file_type().is_symlink() {
            if !left_meta.file_type().is_symlink()
                || !right_meta.file_type().is_symlink()
                || fs::read_link(&left).ok() != fs::read_link(&right).ok()
            {
                return false;
            }
            continue;
        }
        if left_meta.is_dir() != right_meta.is_dir() {
            return false;
        }
        if !left_meta.is_dir() {
            if !crate::fs_util::files_equal(&left, &right) {
                return false;
            }
            continue;
        }
        let Some(left_names) = directory_names(&left) else {
            return false;
        };
        let Some(right_names) = directory_names(&right) else {
            return false;
        };
        if left_names != right_names {
            return false;
        }
        for name in left_names.into_iter().rev() {
            tasks.push((left.join(&name), right.join(name)));
        }
    }
    true
}

fn directory_names(path: &Path) -> Option<Vec<std::ffi::OsString>> {
    let mut names = fs::read_dir(path)
        .ok()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    names.sort();
    Some(names)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path, _directory: bool) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path, directory: bool) -> std::io::Result<()> {
    if directory {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &Path, _destination: &Path, _directory: bool) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink versions are unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn copy_and_verify_handles_nested_directories() {
        let temp = TempDir::new();
        let source = temp.dir("source");
        temp.file("source/nested/file.txt", "versioned");
        let destination = temp.path().join("destination");
        copy_path(&source, &destination).unwrap();
        assert!(paths_equal(&source, &destination));
    }

    #[test]
    fn version_copy_and_verification_handle_deep_trees_iteratively() {
        let temp = TempDir::new();
        let source = temp.dir("source");
        let mut current = source.clone();
        for _ in 0..128 {
            current = current.join("d");
            fs::create_dir(&current).unwrap();
        }
        fs::write(current.join("payload.bin"), "payload").unwrap();
        let destination = temp.path().join("destination");

        copy_path(&source, &destination).unwrap();

        assert!(paths_equal(&source, &destination));
    }

    #[cfg(unix)]
    #[test]
    fn failed_version_copy_removes_partial_data_without_manifest_publication() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let source = temp.dir("source");
        temp.file("source/a.txt", "copied first");
        let socket = std::os::unix::net::UnixListener::bind(source.join("z.sock")).unwrap();
        let operation = OperationId::new();
        let key = operation.step_key(0, &source);
        let expected = PathIdentity::observe(&source).unwrap();
        let stored = record_path(&versions, &operation, &key, &source);

        let result = preserve_expected_at(
            &versions,
            &source,
            &expected,
            &operation,
            key,
            VersionRetentionPolicy::Recent,
        );

        drop(socket);
        assert!(result.is_err());
        assert!(!stored.exists());
        assert!(load_manifest(&versions).records.is_empty());
    }

    #[test]
    fn restore_refuses_to_clobber_an_occupied_original() {
        let temp = TempDir::new();
        let stored = temp.file("stored.txt", "old");
        let original = temp.file("original.txt", "new");
        let record = VersionRecord {
            operation_id: OperationId("test".to_string()),
            key: IdempotencyKey("step".to_string()),
            original,
            stored,
            created_at_secs: 0,
        };
        assert!(restore(&record).unwrap_err().contains("occupied"));
    }

    #[test]
    fn preserve_records_a_verified_restorable_version() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "before");
        let operation = OperationId("test-preserve".to_string());
        let record = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();

        std::fs::remove_file(&original).unwrap();
        restore(&record).unwrap();

        assert_eq!(std::fs::read_to_string(&original).unwrap(), "before");
        assert_eq!(load_manifest(&versions).records, vec![record]);
    }

    #[test]
    fn expected_binding_rejects_replacement_without_publishing_a_version() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "before");
        let expected = PathIdentity::observe(&original).unwrap();
        let replacement = temp.file("replacement.txt", "after");
        std::fs::rename(replacement, &original).unwrap();
        let operation = OperationId("test-stale-preserve".to_string());

        let failure = preserve_expected_at(
            &versions,
            &original,
            &expected,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        )
        .unwrap_err();

        assert_eq!(failure.kind, crate::ports::NativeFailureKind::Stale);
        assert!(load_manifest(&versions).records.is_empty());
        assert!(!versions.exists());
    }

    #[test]
    fn retention_keeps_newest_per_path_and_applies_age_and_count() {
        let record = |path: &str, sequence: u64, created_at_secs: u64| VersionRecord {
            operation_id: OperationId(format!("operation-{sequence}")),
            key: IdempotencyKey(format!("key-{sequence}")),
            original: PathBuf::from(path),
            stored: PathBuf::from(format!("stored-{sequence}")),
            created_at_secs,
        };
        let records = vec![
            record("a.txt", 1, 10),
            record("a.txt", 2, 20),
            record("a.txt", 3, 30),
            record("a.txt", 4, 40),
            record("b.txt", 5, 1),
        ];
        let (retained, expired) = retained_records(&records, VersionRetentionPolicy::Compact, 100);
        assert_eq!(
            retained
                .iter()
                .map(|record| record.key.0.as_str())
                .collect::<Vec<_>>(),
            vec!["key-2", "key-3", "key-4", "key-5"]
        );
        assert_eq!(expired.len(), 1);

        let old_now = VersionRetentionPolicy::Compact
            .max_age_secs()
            .unwrap()
            .saturating_add(1_000);
        let (retained, expired) =
            retained_records(&records, VersionRetentionPolicy::Compact, old_now);
        assert_eq!(
            retained.len(),
            2,
            "the newest record for each path survives"
        );
        assert_eq!(expired.len(), 3);
    }

    #[test]
    fn compact_policy_prunes_the_oldest_stored_copy_after_manifest_commit() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "version-0");
        let mut created = Vec::new();
        for index in 0..4 {
            std::fs::write(&original, format!("version-{index}")).unwrap();
            let operation = OperationId(format!("operation-{index}"));
            created.push(
                preserve_at(
                    &versions,
                    &original,
                    &operation,
                    operation.step_key(index, &original),
                    VersionRetentionPolicy::Compact,
                )
                .unwrap()
                .unwrap(),
            );
        }

        let manifest = load_manifest(&versions);
        assert_eq!(manifest.records.len(), 3);
        assert!(!created[0].stored.exists());
        assert!(created[1..].iter().all(|record| record.stored.exists()));
    }
}
