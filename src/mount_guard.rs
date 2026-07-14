//! Bounded disconnect detection for long-running operations.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectPolicy {
    pub timeout_ms: u64,
    pub poll_interval_ms: u64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            poll_interval_ms: 500,
        }
    }
}

impl ReconnectPolicy {
    fn timeout(self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    fn poll_interval(self) -> Duration {
        Duration::from_millis(self.poll_interval_ms.max(1))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountGuard {
    pub volume_id: u64,
    pub generation: u64,
    pub mount_point: PathBuf,
    pub policy: ReconnectPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountAvailability {
    Available,
    Disconnected,
    Replaced,
}

impl MountGuard {
    pub fn capture(path: &Path, policy: ReconnectPolicy) -> Self {
        let profile = crate::volume_profile::profile(path);
        Self::from_profile(&profile, policy)
    }

    pub fn from_profile(
        profile: &crate::volume_profile::VolumeProfile,
        policy: ReconnectPolicy,
    ) -> Self {
        Self {
            volume_id: profile.volume_id,
            generation: profile.generation,
            mount_point: profile.mount_point.clone(),
            policy,
        }
    }

    pub fn check(&self) -> MountAvailability {
        if !self.mount_point.exists() {
            return MountAvailability::Disconnected;
        }
        let current = crate::volume_profile::refresh(&self.mount_point);
        if current.mount_point != self.mount_point {
            MountAvailability::Disconnected
        } else if current.volume_id != self.volume_id || current.generation != self.generation {
            MountAvailability::Replaced
        } else {
            MountAvailability::Available
        }
    }

    pub fn wait_until_available(&self, mut on_wait: impl FnMut() -> bool) -> std::io::Result<()> {
        self.wait_with(|| self.check(), &mut on_wait)
    }

    fn wait_with(
        &self,
        mut check: impl FnMut() -> MountAvailability,
        mut on_wait: impl FnMut() -> bool,
    ) -> std::io::Result<()> {
        let deadline = Instant::now() + self.policy.timeout();
        loop {
            match check() {
                MountAvailability::Available => return Ok(()),
                MountAvailability::Replaced => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::StaleNetworkFileHandle,
                        "mounted volume identity changed while the operation was paused",
                    ));
                }
                MountAvailability::Disconnected if Instant::now() >= deadline => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "volume did not reconnect before the operation timeout",
                    ));
                }
                MountAvailability::Disconnected => {
                    if !on_wait() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "operation cancelled while waiting for the volume",
                        ));
                    }
                    std::thread::sleep(self.policy.poll_interval());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(policy: ReconnectPolicy) -> MountGuard {
        MountGuard {
            volume_id: 1,
            generation: 1,
            mount_point: PathBuf::from("/fixture"),
            policy,
        }
    }

    #[test]
    fn reconnect_wait_retains_state_until_the_same_mount_returns() {
        let mut checks = 0;
        let mut waits = 0;
        guard(ReconnectPolicy {
            timeout_ms: 100,
            poll_interval_ms: 1,
        })
        .wait_with(
            || {
                checks += 1;
                if checks < 3 {
                    MountAvailability::Disconnected
                } else {
                    MountAvailability::Available
                }
            },
            || {
                waits += 1;
                true
            },
        )
        .unwrap();
        assert_eq!(waits, 2);
    }

    #[test]
    fn a_different_mount_never_resumes_the_old_operation() {
        let error = guard(ReconnectPolicy::default())
            .wait_with(|| MountAvailability::Replaced, || true)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::StaleNetworkFileHandle);
    }
}
