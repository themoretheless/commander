//! Shared operation policy and identity values. Filesystem execution stays in
//! `transfer`; persistence and recovery build on these small serializable types.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DurabilityProfile {
    Fast,
    #[default]
    Verified,
    Versioned,
}

impl DurabilityProfile {
    pub const ALL: [Self; 3] = [Self::Fast, Self::Verified, Self::Versioned];

    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Verified => "Verified",
            Self::Versioned => "Versioned",
        }
    }

    pub fn verifies(self) -> bool {
        matches!(self, Self::Verified | Self::Versioned)
    }

    pub fn keeps_versions(self) -> bool {
        self == Self::Versioned
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionRetentionPolicy {
    Compact,
    #[default]
    Recent,
    Archive,
    Forever,
}

impl VersionRetentionPolicy {
    pub const ALL: [Self; 4] = [Self::Compact, Self::Recent, Self::Archive, Self::Forever];

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Recent => "Recent",
            Self::Archive => "Archive",
            Self::Forever => "Forever",
        }
    }

    pub fn consequence(self) -> &'static str {
        match self {
            Self::Compact => "Keep 3 versions per path for up to 30 days",
            Self::Recent => "Keep 10 versions per path for up to 90 days",
            Self::Archive => "Keep 50 versions per path for up to one year",
            Self::Forever => "Keep every version until it is removed manually",
        }
    }

    pub const fn max_per_path(self) -> Option<usize> {
        match self {
            Self::Compact => Some(3),
            Self::Recent => Some(10),
            Self::Archive => Some(50),
            Self::Forever => None,
        }
    }

    pub const fn max_age_secs(self) -> Option<u64> {
        const DAY: u64 = 24 * 60 * 60;
        match self {
            Self::Compact => Some(30 * DAY),
            Self::Recent => Some(90 * DAY),
            Self::Archive => Some(365 * DAY),
            Self::Forever => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    Retryable,
    Blocked,
    UserDecision,
    IntegrityUncertain,
}

impl FailureClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Retryable => "Retryable",
            Self::Blocked => "Blocked",
            Self::UserDecision => "Needs decision",
            Self::IntegrityUncertain => "Review required",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedFailure {
    pub class: FailureClass,
    pub message: String,
    pub path: Option<PathBuf>,
}

impl ClassifiedFailure {
    pub fn io(path: Option<PathBuf>, context: &str, error: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        let class = match error.kind() {
            ErrorKind::Interrupted
            | ErrorKind::TimedOut
            | ErrorKind::WouldBlock
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionRefused => FailureClass::Retryable,
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem | ErrorKind::NotFound => {
                FailureClass::Blocked
            }
            ErrorKind::AlreadyExists | ErrorKind::InvalidInput | ErrorKind::InvalidFilename => {
                FailureClass::UserDecision
            }
            ErrorKind::UnexpectedEof
            | ErrorKind::WriteZero
            | ErrorKind::InvalidData
            | ErrorKind::StorageFull
            | ErrorKind::Other => FailureClass::IntegrityUncertain,
            _ => FailureClass::Blocked,
        };
        Self {
            class,
            message: format!("{context}: {error}"),
            path,
        }
    }

    pub fn message(class: FailureClass, path: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            path,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafeState {
    pub operation_id: OperationId,
    pub reason: String,
    pub paths: Vec<PathBuf>,
    pub failures: Vec<ClassifiedFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationGroupId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(pub String);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl OperationId {
    pub fn new() -> Self {
        Self(unique_id("op"))
    }

    pub fn step_key(&self, step: usize, path: &std::path::Path) -> IdempotencyKey {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in self
            .0
            .as_bytes()
            .iter()
            .copied()
            .chain(step.to_le_bytes())
            .chain(path.to_string_lossy().as_bytes().iter().copied())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        IdempotencyKey(format!("{}-{hash:016x}", self.0))
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationGroupId {
    pub fn new() -> Self {
        Self(unique_id("group"))
    }
}

fn unique_id(prefix: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{timestamp:032x}-{sequence:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durability_capabilities_are_explicit() {
        assert!(!DurabilityProfile::Fast.verifies());
        assert!(DurabilityProfile::Verified.verifies());
        assert!(DurabilityProfile::Versioned.verifies());
        assert!(DurabilityProfile::Versioned.keeps_versions());
        assert_eq!(
            VersionRetentionPolicy::default(),
            VersionRetentionPolicy::Recent
        );
        assert_eq!(VersionRetentionPolicy::Compact.max_per_path(), Some(3));
        assert_eq!(VersionRetentionPolicy::Forever.max_age_secs(), None);
    }

    #[test]
    fn idempotency_keys_are_stable_per_step_and_distinct_across_steps() {
        let operation = OperationId("op-fixed".to_string());
        let path = std::path::Path::new("/tmp/file");
        assert_eq!(operation.step_key(2, path), operation.step_key(2, path));
        assert_ne!(operation.step_key(2, path), operation.step_key(3, path));
    }

    #[test]
    fn io_failures_map_to_actionable_classes() {
        let retry = ClassifiedFailure::io(
            None,
            "copy",
            &std::io::Error::from(std::io::ErrorKind::TimedOut),
        );
        let decision = ClassifiedFailure::io(
            None,
            "place",
            &std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        );
        assert_eq!(retry.class, FailureClass::Retryable);
        assert_eq!(decision.class, FailureClass::UserDecision);
    }

    #[test]
    fn colocated_adrs_record_invariants_ownership_and_failure_policy() {
        let invariants = include_str!("operation/ADR-0001-durable-operation-invariants.md");
        let ownership = include_str!("operation/ADR-0002-operation-state-ownership.md");
        let failures = include_str!("operation/ADR-0003-failure-and-recovery-policy.md");
        for decision in [invariants, ownership, failures] {
            assert!(decision.contains("Status: Accepted"));
            assert!(decision.contains("## Decision"));
            assert!(decision.contains("## Consequences"));
        }
        assert!(invariants.contains("hidden sibling staging"));
        assert!(invariants.contains("idempotency key"));
        assert!(ownership.contains("operation_journal.rs"));
        assert!(ownership.contains("Presentation cannot weaken policy"));
        assert!(failures.contains("IntegrityUncertain"));
        assert!(failures.contains("kill switches"));
    }
}
