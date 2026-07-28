//! Local verified versions for the `Versioned` durability profile.

use crate::operation::{IdempotencyKey, OperationId, VersionRetentionPolicy};
use crate::path_identity::PathIdentity;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MANIFEST_STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.version_manifest", 1, 16 * 1024 * 1024);

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

struct LoadedManifest {
    manifest: VersionManifest,
    gate: crate::persistence::StoreGate,
    blocked: bool,
}

fn manifest_persist() -> &'static crate::persistence::FsPersist {
    static PERSIST: OnceLock<crate::persistence::FsPersist> = OnceLock::new();
    PERSIST.get_or_init(crate::persistence::FsPersist::default)
}

fn manifest_transaction() -> &'static Mutex<()> {
    static TRANSACTION: OnceLock<Mutex<()>> = OnceLock::new();
    TRANSACTION.get_or_init(|| Mutex::new(()))
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
    let persist = manifest_persist();
    let loaded = load_manifest_with(persist, root);
    if loaded.blocked {
        return Err(version_failure(
            "Version manifest is unreadable or incompatible; preservation is blocked",
        ));
    }
    let stored = record_path(root, operation_id, &key, path);
    if !lexically_descends_from(root, &stored) {
        return Err(version_failure(
            "Generated version path escaped the configured versions root",
        ));
    }
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
    if let Err(message) = validate_stored_path(root, &stored) {
        let _ = remove_path(&stored);
        return Err(version_failure(message));
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
    let _manifest_guard = crate::lock_util::recover(manifest_transaction());
    let mut loaded = load_manifest_with(persist, root);
    if loaded.blocked {
        let _ = remove_path(&stored);
        return Err(version_failure(
            "Version manifest changed to an unreadable or incompatible state",
        ));
    }
    loaded
        .manifest
        .records
        .retain(|existing| existing.key != record.key);
    loaded.manifest.records.push(record.clone());
    match save_manifest_with(persist, root, &loaded.manifest, &mut loaded.gate) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => {
            prune_manifest(
                persist,
                root,
                &loaded.manifest,
                &mut loaded.gate,
                retention,
                record.created_at_secs,
            );
        }
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Version manifest", &error);
        }
        Err(error) => {
            let _ = remove_path(&stored);
            crate::persistence::record_json_save_failure("Version manifest", &error);
            return Err(version_failure("Could not save version manifest"));
        }
    }
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
    let persist = manifest_persist();
    let _manifest_guard = crate::lock_util::recover(manifest_transaction());
    let mut loaded = load_manifest_with(persist, root);
    if loaded.blocked {
        return Err(
            "Version manifest is unreadable or incompatible; discard is blocked".to_string(),
        );
    }
    let Some(authoritative) = loaded
        .manifest
        .records
        .iter()
        .find(|candidate| candidate.key == record.key)
        .cloned()
    else {
        return Ok(());
    };
    if authoritative != *record {
        return Err("Version record changed before discard".to_string());
    }
    validate_stored_path(root, &authoritative.stored)?;
    loaded
        .manifest
        .records
        .retain(|candidate| candidate.key != record.key);
    match save_manifest_with(persist, root, &loaded.manifest, &mut loaded.gate) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => remove_path(&authoritative.stored)
            .map_err(|error| {
                format!(
                    "Could not remove rejected version {}: {error}",
                    authoritative.stored.display()
                )
            }),
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Version manifest", &error);
            Ok(())
        }
        Err(error) => {
            crate::persistence::record_json_save_failure("Version manifest", &error);
            Err("Could not remove rejected version from the manifest".to_string())
        }
    }
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
    persist: &dyn crate::persistence::Persist,
    root: &Path,
    manifest: &VersionManifest,
    gate: &mut crate::persistence::StoreGate,
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
    match save_manifest_with(persist, root, &pruned, gate) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => {}
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Version manifest", &error);
            return;
        }
        Err(error) => {
            crate::persistence::record_json_save_failure("Version manifest", &error);
            return;
        }
    }
    for record in expired {
        if validate_stored_path(root, &record.stored).is_err() {
            continue;
        }
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
    restore_at(&versions_dir(), record)
}

