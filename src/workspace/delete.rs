//! Identity-safe, sequential background deletion.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use super::{DeleteItemResult, DeleteOrigin, DeleteOutcome};
use crate::operation::{ClassifiedFailure, FailureClass, OperationId, TransferAttemptId};
use crate::ports::{
    NativeFailure, NativeFailureKind, TrashBatchItem, TrashItemOutcome, TrashPort, TrashTarget,
};

trait DeleteVersionStore: Send + Sync {
    fn preserve(
        &self,
        target: &TrashTarget,
        operation_id: &OperationId,
        key: crate::operation::IdempotencyKey,
        retention: crate::operation::VersionRetentionPolicy,
    ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure>;

    fn discard(&self, record: &crate::version_store::VersionRecord) -> Result<(), NativeFailure>;
}

struct NativeDeleteVersionStore;

impl DeleteVersionStore for NativeDeleteVersionStore {
    fn preserve(
        &self,
        target: &TrashTarget,
        operation_id: &OperationId,
        key: crate::operation::IdempotencyKey,
        retention: crate::operation::VersionRetentionPolicy,
    ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure> {
        crate::version_store::preserve_expected_with_policy(
            &target.path,
            &target.expected,
            operation_id,
            key,
            retention,
        )
    }

    fn discard(&self, record: &crate::version_store::VersionRecord) -> Result<(), NativeFailure> {
        crate::version_store::discard_record(record).map_err(|message| NativeFailure {
            kind: NativeFailureKind::Unknown,
            message,
        })
    }
}

#[cfg(test)]
struct NoopDeleteVersionStore;

#[cfg(test)]
impl DeleteVersionStore for NoopDeleteVersionStore {
    fn preserve(
        &self,
        _target: &TrashTarget,
        _operation_id: &OperationId,
        _key: crate::operation::IdempotencyKey,
        _retention: crate::operation::VersionRetentionPolicy,
    ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure> {
        Ok(None)
    }

    fn discard(&self, _record: &crate::version_store::VersionRecord) -> Result<(), NativeFailure> {
        Ok(())
    }
}

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

#[derive(Default)]
struct DeleteProgress {
    results: Vec<DeleteItemResult>,
}

struct ActiveDelete {
    generation: u64,
    context: DeleteRunContext,
    receiver: Option<mpsc::Receiver<DeleteReport>>,
    immediate: Option<DeleteReport>,
    progress: Arc<Mutex<DeleteProgress>>,
    cancel: Arc<AtomicBool>,
}

pub(super) struct DeleteController {
    generation: u64,
    active: Option<ActiveDelete>,
    port: Arc<dyn TrashPort>,
    versions: Arc<dyn DeleteVersionStore>,
}

impl DeleteController {
    pub(super) fn new(port: Arc<dyn TrashPort>) -> Self {
        #[cfg(not(test))]
        let versions: Arc<dyn DeleteVersionStore> = Arc::new(NativeDeleteVersionStore);
        #[cfg(test)]
        let versions: Arc<dyn DeleteVersionStore> = Arc::new(NoopDeleteVersionStore);
        Self::with_version_store(port, versions)
    }

    #[cfg(test)]
    pub(super) fn with_native_versions(port: Arc<dyn TrashPort>) -> Self {
        Self::with_version_store(port, Arc::new(NativeDeleteVersionStore))
    }

    fn with_version_store(port: Arc<dyn TrashPort>, versions: Arc<dyn DeleteVersionStore>) -> Self {
        Self {
            generation: 0,
            active: None,
            port,
            versions,
        }
    }

    /// Trash one item on the caller's thread (Trash redo). An adapter panic
    /// is reported as indeterminate instead of unwinding into the UI.
    pub(super) fn trash_one(&self, target: &TrashTarget) -> TrashItemOutcome {
        guarded_move_to_trash(self.port.as_ref(), target)
    }

    pub(super) const fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn activity(&self) -> Option<(usize, usize, bool)> {
        self.active.as_ref().map(|active| {
            (
                crate::lock_util::recover(&active.progress).results.len(),
                active.context.paths.len(),
                active.cancel.load(Ordering::Acquire),
            )
        })
    }

