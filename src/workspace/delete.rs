//! Identity-safe, sequential background deletion.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use super::{DeleteItemResult, DeleteOrigin, DeleteOutcome};
use crate::operation::{ClassifiedFailure, FailureClass, OperationId, TransferAttemptId};
use crate::ports::{
    NativeFailure, NativeFailureKind, TrashBatchItem, TrashItemOutcome, TrashPort, TrashTarget,
};

#[derive(Clone)]
struct DeleteRunContext {
    attempt_id: TransferAttemptId,
    operation_id: OperationId,
    submitted: crate::operation_view::SubmittedSummary,
    origin: DeleteOrigin,
    paths: Vec<PathBuf>,
}

impl DeleteRunContext {
    fn capture(items: &[TrashBatchItem], origin: DeleteOrigin) -> Self {
        let paths = items
            .iter()
            .map(|item| item.path().to_path_buf())
            .collect::<Vec<_>>();
        Self {
            attempt_id: TransferAttemptId::new(),
            operation_id: OperationId::new(),
            submitted: crate::operation_view::SubmittedSummary::capture(
                "Delete",
                &paths,
                PathBuf::from("Trash"),
            ),
            origin,
            paths,
        }
    }
}

struct DeleteReport {
    generation: u64,
    outcome: DeleteOutcome,
}

struct ActiveDelete {
    generation: u64,
    context: DeleteRunContext,
    receiver: Option<mpsc::Receiver<DeleteReport>>,
    immediate: Option<DeleteReport>,
}

pub(super) struct DeleteController {
    generation: u64,
    active: Option<ActiveDelete>,
    port: Arc<dyn TrashPort>,
}

impl DeleteController {
    pub(super) fn new(port: Arc<dyn TrashPort>) -> Self {
        Self {
            generation: 0,
            active: None,
            port,
        }
    }

    pub(super) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn activity_count(&self) -> Option<usize> {
        self.active
            .as_ref()
            .map(|active| active.context.paths.len())
    }

    pub(super) fn start(
        &mut self,
        items: Vec<TrashBatchItem>,
        origin: DeleteOrigin,
        durability: crate::operation::DurabilityProfile,
        retention: crate::operation::VersionRetentionPolicy,
        notify: impl Fn() + Send + 'static,
    ) -> bool {
        if items.is_empty() || self.active.is_some() {
            return false;
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        let context = DeleteRunContext::capture(&items, origin);
        let worker_context = context.clone();
        let panic_context = context.clone();
        let port = Arc::clone(&self.port);
        let (sender, receiver) = mpsc::sync_channel(1);
        let notify = Arc::new(std::sync::Mutex::new(notify));
        let worker_notify = Arc::clone(&notify);
        let spawn = std::thread::Builder::new()
            .name("trash-batch".to_string())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_batch(items, worker_context, durability, retention, port.as_ref())
                }))
                .unwrap_or_else(|_| {
                    disconnected_outcome(
                        &panic_context,
                        "Trash worker panicked before publishing a complete result",
                    )
                });
                let _ = sender.send(DeleteReport {
                    generation,
                    outcome,
                });
                (crate::lock_util::recover(&worker_notify))();
            });
        let immediate = spawn.err().map(|error| DeleteReport {
            generation,
            outcome: failed_outcome(
                &context,
                NativeFailure {
                    kind: NativeFailureKind::Busy,
                    message: format!("Could not start Trash worker: {error}"),
                },
                false,
            ),
        });
        if immediate.is_some() {
            (crate::lock_util::recover(&notify))();
        }
        self.active = Some(ActiveDelete {
            generation,
            context,
            receiver: immediate.is_none().then_some(receiver),
            immediate,
        });
        true
    }

    pub(super) fn poll(&mut self) -> Option<DeleteOutcome> {
        let mut active = self.active.take()?;
        if let Some(report) = active.immediate.take() {
            return Some(report.outcome);
        }
        let Some(receiver) = active.receiver.as_ref() else {
            return Some(disconnected_outcome(
                &active.context,
                "Trash worker channel was not available",
            ));
        };
        match receiver.try_recv() {
            Ok(report)
                if report.generation == active.generation
                    && active.generation == self.generation =>
            {
                Some(report.outcome)
            }
            Ok(_) => Some(disconnected_outcome(
                &active.context,
                "Trash worker returned a mismatched operation binding",
            )),
            Err(mpsc::TryRecvError::Empty) => {
                self.active = Some(active);
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => Some(disconnected_outcome(
                &active.context,
                "Trash worker stopped before publishing a complete result",
            )),
        }
    }

    #[cfg(test)]
    pub(super) fn finish(&mut self) -> Option<DeleteOutcome> {
        let mut active = self.active.take()?;
        if let Some(report) = active.immediate.take() {
            return Some(report.outcome);
        }
        let report = active.receiver.take()?.recv();
        match report {
            Ok(report)
                if report.generation == active.generation
                    && active.generation == self.generation =>
            {
                Some(report.outcome)
            }
            Ok(_) => Some(disconnected_outcome(
                &active.context,
                "Trash worker returned a mismatched operation binding",
            )),
            Err(_) => Some(disconnected_outcome(
                &active.context,
                "Trash worker stopped before publishing a complete result",
            )),
        }
    }
}

