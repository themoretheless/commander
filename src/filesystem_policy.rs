//! Cross-filesystem name, link, and capability policy.
//!
//! These values are serializable so a reviewed operation can retain the exact
//! policy that produced its preflight instead of consulting mutable UI state.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalizationForm {
    #[default]
    Preserve,
    Nfc,
    Nfd,
}

impl NormalizationForm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preserve => "Preserve spelling",
            Self::Nfc => "Unicode NFC",
            Self::Nfd => "Unicode NFD",
        }
    }

    pub fn apply(self, name: &str) -> String {
        match self {
            Self::Preserve => name.to_string(),
            Self::Nfc => name.nfc().collect(),
            Self::Nfd => name.nfd().collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionPolicy {
    #[default]
    Ask,
    KeepBoth,
    Skip,
}

impl CollisionPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask on collision",
            Self::KeepBoth => "Keep both",
            Self::Skip => "Skip incoming",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamePolicy {
    pub normalization: NormalizationForm,
    pub collision: CollisionPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameChange {
    pub original: String,
    pub normalized: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NormalizationPreview {
    pub changes: Vec<NameChange>,
    pub collisions: Vec<Vec<String>>,
}

pub fn preview_normalization<'a>(
    names: impl IntoIterator<Item = &'a str>,
    form: NormalizationForm,
    case_sensitive: bool,
) -> NormalizationPreview {
    let mut preview = NormalizationPreview::default();
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in names {
        let normalized = form.apply(name);
        if normalized != name {
            preview.changes.push(NameChange {
                original: name.to_string(),
                normalized: normalized.clone(),
            });
        }
        let key = if case_sensitive {
            normalized
        } else {
            normalized.to_lowercase()
        };
        groups.entry(key).or_default().push(name.to_string());
    }
    preview.collisions = groups
        .into_values()
        .filter(|group| group.len() > 1)
        .collect();
    preview
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PortabilityTarget {
    Posix,
    MacOs,
    Windows,
    Fat,
}

impl PortabilityTarget {
    pub fn label(self) -> &'static str {
        match self {
            Self::Posix => "POSIX",
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Fat => "FAT/exFAT",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortabilityIssue {
    pub name: String,
    pub targets: BTreeSet<PortabilityTarget>,
    pub reason: String,
}

pub fn audit_name(name: &str) -> Vec<PortabilityIssue> {
    let mut issues = Vec::new();
    let all = BTreeSet::from([
        PortabilityTarget::Posix,
        PortabilityTarget::MacOs,
        PortabilityTarget::Windows,
        PortabilityTarget::Fat,
    ]);
    if name.is_empty() || matches!(name, "." | "..") || name.contains('\0') || name.contains('/') {
        issues.push(PortabilityIssue {
            name: name.to_string(),
            targets: all,
            reason: "empty, reserved, NUL, and slash names are not portable".to_string(),
        });
        return issues;
    }

    let windows = BTreeSet::from([PortabilityTarget::Windows, PortabilityTarget::Fat]);
    if name.chars().any(|character| {
        character.is_control()
            || matches!(character, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')
    }) {
        issues.push(PortabilityIssue {
            name: name.to_string(),
            targets: windows.clone(),
            reason: "contains a Windows/FAT-reserved character".to_string(),
        });
    }
    if name.ends_with([' ', '.']) {
        issues.push(PortabilityIssue {
            name: name.to_string(),
            targets: windows.clone(),
            reason: "trailing spaces and dots are discarded on Windows/FAT".to_string(),
        });
    }
    let stem = name.split('.').next().unwrap_or(name);
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.iter().any(|item| stem.eq_ignore_ascii_case(item)) {
        issues.push(PortabilityIssue {
            name: name.to_string(),
            targets: windows.clone(),
            reason: "uses a reserved Windows device name".to_string(),
        });
    }
    if name.encode_utf16().count() > 255 {
        issues.push(PortabilityIssue {
            name: name.to_string(),
            targets: windows,
            reason: "exceeds the common 255 UTF-16-unit component limit".to_string(),
        });
    }
    issues
}

pub fn audit_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<PortabilityIssue> {
    names.into_iter().flat_map(audit_name).collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymlinkPolicy {
    #[default]
    Preserve,
    Follow,
    Skip,
}

impl SymlinkPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preserve => "Preserve links",
            Self::Follow => "Copy link targets",
            Self::Skip => "Skip links",
        }
    }

    pub fn consequence(self) -> &'static str {
        match self {
            Self::Preserve => "links remain links and targets are not traversed",
            Self::Follow => "target data is copied with cycle detection",
            Self::Skip => "links are omitted from the operation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    Read,
    Write,
    Rename,
    Clone,
    Trash,
    ExtendedAttributes,
}

impl Capability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Rename => "Atomic rename",
            Self::Clone => "Clone",
            Self::Trash => "Trash",
            Self::ExtendedAttributes => "Extended attributes",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityState {
    Available,
    Fallback,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityVerdict {
    pub capability: Capability,
    pub state: CapabilityState,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityMatrix {
    pub volume_id: u64,
    pub generation: u64,
    pub backend: crate::volume_profile::BackendKind,
    pub case_sensitive: Option<bool>,
    pub rows: Vec<CapabilityVerdict>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationPreflight {
    pub normalization: NormalizationPreview,
    pub portability: Vec<PortabilityIssue>,
    pub capabilities: CapabilityMatrix,
}

impl CapabilityMatrix {
    pub fn state(&self, capability: Capability) -> CapabilityState {
        self.rows
            .iter()
            .find(|row| row.capability == capability)
            .map_or(CapabilityState::Unavailable, |row| row.state)
    }

    pub fn unavailable_labels(&self) -> Vec<&'static str> {
        self.rows
            .iter()
            .filter(|row| row.state == CapabilityState::Unavailable)
            .map(|row| row.capability.label())
            .collect()
    }
}

pub fn matrix_for_profile(
    profile: &crate::volume_profile::VolumeProfile,
    readable: bool,
) -> CapabilityMatrix {
    use crate::volume_profile::BackendKind;
    let writable = !profile.read_only;
    let xattrs = matches!(
        profile.filesystem.to_ascii_lowercase().as_str(),
        "apfs" | "hfs" | "hfs+" | "ext4" | "btrfs" | "xfs"
    );
    let row = |capability, state, reason: &str| CapabilityVerdict {
        capability,
        state,
        reason: reason.to_string(),
    };
    CapabilityMatrix {
        volume_id: profile.volume_id,
        generation: profile.generation,
        backend: profile.backend,
        case_sensitive: profile.case_sensitive,
        rows: vec![
            row(
                Capability::Read,
                if readable {
                    CapabilityState::Available
                } else {
                    CapabilityState::Unavailable
                },
                if readable {
                    "root is readable"
                } else {
                    "root could not be inspected"
                },
            ),
            row(
                Capability::Write,
                if writable {
                    CapabilityState::Available
                } else {
                    CapabilityState::Unavailable
                },
                if writable {
                    "volume is writable"
                } else {
                    "volume is read-only"
                },
            ),
            row(
                Capability::Rename,
                if profile.capabilities.atomic_rename {
                    CapabilityState::Available
                } else if writable {
                    CapabilityState::Fallback
                } else {
                    CapabilityState::Unavailable
                },
                "same-volume atomic placement is preferred; copy is the fallback",
            ),
            row(
                Capability::Clone,
                if profile.capabilities.clone {
                    CapabilityState::Available
                } else if writable {
                    CapabilityState::Fallback
                } else {
                    CapabilityState::Unavailable
                },
                "buffered copy is used when cloning is unavailable",
            ),
            row(
                Capability::Trash,
                if !writable {
                    CapabilityState::Unavailable
                } else if matches!(profile.backend, BackendKind::Remote | BackendKind::Unknown) {
                    CapabilityState::Fallback
                } else {
                    CapabilityState::Available
                },
                "remote backends may delete directly when no trash contract exists",
            ),
            row(
                Capability::ExtendedAttributes,
                if xattrs && writable {
                    CapabilityState::Available
                } else if writable {
                    CapabilityState::Fallback
                } else {
                    CapabilityState::Unavailable
                },
                "metadata is copied when the destination filesystem supports it",
            ),
        ],
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureKind {
    CaseSensitiveApfs,
    CaseInsensitiveApfs,
    ReadOnly,
    Smb,
    Nfs,
    Disconnect,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixtureContract {
    pub case_sensitive: bool,
    pub read_only: bool,
    pub remote: bool,
    pub disconnectable: bool,
    pub filesystem: &'static str,
}

#[cfg(test)]
impl FixtureKind {
    pub fn contract(self) -> FixtureContract {
        match self {
            Self::CaseSensitiveApfs => FixtureContract {
                case_sensitive: true,
                read_only: false,
                remote: false,
                disconnectable: false,
                filesystem: "apfs",
            },
            Self::CaseInsensitiveApfs => FixtureContract {
                case_sensitive: false,
                read_only: false,
                remote: false,
                disconnectable: false,
                filesystem: "apfs",
            },
            Self::ReadOnly => FixtureContract {
                case_sensitive: true,
                read_only: true,
                remote: false,
                disconnectable: false,
                filesystem: "apfs",
            },
            Self::Smb => FixtureContract {
                case_sensitive: true,
                read_only: false,
                remote: true,
                disconnectable: true,
                filesystem: "smbfs",
            },
            Self::Nfs => FixtureContract {
                case_sensitive: true,
                read_only: false,
                remote: true,
                disconnectable: true,
                filesystem: "nfs",
            },
            Self::Disconnect => FixtureContract {
                case_sensitive: true,
                read_only: false,
                remote: true,
                disconnectable: true,
                filesystem: "smbfs",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preview_exposes_changes_and_case_collisions() {
        let decomposed = "Cafe\u{301}.txt";
        let preview = preview_normalization(
            [decomposed, "Caf\u{e9}.txt", "README", "readme"],
            NormalizationForm::Nfc,
            false,
        );
        assert_eq!(preview.changes[0].normalized, "Caf\u{e9}.txt");
        assert_eq!(preview.collisions.len(), 2);
    }

    #[test]
    fn portability_audit_reports_reserved_and_lossy_names() {
        let issues = audit_names(["CON.txt", "draft. ", "valid.txt"]);
        assert!(issues.iter().any(|issue| issue.name == "CON.txt"));
        assert!(issues.iter().any(|issue| issue.name == "draft. "));
        assert!(!issues.iter().any(|issue| issue.name == "valid.txt"));
    }

    #[test]
    fn fixture_matrix_covers_required_filesystem_contracts() {
        let fixtures = [
            FixtureKind::CaseSensitiveApfs,
            FixtureKind::CaseInsensitiveApfs,
            FixtureKind::ReadOnly,
            FixtureKind::Smb,
            FixtureKind::Nfs,
            FixtureKind::Disconnect,
        ];
        assert!(fixtures.iter().any(|fixture| fixture.contract().read_only));
        assert!(
            fixtures
                .iter()
                .any(|fixture| !fixture.contract().case_sensitive)
        );
        assert_eq!(
            fixtures
                .iter()
                .filter(|fixture| fixture.contract().remote)
                .count(),
            3
        );
    }

    #[test]
    fn operation_preflight_runs_across_the_filesystem_fixture_matrix() {
        use crate::volume_profile::{BackendKind, VolumeCapabilities, VolumeProfile};

        let fixtures = [
            FixtureKind::CaseSensitiveApfs,
            FixtureKind::CaseInsensitiveApfs,
            FixtureKind::ReadOnly,
            FixtureKind::Smb,
            FixtureKind::Nfs,
            FixtureKind::Disconnect,
        ];
        for (index, fixture) in fixtures.into_iter().enumerate() {
            let contract = fixture.contract();
            let backend = if contract.remote {
                BackendKind::Remote
            } else {
                BackendKind::LocalFast
            };
            let profile = VolumeProfile {
                volume_id: index as u64 + 1,
                generation: 1,
                backend,
                filesystem: contract.filesystem.to_string(),
                mount_point: format!("/fixture/{index}").into(),
                read_only: contract.read_only,
                case_sensitive: Some(contract.case_sensitive),
                capabilities: VolumeCapabilities {
                    atomic_rename: !contract.read_only && !contract.remote,
                    clone: !contract.read_only && !contract.remote,
                    sparse: !contract.read_only,
                    resumable: !contract.read_only,
                    delta: !contract.read_only && contract.remote,
                    max_concurrency: if contract.remote { 2 } else { 4 },
                },
                reason: format!("{} fixture", contract.filesystem),
            };
            let matrix = matrix_for_profile(&profile, true);
            assert_eq!(matrix.backend == BackendKind::Remote, contract.remote);
            assert_eq!(
                matrix.state(Capability::Write) == CapabilityState::Unavailable,
                contract.read_only
            );
            let names = preview_normalization(
                ["README", "readme"],
                NormalizationForm::Nfc,
                contract.case_sensitive,
            );
            assert_eq!(names.collisions.is_empty(), contract.case_sensitive);
            if fixture == FixtureKind::Disconnect {
                assert!(contract.disconnectable);
            }
        }
    }
}