fn restore_at(root: &Path, record: &VersionRecord) -> Result<(), String> {
    if crate::fs_util::path_is_taken(&record.original) {
        return Err(format!(
            "Restore destination is occupied: {}",
            record.original.display()
        ));
    }
    let loaded = load_manifest_with(manifest_persist(), root);
    if loaded.blocked {
        return Err(
            "Version manifest is unreadable or incompatible; restore is blocked".to_string(),
        );
    }
    let Some(authoritative) = loaded
        .manifest
        .records
        .iter()
        .find(|candidate| *candidate == record)
    else {
        return Err("Version record is not present in the authoritative manifest".to_string());
    };
    validate_stored_path(root, &authoritative.stored)?;
    copy_path(&authoritative.stored, &authoritative.original)
        .map_err(|error| format!("Could not restore {}: {error}", record.original.display()))?;
    if !paths_equal(&authoritative.stored, &authoritative.original) {
        let _ = remove_path(&authoritative.original);
        return Err(format!(
            "Restored version failed verification: {}",
            authoritative.original.display()
        ));
    }
    Ok(())
}

pub fn records_for(operation_id: &OperationId) -> Vec<VersionRecord> {
    let root = versions_dir();
    let loaded = load_manifest_with(manifest_persist(), &root);
    if loaded.blocked {
        return Vec::new();
    }
    loaded
        .manifest
        .records
        .into_iter()
        .filter(|record| &record.operation_id == operation_id)
        .collect()
}

pub fn records() -> Vec<VersionRecord> {
    let root = versions_dir();
    let loaded = load_manifest_with(manifest_persist(), &root);
    if loaded.blocked {
        Vec::new()
    } else {
        loaded.manifest.records
    }
}

pub fn record_for_key(key: &IdempotencyKey) -> Option<VersionRecord> {
    let root = versions_dir();
    let loaded = load_manifest_with(manifest_persist(), &root);
    if loaded.blocked {
        return None;
    }
    loaded
        .manifest
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

fn normalized_absolute(path: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => {
                normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::Normal(component) => normalized.push(component),
        }
    }
    Some(normalized)
}

fn lexically_descends_from(root: &Path, candidate: &Path) -> bool {
    let (Some(root), Some(candidate)) = (normalized_absolute(root), normalized_absolute(candidate))
    else {
        return false;
    };
    candidate != root && candidate.starts_with(root)
}

fn validate_stored_path(root: &Path, stored: &Path) -> Result<(), String> {
    if !lexically_descends_from(root, stored) {
        return Err("Version record points outside the versions root".to_string());
    }
    let root_metadata =
        fs::symlink_metadata(root).map_err(|_| "Versions root is unavailable".to_string())?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("Versions root is not a trusted directory".to_string());
    }
    let root_canonical =
        fs::canonicalize(root).map_err(|_| "Versions root could not be resolved".to_string())?;
    let parent = stored
        .parent()
        .ok_or_else(|| "Version record has no parent directory".to_string())?;
    let parent_canonical =
        fs::canonicalize(parent).map_err(|_| "Stored version parent is unavailable".to_string())?;
    if !parent_canonical.starts_with(&root_canonical) {
        return Err("Stored version parent escapes through a symlink".to_string());
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "Stored version parent is outside the versions root".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| "Stored version ancestry is unavailable".to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Stored version ancestry is not a trusted directory".to_string());
        }
    }
    fs::symlink_metadata(stored).map_err(|_| "Stored version is unavailable".to_string())?;
    Ok(())
}

fn manifest_paths_are_valid(root: &Path, manifest: &VersionManifest) -> bool {
    let mut paths = std::collections::HashSet::new();
    manifest.records.iter().all(|record| {
        paths.insert(record.stored.clone()) && validate_stored_path(root, &record.stored).is_ok()
    })
}

fn load_manifest_with(persist: &dyn crate::persistence::Persist, root: &Path) -> LoadedManifest {
    let loaded = crate::persistence::load_enveloped::<VersionManifest>(
        persist,
        &manifest_path(root),
        MANIFEST_STORE,
    );
    let status = loaded.gate.status();
    let mut gate = loaded.gate;
    let mut blocked = matches!(
        status,
        crate::persistence::LoadStatus::Corrupt
            | crate::persistence::LoadStatus::FutureVersion
            | crate::persistence::LoadStatus::Unreadable
    );
    let manifest = loaded.value.unwrap_or_default();
    if !blocked
        && status != crate::persistence::LoadStatus::Missing
        && !manifest_paths_are_valid(root, &manifest)
    {
        gate.block(crate::persistence::LoadStatus::Corrupt);
        blocked = true;
    }
    if blocked {
        crate::persistence::record_unreadable("Version manifest");
    }
    LoadedManifest {
        manifest,
        gate,
        blocked,
    }
}