fn run_batch(
    items: Vec<TrashBatchItem>,
    context: DeleteRunContext,
    durability: crate::operation::DurabilityProfile,
    retention: crate::operation::VersionRetentionPolicy,
    port: &dyn TrashPort,
) -> DeleteOutcome {
    let mut results = Vec::with_capacity(items.len());
    let mut failures = Vec::new();
    let mut trashed = 0;

    for (index, item) in items.into_iter().enumerate() {
        let (path, outcome) = match item {
            TrashBatchItem::CaptureFailed { path, failure } => {
                (path, TrashItemOutcome::Failed(failure))
            }
            TrashBatchItem::Ready(target) => {
                let outcome = validate_target(&target).unwrap_or_else(|outcome| outcome);
                let outcome = if outcome == TrashItemOutcome::Trashed {
                    let preservation = if durability.keeps_versions() {
                        crate::version_store::preserve_with_policy(
                            &target.path,
                            &context.operation_id,
                            context.operation_id.step_key(index, &target.path),
                            retention,
                        )
                        .map(|_| ())
                        .map_err(|message| NativeFailure {
                            kind: NativeFailureKind::Unknown,
                            message,
                        })
                    } else {
                        Ok(())
                    };
                    match preservation {
                        Err(failure) => TrashItemOutcome::Failed(failure),
                        Ok(()) => match validate_target(&target) {
                            Ok(TrashItemOutcome::Trashed) => port.move_to_trash(&target),
                            Ok(other) | Err(other) => other,
                        },
                    }
                } else {
                    outcome
                };
                (target.path, outcome)
            }
        };

        if outcome == TrashItemOutcome::Trashed {
            trashed += 1;
        } else {
            failures.push(classify_failure(&path, &outcome));
        }
        results.push(DeleteItemResult { path, outcome });
    }

    DeleteOutcome {
        attempt_id: context.attempt_id,
        operation_id: context.operation_id,
        submitted: context.submitted,
        origin: context.origin,
        failed: results.len().saturating_sub(trashed),
        items: results,
        trashed,
        failures,
        indeterminate: false,
    }
}

fn disconnected_outcome(context: &DeleteRunContext, message: &str) -> DeleteOutcome {
    failed_outcome(
        context,
        NativeFailure {
            kind: NativeFailureKind::Unknown,
            message: message.to_string(),
        },
        true,
    )
}