    pub(super) fn cancel(&self) -> bool {
        let Some(active) = &self.active else {
            return false;
        };
        active.cancel.store(true, Ordering::Release);
        true
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
        let versions = Arc::clone(&self.versions);
        let progress = Arc::new(Mutex::new(DeleteProgress::default()));
        let worker_progress = Arc::clone(&progress);
        let panic_progress = Arc::clone(&progress);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::sync_channel(1);
        let notify = Arc::new(std::sync::Mutex::new(notify));
        let worker_notify = Arc::clone(&notify);
        let spawn = std::thread::Builder::new()
            .name("trash-batch".to_string())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let notify_progress = || (crate::lock_util::recover(&worker_notify))();
                    run_batch(
                        items,
                        DeleteBatchRuntime {
                            context: &worker_context,
                            port: port.as_ref(),
                            versions: versions.as_ref(),
                            cancel: worker_cancel.as_ref(),
                            progress: worker_progress.as_ref(),
                            notify: &notify_progress,
                            durability,
                            retention,
                        },
                    )
                }))
                .unwrap_or_else(|_| {
                    indeterminate_outcome(
                        &panic_context,
                        panic_progress.as_ref(),
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
            progress,
            cancel,
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
                active.progress.as_ref(),
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
                active.progress.as_ref(),
                "Trash worker returned a mismatched operation binding",
            )),
            Err(mpsc::TryRecvError::Empty) => {
                self.active = Some(active);
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => Some(disconnected_outcome(
                &active.context,
                active.progress.as_ref(),
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
                active.progress.as_ref(),
                "Trash worker returned a mismatched operation binding",
            )),
            Err(_) => Some(disconnected_outcome(
                &active.context,
                active.progress.as_ref(),
                "Trash worker stopped before publishing a complete result",
            )),
        }
    }
}

struct DeleteBatchRuntime<'a> {
    context: &'a DeleteRunContext,
    port: &'a dyn TrashPort,
    versions: &'a dyn DeleteVersionStore,
    cancel: &'a AtomicBool,
    progress: &'a Mutex<DeleteProgress>,
    notify: &'a dyn Fn(),
    durability: crate::operation::DurabilityProfile,
    retention: crate::operation::VersionRetentionPolicy,
}

fn run_batch(items: Vec<TrashBatchItem>, runtime: DeleteBatchRuntime<'_>) -> DeleteOutcome {
    for (index, item) in items.iter().cloned().enumerate() {
        if runtime.cancel.load(Ordering::Acquire) {
            append_remaining(
                &items[index..],
                TrashItemOutcome::Cancelled,
                runtime.progress,
                runtime.notify,
            );
            break;
        }
        let (path, outcome, version) = match item {
            TrashBatchItem::CaptureFailed { path, failure } => {
                (path, TrashItemOutcome::Failed(failure), None)
            }
            TrashBatchItem::Ready(target) => {
                let (outcome, version) = process_ready_target(&target, index, &runtime);
                (target.path, outcome, version)
            }
        };
        let indeterminate = matches!(outcome, TrashItemOutcome::Indeterminate(_));
        {
            let mut state = crate::lock_util::recover(runtime.progress);
            state.results.push(DeleteItemResult {
                path,
                outcome,
                version,
            });
        }
        (runtime.notify)();
        if indeterminate {
            append_remaining(
                &items[index + 1..],
                TrashItemOutcome::Indeterminate(NativeFailure {
                    kind: NativeFailureKind::Unknown,
                    message: "Trash batch stopped after an indeterminate native result".to_string(),
                }),
                runtime.progress,
                runtime.notify,
            );
            break;
        }
    }
    outcome_from_results(
        runtime.context,
        crate::lock_util::recover(runtime.progress).results.clone(),
    )
}

