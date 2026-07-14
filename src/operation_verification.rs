//! Deterministic fault, crash-restart, and small-state verification harness.

use crate::ports::{FileSystemEffect, FileSystemProvider, NativeFileSystemProvider};
use crate::testutil::TempDir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const SOURCE_BYTES: &[u8] = b"incoming-content";
const DESTINATION_BYTES: &[u8] = b"existing-content";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ModelOperation {
    Copy,
    Move,
    Sync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ConflictPolicy {
    Skip,
    Replace,
    KeepBoth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Planned,
    Staged,
    DestinationBackedUp,
    Installed,
    SourceRemoved,
    Committed,
    Completed,
    Skipped,
}

impl Phase {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Skipped)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DurableMachine {
    operation: ModelOperation,
    policy: ConflictPolicy,
    destination_existed: bool,
    phase: Phase,
    source: PathBuf,
    destination: PathBuf,
    landing: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
}

impl DurableMachine {
    fn fixture(
        temp: &TempDir,
        operation: ModelOperation,
        policy: ConflictPolicy,
        destination_existed: bool,
    ) -> Self {
        let source = temp.file(
            "source/item.txt",
            std::str::from_utf8(SOURCE_BYTES).unwrap(),
        );
        let destination_dir = temp.dir("destination");
        let destination = destination_dir.join("item.txt");
        if destination_existed {
            std::fs::write(&destination, DESTINATION_BYTES).unwrap();
        }
        let landing = if destination_existed && policy == ConflictPolicy::KeepBoth {
            destination_dir.join("item copy.txt")
        } else {
            destination.clone()
        };
        Self {
            operation,
            policy,
            destination_existed,
            phase: Phase::Planned,
            source,
            destination,
            landing,
            staging: destination_dir.join(".item.staging"),
            backup: destination_dir.join(".item.backup"),
        }
    }

    fn should_skip(&self) -> bool {
        self.destination_existed && self.policy == ConflictPolicy::Skip
    }

    fn needs_backup(&self) -> bool {
        self.destination_existed && self.policy == ConflictPolicy::Replace
    }

    fn effect_count(&self) -> usize {
        if self.should_skip() {
            return 0;
        }
        2 + usize::from(self.operation == ModelOperation::Move)
            + 2 * usize::from(self.needs_backup())
    }

    fn transition_count(&self) -> usize {
        if self.should_skip() { 1 } else { 6 }
    }

    fn step(&mut self, file_system: &dyn FileSystemProvider) -> std::io::Result<bool> {
        match self.phase {
            Phase::Planned if self.should_skip() => self.phase = Phase::Skipped,
            Phase::Planned => {
                if !path_has(&self.staging, SOURCE_BYTES) {
                    file_system.apply(&FileSystemEffect::WriteFile {
                        path: self.staging.clone(),
                        bytes: SOURCE_BYTES.to_vec(),
                    })?;
                }
                self.phase = Phase::Staged;
            }
            Phase::Staged if self.needs_backup() => {
                if !(path_has(&self.backup, DESTINATION_BYTES) && !self.destination.exists()) {
                    file_system.apply(&FileSystemEffect::Rename {
                        source: self.destination.clone(),
                        destination: self.backup.clone(),
                        replace: false,
                    })?;
                }
                self.phase = Phase::DestinationBackedUp;
            }
            Phase::Staged => self.phase = Phase::DestinationBackedUp,
            Phase::DestinationBackedUp => {
                if !(path_has(&self.landing, SOURCE_BYTES) && !self.staging.exists()) {
                    file_system.apply(&FileSystemEffect::Rename {
                        source: self.staging.clone(),
                        destination: self.landing.clone(),
                        replace: false,
                    })?;
                }
                self.phase = Phase::Installed;
            }
            Phase::Installed if self.operation == ModelOperation::Move => {
                if self.source.exists() {
                    file_system.apply(&FileSystemEffect::Remove {
                        path: self.source.clone(),
                    })?;
                }
                self.phase = Phase::SourceRemoved;
            }
            Phase::Installed => self.phase = Phase::SourceRemoved,
            Phase::SourceRemoved => self.phase = Phase::Committed,
            Phase::Committed => {
                if self.backup.exists() {
                    file_system.apply(&FileSystemEffect::Remove {
                        path: self.backup.clone(),
                    })?;
                }
                self.phase = Phase::Completed;
            }
            Phase::Completed | Phase::Skipped => return Ok(false),
        }
        Ok(true)
    }

    fn run(&mut self, file_system: &dyn FileSystemProvider) -> std::io::Result<()> {
        for _ in 0..16 {
            if self.phase.is_terminal() {
                return Ok(());
            }
            self.step(file_system)?;
        }
        Err(std::io::Error::other(
            "verification state machine did not terminate",
        ))
    }

    fn no_loss(&self) -> bool {
        let source_recoverable = [&self.source, &self.staging, &self.landing]
            .into_iter()
            .any(|path| path_has(path, SOURCE_BYTES));
        let old_destination_recoverable = !self.destination_existed
            || self.policy == ConflictPolicy::Replace
                && matches!(self.phase, Phase::Committed | Phase::Completed)
            || [&self.destination, &self.backup]
                .into_iter()
                .any(|path| path_has(path, DESTINATION_BYTES));
        source_recoverable && old_destination_recoverable
    }

    fn assert_terminal_contract(&self) {
        assert!(
            self.phase.is_terminal(),
            "non-terminal phase: {:?}",
            self.phase
        );
        assert!(!self.staging.exists());
        assert!(!self.backup.exists());
        if self.should_skip() {
            assert!(path_has(&self.source, SOURCE_BYTES));
            assert!(path_has(&self.destination, DESTINATION_BYTES));
            return;
        }
        assert!(path_has(&self.landing, SOURCE_BYTES));
        if self.operation == ModelOperation::Move {
            assert!(!self.source.exists());
        } else {
            assert!(path_has(&self.source, SOURCE_BYTES));
        }
        if self.destination_existed && self.policy == ConflictPolicy::KeepBoth {
            assert!(path_has(&self.destination, DESTINATION_BYTES));
        }
    }
}

fn path_has(path: &Path, expected: &[u8]) -> bool {
    std::fs::read(path).is_ok_and(|bytes| bytes == expected)
}

#[derive(Clone, Copy, Debug)]
enum InjectionMoment {
    Before,
    After,
}

struct FaultInjectingFileSystem<P> {
    inner: P,
    effect: usize,
    moment: InjectionMoment,
    calls: AtomicUsize,
}

impl<P> FaultInjectingFileSystem<P> {
    fn new(inner: P, effect: usize, moment: InjectionMoment) -> Self {
        Self {
            inner,
            effect,
            moment,
            calls: AtomicUsize::new(0),
        }
    }

    fn injected(&self, index: usize, moment: InjectionMoment) -> bool {
        self.effect == index
            && std::mem::discriminant(&self.moment) == std::mem::discriminant(&moment)
    }
}

impl<P: FileSystemProvider> FileSystemProvider for FaultInjectingFileSystem<P> {
    fn observe(&self, path: &Path) -> std::io::Result<crate::path_identity::PathIdentity> {
        self.inner.observe(path)
    }

    fn apply(&self, effect: &FileSystemEffect) -> std::io::Result<()> {
        let index = self.calls.fetch_add(1, Ordering::AcqRel);
        if self.injected(index, InjectionMoment::Before) {
            return Err(std::io::Error::other("injected before side effect"));
        }
        self.inner.apply(effect)?;
        if self.injected(index, InjectionMoment::After) {
            return Err(std::io::Error::other("injected after side effect"));
        }
        Ok(())
    }
}

fn recover_after_fault(
    operation: ModelOperation,
    policy: ConflictPolicy,
    destination_existed: bool,
    effect: usize,
    moment: InjectionMoment,
) {
    let temp = TempDir::new();
    let mut machine = DurableMachine::fixture(&temp, operation, policy, destination_existed);
    let file_system = FaultInjectingFileSystem::new(NativeFileSystemProvider, effect, moment);
    assert!(machine.run(&file_system).is_err());
    assert!(
        machine.no_loss(),
        "data lost after {moment:?} effect {effect}"
    );

    let journal = serde_json::to_vec(&machine).unwrap();
    let mut restarted: DurableMachine = serde_json::from_slice(&journal).unwrap();
    restarted.run(&NativeFileSystemProvider).unwrap();
    assert!(restarted.no_loss());
    restarted.assert_terminal_contract();
}

#[test]
fn faults_before_and_after_every_filesystem_side_effect_preserve_data() {
    for operation in [
        ModelOperation::Copy,
        ModelOperation::Move,
        ModelOperation::Sync,
    ] {
        let probe = {
            let temp = TempDir::new();
            DurableMachine::fixture(&temp, operation, ConflictPolicy::Replace, true)
        };
        for effect in 0..probe.effect_count() {
            recover_after_fault(
                operation,
                ConflictPolicy::Replace,
                true,
                effect,
                InjectionMoment::Before,
            );
            recover_after_fault(
                operation,
                ConflictPolicy::Replace,
                true,
                effect,
                InjectionMoment::After,
            );
        }
    }
}

#[test]
fn crash_kill_harness_restarts_at_every_journal_transition() {
    for operation in [
        ModelOperation::Copy,
        ModelOperation::Move,
        ModelOperation::Sync,
    ] {
        for policy in [
            ConflictPolicy::Skip,
            ConflictPolicy::Replace,
            ConflictPolicy::KeepBoth,
        ] {
            for destination_existed in [false, true] {
                let transition_count = {
                    let temp = TempDir::new();
                    DurableMachine::fixture(&temp, operation, policy, destination_existed)
                        .transition_count()
                };
                for kill_after in 1..=transition_count {
                    let temp = TempDir::new();
                    let mut machine =
                        DurableMachine::fixture(&temp, operation, policy, destination_existed);
                    let mut transitions = 0;
                    let mut restarted = false;
                    while !machine.phase.is_terminal() {
                        machine.step(&NativeFileSystemProvider).unwrap();
                        transitions += 1;
                        assert!(machine.no_loss());
                        if transitions == kill_after {
                            let persisted = serde_json::to_vec(&machine).unwrap();
                            machine = serde_json::from_slice(&persisted).unwrap();
                            restarted = true;
                        }
                    }
                    assert!(restarted, "kill point {kill_after} was not reached");
                    machine.assert_terminal_contract();
                }
            }
        }
    }
}

#[test]
fn model_checks_small_copy_move_sync_conflict_state_space() {
    for operation in [
        ModelOperation::Copy,
        ModelOperation::Move,
        ModelOperation::Sync,
    ] {
        for policy in [
            ConflictPolicy::Skip,
            ConflictPolicy::Replace,
            ConflictPolicy::KeepBoth,
        ] {
            for destination_existed in [false, true] {
                let temp = TempDir::new();
                let mut machine =
                    DurableMachine::fixture(&temp, operation, policy, destination_existed);
                while !machine.phase.is_terminal() {
                    let encoded = serde_json::to_vec(&machine).unwrap();
                    machine = serde_json::from_slice(&encoded).unwrap();
                    machine.step(&NativeFileSystemProvider).unwrap();
                    assert!(machine.no_loss());
                }
                machine.assert_terminal_contract();
            }
        }
    }
}
