//! Privacy-preserving support bundle assembled from structured runtime state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SCHEMA: u32 = 2;
const MAX_OPERATION_SPANS: usize = 500;
const MAX_VERSION_SUMMARIES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedPath {
    pub token: String,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildVersions {
    pub app: String,
    pub support_bundle_schema: u32,
    pub operation_journal_schema: u32,
    pub content_index_schema: u32,
    pub provider_contract_schema: u32,
    pub os: String,
    pub architecture: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSpan {
    pub operation_ref: String,
    pub group_ref: Option<String>,
    pub target: RedactedPath,
    pub kind: crate::transfer::TransferKind,
    pub status: crate::operation_journal::OperationStatus,
    pub duration_secs: u64,
    pub steps: usize,
    pub completed_steps: usize,
    pub failed_steps: usize,
    pub requeued_steps: usize,
    pub attempts: u64,
    pub fast_paths: BTreeMap<String, usize>,
    pub failure_classes: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionSummary {
    pub operation_ref: String,
    pub version_ref: String,
    pub original: RedactedPath,
    pub age_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSupport {
    pub requested: RedactedPath,
    pub mount: RedactedPath,
    pub volume_ref: String,
    pub generation: u64,
    pub backend: crate::volume_profile::BackendKind,
    pub filesystem: String,
    pub read_only: bool,
    pub case_sensitive: Option<bool>,
    pub capabilities: crate::volume_profile::VolumeCapabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSupport {
    pub workload: crate::workload::SchedulerStats,
    pub latency_metrics: Vec<(
        crate::measurement::MetricName,
        crate::measurement::LatencyPercentiles,
    )>,
    pub startup: Option<crate::measurement::StartupSnapshot>,
    pub feature_controls: Vec<crate::feature_flags::FeatureSnapshot>,
    pub watcher_health: crate::watcher_health::WatcherHealth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SupportBundle {
    pub schema: u32,
    pub generated_at_secs: u64,
    pub versions: BuildVersions,
    pub runtime: RuntimeSupport,
    pub operation_spans_truncated: bool,
    pub operation_spans: Vec<OperationSpan>,
    pub preserved_versions_truncated: bool,
    pub preserved_versions: Vec<VersionSummary>,
    pub volumes: Vec<VolumeSupport>,
    pub collection_warnings: Vec<String>,
}

struct Redactor {
    salt: [u8; 32],
}

impl Redactor {
    /// The salt comes from the OS CSPRNG on every platform (there is no
    /// /dev/urandom on Windows or in some sandboxes), so path tokens can never
    /// fall back to a guessable time+pid seed.
    fn new() -> Self {
        use rand::RngCore;
        let mut salt = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut salt);
        Self { salt }
    }

    fn token(&self, prefix: &str, value: &str) -> String {
        let mut hasher = blake3::Hasher::new_keyed(&self.salt);
        hasher.update(value.as_bytes());
        let hash = hasher.finalize().to_hex();
        format!("{prefix}-{}", &hash[..16])
    }

    fn path(&self, path: &Path) -> RedactedPath {
        RedactedPath {
            token: self.token("path", &path.to_string_lossy()),
            depth: path.components().count(),
        }
    }
}

#[must_use]
pub fn collect(paths: &[PathBuf]) -> SupportBundle {
    let mut warnings = Vec::new();
    let operations = match crate::operation_journal::load() {
        Ok(journal) => journal.operations,
        Err(error) => {
            warnings.push(format!(
                "operation journal unavailable: {}",
                journal_unavailable_reason(&error)
            ));
            Vec::new()
        }
    };
    let versions = match crate::version_store::records_or_blocked() {
        Ok(versions) => versions,
        Err(reason) => {
            warnings.push(format!("version store unavailable: {reason}"));
            Vec::new()
        }
    };
    build(paths, &operations, &versions, warnings)
}

/// A path-free reason for an operation-journal load failure: the bundle is
/// redacted, and the raw error text can embed absolute paths.
fn journal_unavailable_reason(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("lock") {
        "its store lock could not be acquired"
    } else if error.contains("directory") {
        "its directory could not be created"
    } else if error.contains("unreadable") {
        "the file is unreadable"
    } else if error.contains("newer") || error.contains("schema") {
        "it was written by an unsupported version"
    } else if error.contains("corrupt") {
        "the file is corrupt"
    } else {
        "its records failed validation"
    }
}

pub(crate) fn build(
    paths: &[PathBuf],
    operations: &[crate::operation_journal::OperationRecord],
    versions: &[crate::version_store::VersionRecord],
    collection_warnings: Vec<String>,
) -> SupportBundle {
    let generated_at_secs = now_secs();
    let redactor = Redactor::new();
    let operation_spans_truncated = operations.len() > MAX_OPERATION_SPANS;
    let operation_start = operations.len().saturating_sub(MAX_OPERATION_SPANS);
    let operation_spans = operations[operation_start..]
        .iter()
        .map(|operation| operation_span(&redactor, operation))
        .collect();
    let preserved_versions_truncated = versions.len() > MAX_VERSION_SUMMARIES;
    let version_start = versions.len().saturating_sub(MAX_VERSION_SUMMARIES);
    let preserved_versions = versions[version_start..]
        .iter()
        .map(|version| VersionSummary {
            operation_ref: redactor.token("operation", &version.operation_id.0),
            version_ref: redactor.token("version", &version.key.0),
            original: redactor.path(&version.original),
            age_secs: generated_at_secs.saturating_sub(version.created_at_secs),
        })
        .collect();
    let volumes = paths
        .iter()
        .map(|path| {
            let profile = crate::volume_profile::profile(path);
            VolumeSupport {
                requested: redactor.path(path),
                mount: redactor.path(&profile.mount_point),
                volume_ref: redactor.token("volume", &profile.volume_id.to_string()),
                generation: profile.generation,
                backend: profile.backend,
                filesystem: profile.filesystem,
                read_only: profile.read_only,
                case_sensitive: profile.case_sensitive,
                capabilities: profile.capabilities,
            }
        })
        .collect();
    SupportBundle {
        schema: SCHEMA,
        generated_at_secs,
        versions: BuildVersions {
            app: env!("CARGO_PKG_VERSION").to_string(),
            support_bundle_schema: SCHEMA,
            operation_journal_schema: crate::operation_journal::schema_version(),
            content_index_schema: crate::content_index::schema_version(),
            provider_contract_schema: 1,
            os: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
        },
        runtime: RuntimeSupport {
            workload: crate::workload::stats(),
            latency_metrics: crate::measurement::snapshots(),
            startup: crate::measurement::latest_startup(),
            feature_controls: crate::feature_flags::snapshots(),
            watcher_health: crate::watcher_health::snapshot(),
        },
        operation_spans_truncated,
        operation_spans,
        preserved_versions_truncated,
        preserved_versions,
        volumes,
        collection_warnings,
    }
}

fn operation_span(
    redactor: &Redactor,
    operation: &crate::operation_journal::OperationRecord,
) -> OperationSpan {
    let mut fast_paths = BTreeMap::new();
    let mut failure_classes = BTreeMap::new();
    let mut requeued_steps = 0;
    let mut attempts = 0_u64;
    for step in &operation.steps {
        attempts = attempts.saturating_add(u64::from(step.attempts));
        if step.status == crate::operation_journal::StepStatus::Requeued {
            requeued_steps += 1;
        }
        if let Some(path) = step.fast_path {
            *fast_paths.entry(path.label().to_string()).or_insert(0) += 1;
        }
        if let Some(failure) = &step.failure {
            *failure_classes
                .entry(failure.class.label().to_string())
                .or_insert(0) += 1;
        }
    }
    OperationSpan {
        operation_ref: redactor.token("operation", &operation.id.0),
        group_ref: operation
            .group_id
            .as_ref()
            .map(|group| redactor.token("group", &group.0)),
        target: redactor.path(&operation.target),
        kind: operation.kind,
        status: operation.status,
        duration_secs: operation
            .updated_at_secs
            .saturating_sub(operation.created_at_secs),
        steps: operation.steps.len(),
        completed_steps: operation.completed_steps(),
        failed_steps: operation.failed_steps(),
        requeued_steps,
        attempts,
        fast_paths,
        failure_classes,
    }
}

pub fn export(paths: &[PathBuf]) -> Result<PathBuf, String> {
    let directory = crate::fs_util::config_dir().join("support-bundles");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create support bundle directory: {error}"))?;
    let path = directory.join(format!("commander-support-{}.json", now_millis()));
    export_to(&path, &collect(paths))?;
    Ok(path)
}

/// Recipient-encrypted export with explicit expiry and plaintext preview (J005).
///
/// Encrypts and writes through [`crate::encrypted_bundle::export_to`], then
/// verifies by decrypting the envelope read back from disk.
pub fn export_encrypted(
    paths: &[PathBuf],
    recipient_id: &str,
    recipient_secret: &str,
    ttl_secs: u64,
) -> Result<(PathBuf, crate::encrypted_bundle::EncryptedBundleManifest), String> {
    let config_dir = crate::fs_util::config_dir();
    let path = crate::encrypted_bundle::default_export_path(&config_dir, now_millis());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create support bundle directory: {error}"))?;
    }
    let bundle = collect(paths);
    let preview = plaintext_preview(&bundle);
    let json = serde_json::to_vec_pretty(&bundle)
        .map_err(|error| format!("Could not encode support bundle: {error}"))?;
    let created_at_secs = now_secs();
    let manifest = crate::encrypted_bundle::export_to(
        &path,
        &json,
        recipient_id,
        recipient_secret,
        &preview,
        created_at_secs,
        ttl_secs,
    )?;
    let loaded = crate::encrypted_bundle::load_manifest(&path)?;
    let recovered_disk =
        crate::encrypted_bundle::decrypt(&loaded, recipient_secret, created_at_secs)?;
    if recovered_disk != json {
        return Err("encrypted support bundle failed post-export verification".into());
    }
    Ok((path, manifest))
}

#[must_use]
pub fn plaintext_preview(bundle: &SupportBundle) -> String {
    format!(
        "schema={} ops={} versions={} volumes={} warnings={}",
        bundle.schema,
        bundle.operation_spans.len(),
        bundle.preserved_versions.len(),
        bundle.volumes.len(),
        bundle.collection_warnings.len()
    )
}

fn export_to(path: &Path, bundle: &SupportBundle) -> Result<(), String> {
    let json = serde_json::to_string_pretty(bundle)
        .map_err(|error| format!("Could not encode support bundle: {error}"))?;
    if crate::fs_util::write_atomic(path, &json) {
        Ok(())
    } else {
        Err(format!(
            "Could not write support bundle to {}",
            path.display()
        ))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_failure_reasons_are_classified_without_paths() {
        assert_eq!(
            journal_unavailable_reason(
                "Operation journal is corrupt: envelope or payload rejected"
            ),
            "the file is corrupt"
        );
        assert_eq!(
            journal_unavailable_reason(
                "Could not open operation journal lock: Permission denied (os error 13)"
            ),
            "its store lock could not be acquired"
        );
        assert_eq!(
            journal_unavailable_reason("Unsupported operation journal schema 9; expected 3"),
            "it was written by an unsupported version"
        );
        let reason = journal_unavailable_reason("step /home/user/private/file.txt is invalid");
        assert!(!reason.contains("/home"), "{reason}");
    }

    #[test]
    fn redaction_salt_is_fresh_per_bundle() {
        let (a, b) = (Redactor::new(), Redactor::new());
        assert_ne!(a.salt, b.salt);
        assert_ne!(a.salt, [0_u8; 32]);
        assert_ne!(a.token("path", "/home/u"), b.token("path", "/home/u"));
    }
    use crate::operation::{
        ClassifiedFailure, DurabilityProfile, FailureClass, IdempotencyKey, OperationId,
    };
    use crate::operation_journal::{OperationRecord, OperationStatus, OperationStep, StepStatus};
    use crate::path_identity::PathIdentity;
    use crate::testutil::TempDir;
    use crate::transfer::{CopyMethod, OverwritePolicy, TransferKind};

    #[test]
    fn bundle_redacts_paths_identifiers_and_failure_text_while_preserving_aggregates() {
        let secret = PathBuf::from("/Users/alice/secret-project/client-list.txt");
        let operation = OperationRecord {
            id: OperationId("operation-secret".to_string()),
            group_id: None,
            kind: TransferKind::Copy,
            target: secret.clone(),
            policy: OverwritePolicy::OverwriteAll,
            method: CopyMethod::Native,
            durability: DurabilityProfile::Verified,
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: None,
            rollback_cleanup: Some(secret.clone()),
            rollback_cleanup_identity: Some(PathIdentity::missing(&secret)),
            rollback_cleanup_quarantine: None,
            status: OperationStatus::Failed,
            created_at_secs: 10,
            updated_at_secs: 16,
            steps: vec![OperationStep {
                key: IdempotencyKey("step-secret".to_string()),
                source: secret.clone(),
                destination: secret.with_file_name("copied.txt"),
                source_before: None,
                source_followed: Vec::new(),
                source_logical_bytes: None,
                source_proof_complete: false,
                destination_before: None,
                landing: None,
                landing_before: None,
                destination_after: None,
                staging: None,
                checkpoint: None,
                fast_path: Some(crate::transfer_tuning::FastPath::Native),
                replacement: None,
                placement: None,
                rollback: None,
                rollback_quarantine: None,
                status: StepStatus::Failed,
                attempts: 2,
                failure: Some(ClassifiedFailure::message(
                    FailureClass::Blocked,
                    Some(secret.clone()),
                    "alice secret access denied",
                )),
                preflight_error: Some("secret preflight".to_string()),
            }],
        };
        let version = crate::version_store::VersionRecord {
            operation_id: operation.id.clone(),
            key: IdempotencyKey("version-secret".to_string()),
            original: secret.clone(),
            stored: secret.with_file_name("stored-secret.txt"),
            created_at_secs: 12,
        };
        let bundle = build(
            std::slice::from_ref(&secret),
            std::slice::from_ref(&operation),
            &[version],
            Vec::new(),
        );
        let json = serde_json::to_string(&bundle).unwrap();
        for private in [
            "alice",
            "secret-project",
            "client-list",
            "operation-secret",
            "step-secret",
            "access denied",
            "/Users/",
        ] {
            assert!(!json.contains(private), "bundle leaked {private}");
        }
        let span = &bundle.operation_spans[0];
        assert_eq!(span.duration_secs, 6);
        assert_eq!(span.failed_steps, 1);
        assert_eq!(span.attempts, 2);
        assert_eq!(span.failure_classes.get("Blocked"), Some(&1));
        assert_eq!(span.target, bundle.volumes[0].requested);
        assert!(!bundle.operation_spans_truncated);
        assert!(!bundle.preserved_versions_truncated);
    }

    #[test]
    fn bundle_keeps_only_the_newest_operations_and_versions() {
        let operations: Vec<OperationRecord> = (0..=MAX_OPERATION_SPANS as u64)
            .map(|index| OperationRecord {
                id: OperationId(format!("operation-{index}")),
                group_id: None,
                kind: TransferKind::Copy,
                target: PathBuf::from("/tmp/target"),
                policy: OverwritePolicy::OverwriteAll,
                method: CopyMethod::Native,
                durability: DurabilityProfile::Verified,
                version_retention: crate::operation::VersionRetentionPolicy::default(),
                name_policy: crate::filesystem_policy::NamePolicy::default(),
                symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
                post_success: None,
                rollback_cleanup: None,
                rollback_cleanup_identity: None,
                rollback_cleanup_quarantine: None,
                status: OperationStatus::Completed,
                created_at_secs: 0,
                // The duration tells records apart after redaction.
                updated_at_secs: index,
                steps: Vec::new(),
            })
            .collect();
        let versions: Vec<_> = (0..=MAX_VERSION_SUMMARIES as u64)
            .map(|index| crate::version_store::VersionRecord {
                operation_id: OperationId(format!("operation-{index}")),
                key: IdempotencyKey(format!("version-{index}")),
                original: PathBuf::from("/tmp/original"),
                stored: PathBuf::from("/tmp/stored"),
                created_at_secs: index,
            })
            .collect();

        let bundle = build(&[], &operations, &versions, Vec::new());

        assert!(bundle.operation_spans_truncated);
        assert_eq!(bundle.operation_spans.len(), MAX_OPERATION_SPANS);
        // The oldest (first) record is the one dropped.
        assert_eq!(bundle.operation_spans[0].duration_secs, 1);
        assert_eq!(
            bundle.operation_spans.last().unwrap().duration_secs,
            MAX_OPERATION_SPANS as u64
        );
        assert!(bundle.preserved_versions_truncated);
        assert_eq!(bundle.preserved_versions.len(), MAX_VERSION_SUMMARIES);
        assert_eq!(
            bundle.preserved_versions[0].age_secs,
            bundle.generated_at_secs - 1
        );

        let exact = build(&[], &operations[1..], &versions[1..], Vec::new());
        assert!(!exact.operation_spans_truncated);
        assert!(!exact.preserved_versions_truncated);
        assert_eq!(exact.operation_spans.len(), MAX_OPERATION_SPANS);
    }

    #[test]
    fn support_bundle_is_written_as_valid_structured_json() {
        let temp = TempDir::new();
        let path = temp.path().join("support.json");
        let bundle = build(&[], &[], &[], vec!["journal unavailable".to_string()]);
        export_to(&path, &bundle).unwrap();
        let decoded: SupportBundle = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(decoded.schema, 2);
        assert_eq!(decoded.collection_warnings, vec!["journal unavailable"]);
    }

    #[test]
    fn encrypted_export_keeps_plaintext_preview_and_round_trips() {
        let bundle = build(&[], &[], &[], Vec::new());
        let preview = plaintext_preview(&bundle);
        assert!(preview.contains("schema=2"));
        let temp = TempDir::new();
        let json = serde_json::to_vec_pretty(&bundle).unwrap();
        let sealed = crate::support_encrypt::seal_support_bytes_with_secret(
            "qa",
            "qa-secret",
            &json,
            &preview,
            50,
            100,
        )
        .unwrap();
        assert!(!sealed.manifest.expired(100));
        let path = crate::support_encrypt::write_sealed_bundle(temp.path(), &sealed).unwrap();
        let loaded = crate::encrypted_bundle::load_manifest(&path).unwrap();
        let recovered = crate::encrypted_bundle::decrypt(&loaded, "qa-secret", 100).unwrap();
        let decoded: SupportBundle = serde_json::from_slice(&recovered).unwrap();
        assert_eq!(decoded.schema, bundle.schema);
    }
}