#[cfg(test)]
fn load_manifest(root: &Path) -> VersionManifest {
    load_manifest_with(manifest_persist(), root).manifest
}

fn save_manifest_with(
    persist: &dyn crate::persistence::Persist,
    root: &Path,
    manifest: &VersionManifest,
    gate: &mut crate::persistence::StoreGate,
) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::JsonSaveError> {
    crate::persistence::save_enveloped(
        persist,
        &manifest_path(root),
        MANIFEST_STORE,
        manifest,
        gate,
        crate::persistence::SaveIntent::Explicit,
    )
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
        restore_at(&versions, &record).unwrap();

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

    #[test]
    fn corrupt_manifest_blocks_preserve_without_rewriting_original_bytes() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let manifest_path = manifest_path(&versions);
        let original_manifest = b"{not-json".to_vec();
        std::fs::write(&manifest_path, &original_manifest).unwrap();
        let source = temp.file("source.txt", "payload");
        let operation = OperationId("blocked-preserve".to_string());

        let failure = preserve_at(
            &versions,
            &source,
            &operation,
            operation.step_key(0, &source),
            VersionRetentionPolicy::Recent,
        )
        .unwrap_err();

        assert!(failure.contains("manifest"));
        assert_eq!(std::fs::read(manifest_path).unwrap(), original_manifest);
        assert!(!versions.join(&operation.0).exists());
    }

    #[test]
    fn future_manifest_blocks_preserve_without_downgrade() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let manifest_path = manifest_path(&versions);
        let future = br#"{
          "format":"commander.persist",
          "store":"commander.version_manifest",
          "schema":99,
          "generation":4,
          "payload":{"records":[]}
        }"#;
        std::fs::write(&manifest_path, future).unwrap();
        let source = temp.file("source.txt", "payload");
        let operation = OperationId("future-preserve".to_string());

        let loaded = load_manifest_with(manifest_persist(), &versions);
        assert!(loaded.blocked);
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::FutureVersion
        );
        assert!(
            preserve_at(
                &versions,
                &source,
                &operation,
                operation.step_key(0, &source),
                VersionRetentionPolicy::Recent,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(manifest_path).unwrap(), future);
    }

    #[test]
    fn manifest_path_escape_cannot_restore_or_delete_external_data() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let external = temp.file("outside/version.txt", "protected");
        let original = temp.path().join("restore.txt");
        let record = VersionRecord {
            operation_id: OperationId("escape".to_string()),
            key: IdempotencyKey("escape-key".to_string()),
            original: original.clone(),
            stored: external.clone(),
            created_at_secs: 1,
        };
        let legacy = VersionManifest {
            records: vec![record.clone()],
        };
        std::fs::write(
            manifest_path(&versions),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let loaded = load_manifest_with(manifest_persist(), &versions);
        assert!(loaded.blocked);
        assert!(discard_record_at(&versions, &record).is_err());
        assert!(restore_at(&versions, &record).is_err());
        assert_eq!(std::fs::read_to_string(external).unwrap(), "protected");
        assert!(!original.exists());
    }

    #[cfg(unix)]
    #[test]
    fn manifest_symlink_ancestry_cannot_escape_versions_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let external = temp.dir("external");
        let payload = temp.file("external/version.txt", "protected");
        symlink(&external, versions.join("linked")).unwrap();
        let record = VersionRecord {
            operation_id: OperationId("symlink-escape".to_string()),
            key: IdempotencyKey("symlink-key".to_string()),
            original: temp.path().join("restore.txt"),
            stored: versions.join("linked/version.txt"),
            created_at_secs: 1,
        };
        std::fs::write(
            manifest_path(&versions),
            serde_json::to_vec_pretty(&VersionManifest {
                records: vec![record.clone()],
            })
            .unwrap(),
        )
        .unwrap();

        assert!(load_manifest_with(manifest_persist(), &versions).blocked);
        assert!(discard_record_at(&versions, &record).is_err());
        assert_eq!(std::fs::read_to_string(payload).unwrap(), "protected");
    }
}
