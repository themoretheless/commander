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

pub fn collect(paths: &[PathBuf]) -> CapabilityDiagnostic {
    CapabilityDiagnostic {
        schema: SCHEMA,
        generated_at_secs: now_secs(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        volumes: paths
            .iter()
            .map(|path| {
                let profile = crate::volume_profile::profile(path);
                let readable = std::fs::symlink_metadata(path).is_ok();
                for_profile(path.clone(), profile, readable)
            })
            .collect(),
    }
}

pub fn for_profile(
    requested_path: PathBuf,
    profile: crate::volume_profile::VolumeProfile,
    readable: bool,
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

    let read = if readable {
        CapabilityDecision {
            capability: "read".to_string(),
            outcome: FastPath,
            selected_path: Some("native filesystem read".to_string()),
            fallback: None,
            reason: "requested root can be inspected".to_string(),
        }
    } else {
        unavailable("read", "requested root could not be inspected")
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
    if crate::fs_util::write_atomic(path, &json) {
        Ok(())
    } else {
        Err(format!(
            "Could not write capability diagnostic to {}",
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
    use crate::testutil::TempDir;
    use crate::volume_profile::{BackendKind, VolumeCapabilities, VolumeProfile};

    fn profile(read_only: bool, backend: BackendKind) -> VolumeProfile {
        VolumeProfile {
            volume_id: 9,
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
            true,
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
            true,
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
                true,
            )],
        };
        export_to(&path, &report).unwrap();
        let decoded: CapabilityDiagnostic =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(decoded, report);
    }
}
