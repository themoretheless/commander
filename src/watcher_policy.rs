//! Pure backend policy for filesystem observation.

use crate::volume_profile::BackendKind;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatcherBackend {
    Native,
    Polling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchDepth {
    Recursive,
    DirectoryOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatcherPolicy {
    pub backend: WatcherBackend,
    pub depth: WatchDepth,
    pub coalesce_window: Duration,
    pub poll_interval: Duration,
}

/// Match observation cost and reliability to the mounted backend. Native
/// recursive events retain deep directory-size invalidation on local disks;
/// uncertain or remote trees use a shallow, paced scan.
pub const fn policy_for(backend: BackendKind) -> WatcherPolicy {
    match backend {
        BackendKind::LocalFast => WatcherPolicy {
            backend: WatcherBackend::Native,
            depth: WatchDepth::Recursive,
            coalesce_window: Duration::from_millis(75),
            poll_interval: Duration::from_secs(2),
        },
        BackendKind::LocalSlow => WatcherPolicy {
            backend: WatcherBackend::Native,
            depth: WatchDepth::Recursive,
            coalesce_window: Duration::from_millis(150),
            poll_interval: Duration::from_secs(3),
        },
        BackendKind::Removable => WatcherPolicy {
            backend: WatcherBackend::Native,
            depth: WatchDepth::DirectoryOnly,
            coalesce_window: Duration::from_millis(250),
            poll_interval: Duration::from_secs(3),
        },
        BackendKind::Remote => WatcherPolicy {
            backend: WatcherBackend::Polling,
            depth: WatchDepth::DirectoryOnly,
            coalesce_window: Duration::from_millis(350),
            poll_interval: Duration::from_secs(2),
        },
        BackendKind::Unknown => WatcherPolicy {
            backend: WatcherBackend::Polling,
            depth: WatchDepth::DirectoryOnly,
            coalesce_window: Duration::from_millis(500),
            poll_interval: Duration::from_secs(5),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_and_unknown_backends_use_shallow_polling() {
        for backend in [BackendKind::Remote, BackendKind::Unknown] {
            let policy = policy_for(backend);
            assert_eq!(policy.backend, WatcherBackend::Polling);
            assert_eq!(policy.depth, WatchDepth::DirectoryOnly);
            assert!(policy.poll_interval >= Duration::from_secs(2));
        }
    }

    #[test]
    fn local_fast_backend_keeps_recursive_native_events() {
        let policy = policy_for(BackendKind::LocalFast);
        assert_eq!(policy.backend, WatcherBackend::Native);
        assert_eq!(policy.depth, WatchDepth::Recursive);
        assert!(policy.coalesce_window < Duration::from_millis(100));
    }
}
