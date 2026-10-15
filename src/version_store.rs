//! Local verified versions for the `Versioned` durability profile.
//! Content-chunk digests and store quota: [`crate::version_dedup`] (J002).

use crate::operation::{IdempotencyKey, OperationId, VersionRetentionPolicy, VersionStoreQuota};
use crate::path_identity::PathIdentity;
use crate::version_dedup::{ContentBlob, PayloadFile};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Health-report label for the version manifest store.
const MANIFEST_LABEL: &str = "Version manifest";

const MANIFEST_STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.version_manifest", 1, 16 * 1024 * 1024);

// Process-wide (not thread-local): delete/transfer workers preserve versions on
// background threads, while tests install the override on the test thread.
#[cfg(test)]
fn test_versions_dir_slot() -> &'static Mutex<Option<PathBuf>> {
    static SLOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Serializes tests that redirect the process-wide versions directory, so
/// concurrent tests never read each other's manifests.
#[cfg(test)]
fn test_versions_dir_serial() -> &'static Mutex<()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
pub struct TestVersionsDirGuard {
    previous: Option<PathBuf>,
    // Released after `drop` restores the previous directory.
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestVersionsDirGuard {
    fn drop(&mut self) {
        *crate::lock_util::recover(test_versions_dir_slot()) = self.previous.take();
    }
}

#[cfg(test)]
pub fn use_test_versions_dir(path: PathBuf) -> TestVersionsDirGuard {
    let serial = crate::lock_util::recover(test_versions_dir_serial());
    let previous = crate::lock_util::recover(test_versions_dir_slot()).replace(path);
    TestVersionsDirGuard {
        previous,
        _serial: serial,
    }
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
    #[serde(default)]
    identities: std::collections::BTreeMap<String, PathIdentity>,
    #[serde(default)]
    blobs: std::collections::BTreeMap<String, ContentBlob>,
    #[serde(default)]
    payload_files: std::collections::BTreeMap<String, Vec<PayloadFile>>,
}

struct LoadedManifest {
    manifest: VersionManifest,
    gate: crate::persistence::StoreGate,
    blocked: bool,
}

fn store_quota_slot() -> &'static Mutex<VersionStoreQuota> {
    static SLOT: OnceLock<Mutex<VersionStoreQuota>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(VersionStoreQuota::default()))
}

pub fn store_quota() -> VersionStoreQuota {
    *crate::lock_util::recover(store_quota_slot())
}

pub fn set_store_quota(quota: VersionStoreQuota) {
    *crate::lock_util::recover(store_quota_slot()) = quota;
}

