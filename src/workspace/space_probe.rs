use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use crate::panel::FileEntry;
use crate::ports::{FreeSpacePort, NativeFailure, NativeFailureKind, VolumeRelation};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SpaceProbeReport {
    pub generation: u64,
    pub target: PathBuf,
    pub need_bytes: Result<u64, NativeFailure>,
    pub expectations: Result<Vec<crate::transfer::TransferExpectation>, NativeFailure>,
    pub free: crate::ports::SpaceProbeOutcome,
    pub relation: VolumeRelation,
}

struct ActiveProbe {
    generation: u64,
    target: PathBuf,
    receiver: Option<mpsc::Receiver<SpaceProbeReport>>,
    immediate: Option<SpaceProbeReport>,
}

#[derive(Default)]
pub(super) struct SpaceProbeController {
    generation: u64,
    active: Option<ActiveProbe>,
}

impl SpaceProbeController {
    pub(super) fn start(
        &mut self,
        entries: Vec<FileEntry>,
        target: PathBuf,
        flat: crate::scan::FlatList,
        symlink_policy: crate::filesystem_policy::SymlinkPolicy,
        port: Arc<dyn FreeSpacePort>,
        notify: impl Fn() + Send + 'static,
    ) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let worker_target = target.clone();
        let panic_target = target.clone();
        let fallback = crate::scan::shallow_preview(&entries);
        let worker_fallback = fallback.clone();
        let worker_flat = flat.clone();
        let failed_spawn_flat = flat.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let notify = Arc::new(std::sync::Mutex::new(notify));
        let worker_notify = Arc::clone(&notify);
        let spawn = std::thread::Builder::new()
            .name("space-probe".to_string())
            .spawn(move || {
                let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_probe(
                        generation,
                        entries,
                        worker_target,
                        flat,
                        symlink_policy,
                        port.as_ref(),
                    )
                }))
                .unwrap_or_else(|_| {
                    failed_report(
                        generation,
                        panic_target,
                        "Resource preflight worker panicked".to_string(),
                    )
                });
                let mut published = crate::lock_util::recover(&worker_flat);
                if published.is_none() {
                    *published = Some(worker_fallback);
                }
                drop(published);
                let _ = sender.send(report);
                (crate::lock_util::recover(&worker_notify))();
            });
        let immediate = spawn.err().map(|error| {
            failed_report(
                generation,
                target.clone(),
                format!("Could not start resource preflight: {error}"),
            )
        });
        if immediate.is_some() {
            *crate::lock_util::recover(&failed_spawn_flat) = Some(fallback);
            (crate::lock_util::recover(&notify))();
        }
        self.active = Some(ActiveProbe {
            generation,
            target,
            receiver: immediate.is_none().then_some(receiver),
            immediate,
        });
        generation
    }

    pub(super) fn poll(&mut self) -> Option<SpaceProbeReport> {
        let mut active = self.active.take()?;
        if let Some(report) = active.immediate.take() {
            return Some(report);
        }
        let Some(receiver) = active.receiver.as_ref() else {
            return Some(failed_report(
                active.generation,
                active.target,
                "Resource preflight lost its worker channel".to_string(),
            ));
        };
        match receiver.try_recv() {
            Ok(report)
                if report.generation == active.generation && report.target == active.target =>
            {
                Some(report)
            }
            Ok(_) => Some(failed_report(
                active.generation,
                active.target,
                "Resource preflight returned a mismatched binding".to_string(),
            )),
            Err(mpsc::TryRecvError::Empty) => {
                self.active = Some(active);
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => Some(failed_report(
                active.generation,
                active.target,
                "Resource preflight worker stopped before publishing a result".to_string(),
            )),
        }
    }

    #[cfg(test)]
    pub(super) fn finish(&mut self) -> Option<SpaceProbeReport> {
        let mut active = self.active.take()?;
        if let Some(report) = active.immediate.take() {
            return Some(report);
        }
        let report = active.receiver.take()?.recv();
        match report {
            Ok(report)
                if report.generation == active.generation && report.target == active.target =>
            {
                Some(report)
            }
            Ok(_) => Some(failed_report(
                active.generation,
                active.target,
                "Resource preflight returned a mismatched binding".to_string(),
            )),
            Err(_) => Some(failed_report(
                active.generation,
                active.target,
                "Resource preflight worker stopped before publishing a result".to_string(),
            )),
        }
    }
}

fn failed_report(generation: u64, target: PathBuf, message: String) -> SpaceProbeReport {
    let failure = NativeFailure {
        kind: NativeFailureKind::Unknown,
        message,
    };
    SpaceProbeReport {
        generation,
        target,
        need_bytes: Err(failure.clone()),
        expectations: Err(failure.clone()),
        free: crate::ports::SpaceProbeOutcome::Unknown(failure.clone()),
        relation: VolumeRelation::Unknown(failure),
    }
}