fn process_ready_target(
    target: &TrashTarget,
    index: usize,
    runtime: &DeleteBatchRuntime<'_>,
) -> (
    TrashItemOutcome,
    Option<crate::version_store::VersionRecord>,
) {
    if let Err(outcome) = validate_target(target) {
        return (outcome, None);
    }
    let record = if runtime.durability.keeps_versions() {
        match runtime.versions.preserve(
            target,
            &runtime.context.operation_id,
            runtime.context.operation_id.step_key(index, &target.path),
            runtime.retention,
        ) {
            Ok(record) => record,
            Err(failure) if failure.kind == NativeFailureKind::Stale => {
                return (TrashItemOutcome::StaleBinding, None);
            }
            Err(failure) => return (TrashItemOutcome::Failed(failure), None),
        }
    } else {
        None
    };
    if let Err(outcome) = validate_target(target) {
        return (
            discard_rejected_version(outcome, record.as_ref(), runtime.versions),
            None,
        );
    }
    // Preserving a large folder can take long; honor a cancel that arrived
    // meanwhile before the item is mutated.
    if runtime.cancel.load(Ordering::Acquire) {
        return (
            discard_rejected_version(
                TrashItemOutcome::Cancelled,
                record.as_ref(),
                runtime.versions,
            ),
            None,
        );
    }
    let outcome = guarded_move_to_trash(runtime.port, target);
    if matches!(
        outcome,
        TrashItemOutcome::Missing | TrashItemOutcome::StaleBinding
    ) {
        (
            discard_rejected_version(outcome, record.as_ref(), runtime.versions),
            None,
        )
    } else if outcome == TrashItemOutcome::Trashed {
        (outcome, record)
    } else {
        // Discard unused versions on hard failure; keep them for indeterminate results.
        if matches!(
            outcome,
            TrashItemOutcome::Failed(_)
                | TrashItemOutcome::Unsupported(_)
                | TrashItemOutcome::Cancelled
        ) {
            (
                discard_rejected_version(outcome, record.as_ref(), runtime.versions),
                None,
            )
        } else {
            (outcome, record)
        }
    }
}

/// Call the Trash port, turning an adapter panic into an indeterminate
/// result: the item may or may not have reached the Trash.
fn guarded_move_to_trash(port: &dyn TrashPort, target: &TrashTarget) -> TrashItemOutcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| port.move_to_trash(target)))
        .unwrap_or_else(|_| {
            TrashItemOutcome::Indeterminate(NativeFailure {
                kind: NativeFailureKind::Unknown,
                message: format!(
                    "Trash adapter panicked while processing {}",
                    target.path.display()
                ),
            })
        })
}

fn discard_rejected_version(
    outcome: TrashItemOutcome,
    record: Option<&crate::version_store::VersionRecord>,
    versions: &dyn DeleteVersionStore,
) -> TrashItemOutcome {
    let Some(record) = record else {
        return outcome;
    };
    // A version that outlives its rejected delete only costs space; the
    // item's own outcome (Missing, StaleBinding, Cancelled, ...) is what the
    // user must see, so a discard failure is logged instead of replacing it.
    if let Err(failure) = versions.discard(record) {
        log::warn!(
            target: "commander::delete",
            "Could not discard the unused version of {}: {}",
            record.original.display(),
            failure.message
        );
    }
    outcome
}

fn append_remaining(
    items: &[TrashBatchItem],
    outcome: TrashItemOutcome,
    progress: &Mutex<DeleteProgress>,
    notify: &dyn Fn(),
) {
    for item in items {
        let mut state = crate::lock_util::recover(progress);
        state.results.push(DeleteItemResult {
            path: item.path().to_path_buf(),
            outcome: outcome.clone(),
            version: None,
        });
        drop(state);
        notify();
    }
}

fn outcome_from_results(
    context: &DeleteRunContext,
    results: Vec<DeleteItemResult>,
) -> DeleteOutcome {
    let trashed = results
        .iter()
        .filter(|item| item.outcome == TrashItemOutcome::Trashed)
        .count();
    let failures = results
        .iter()
        .filter(|item| {
            !matches!(
                item.outcome,
                TrashItemOutcome::Trashed | TrashItemOutcome::Cancelled
            )
        })
        .map(|item| classify_failure(&item.path, &item.outcome))
        .collect::<Vec<_>>();
    let failed = failures.len();
    let indeterminate = results
        .iter()
        .any(|item| matches!(item.outcome, TrashItemOutcome::Indeterminate(_)));
    let cancelled = results
        .iter()
        .any(|item| item.outcome == TrashItemOutcome::Cancelled);
    DeleteOutcome {
        attempt_id: context.attempt_id,
        operation_id: context.operation_id.clone(),
        submitted: context.submitted.clone(),
        origin: context.origin,
        failed,
        items: results,
        trashed,
        failures,
        indeterminate,
        cancelled,
    }
}