pub fn store_usage() -> crate::version_dedup::VersionStoreUsage {
    let root = versions_dir();
    let loaded = load_manifest_with(manifest_persist(), &root);
    if loaded.blocked {
        return crate::version_dedup::VersionStoreUsage {
            bytes_quota: store_quota().max_bytes,
            blocked: true,
            ..Default::default()
        };
    }
    crate::version_dedup::usage_of(
        &loaded.manifest.blobs,
        loaded.manifest.records.len(),
        store_quota(),
    )
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

pub fn preserve_expected_at(
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
    let _manifest_guard = crate::lock_util::recover(manifest_transaction());
    let loaded = load_manifest_with(persist, root);
    if loaded.blocked {
        return Err(version_failure(
            "Version manifest is unreadable or incompatible; preservation is blocked",
        ));
    }
    if let Some(existing) = loaded
        .manifest
        .records
        .iter()
        .find(|record| record.key == key)
    {
        if existing.operation_id != *operation_id || existing.original != path {
            return Err(version_failure(
                "Version key is already bound to a different authoritative record",
            ));
        }
        // Same record: a replay. Report why its payload cannot be trusted
        // instead of claiming a different record owns the key.
        return validate_record(
            root,
            existing,
            loaded.manifest.identities.get(&existing.key.0),
        )
        .map(|()| Some(existing.clone()))
        .map_err(|message| {
            version_failure(format!(
                "Existing version of {} is not usable: {message}",
                path.display()
            ))
        });
    }
    let stored = record_path(root, operation_id, &key, path);
    if !lexically_descends_from(root, &stored) {
        return Err(version_failure(
            "Generated version path escaped the configured versions root",
        ));
    }
    if crate::fs_util::path_is_taken(&stored) {
        return Err(version_failure(
            "Generated version path is occupied by untracked data",
        ));
    }
    if let Some(parent) = stored.parent() {
        ensure_version_parent(root, parent).map_err(version_failure)?;
    }
    // Every failure below abandons the new version: its payload is removed
    // (when still ours), the `<op>/<key>` directories created for it are
    // pruned once empty, and a failed removal is reported because it leaves
    // the deterministic stored path occupied for a retry of this step.
    let abandon = |cleanup: String| {
        remove_empty_version_dirs(stored.parent(), root);
        cleanup
    };
    let copy_receipt = match copy_path_tracked(path, &stored) {
        Ok(receipt) => receipt,
        Err(failure) => {
            let cleanup = abandon(
                failure
                    .destination_identity
                    .as_ref()
                    .map(|identity| remove_path_if_object_matches(&stored, identity))
                    .unwrap_or_default(),
            );
            return Err(version_failure(format!(
                "Could not preserve {} at {}: {}{cleanup}",
                path.display(),
                stored.display(),
                failure.error
            )));
        }
    };
    let Some(created_shallow) = copy_receipt.destination_identity else {
        return Err(version_failure(format!(
            "Could not bind the new version {}",
            stored.display()
        )));
    };
    let created_identity = PathIdentity::observe_deep(&stored).map_err(|error| {
        let cleanup = abandon(remove_path_if_object_matches(&stored, &created_shallow));
        version_failure(format!(
            "Could not bind the new version {}: {error}{cleanup}",
            stored.display()
        ))
    })?;
    let after = PathIdentity::observe(path).map_err(|error| {
        let cleanup = abandon(remove_path_if_identity_matches(&stored, &created_identity));
        let mut failure = crate::ports::NativeFailure::from_io(&error);
        failure.message = format!(
            "Could not recheck {} after versioning: {}{cleanup}",
            path.display(),
            failure.message
        );
        failure
    })?;
    if !before.same_version(&after) || !paths_equal(path, &stored) {
        let cleanup = abandon(remove_path_if_identity_matches(&stored, &created_identity));
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Stale,
            message: format!(
                "Source changed or version verification failed for {}{cleanup}",
                path.display()
            ),
        });
    }
    if let Err(message) = validate_stored_path(root, &stored) {
        let cleanup = abandon(remove_path_if_identity_matches(&stored, &created_identity));
        return Err(version_failure(format!("{message}{cleanup}")));
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
    let mut loaded = loaded;
    if let Err(message) = crate::version_dedup::register_payload_dedup(
        &stored,
        &record.key.0,
        &mut loaded.manifest.blobs,
        &mut loaded.manifest.payload_files,
    ) {
        let cleanup = abandon(remove_path_if_identity_matches(&stored, &created_identity));
        return Err(version_failure(format!("{message}{cleanup}")));
    }
    let created_identity = match PathIdentity::observe_deep(&stored) {
        Ok(identity) => identity,
        Err(error) => {
            crate::version_dedup::release_payload_refs(
                &mut loaded.manifest.blobs,
                &mut loaded.manifest.payload_files,
                &record.key.0,
            );
            let cleanup = abandon(remove_path_if_identity_matches(&stored, &created_identity));
            return Err(version_failure(format!(
                "Could not rebind version after content dedup {}: {error}{cleanup}",
                stored.display()
            )));
        }
    };
    loaded.manifest.records.push(record.clone());
    loaded
        .manifest
        .identities
        .insert(record.key.0.clone(), created_identity.clone());
    match crate::persistence::report_save(
        MANIFEST_LABEL,
        save_manifest_with(persist, root, &loaded.manifest, &mut loaded.gate),
    ) {
        Some(crate::persistence::Committed::Durable) => {
            prune_manifest(
                persist,
                root,
                &loaded.manifest,
                &mut loaded.gate,
                retention,
                store_quota(),
                record.created_at_secs,
            );
        }
        Some(crate::persistence::Committed::NotDurable) => {}
        None => {
            crate::version_dedup::release_payload_refs(
                &mut loaded.manifest.blobs,
                &mut loaded.manifest.payload_files,
                &record.key.0,
            );
            let cleanup = abandon(remove_path_if_identity_matches(&stored, &created_identity));
            return Err(version_failure(format!(
                "Could not save version manifest{cleanup}"
            )));
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
pub fn discard_record_at(root: &Path, record: &VersionRecord) -> Result<(), String> {
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
    let authoritative_receipt = loaded
        .manifest
        .identities
        .get(&authoritative.key.0)
        .cloned();
    validate_record(root, &authoritative, authoritative_receipt.as_ref())?;
    let stored_identity = PathIdentity::observe_deep(&authoritative.stored)
        .map_err(|error| format!("Could not bind rejected version before discard: {error}"))?;
    loaded
        .manifest
        .records
        .retain(|candidate| candidate.key != record.key);
    loaded.manifest.identities.remove(&record.key.0);
    crate::version_dedup::release_payload_refs(
        &mut loaded.manifest.blobs,
        &mut loaded.manifest.payload_files,
        &record.key.0,
    );
    match crate::persistence::report_save(
        MANIFEST_LABEL,
        save_manifest_with(persist, root, &loaded.manifest, &mut loaded.gate),
    ) {
        Some(crate::persistence::Committed::Durable) => {
            validate_stored_path(root, &authoritative.stored)?;
            let current = PathIdentity::observe_deep(&authoritative.stored)
                .map_err(|error| format!("Could not revalidate rejected version: {error}"))?;
            if !stored_identity.same_binding(&current) {
                return Err(
                    "Rejected version changed after manifest commit; payload was retained"
                        .to_string(),
                );
            }
            remove_path(&authoritative.stored).map_err(|error| {
                format!(
                    "Could not remove rejected version {}: {error}",
                    authoritative.stored.display()
                )
            })?;
            remove_empty_version_dirs(authoritative.stored.parent(), root);
            Ok(())
        }
        Some(crate::persistence::Committed::NotDurable) => Ok(()),
        None => Err("Could not remove rejected version from the manifest".to_string()),
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
    quota: VersionStoreQuota,
    now_secs: u64,
) {
    let (retained, mut expired) = retained_records(&manifest.records, policy, now_secs);
    let retained_keys: std::collections::HashSet<_> =
        retained.iter().map(|record| record.key.0.clone()).collect();
    let mut working = VersionManifest {
        records: retained,
        identities: manifest
            .identities
            .iter()
            .filter(|(key, _)| retained_keys.contains(*key))
            .map(|(key, identity)| (key.clone(), identity.clone()))
            .collect(),
        blobs: manifest.blobs.clone(),
        payload_files: manifest
            .payload_files
            .iter()
            .filter(|(key, _)| retained_keys.contains(*key))
            .map(|(key, files)| (key.clone(), files.clone()))
            .collect(),
    };
    for record in &expired {
        crate::version_dedup::release_payload_refs(
            &mut working.blobs,
            &mut working.payload_files,
            &record.key.0,
        );
    }
    let mut indexed = working.records.clone();
    indexed.sort_by(|a, b| {
        a.created_at_secs
            .cmp(&b.created_at_secs)
            .then_with(|| a.key.0.cmp(&b.key.0))
    });
    let ordered: Vec<String> = indexed.iter().map(|r| r.key.0.clone()).collect();
    for key in crate::version_dedup::quota_overflow_keys(
        &ordered,
        &working.blobs,
        &working.payload_files,
        quota,
    ) {
        if let Some(record) = working.records.iter().find(|r| r.key.0 == key).cloned() {
            crate::version_dedup::release_payload_refs(
                &mut working.blobs,
                &mut working.payload_files,
                &key,
            );
            working.identities.remove(&key);
            working.records.retain(|c| c.key.0 != key);
            expired.push(record);
        }
    }
    if expired.is_empty() {
        return;
    }
    let mut expired_identities = Vec::with_capacity(expired.len());
    for record in &expired {
        if validate_record(root, record, manifest.identities.get(&record.key.0)).is_err() {
            return;
        }
        let Ok(identity) = PathIdentity::observe_deep(&record.stored) else {
            return;
        };
        expired_identities.push(identity);
    }
    let pruned = working;
    // Publish the manifest before deleting data. A failed policy update keeps
    // extra recovery data; it never leaves a manifest pointing at a deleted
    // version.
    if crate::persistence::report_save(
        MANIFEST_LABEL,
        save_manifest_with(persist, root, &pruned, gate),
    ) != Some(crate::persistence::Committed::Durable)
    {
        return;
    }
    for (record, expected) in expired.into_iter().zip(expired_identities) {
        if validate_stored_path(root, &record.stored).is_err() {
            continue;
        }
        let Ok(current) = PathIdentity::observe_deep(&record.stored) else {
            continue;
        };
        if !expected.same_binding(&current) {
            continue;
        }
        // The manifest no longer references this payload, so a failed removal
        // leaves orphaned data that quota accounting cannot see: report it.
        if let Err(error) = remove_path(&record.stored) {
            crate::persistence::record_cleanup_failure(MANIFEST_LABEL, &error);
        }
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
    let _manifest_guard = crate::lock_util::recover(manifest_transaction());
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
    validate_record(
        root,
        authoritative,
        loaded.manifest.identities.get(&authoritative.key.0),
    )?;
    let stored_identity = PathIdentity::observe_deep(&authoritative.stored)
        .map_err(|error| format!("Could not bind stored version before restore: {error}"))?;
    if crate::fs_util::path_is_taken(&authoritative.original) {
        return Err(format!(
            "Restore destination is occupied: {}",
            authoritative.original.display()
        ));
    }
    if let Some(parent) = authoritative
        .original
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        match fs::metadata(parent) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "Restore folder {} no longer exists",
                    parent.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "Restore folder {} is no longer a folder",
                    parent.display()
                ));
            }
            _ => {}
        }
    }
    let restore_receipt = match copy_path_tracked(&authoritative.stored, &authoritative.original) {
        Ok(receipt) => receipt,
        Err(failure) => {
            let cleanup = failure
                .destination_identity
                .as_ref()
                .map(|identity| remove_path_if_object_matches(&authoritative.original, identity))
                .unwrap_or_default();
            return Err(format!(
                "Could not restore {}: {}{cleanup}",
                record.original.display(),
                failure.error
            ));
        }
    };
    let stored_after = PathIdentity::observe_deep(&authoritative.stored);
    let stored_changed = match stored_after {
        Ok(current) => !stored_identity.same_binding(&current),
        Err(_) => true,
    };
    if stored_changed || !paths_equal(&authoritative.stored, &authoritative.original) {
        let cleanup = restore_receipt
            .destination_identity
            .as_ref()
            .map(|identity| remove_path_if_object_matches(&authoritative.original, identity))
            .unwrap_or_default();
        return Err(format!(
            "Restored version failed verification: {}{cleanup}",
            authoritative.original.display()
        ));
    }
    Ok(())
}

