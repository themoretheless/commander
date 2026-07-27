//! Privacy-preserving support bundle assembled from structured runtime state.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
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
    fn new() -> Self {
        let mut salt = [0_u8; 32];
        let random =
            std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut salt));
        if random.is_err() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let mut hasher = blake3::Hasher::new();
            hasher.update(&nanos.to_le_bytes());
            hasher.update(&std::process::id().to_le_bytes());
            salt.copy_from_slice(hasher.finalize().as_bytes());
        }
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

pub fn collect(paths: &[PathBuf]) -> SupportBundle {
    let mut warnings = Vec::new();
    let operations = match crate::operation_journal::load() {
        Ok(journal) => journal.operations,
        Err(_) => {
            warnings.push("operation journal unavailable".to_string());
            Vec::new()
        }
    };
    let versions = crate::version_store::records();
    build(paths, &operations, &versions, warnings)
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
                destination_before: None,
                landing: None,
                landing_before: None,
                destination_after: None,
                staging: None,
                checkpoint: None,
                fast_path: Some(crate::transfer_tuning::FastPath::Native),
                replacement: None,
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
    fn support_bundle_is_written_as_valid_structured_json() {
        let temp = TempDir::new();
        let path = temp.path().join("support.json");
        let bundle = build(&[], &[], &[], vec!["journal unavailable".to_string()]);
        export_to(&path, &bundle).unwrap();
        let decoded: SupportBundle = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(decoded.schema, 2);
        assert_eq!(decoded.collection_warnings, vec!["journal unavailable"]);
    }
}
