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
    const fn timeout(self) -> Duration {
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

/// Run `probe` on a helper thread and wait at most `timeout` for it. A probe
/// that has not answered in time counts as disconnected; its thread is left
/// to finish (or hang) on its own.
fn probe_bounded(
    probe: impl FnOnce() -> MountAvailability + Send + 'static,
    timeout: Duration,
) -> MountAvailability {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(probe());
    });
    receiver
        .recv_timeout(timeout)
        .unwrap_or(MountAvailability::Disconnected)
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

    /// One availability probe that gives up after a poll interval. A probe
    /// still running (a hung hard-mounted share) counts as disconnected, so
    /// callers can go on to the bounded reconnect wait instead of blocking.
    pub fn check_bounded(&self) -> MountAvailability {
        let guard = self.clone();
        probe_bounded(move || guard.check(), self.policy.poll_interval())
    }

    /// Run each availability probe on a helper thread and give it one poll
    /// interval: on a hung hard-mounted share `exists()`/`statfs` can block
    /// indefinitely, which would otherwise stall the timeout and cancel
    /// checks. A probe still running counts as disconnected, and no second
    /// probe is started until it returns, so a hung mount leaks one thread.
    pub(crate) fn wait_bounded(
        &self,
        probe: impl Fn() -> MountAvailability + Send + Clone + 'static,
        on_wait: impl FnMut() -> bool,
    ) -> std::io::Result<()> {
        let mut pending: Option<std::sync::mpsc::Receiver<MountAvailability>> = None;
        let interval = self.policy.poll_interval();
        let check = || {
            let receiver = pending.take().unwrap_or_else(|| {
                let (sender, receiver) = std::sync::mpsc::channel();
                let probe = probe.clone();
                std::thread::spawn(move || {
                    let _ = sender.send(probe());
                });
                receiver
            });
            match receiver.recv_timeout(interval) {
                Ok(availability) => availability,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    pending = Some(receiver);
                    MountAvailability::Disconnected
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    MountAvailability::Disconnected
                }
            }
        };
        self.wait_with(check, on_wait)
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

    #[test]
    fn a_hung_availability_probe_still_times_out() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = guard(ReconnectPolicy {
                timeout_ms: 50,
                poll_interval_ms: 5,
            })
            .wait_bounded(
                || {
                    std::thread::sleep(Duration::from_secs(3_600));
                    MountAvailability::Available
                },
                || true,
            );
            let _ = done.send(result.map_err(|error| error.kind()));
        });

        let result = finished
            .recv_timeout(Duration::from_secs(10))
            .expect("hung probe blocked the reconnect wait");

        assert_eq!(result, Err(std::io::ErrorKind::TimedOut));
    }

    #[test]
    fn a_hung_single_probe_counts_as_disconnected() {
        let started = Instant::now();
        let availability = probe_bounded(
            || {
                std::thread::sleep(Duration::from_secs(3_600));
                MountAvailability::Available
            },
            Duration::from_millis(20),
        );
        assert_eq!(availability, MountAvailability::Disconnected);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            probe_bounded(|| MountAvailability::Replaced, Duration::from_secs(5)),
            MountAvailability::Replaced
        );
    }

    #[test]
    fn bounded_wait_returns_once_the_probe_reports_available() {
        guard(ReconnectPolicy {
            timeout_ms: 1_000,
            poll_interval_ms: 200,
        })
        .wait_bounded(|| MountAvailability::Available, || true)
        .unwrap();
    }
}