/// The operation's preserved versions, or an error when the manifest is
/// unreadable or incompatible, so callers can say versions are hidden
/// instead of showing none.
pub fn records_for(operation_id: &OperationId) -> Result<Vec<VersionRecord>, String> {
    let root = versions_dir();
    let loaded = load_manifest_with(manifest_persist(), &root);
    if loaded.blocked {
        return Err(format!(
            "Version manifest in {} is unreadable or incompatible; preserved versions are hidden",
            root.display()
        ));
    }
    Ok(loaded
        .manifest
        .records
        .into_iter()
        .filter(|record| &record.operation_id == operation_id)
        .collect())
}

/// Every preserved version. A manifest that is unreadable or incompatible is
/// an error rather than an empty list, so callers (the support bundle) do not
/// report "no versions" when the store could not be read.
pub fn records_or_blocked() -> Result<Vec<VersionRecord>, String> {
    let root = versions_dir();
    let loaded = load_manifest_with(manifest_persist(), &root);
    if loaded.blocked {
        Err(format!(
            "version manifest is {}",
            format!("{:?}", loaded.gate.status()).to_lowercase()
        ))
    } else {
        Ok(loaded.manifest.records)
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
    if let Some(path) = crate::lock_util::recover(test_versions_dir_slot()).clone() {
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

fn ensure_version_parent(root: &Path, parent: &Path) -> Result<(), String> {
    if !normalized_absolute(parent).is_some_and(|parent| {
        normalized_absolute(root).is_some_and(|root| parent.starts_with(root))
    }) {
        return Err("Version directory escapes the versions root".to_string());
    }
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("Versions root is not a trusted directory".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(root).map_err(|error| {
                format!("Could not create versions root {}: {error}", root.display())
            })?;
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect versions root {}: {error}",
                root.display()
            ));
        }
    }
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not verify versions root {}: {error}", root.display()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("Versions root is not a trusted directory".to_string());
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "Version directory is outside the versions root".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err("Version directory contains an unsafe component".to_string());
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(format!(
                    "Version directory component is not trusted: {}",
                    current.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    format!(
                        "Could not create version directory {}: {error}",
                        current.display()
                    )
                })?;
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    format!(
                        "Could not verify version directory {}: {error}",
                        current.display()
                    )
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(format!(
                        "Version directory component is not trusted: {}",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "Could not inspect version directory {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_record(
    root: &Path,
    record: &VersionRecord,
    expected_identity: Option<&PathIdentity>,
) -> Result<(), String> {
    let expected = record_path(root, &record.operation_id, &record.key, &record.original);
    if normalized_absolute(&expected) != normalized_absolute(&record.stored) {
        return Err("Version record path does not match its authoritative key".to_string());
    }
    validate_stored_path(root, &record.stored)?;
    let expected_identity =
        expected_identity.ok_or_else(|| "Version record has no payload identity".to_string())?;
    let current = PathIdentity::observe_deep(&record.stored)
        .map_err(|_| "Stored version identity could not be verified".to_string())?;
    if !expected_identity.same_binding(&current) {
        return Err("Stored version changed after it was recorded".to_string());
    }
    Ok(())
}

fn validate_manifest(root: &Path, manifest: &mut VersionManifest) -> Result<bool, ()> {
    let mut paths = std::collections::HashSet::new();
    let mut keys = std::collections::HashSet::new();
    let mut recovered = false;
    for record in &manifest.records {
        if !keys.insert(record.key.clone()) || !paths.insert(record.stored.clone()) {
            return Err(());
        }
        let expected = record_path(root, &record.operation_id, &record.key, &record.original);
        if normalized_absolute(&expected) != normalized_absolute(&record.stored)
            || validate_stored_path(root, &record.stored).is_err()
        {
            return Err(());
        }
        let current = PathIdentity::observe_deep(&record.stored).map_err(|_| ())?;
        match manifest.identities.get(&record.key.0) {
            Some(expected) if expected.same_binding(&current) => {}
            Some(_) => return Err(()),
            None => {
                manifest.identities.insert(record.key.0.clone(), current);
                recovered = true;
            }
        }
    }
    if manifest.identities.len() != manifest.records.len() {
        return Err(());
    }
    Ok(recovered)
}

/// Remove data this module just created at `path` if it is still bound to
/// `expected`. Returns "" or a "; cleanup of <path> failed: <error>" suffix
/// for the caller's error, since leftover data blocks a retry.
fn remove_path_if_identity_matches(path: &Path, expected: &PathIdentity) -> String {
    match PathIdentity::observe_deep(path) {
        Ok(current) if expected.same_binding(&current) => cleanup_note(path, remove_path(path)),
        _ => String::new(),
    }
}

fn remove_path_if_object_matches(path: &Path, expected: &PathIdentity) -> String {
    match PathIdentity::observe(path) {
        Ok(current) if current.path == expected.path && expected.same_object(&current) => {
            cleanup_note(path, remove_path(path))
        }
        _ => String::new(),
    }
}

fn cleanup_note(path: &Path, result: std::io::Result<()>) -> String {
    result.err().map_or_else(String::new, |error| {
        format!("; cleanup of {} failed: {error}", path.display())
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
    let mut blocked = status.fails_closed();
    let mut manifest = loaded.value.unwrap_or_default();
    if !blocked && status != crate::persistence::LoadStatus::Missing {
        match validate_manifest(root, &mut manifest) {
            Ok(true) => gate.mark_recovered(),
            Ok(false) => {}
            Err(()) => {
                gate.block(crate::persistence::LoadStatus::Corrupt);
                blocked = true;
            }
        }
    }
    if note_manifest_blocked(&manifest_path(root), blocked) {
        crate::persistence::record_blocked(MANIFEST_LABEL);
    }
    LoadedManifest {
        manifest,
        gate,
        blocked,
    }
}

/// Whether a blocked load of `manifest` should be published as a health
/// issue: only on the transition into the blocked state, since every
/// preserve, listing and usage refresh reloads the manifest and each issue
/// raises an error toast.
fn note_manifest_blocked(manifest: &Path, blocked: bool) -> bool {
    static REPORTED: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    let mut reported = crate::lock_util::recover(REPORTED.get_or_init(Mutex::default));
    if blocked {
        reported.insert(manifest.to_path_buf())
    } else {
        reported.remove(manifest);
        false
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

struct CopyPathFailure {
    error: std::io::Error,
    destination_identity: Option<Box<PathIdentity>>,
}

struct CopyPathReceipt {
    destination_identity: Option<PathIdentity>,
}

fn copy_path_tracked(
    source: &Path,
    destination: &Path,
) -> Result<CopyPathReceipt, CopyPathFailure> {
    enum Task {
        Copy(PathBuf, PathBuf),
        /// Runs after the directory's children: creating them bumps its mtime.
        FinishDirectory(PathBuf, fs::Metadata),
    }

    let root_destination = destination.to_path_buf();
    let mut destination_identity = None;
    let mut tasks = vec![Task::Copy(source.to_path_buf(), destination.to_path_buf())];
    let result = (|| {
        while let Some(task) = tasks.pop() {
            match task {
                Task::FinishDirectory(path, metadata) => {
                    #[cfg(unix)]
                    let _ = fs::File::open(&path)
                        .and_then(|directory| directory.set_times(copied_times(&metadata)));
                    fs::set_permissions(path, metadata.permissions())?;
                }
                Task::Copy(source, destination) => {
                    let metadata = fs::symlink_metadata(&source)?;
                    if metadata.file_type().is_symlink() {
                        let target = fs::read_link(&source)?;
                        crate::fs_util::create_symlink(&target, &destination, source.is_dir())?;
                        if destination == root_destination {
                            destination_identity = PathIdentity::observe(&destination).ok();
                        }
                        continue;
                    }
                    if metadata.is_dir() {
                        fs::create_dir(&destination)?;
                        if destination == root_destination {
                            destination_identity = PathIdentity::observe(&destination).ok();
                        }
                        tasks.push(Task::FinishDirectory(destination.clone(), metadata));
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
                    // Opening a FIFO or device would block or read a stream;
                    // only regular files can be preserved as content.
                    if !metadata.is_file() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("unsupported special file {}", source.display()),
                        ));
                    }
                    let mut options = fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    let mut input = fs::File::open(&source)?;
                    let mut output = options.open(&destination)?;
                    if destination == root_destination {
                        destination_identity = PathIdentity::observe(&destination).ok();
                    }
                    std::io::copy(&mut input, &mut output)?;
                    // Best effort: a filesystem without settable times still
                    // preserves the content.
                    let _ = output.set_times(copied_times(&metadata));
                    output.sync_all()?;
                    fs::set_permissions(destination, metadata.permissions())?;
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(CopyPathReceipt {
            destination_identity,
        }),
        Err(error) => Err(CopyPathFailure {
            error,
            destination_identity: destination_identity.map(Box::new),
        }),
    }
}

/// The source's modified and accessed times, so a preserved or restored
/// version keeps the original timestamps instead of the copy time.
fn copied_times(metadata: &fs::Metadata) -> fs::FileTimes {
    let mut times = fs::FileTimes::new();
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    times
}

#[cfg(test)]
fn copy_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    match copy_path_tracked(source, destination) {
        Ok(_) => Ok(()),
        Err(failure) => Err(failure.error),
    }
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
            make_directories_removable(path);
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Versions keep their directories' modes, so a preserved read-only folder
/// (0o555) would refuse unlinking its entries. Give every real directory in
/// the tree owner rwx first; symlinks are never followed.
fn make_directories_removable(root: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(metadata) = fs::symlink_metadata(&directory) else {
                continue;
            };
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                continue;
            }
            let mode = metadata.permissions().mode();
            if mode & 0o700 != 0o700 {
                let _ = fs::set_permissions(&directory, fs::Permissions::from_mode(mode | 0o700));
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    pending.push(entry.path());
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = root;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn blocked_manifest_is_reported_not_listed_as_empty() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let _guard = use_test_versions_dir(versions.clone());
        assert_eq!(records_or_blocked(), Ok(Vec::new()));

        std::fs::write(manifest_path(&versions), b"{ not json").unwrap();

        assert_eq!(
            records_or_blocked(),
            Err("version manifest is corrupt".to_string())
        );
    }

    #[test]
    fn blocked_manifest_is_reported_once_and_shown_as_unreadable_usage() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let _guard = use_test_versions_dir(versions.clone());
        let path = manifest_path(&versions);
        std::fs::write(&path, b"{ not json").unwrap();

        let usage = store_usage();
        assert!(usage.blocked);
        assert!(usage.label().contains("unreadable"), "{}", usage.label());
        assert!(load_manifest_with(manifest_persist(), &versions).blocked);
        assert!(
            !note_manifest_blocked(&path, true),
            "repeated blocked loads must not publish the issue again"
        );

        std::fs::remove_file(&path).unwrap();
        let usage = store_usage();
        assert!(!usage.blocked);
        assert!(
            note_manifest_blocked(&path, true),
            "a readable load re-arms the report"
        );
    }

    #[test]
    fn test_versions_dir_override_is_visible_to_worker_threads() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let _guard = use_test_versions_dir(versions.clone());
        let observed = std::thread::spawn(versions_dir)
            .join()
            .expect("worker thread");
        assert_eq!(observed, versions);
    }

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

    #[test]
    fn preserved_and_restored_versions_keep_the_source_modified_time() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let folder = temp.dir("folder");
        let file = temp.file("folder/file.txt", "dated");
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let stamp = |path: &Path| {
            fs::OpenOptions::new()
                .read(true)
                .open(path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        };
        stamp(&file);
        #[cfg(unix)]
        stamp(&folder);
        let operation = OperationId("keep-mtime".to_string());
        let record = preserve_at(
            &versions,
            &folder,
            &operation,
            operation.step_key(0, &folder),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();
        let mtime = |path: &Path| fs::metadata(path).unwrap().modified().unwrap();
        assert_eq!(mtime(&record.stored.join("file.txt")), modified);

        fs::remove_dir_all(&folder).unwrap();
        restore_at(&versions, &record).unwrap();

        assert_eq!(mtime(&file), modified);
        #[cfg(unix)]
        {
            assert_eq!(mtime(&record.stored), modified);
            assert_eq!(mtime(&folder), modified);
        }
    }

    #[cfg(unix)]
    #[test]
    fn version_copy_refuses_a_fifo_instead_of_blocking_on_it() {
        let temp = TempDir::new();
        let source = temp.dir("source");
        let status = std::process::Command::new("mkfifo")
            .arg(source.join("pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        let destination = temp.path().join("destination");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(copy_path(&source, &destination));
        });

        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("copying a FIFO must not block");

        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("special file"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn discard_removes_a_version_of_a_read_only_folder() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let folder = temp.dir("folder");
        temp.file("folder/locked/file.txt", "kept read-only");
        let locked = folder.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let operation = OperationId("read-only-folder".to_string());
        let record = preserve_at(
            &versions,
            &folder,
            &operation,
            operation.step_key(0, &folder),
            VersionRetentionPolicy::Recent,
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let record = record.unwrap().unwrap();
        let stored_locked = record.stored.join("locked");
        assert_eq!(
            fs::metadata(&stored_locked).unwrap().permissions().mode() & 0o777,
            0o555
        );

        discard_record_at(&versions, &record).unwrap();

        assert!(!record.stored.exists());
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
        assert!(
            !versions.join(&operation.0).exists(),
            "directories created for the abandoned version are pruned"
        );
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
    fn discard_removes_the_record_its_references_and_its_payload() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "rejected");
        let operation = OperationId("discard-success".to_string());
        let record = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();
        assert!(record.stored.exists());

        discard_record_at(&versions, &record).unwrap();

        let manifest = load_manifest(&versions);
        assert!(manifest.records.is_empty());
        assert!(manifest.identities.is_empty());
        assert!(manifest.blobs.is_empty());
        assert!(manifest.payload_files.is_empty());
        assert!(!record.stored.exists());
        assert!(
            !versions.join(&operation.0).exists(),
            "empty <op>/<key> directories are pruned"
        );
        assert!(versions.exists());
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "rejected");
    }

    #[test]
    fn restore_into_a_deleted_folder_names_the_missing_folder() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("gone/original.txt", "before");
        let operation = OperationId("restore-gone-parent".to_string());
        let record = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();
        std::fs::remove_dir_all(temp.path().join("gone")).unwrap();

        let error = restore_at(&versions, &record).unwrap_err();

        assert!(error.contains("no longer exists"), "{error}");
        assert!(error.contains("gone"), "{error}");
        assert!(record.stored.exists());
    }

    #[test]
    fn repeated_same_key_preserve_keeps_the_authoritative_payload() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "authoritative");
        let operation = OperationId("same-key".to_string());
        let key = operation.step_key(0, &original);
        let first = preserve_at(
            &versions,
            &original,
            &operation,
            key.clone(),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();
        std::fs::write(&original, "newer source").unwrap();

        let replay = preserve_at(
            &versions,
            &original,
            &operation,
            key,
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();

        assert_eq!(replay, first);
        assert_eq!(
            std::fs::read_to_string(&first.stored).unwrap(),
            "authoritative"
        );
        assert_eq!(load_manifest(&versions).records, vec![first]);
    }

    #[test]
    fn replaced_version_payload_blocks_restore_and_discard() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "authoritative");
        let operation = OperationId("replaced-payload".to_string());
        let record = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Compact,
        )
        .unwrap()
        .unwrap();
        let replacement = temp.file("replacement.txt", "substituted!!");
        std::fs::rename(replacement, &record.stored).unwrap();
        std::fs::remove_file(&original).unwrap();

        let loaded = load_manifest_with(manifest_persist(), &versions);

        assert!(loaded.blocked);
        assert!(restore_at(&versions, &record).is_err());
        assert!(discard_record_at(&versions, &record).is_err());
        assert!(record.stored.exists());
        assert!(!original.exists());
    }

    #[test]
    fn legacy_manifest_recovers_identity_receipts_without_write_on_load() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "authoritative");
        let operation = OperationId("legacy-receipt".to_string());
        let record = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();
        let legacy = serde_json::to_vec_pretty(&VersionManifest {
            records: vec![record],
            ..Default::default()
        })
        .unwrap();
        std::fs::write(manifest_path(&versions), &legacy).unwrap();

        let loaded = load_manifest_with(manifest_persist(), &versions);

        assert!(!loaded.blocked);
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::Recovered
        );
        assert_eq!(loaded.manifest.identities.len(), 1);
        assert_eq!(std::fs::read(manifest_path(&versions)).unwrap(), legacy);
    }

    #[test]
    fn stale_forged_record_cannot_restore_or_discard_authoritative_data() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let original = temp.file("original.txt", "authoritative");
        let operation = OperationId("stale-record".to_string());
        let record = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        )
        .unwrap()
        .unwrap();
        let mut forged = record.clone();
        forged.created_at_secs = forged.created_at_secs.saturating_add(1);
        std::fs::remove_file(&original).unwrap();

        assert!(restore_at(&versions, &forged).is_err());
        assert!(discard_record_at(&versions, &forged).is_err());
        assert!(!original.exists());
        assert!(record.stored.exists());
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
        // Binding rejection must not create the versions root.
        assert!(!versions.exists());
        assert!(load_manifest(&versions).records.is_empty());
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
    fn quota_overflow_evicts_the_oldest_version_after_manifest_commit() {
        let temp = TempDir::new();
        let versions = temp.path().join("versions");
        let mut created = Vec::new();
        for index in 0..3 {
            let original = temp.file(
                &format!("file-{index}.txt"),
                &format!("{index}").repeat(100),
            );
            let operation = OperationId(format!("quota-{index}"));
            created.push(
                preserve_at(
                    &versions,
                    &original,
                    &operation,
                    operation.step_key(0, &original),
                    VersionRetentionPolicy::Forever,
                )
                .unwrap()
                .unwrap(),
            );
        }
        // prune_manifest evicts in (created_at_secs, key) order.
        created.sort_by(|a, b| {
            a.created_at_secs
                .cmp(&b.created_at_secs)
                .then_with(|| a.key.0.cmp(&b.key.0))
        });
        let mut loaded = load_manifest_with(manifest_persist(), &versions);
        assert_eq!(loaded.manifest.records.len(), 3);

        prune_manifest(
            manifest_persist(),
            &versions,
            &loaded.manifest,
            &mut loaded.gate,
            VersionRetentionPolicy::Forever,
            VersionStoreQuota { max_bytes: 200 },
            created[2].created_at_secs,
        );

        let manifest = load_manifest(&versions);
        let mut kept = manifest.records.clone();
        kept.sort_by(|a, b| a.key.0.cmp(&b.key.0));
        let mut expected = created[1..].to_vec();
        expected.sort_by(|a, b| a.key.0.cmp(&b.key.0));
        assert_eq!(kept, expected);
        assert!(!manifest.identities.contains_key(&created[0].key.0));
        assert!(!manifest.payload_files.contains_key(&created[0].key.0));
        assert_eq!(manifest.blobs.len(), 2);
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
            ..Default::default()
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

    #[test]
    fn manifest_cannot_rebind_a_record_to_an_internal_control_file() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let path = manifest_path(&versions);
        let record = VersionRecord {
            operation_id: OperationId("control-file".to_string()),
            key: IdempotencyKey("control-key".to_string()),
            original: temp.path().join("restore.txt"),
            stored: path.clone(),
            created_at_secs: 1,
        };
        let bytes = serde_json::to_vec_pretty(&VersionManifest {
            records: vec![record.clone()],
            ..Default::default()
        })
        .unwrap();
        std::fs::write(&path, &bytes).unwrap();

        assert!(load_manifest_with(manifest_persist(), &versions).blocked);
        assert!(discard_record_at(&versions, &record).is_err());
        assert!(restore_at(&versions, &record).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn duplicate_manifest_keys_fail_closed_without_deleting_payloads() {
        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let original = temp.path().join("restore.txt");
        let key = IdempotencyKey("duplicate-key".to_string());
        let make_record = |operation: &str| {
            let operation_id = OperationId(operation.to_string());
            let stored = record_path(&versions, &operation_id, &key, &original);
            std::fs::create_dir_all(stored.parent().unwrap()).unwrap();
            std::fs::write(&stored, operation).unwrap();
            VersionRecord {
                operation_id,
                key: key.clone(),
                original: original.clone(),
                stored,
                created_at_secs: 1,
            }
        };
        let first = make_record("first");
        let second = make_record("second");
        std::fs::write(
            manifest_path(&versions),
            serde_json::to_vec_pretty(&VersionManifest {
                records: vec![first.clone(), second.clone()],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();

        assert!(load_manifest_with(manifest_persist(), &versions).blocked);
        assert!(discard_record_at(&versions, &first).is_err());
        assert!(first.stored.exists());
        assert!(second.stored.exists());
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
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();

        assert!(load_manifest_with(manifest_persist(), &versions).blocked);
        assert!(discard_record_at(&versions, &record).is_err());
        assert_eq!(std::fs::read_to_string(payload).unwrap(), "protected");
    }

    #[cfg(unix)]
    #[test]
    fn preserve_rejects_symlink_ancestry_before_copying_payload() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let versions = temp.dir("versions");
        let external = temp.dir("external");
        let original = temp.file("source.txt", "private");
        let operation = OperationId("linked-operation".to_string());
        symlink(&external, versions.join(&operation.0)).unwrap();

        let result = preserve_at(
            &versions,
            &original,
            &operation,
            operation.step_key(0, &original),
            VersionRetentionPolicy::Recent,
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&external).unwrap().count(), 0);
        assert!(!manifest_path(&versions).exists());
    }
}