fn failed_outcome(
    context: &DeleteRunContext,
    failure: NativeFailure,
    indeterminate: bool,
) -> DeleteOutcome {
    let items = context
        .paths
        .iter()
        .cloned()
        .map(|path| DeleteItemResult {
            path,
            outcome: TrashItemOutcome::Failed(failure.clone()),
        })
        .collect::<Vec<_>>();
    let class = if indeterminate {
        FailureClass::IntegrityUncertain
    } else {
        FailureClass::Retryable
    };
    let failures = context
        .paths
        .iter()
        .cloned()
        .map(|path| ClassifiedFailure::message(class, Some(path), failure.message.clone()))
        .collect();
    DeleteOutcome {
        attempt_id: context.attempt_id,
        operation_id: context.operation_id.clone(),
        submitted: context.submitted.clone(),
        origin: context.origin,
        failed: items.len(),
        items,
        trashed: 0,
        failures,
        indeterminate,
    }
}

/// `Trashed` here means "validated and ready for the Trash port"; no mutation
/// has happened yet.
fn validate_target(target: &TrashTarget) -> Result<TrashItemOutcome, TrashItemOutcome> {
    let current = crate::path_identity::PathIdentity::observe(&target.path)
        .map_err(|error| TrashItemOutcome::Failed(NativeFailure::from_io(&error)))?;
    if !current.exists {
        return Err(TrashItemOutcome::Missing);
    }
    if !target.expected.same_binding(&current) {
        return Err(TrashItemOutcome::StaleBinding);
    }
    Ok(TrashItemOutcome::Trashed)
}

