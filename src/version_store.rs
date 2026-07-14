//! Local verified versions for the `Versioned` durability profile.

use crate::operation::{IdempotencyKey, OperationId};
use crate::path_identity::PathIdentity;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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

pub fn preserve(
    path: &Path,
    operation_id: &OperationId,
    key: IdempotencyKey,
) -> Result<Option<VersionRecord>, String> {
    preserve_at(&versions_dir(), path, operation_id, key)
}

fn preserve_at(
    root: &Path,
    path: &Path,
    operation_id: &OperationId,
    key: IdempotencyKey,
) -> Result<Option<VersionRecord>, String> {
    let before = PathIdentity::observe(path).map_err(|error| {
        format!(
            "Could not inspect {} for versioning: {error}",
            path.display()
        )
    })?;
    if !before.exists {
        return Ok(None);
    }
    let stored = record_path(root, operation_id, &key, path);
    if let Some(parent) = stored.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create version directory {}: {error}",
                parent.display()
            )
        })?;
    }
    copy_path(path, &stored).map_err(|error| {
        format!(
            "Could not preserve {} at {}: {error}",
            path.display(),
            stored.display()
        )
    })?;
    let after = PathIdentity::observe(path).map_err(|error| {
        format!(
            "Could not recheck {} after versioning: {error}",
            path.display()
        )
    })?;
    if !before.same_version(&after) || !paths_equal(path, &stored) {
        let _ = remove_path(&stored);
        return Err(format!(
            "Source changed or version verification failed for {}",
            path.display()
        ));
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
        return Err("Could not save version manifest".to_string());
    }
    Ok(Some(record))
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

pub fn record_for_key(key: &IdempotencyKey) -> Option<VersionRecord> {
    load_manifest(&versions_dir())
        .records
        .into_iter()
        .find(|record| &record.key == key)
}

fn versions_dir() -> PathBuf {
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
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        create_symlink(&target, destination, source.is_dir())?;
    } else if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
    } else {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut input = fs::File::open(source)?;
        let mut output = options.open(destination)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        fs::set_permissions(destination, metadata.permissions())?;
    }
    Ok(())
}

pub fn paths_equal(left: &Path, right: &Path) -> bool {
    let Ok(left_meta) = fs::symlink_metadata(left) else {
        return false;
    };
    let Ok(right_meta) = fs::symlink_metadata(right) else {
        return false;
    };
    if left_meta.file_type().is_symlink() || right_meta.file_type().is_symlink() {
        return left_meta.file_type().is_symlink()
            && right_meta.file_type().is_symlink()
            && fs::read_link(left).ok() == fs::read_link(right).ok();
    }
    if left_meta.is_dir() != right_meta.is_dir() {
        return false;
    }
    if !left_meta.is_dir() {
        return crate::fs_util::files_equal(left, right);
    }
    let mut left_names = match fs::read_dir(left) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>(),
        Err(_) => return false,
    };
    let mut right_names = match fs::read_dir(right) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>(),
        Err(_) => return false,
    };
    left_names.sort();
    right_names.sort();
    left_names == right_names
        && left_names
            .iter()
            .all(|name| paths_equal(&left.join(name), &right.join(name)))
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
        )
        .unwrap()
        .unwrap();

        std::fs::remove_file(&original).unwrap();
        restore(&record).unwrap();

        assert_eq!(std::fs::read_to_string(&original).unwrap(), "before");
        assert_eq!(load_manifest(&versions).records, vec![record]);
    }
}