fn disconnected_outcome(
    context: &DeleteRunContext,
    progress: &Mutex<DeleteProgress>,
    message: &str,
) -> DeleteOutcome {
    indeterminate_outcome(context, progress, message)
}

fn indeterminate_outcome(
    context: &DeleteRunContext,
    progress: &Mutex<DeleteProgress>,
    message: &str,
) -> DeleteOutcome {
    let mut results = crate::lock_util::recover(progress).results.clone();
    for path in context.paths.iter().skip(results.len()) {
        results.push(DeleteItemResult {
            path: path.clone(),
            outcome: TrashItemOutcome::Indeterminate(NativeFailure {
                kind: NativeFailureKind::Unknown,
                message: message.to_string(),
            }),
            version: None,
        });
    }
    outcome_from_results(context, results)
}

/// Every item failed before any of them reached the Trash (the worker could
/// not start), so the whole batch is retryable.
fn failed_outcome(context: &DeleteRunContext, failure: NativeFailure) -> DeleteOutcome {
    let items = context
        .paths
        .iter()
        .cloned()
        .map(|path| DeleteItemResult {
            path,
            outcome: TrashItemOutcome::Failed(failure.clone()),
            version: None,
        })
        .collect::<Vec<_>>();
    let failures = context
        .paths
        .iter()
        .cloned()
        .map(|path| {
            ClassifiedFailure::message(FailureClass::Retryable, Some(path), failure.message.clone())
        })
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
        indeterminate: false,
        cancelled: false,
    }
}