fn classify_failure(path: &std::path::Path, outcome: &TrashItemOutcome) -> ClassifiedFailure {
    let (class, message) = match outcome {
        TrashItemOutcome::Trashed => unreachable!("successful Trash result is not a failure"),
        TrashItemOutcome::Missing => (
            FailureClass::Blocked,
            "item disappeared before it could be moved to Trash".to_string(),
        ),
        TrashItemOutcome::StaleBinding => (
            FailureClass::IntegrityUncertain,
            "item was replaced after deletion was requested".to_string(),
        ),
        TrashItemOutcome::Unsupported(failure) | TrashItemOutcome::Failed(failure) => {
            let class = match failure.kind {
                NativeFailureKind::Busy | NativeFailureKind::Cancelled => FailureClass::Retryable,
                NativeFailureKind::Stale => FailureClass::IntegrityUncertain,
                NativeFailureKind::InvalidInput => FailureClass::UserDecision,
                NativeFailureKind::Denied
                | NativeFailureKind::NotFound
                | NativeFailureKind::ReadOnly
                | NativeFailureKind::Unsupported
                | NativeFailureKind::Overflow
                | NativeFailureKind::Unknown => FailureClass::Blocked,
            };
            (class, failure.message.clone())
        }
    };
    ClassifiedFailure::message(class, Some(path.to_path_buf()), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct ScriptedTrash {
        outcomes: Mutex<VecDeque<TrashItemOutcome>>,
        calls: Mutex<Vec<PathBuf>>,
    }

    impl ScriptedTrash {
        fn new(outcomes: impl IntoIterator<Item = TrashItemOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl TrashPort for ScriptedTrash {
        fn move_to_trash(&self, target: &TrashTarget) -> TrashItemOutcome {
            self.calls.lock().unwrap().push(target.path.clone());
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted Trash outcome")
        }
    }

    fn target(path: PathBuf) -> TrashTarget {
        TrashTarget {
            expected: crate::path_identity::PathIdentity::observe(&path).unwrap(),
            path,
        }
    }

    fn ready(path: PathBuf) -> TrashBatchItem {
        TrashBatchItem::Ready(target(path))
    }

    #[test]
    fn stale_replacement_never_reaches_the_trash_port() {
        let temp = TempDir::new();
        let path = temp.file("value.txt", "old");
        let item = ready(path.clone());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "new").unwrap();
        let port = ScriptedTrash::new([TrashItemOutcome::Trashed]);

        let outcome = run_batch(
            vec![item.clone()],
            DeleteRunContext::capture(&[item], DeleteOrigin::Confirmation),
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            &port,
        );

        assert_eq!(outcome.items[0].outcome, TrashItemOutcome::StaleBinding);
        assert!(port.calls.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn lexical_symlink_target_removes_the_link_not_its_destination() {
        struct LexicalDelete;

        impl TrashPort for LexicalDelete {
            fn move_to_trash(&self, target: &TrashTarget) -> TrashItemOutcome {
                std::fs::remove_file(&target.path).unwrap();
                TrashItemOutcome::Trashed
            }
        }

        let temp = TempDir::new();
        let destination = temp.file("destination.txt", "keep");
        let link = temp.path().join("link.txt");
        std::os::unix::fs::symlink("destination.txt", &link).unwrap();
        let items = vec![ready(link.clone())];

        let outcome = run_batch(
            items.clone(),
            DeleteRunContext::capture(&items, DeleteOrigin::Confirmation),
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            &LexicalDelete,
        );

        assert_eq!(outcome.trashed, 1);
        assert!(std::fs::symlink_metadata(link).is_err());
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "keep");
    }

    #[test]
    fn partial_batch_preserves_input_and_result_order() {
        let temp = TempDir::new();
        let first = temp.file("first.txt", "one");
        let second = temp.file("second.txt", "two");
        let port = ScriptedTrash::new([
            TrashItemOutcome::Trashed,
            TrashItemOutcome::Failed(NativeFailure {
                kind: NativeFailureKind::Denied,
                message: "denied".to_string(),
            }),
        ]);

        let items = vec![ready(first.clone()), ready(second.clone())];
        let outcome = run_batch(
            items.clone(),
            DeleteRunContext::capture(&items, DeleteOrigin::Duplicates),
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            &port,
        );

        assert_eq!(
            outcome
                .items
                .iter()
                .map(|item| &item.path)
                .collect::<Vec<_>>(),
            vec![&first, &second]
        );
        assert_eq!(outcome.trashed, 1);
        assert_eq!(outcome.failed, 1);
        assert_eq!(
            *port.calls.lock().unwrap(),
            vec![first.clone(), second.clone()]
        );
    }

    #[test]
    fn capture_failure_is_ordered_and_never_reaches_the_port() {
        let path = PathBuf::from("/unobserved.txt");
        let failure = NativeFailure {
            kind: NativeFailureKind::Denied,
            message: "listing metadata was denied".to_string(),
        };
        let items = vec![TrashBatchItem::CaptureFailed {
            path: path.clone(),
            failure: failure.clone(),
        }];
        let port = ScriptedTrash::new([]);

        let outcome = run_batch(
            items.clone(),
            DeleteRunContext::capture(&items, DeleteOrigin::Confirmation),
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            &port,
        );

        assert_eq!(
            outcome.items,
            vec![DeleteItemResult {
                path,
                outcome: TrashItemOutcome::Failed(failure),
            }]
        );
        assert!(port.calls.lock().unwrap().is_empty());
    }

    struct PanickingTrash;

    impl TrashPort for PanickingTrash {
        fn move_to_trash(&self, _target: &TrashTarget) -> TrashItemOutcome {
            panic!("scripted worker panic");
        }
    }

    #[test]
    fn worker_panic_retires_as_indeterminate_failure() {
        let temp = TempDir::new();
        let path = temp.file("panic.txt", "value");
        let mut controller = DeleteController::new(Arc::new(PanickingTrash));
        assert!(controller.start(
            vec![ready(path)],
            DeleteOrigin::Confirmation,
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            || {},
        ));

        let outcome = controller.finish().expect("typed panic outcome");
        assert!(outcome.indeterminate);
        assert!(!outcome.refresh_required());
        assert!(!controller.is_active());
    }

    #[test]
    fn disconnected_worker_retires_as_typed_indeterminate_failure() {
        let temp = TempDir::new();
        let path = temp.file("disconnected.txt", "value");
        let item = ready(path);
        let context =
            DeleteRunContext::capture(std::slice::from_ref(&item), DeleteOrigin::Confirmation);
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        let mut controller = DeleteController::new(Arc::new(ScriptedTrash::new([])));
        controller.generation = 1;
        controller.active = Some(ActiveDelete {
            generation: 1,
            context,
            receiver: Some(receiver),
            immediate: None,
        });

        let outcome = controller.poll().expect("typed disconnected outcome");

        assert!(outcome.indeterminate);
        assert!(!controller.is_active());
    }
}