fn run_probe(
    generation: u64,
    entries: Vec<FileEntry>,
    target: PathBuf,
    flat: crate::scan::FlatList,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    port: &dyn FreeSpacePort,
) -> SpaceProbeReport {
    let scan = crate::scan::transfer_preflight(&entries, symlink_policy);
    *crate::lock_util::recover(&flat) = Some(scan.flat);
    let expectations = crate::transfer::capture_expectations(&entries, &target);
    let free = port.probe(&target);
    let relation = batch_volume_relation(&entries, &target, port);
    SpaceProbeReport {
        generation,
        target,
        need_bytes: scan.need_bytes,
        expectations: Ok(expectations),
        free,
        relation,
    }
}

fn batch_volume_relation(
    entries: &[FileEntry],
    target: &Path,
    port: &dyn FreeSpacePort,
) -> VolumeRelation {
    if entries.is_empty() {
        return VolumeRelation::Unknown(NativeFailure {
            kind: NativeFailureKind::InvalidInput,
            message: "the transfer has no source entries".to_string(),
        });
    }
    let mut sources = std::collections::BTreeSet::new();
    for entry in entries {
        let Some(source) = entry.path.parent() else {
            return VolumeRelation::Unknown(NativeFailure {
                kind: NativeFailureKind::InvalidInput,
                message: "a transfer source has no parent directory".to_string(),
            });
        };
        sources.insert(source.to_path_buf());
    }
    let mut unknown = None;
    for source in sources {
        match port.volume_relation(&source, target) {
            VolumeRelation::Same => {}
            VolumeRelation::Different => return VolumeRelation::Different,
            VolumeRelation::Unknown(failure) => unknown = Some(failure),
        }
    }
    unknown.map_or(VolumeRelation::Same, VolumeRelation::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    struct PanickingFreeSpace;

    impl FreeSpacePort for PanickingFreeSpace {
        fn probe(&self, _path: &Path) -> crate::ports::SpaceProbeOutcome {
            panic!("scripted probe panic");
        }

        fn volume_relation(&self, _source: &Path, _target: &Path) -> VolumeRelation {
            VolumeRelation::Same
        }
    }

    #[test]
    fn worker_panic_retires_as_typed_indeterminate_report() {
        let source = TempDir::new();
        let target = TempDir::new();
        let path = source.file("value.bin", "123");
        let metadata = std::fs::metadata(&path).unwrap();
        let entry = FileEntry::from_meta(path, &metadata).unwrap();
        let flat = crate::scan::pending_flat_list();
        let mut controller = SpaceProbeController::default();
        let generation = controller.start(
            vec![entry],
            target.path().to_path_buf(),
            flat,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
            Arc::new(PanickingFreeSpace),
            || {},
        );

        let report = controller.finish().expect("typed panic report");
        assert_eq!(report.generation, generation);
        assert!(report.need_bytes.is_err());
        assert!(report.expectations.is_err());
        assert!(matches!(
            report.free,
            crate::ports::SpaceProbeOutcome::Unknown(_)
        ));
        assert!(controller.active.is_none());
    }

    #[test]
    fn mismatched_report_retires_the_probe_with_a_typed_failure() {
        let target = PathBuf::from("/expected-target");
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(SpaceProbeReport {
                generation: 2,
                target: PathBuf::from("/wrong-target"),
                need_bytes: Ok(1),
                expectations: Ok(Vec::new()),
                free: crate::ports::SpaceProbeOutcome::Known {
                    bytes: 1,
                    precision: crate::ports::SpacePrecision::Exact,
                },
                relation: VolumeRelation::Same,
            })
            .unwrap();
        let mut controller = SpaceProbeController {
            generation: 1,
            active: Some(ActiveProbe {
                generation: 1,
                target: target.clone(),
                receiver: Some(receiver),
                immediate: None,
            }),
        };

        let report = controller.poll().expect("typed mismatch report");

        assert_eq!(report.generation, 1);
        assert_eq!(report.target, target);
        assert!(report.need_bytes.is_err());
        assert!(report.expectations.is_err());
        assert!(controller.active.is_none());
    }

    #[test]
    fn disconnected_probe_retires_with_a_typed_failure() {
        let target = PathBuf::from("/expected-target");
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        let mut controller = SpaceProbeController {
            generation: 1,
            active: Some(ActiveProbe {
                generation: 1,
                target: target.clone(),
                receiver: Some(receiver),
                immediate: None,
            }),
        };

        let report = controller.poll().expect("typed disconnected report");

        assert_eq!(report.target, target);
        assert!(report.need_bytes.is_err());
        assert!(report.expectations.is_err());
        assert!(controller.active.is_none());
    }
}
