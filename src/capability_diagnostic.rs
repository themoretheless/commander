//! Exportable explanations of filesystem fast paths and their fallbacks.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOutcome {
    FastPath,
    Fallback,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDecision {
    pub capability: String,
    pub outcome: CapabilityOutcome,
    pub selected_path: Option<String>,
    pub fallback: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeDiagnostic {
    pub requested_path: PathBuf,
    pub volume_id: u64,
    pub generation: u64,
    pub backend: crate::volume_profile::BackendKind,
    pub filesystem: String,
    pub mount_point: PathBuf,
    pub read_only: bool,
    pub case_sensitive: Option<bool>,
    pub max_concurrency: usize,
    pub profile_reason: String,
    pub decisions: Vec<CapabilityDecision>,
}

impl VolumeDiagnostic {
    #[must_use]
    pub fn decision(&self, capability: &str) -> Option<&CapabilityDecision> {
        self.decisions
            .iter()
            .find(|decision| decision.capability == capability)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDiagnostic {
    pub schema: u32,
    pub generated_at_secs: u64,
    pub app_version: String,
    pub volumes: Vec<VolumeDiagnostic>,
}

#[must_use]
pub fn collect(paths: &[PathBuf]) -> CapabilityDiagnostic {
    CapabilityDiagnostic {
        schema: SCHEMA,
        generated_at_secs: now_secs(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        volumes: paths
            .iter()
            .map(|path| {
                let profile = crate::volume_profile::profile(path);
                let read_error = read_probe(path).err();
                for_profile(path.clone(), profile, read_error)
            })
            .collect(),
    }
}

/// Probe whether the requested root can be read, keeping the OS error so the
/// report can tell a missing root from a denied or failing one.
fn read_probe(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let opened = if metadata.is_dir() {
        std::fs::read_dir(path).map(drop)
    } else {
        std::fs::File::open(path).map(drop)
    };
    opened.map_err(|error| error.to_string())
}

/// `read_error` is `None` when the root could be inspected, otherwise the OS
/// error that prevented it.
#[must_use]
pub fn for_profile(
    requested_path: PathBuf,
    profile: crate::volume_profile::VolumeProfile,
    read_error: Option<String>,
) -> VolumeDiagnostic {
    use CapabilityOutcome::{Fallback, FastPath, Unavailable};
    let writable = !profile.read_only;
    let unavailable = |capability: &str, reason: &str| CapabilityDecision {
        capability: capability.to_string(),
        outcome: Unavailable,
        selected_path: None,
        fallback: None,
        reason: reason.to_string(),
    };
    let route =
        |capability: &str, available: bool, fast_path: &str, fallback: &str, reason: &str| {
            if !writable {
                unavailable(capability, "volume is read-only")
            } else if available {
                CapabilityDecision {
                    capability: capability.to_string(),
                    outcome: FastPath,
                    selected_path: Some(fast_path.to_string()),
                    fallback: Some(fallback.to_string()),
                    reason: reason.to_string(),
                }
            } else {
                CapabilityDecision {
                    capability: capability.to_string(),
                    outcome: Fallback,
                    selected_path: Some(fallback.to_string()),
                    fallback: None,
                    reason: reason.to_string(),
                }
            }
        };

    let read = if let Some(error) = read_error {
        unavailable(
            "read",
            &format!("requested root could not be inspected: {error}"),
        )
    } else {
        CapabilityDecision {
            capability: "read".to_string(),
            outcome: FastPath,
            selected_path: Some("native filesystem read".to_string()),
            fallback: None,
            reason: "requested root can be inspected".to_string(),
        }
    };
    let write = if writable {
        CapabilityDecision {
            capability: "write".to_string(),
            outcome: FastPath,
            selected_path: Some("staged native write".to_string()),
            fallback: None,
            reason: "volume accepts mutations".to_string(),
        }
    } else {
        unavailable("write", "volume is read-only")
    };
    let decisions = vec![
        read,
        write,
        route(
            "same_volume_move",
            profile.capabilities.atomic_rename,
            "atomic rename",
            "staged copy, verification, then source removal",
            "atomic placement is used only when the volume contract permits it",
        ),
        route(
            "copy",
            profile.capabilities.clone,
            "copy-on-write clone",
            "buffered verified copy",
            "clone support avoids reading and rewriting unchanged data",
        ),
        route(
            "sparse_files",
            profile.capabilities.sparse,
            "sparse extent preservation",
            "dense buffered write",
            "sparse extents are preserved only on supporting filesystems",
        ),
        route(
            "resume",
            profile.capabilities.resumable,
            "checkpointed staging resume",
            "restart current file from byte zero",
            "resume requires a writable stable staging contract",
        ),
        route(
            "delta_transfer",
            profile.capabilities.delta,
            "fixed-block or content-defined delta",
            "full buffered transfer",
            "delta transfer is selected for measured slow-link profiles",
        ),
    ];
    VolumeDiagnostic {
        requested_path,
        volume_id: profile.volume_id,
        generation: profile.generation,
        backend: profile.backend,
        filesystem: profile.filesystem,
        mount_point: profile.mount_point,
        read_only: profile.read_only,
        case_sensitive: profile.case_sensitive,
        max_concurrency: profile.capabilities.max_concurrency,
        profile_reason: profile.reason,
        decisions,
    }
}

pub fn export(paths: &[PathBuf]) -> Result<PathBuf, String> {
    let directory = crate::fs_util::config_dir().join("diagnostics");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create diagnostics directory: {error}"))?;
    let path = directory.join(format!("capabilities-{}.json", now_millis()));
    export_to(&path, &collect(paths))?;
    Ok(path)
}

fn export_to(path: &Path, diagnostic: &CapabilityDiagnostic) -> Result<(), String> {
    let json = serde_json::to_string_pretty(diagnostic)
        .map_err(|error| format!("Could not encode capability diagnostic: {error}"))?;
    match crate::persistence::commit_bytes_atomic(path, json.as_bytes()) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => Ok(()),
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Capability diagnostic", &error);
            Ok(())
        }
        Err(error) => Err(format!(
            "Could not write capability diagnostic to {}: {}",
            path.display(),
            describe_commit_error(&error)
        )),
    }
}

/// Name the failed save stage and the OS error (e.g. disk full, access denied).
fn describe_commit_error(error: &crate::persistence::PreCommitError) -> String {
    use crate::persistence::PreCommitError;
    match error {
        PreCommitError::Serialize(source) => format!("could not encode: {source}"),
        PreCommitError::Conflict => "the file changed while it was being written".to_string(),
        PreCommitError::Io { stage, source, .. } => format!("could not {stage}: {source}"),
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
    use crate::testutil::TempDir;
    use crate::volume_profile::{BackendKind, VolumeCapabilities, VolumeProfile};

    fn profile(read_only: bool, backend: BackendKind) -> VolumeProfile {
        VolumeProfile {
            volume_id: 9,
            stable_id: None,
            generation: 3,
            backend,
            filesystem: "testfs".to_string(),
            mount_point: PathBuf::from("/volume"),
            read_only,
            case_sensitive: Some(true),
            capabilities: VolumeCapabilities {
                atomic_rename: !read_only && backend == BackendKind::LocalFast,
                clone: !read_only && backend == BackendKind::LocalFast,
                sparse: !read_only,
                resumable: !read_only,
                delta: !read_only && backend == BackendKind::Remote,
                max_concurrency: 2,
            },
            reason: "test profile".to_string(),
        }
    }

    #[test]
    fn diagnostic_explains_fast_fallback_and_unavailable_routes() {
        let local = for_profile(
            PathBuf::from("/volume/project"),
            profile(false, BackendKind::LocalFast),
            None,
        );
        assert_eq!(
            local.decision("same_volume_move").unwrap().outcome,
            CapabilityOutcome::FastPath
        );
        assert_eq!(
            local.decision("delta_transfer").unwrap().outcome,
            CapabilityOutcome::Fallback
        );

        let read_only = for_profile(
            PathBuf::from("/volume/readonly"),
            profile(true, BackendKind::Remote),
            None,
        );
        assert_eq!(
            read_only.decision("copy").unwrap().outcome,
            CapabilityOutcome::Unavailable
        );
        assert!(read_only.decision("copy").unwrap().selected_path.is_none());
    }

    #[test]
    fn capability_report_round_trips_as_structured_json() {
        let temp = TempDir::new();
        let path = temp.path().join("capabilities.json");
        let report = CapabilityDiagnostic {
            schema: 1,
            generated_at_secs: 7,
            app_version: "test".to_string(),
            volumes: vec![for_profile(
                PathBuf::from("/volume/project"),
                profile(false, BackendKind::Remote),
                None,
            )],
        };
        export_to(&path, &report).unwrap();
        let decoded: CapabilityDiagnostic =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(decoded, report);
    }

    #[test]
    fn export_failure_names_the_os_error() {
        let temp = TempDir::new();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, b"file").unwrap();
        let path = blocker.join("capabilities.json");
        let report = CapabilityDiagnostic {
            schema: 1,
            generated_at_secs: 7,
            app_version: "test".to_string(),
            volumes: Vec::new(),
        };
        let message = export_to(&path, &report).unwrap_err();
        let prefix = format!(
            "Could not write capability diagnostic to {}",
            path.display()
        );
        let cause = message.strip_prefix(&prefix).unwrap();
        assert!(cause.starts_with(": could not "), "{message}");
    }

    #[test]
    fn unreadable_root_reports_the_os_error() {
        let temp = TempDir::new();
        let missing = temp.path().join("missing");
        let error = read_probe(&missing).unwrap_err();
        let expected = std::fs::metadata(&missing).unwrap_err().to_string();
        assert_eq!(error, expected);
        let diagnostic = for_profile(
            missing,
            profile(false, BackendKind::LocalFast),
            Some(error.clone()),
        );
        let read = diagnostic.decision("read").unwrap();
        assert_eq!(read.outcome, CapabilityOutcome::Unavailable);
        assert!(read.reason.ends_with(&error), "{}", read.reason);
    }
}