/// `Trashed` here means "validated and ready for the Trash port"; no mutation
/// has happened yet.
fn validate_target(target: &TrashTarget) -> Result<(), TrashItemOutcome> {
    let current = crate::path_identity::PathIdentity::observe(&target.path)
        .map_err(|error| TrashItemOutcome::Failed(NativeFailure::from_io(&error)))?;
    if !current.exists {
        return Err(TrashItemOutcome::Missing);
    }
    if !target.expected.same_binding(&current) {
        return Err(TrashItemOutcome::StaleBinding);
    }
    Ok(())
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
        TrashItemOutcome::Cancelled => (
            FailureClass::Retryable,
            "item was not moved to Trash because the batch was cancelled".to_string(),
        ),
        TrashItemOutcome::Indeterminate(failure) => {
            (FailureClass::IntegrityUncertain, failure.message.clone())
        }
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
    use std::sync::atomic::AtomicUsize;

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

    fn run_test_batch(
        items: Vec<TrashBatchItem>,
        context: DeleteRunContext,
        durability: crate::operation::DurabilityProfile,
        retention: crate::operation::VersionRetentionPolicy,
        port: &dyn TrashPort,
    ) -> DeleteOutcome {
        let versions = NoopDeleteVersionStore;
        let cancel = AtomicBool::new(false);
        let progress = Mutex::new(DeleteProgress::default());
        run_batch(
            items,
            DeleteBatchRuntime {
                context: &context,
                port,
                versions: &versions,
                cancel: &cancel,
                progress: &progress,
                notify: &|| {},
                durability,
                retention,
            },
        )
    }

    #[test]
    fn stale_replacement_never_reaches_the_trash_port() {
        let temp = TempDir::new();
        let path = temp.file("value.txt", "old");
        let item = ready(path.clone());
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "new").unwrap();
        let port = ScriptedTrash::new([TrashItemOutcome::Trashed]);

        let outcome = run_test_batch(
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

        let outcome = run_test_batch(
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
        let outcome = run_test_batch(
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

        let outcome = run_test_batch(
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
                version: None,
            }]
        );
        assert!(port.calls.lock().unwrap().is_empty());
    }

    struct TempVersionStore {
        root: PathBuf,
    }

    impl DeleteVersionStore for TempVersionStore {
        fn preserve(
            &self,
            target: &TrashTarget,
            operation_id: &OperationId,
            key: crate::operation::IdempotencyKey,
            retention: crate::operation::VersionRetentionPolicy,
        ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure> {
            crate::version_store::preserve_expected_at(
                &self.root,
                &target.path,
                &target.expected,
                operation_id,
                key,
                retention,
            )
        }

        fn discard(
            &self,
            record: &crate::version_store::VersionRecord,
        ) -> Result<(), NativeFailure> {
            crate::version_store::discard_record_at(&self.root, record).map_err(|message| {
                NativeFailure {
                    kind: NativeFailureKind::Unknown,
                    message,
                }
            })
        }
    }

    #[test]
    fn versioned_delete_uses_the_injected_worker_visible_store() {
        let temp = TempDir::new();
        let versions = temp.path().join("isolated-versions");
        let path = temp.file("versioned.txt", "value");
        let mut controller = DeleteController::with_version_store(
            Arc::new(ScriptedTrash::new([TrashItemOutcome::Trashed])),
            Arc::new(TempVersionStore {
                root: versions.clone(),
            }),
        );

        assert!(controller.start(
            vec![ready(path.clone())],
            DeleteOrigin::Confirmation,
            crate::operation::DurabilityProfile::Versioned,
            crate::operation::VersionRetentionPolicy::Recent,
            || {},
        ));
        let outcome = controller.finish().unwrap();

        assert_eq!(outcome.trashed, 1);
        let manifest = std::fs::read_to_string(versions.join("manifest.json")).unwrap();
        assert!(manifest.contains("versioned.txt"));
        assert!(manifest.contains(temp.path().to_string_lossy().as_ref()));
    }

    struct ReplacingVersionStore {
        root: PathBuf,
    }

    impl DeleteVersionStore for ReplacingVersionStore {
        fn preserve(
            &self,
            target: &TrashTarget,
            operation_id: &OperationId,
            key: crate::operation::IdempotencyKey,
            retention: crate::operation::VersionRetentionPolicy,
        ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure> {
            std::fs::write(&target.path, "replacement-content").unwrap();
            crate::version_store::preserve_expected_at(
                &self.root,
                &target.path,
                &target.expected,
                operation_id,
                key,
                retention,
            )
        }

        fn discard(
            &self,
            record: &crate::version_store::VersionRecord,
        ) -> Result<(), NativeFailure> {
            crate::version_store::discard_record_at(&self.root, record).map_err(|message| {
                NativeFailure {
                    kind: NativeFailureKind::Unknown,
                    message,
                }
            })
        }
    }

    #[test]
    fn replacement_before_preserve_publishes_no_version_and_never_calls_trash() {
        let temp = TempDir::new();
        let versions = temp.path().join("isolated-versions");
        let path = temp.file("stale.txt", "old");
        let port = Arc::new(ScriptedTrash::new([TrashItemOutcome::Trashed]));
        let mut controller = DeleteController::with_version_store(
            port.clone(),
            Arc::new(ReplacingVersionStore {
                root: versions.clone(),
            }),
        );

        assert!(controller.start(
            vec![ready(path.clone())],
            DeleteOrigin::Confirmation,
            crate::operation::DurabilityProfile::Versioned,
            crate::operation::VersionRetentionPolicy::Recent,
            || {},
        ));
        let outcome = controller.finish().unwrap();

        assert_eq!(outcome.items[0].outcome, TrashItemOutcome::StaleBinding);
        assert!(port.calls.lock().unwrap().is_empty());
        assert!(!versions.exists());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "replacement-content"
        );
    }

    struct BlockingFirstTrash {
        calls: Mutex<Vec<PathBuf>>,
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl TrashPort for BlockingFirstTrash {
        fn move_to_trash(&self, target: &TrashTarget) -> TrashItemOutcome {
            let first = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(target.path.clone());
                calls.len() == 1
            };
            if first {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            }
            TrashItemOutcome::Trashed
        }
    }

    #[test]
    fn cancellation_after_current_item_skips_the_untouched_remainder() {
        let temp = TempDir::new();
        let paths = [
            temp.file("first.txt", "one"),
            temp.file("second.txt", "two"),
            temp.file("third.txt", "three"),
        ];
        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let port = Arc::new(BlockingFirstTrash {
            calls: Mutex::new(Vec::new()),
            entered: entered_tx,
            release: Mutex::new(release_rx),
        });
        let mut controller = DeleteController::new(port.clone());
        assert!(controller.start(
            paths.iter().cloned().map(ready).collect(),
            DeleteOrigin::Confirmation,
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            || {},
        ));

        entered_rx.recv().unwrap();
        assert!(controller.cancel());
        assert_eq!(controller.activity(), Some((0, 3, true)));
        release_tx.send(()).unwrap();
        let outcome = controller.finish().unwrap();

        assert_eq!(*port.calls.lock().unwrap(), vec![paths[0].clone()]);
        assert_eq!(
            outcome
                .items
                .iter()
                .map(|item| &item.outcome)
                .collect::<Vec<_>>(),
            vec![
                &TrashItemOutcome::Trashed,
                &TrashItemOutcome::Cancelled,
                &TrashItemOutcome::Cancelled,
            ]
        );
        assert_eq!(outcome.trashed, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.failures.is_empty());
        assert!(outcome.cancelled);
        assert!(outcome.refresh_required());
    }

    struct PanickingTrash;

    impl TrashPort for PanickingTrash {
        fn move_to_trash(&self, _target: &TrashTarget) -> TrashItemOutcome {
            panic!("scripted worker panic");
        }
    }

    struct TrashedThenPanic {
        calls: AtomicUsize,
    }

    impl TrashPort for TrashedThenPanic {
        fn move_to_trash(&self, _target: &TrashTarget) -> TrashItemOutcome {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                TrashItemOutcome::Trashed
            } else {
                panic!("panic after a known mutation");
            }
        }
    }

    #[test]
    fn panic_after_partial_mutation_preserves_known_results_and_marks_the_rest_uncertain() {
        let temp = TempDir::new();
        let paths = [
            temp.file("first.txt", "one"),
            temp.file("second.txt", "two"),
            temp.file("third.txt", "three"),
        ];
        let mut controller = DeleteController::new(Arc::new(TrashedThenPanic {
            calls: AtomicUsize::new(0),
        }));
        assert!(controller.start(
            paths.iter().cloned().map(ready).collect(),
            DeleteOrigin::Confirmation,
            crate::operation::DurabilityProfile::Fast,
            crate::operation::VersionRetentionPolicy::Recent,
            || {},
        ));

        let outcome = controller.finish().unwrap();

        assert_eq!(outcome.trashed, 1);
        assert!(outcome.indeterminate);
        assert_eq!(outcome.items[0].outcome, TrashItemOutcome::Trashed);
        assert!(matches!(
            outcome.items[1].outcome,
            TrashItemOutcome::Indeterminate(_)
        ));
        assert!(matches!(
            outcome.items[2].outcome,
            TrashItemOutcome::Indeterminate(_)
        ));
        assert!(outcome.refresh_required());
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
        // The item may have reached the Trash before the panic: the panels
        // must re-list even though nothing was confirmed trashed.
        assert_eq!(outcome.trashed, 0);
        assert!(outcome.refresh_required());
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
            progress: Arc::new(Mutex::new(DeleteProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
        });

        let outcome = controller.poll().expect("typed disconnected outcome");

        assert!(outcome.indeterminate);
        assert!(!controller.is_active());
    }

    /// Real version store that records discards and can be told to refuse
    /// them, so rejected-version cleanup is observable.
    struct RecordingVersionStore {
        inner: TempVersionStore,
        discarded: Mutex<Vec<crate::version_store::VersionRecord>>,
        fail_discard: bool,
    }

    impl RecordingVersionStore {
        fn new(root: PathBuf, fail_discard: bool) -> Self {
            Self {
                inner: TempVersionStore { root },
                discarded: Mutex::new(Vec::new()),
                fail_discard,
            }
        }
    }

    impl DeleteVersionStore for RecordingVersionStore {
        fn preserve(
            &self,
            target: &TrashTarget,
            operation_id: &OperationId,
            key: crate::operation::IdempotencyKey,
            retention: crate::operation::VersionRetentionPolicy,
        ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure> {
            self.inner.preserve(target, operation_id, key, retention)
        }

        fn discard(
            &self,
            record: &crate::version_store::VersionRecord,
        ) -> Result<(), NativeFailure> {
            self.discarded.lock().unwrap().push(record.clone());
            if self.fail_discard {
                return Err(NativeFailure {
                    kind: NativeFailureKind::Unknown,
                    message: "scripted discard failure".to_string(),
                });
            }
            self.inner.discard(record)
        }
    }

    fn run_versioned_batch(
        items: Vec<TrashBatchItem>,
        port: &dyn TrashPort,
        versions: &dyn DeleteVersionStore,
        cancel: &AtomicBool,
    ) -> DeleteOutcome {
        let context = DeleteRunContext::capture(&items, DeleteOrigin::Confirmation);
        let progress = Mutex::new(DeleteProgress::default());
        run_batch(
            items,
            DeleteBatchRuntime {
                context: &context,
                port,
                versions,
                cancel,
                progress: &progress,
                notify: &|| {},
                durability: crate::operation::DurabilityProfile::Versioned,
                retention: crate::operation::VersionRetentionPolicy::Recent,
            },
        )
    }

    fn denied(message: &str) -> NativeFailure {
        NativeFailure {
            kind: NativeFailureKind::Denied,
            message: message.to_string(),
        }
    }

    #[test]
    fn preserved_version_is_discarded_when_the_trash_did_not_happen() {
        for rejected in [
            TrashItemOutcome::Failed(denied("denied")),
            TrashItemOutcome::Unsupported(denied("no Trash on this volume")),
            TrashItemOutcome::Cancelled,
        ] {
            let temp = TempDir::new();
            let path = temp.file("kept.txt", "value");
            let versions = RecordingVersionStore::new(temp.path().join("versions"), false);
            let port = ScriptedTrash::new([rejected.clone()]);

            let outcome = run_versioned_batch(
                vec![ready(path.clone())],
                &port,
                &versions,
                &AtomicBool::new(false),
            );

            assert_eq!(outcome.items[0].outcome, rejected);
            assert_eq!(outcome.items[0].version, None);
            let discarded = versions.discarded.lock().unwrap();
            assert_eq!(discarded.len(), 1, "{rejected:?}");
            assert!(!discarded[0].stored.exists(), "{rejected:?}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "value");
        }
    }

    #[test]
    fn preserved_version_is_kept_when_the_trash_result_is_indeterminate() {
        let temp = TempDir::new();
        let path = temp.file("unknown.txt", "value");
        let versions = RecordingVersionStore::new(temp.path().join("versions"), false);
        let port = ScriptedTrash::new([TrashItemOutcome::Indeterminate(denied("lost reply"))]);

        let outcome =
            run_versioned_batch(vec![ready(path)], &port, &versions, &AtomicBool::new(false));

        assert!(matches!(
            outcome.items[0].outcome,
            TrashItemOutcome::Indeterminate(_)
        ));
        let record = outcome.items[0].version.as_ref().expect("kept version");
        assert!(record.stored.exists());
        assert!(versions.discarded.lock().unwrap().is_empty());
    }

    #[test]
    fn failed_version_discard_keeps_the_item_outcome() {
        for rejected in [
            TrashItemOutcome::Failed(denied("denied")),
            TrashItemOutcome::StaleBinding,
            TrashItemOutcome::Missing,
            TrashItemOutcome::Cancelled,
        ] {
            let temp = TempDir::new();
            let path = temp.file("kept.txt", "value");
            let versions = RecordingVersionStore::new(temp.path().join("versions"), true);
            let port = ScriptedTrash::new([rejected.clone()]);

            let outcome =
                run_versioned_batch(vec![ready(path)], &port, &versions, &AtomicBool::new(false));

            assert_eq!(outcome.items[0].outcome, rejected);
            assert_eq!(outcome.items[0].version, None);
            assert_eq!(versions.discarded.lock().unwrap().len(), 1, "{rejected:?}");
            let counted_as_failure = rejected != TrashItemOutcome::Cancelled;
            assert_eq!(outcome.failed, usize::from(counted_as_failure));
            assert_eq!(outcome.cancelled, !counted_as_failure);
        }
    }

    /// Version store whose preserve raises the batch's cancel flag, as a
    /// user cancelling during a long folder copy would.
    struct CancellingVersionStore<'a> {
        inner: RecordingVersionStore,
        cancel: &'a AtomicBool,
    }

    impl DeleteVersionStore for CancellingVersionStore<'_> {
        fn preserve(
            &self,
            target: &TrashTarget,
            operation_id: &OperationId,
            key: crate::operation::IdempotencyKey,
            retention: crate::operation::VersionRetentionPolicy,
        ) -> Result<Option<crate::version_store::VersionRecord>, NativeFailure> {
            let record = self.inner.preserve(target, operation_id, key, retention);
            self.cancel.store(true, Ordering::Release);
            record
        }

        fn discard(
            &self,
            record: &crate::version_store::VersionRecord,
        ) -> Result<(), NativeFailure> {
            self.inner.discard(record)
        }
    }

    #[test]
    fn cancel_during_version_preserve_skips_the_trash_and_discards_the_copy() {
        let temp = TempDir::new();
        let first = temp.file("first.txt", "one");
        let second = temp.file("second.txt", "two");
        let cancel = AtomicBool::new(false);
        let versions = CancellingVersionStore {
            inner: RecordingVersionStore::new(temp.path().join("versions"), false),
            cancel: &cancel,
        };
        let port = ScriptedTrash::new([TrashItemOutcome::Trashed]);

        let outcome = run_versioned_batch(
            vec![ready(first.clone()), ready(second.clone())],
            &port,
            &versions,
            &cancel,
        );

        assert!(port.calls.lock().unwrap().is_empty());
        assert_eq!(
            outcome
                .items
                .iter()
                .map(|item| &item.outcome)
                .collect::<Vec<_>>(),
            vec![&TrashItemOutcome::Cancelled, &TrashItemOutcome::Cancelled]
        );
        assert_eq!(outcome.items[0].version, None);
        let discarded = versions.inner.discarded.lock().unwrap();
        assert_eq!(discarded.len(), 1);
        assert!(!discarded[0].stored.exists());
        assert!(outcome.cancelled);
        assert_eq!(outcome.failed, 0);
        assert_eq!(std::fs::read_to_string(first).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(second).unwrap(), "two");
    }

    #[test]
    fn single_item_trash_reports_an_adapter_panic_as_indeterminate() {
        let temp = TempDir::new();
        let path = temp.file("redo.txt", "value");
        let controller = DeleteController::new(Arc::new(PanickingTrash));

        let outcome = controller.trash_one(&target(path));

        assert!(matches!(outcome, TrashItemOutcome::Indeterminate(_)));
    }

    #[test]
    fn worker_start_failure_marks_every_item_retryable() {
        let items = vec![
            TrashBatchItem::CaptureFailed {
                path: PathBuf::from("/a.txt"),
                failure: denied("unused"),
            },
            TrashBatchItem::CaptureFailed {
                path: PathBuf::from("/b.txt"),
                failure: denied("unused"),
            },
        ];
        let context = DeleteRunContext::capture(&items, DeleteOrigin::Confirmation);

        let outcome = failed_outcome(&context, denied("no thread"));

        assert_eq!(outcome.failed, 2);
        assert_eq!(outcome.trashed, 0);
        assert!(!outcome.indeterminate);
        assert!(!outcome.cancelled);
        assert_eq!(outcome.failures.len(), 2);
        assert!(
            outcome
                .failures
                .iter()
                .all(|failure| failure.class == FailureClass::Retryable)
        );
    }
}
