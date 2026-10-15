use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
struct TerminalContext {
    operation_id: OperationId,
    target: PathBuf,
    #[cfg(test)]
    before_publish: Option<BeforeTerminalPublishHook>,
}

impl TerminalContext {
    fn capture(spec: &TransferSpec) -> Self {
        Self {
            operation_id: spec.operation_id.clone(),
            target: spec.target.clone(),
            #[cfg(test)]
            before_publish: spec.before_terminal_publish.clone(),
        }
    }
}

struct PanicTerminalGuard<'a, N: Fn()> {
    terminal: TerminalContext,
    progress: &'a TransferState,
    notify: &'a N,
    journal_enabled: bool,
    journal_started: bool,
    armed: bool,
}

impl<'a, N: Fn()> PanicTerminalGuard<'a, N> {
    const fn new(
        terminal: TerminalContext,
        progress: &'a TransferState,
        notify: &'a N,
        journal_enabled: bool,
    ) -> Self {
        Self {
            terminal,
            progress,
            notify,
            journal_enabled,
            journal_started: false,
            armed: true,
        }
    }

    const fn journal_started(&mut self) {
        self.journal_started = true;
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<N: Fn()> Drop for PanicTerminalGuard<'_, N> {
    fn drop(&mut self) {
        if !self.armed || !std::thread::panicking() {
            return;
        }
        record_failure(
            self.progress,
            "Operation",
            ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                Some(self.terminal.target.clone()),
                "transfer worker panicked before completing finalization",
            ),
        );
        prepare_terminal_progress(self.progress, FinalizationOutcome::NotReached);
        if self.journal_enabled && self.journal_started {
            let journal_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::operation_journal::finish(
                    &self.terminal.operation_id,
                    crate::operation_journal::OperationStatus::NeedsReview,
                )
            }));
            match journal_result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => record_journal_error(
                    self.progress,
                    "Operation",
                    Some(self.terminal.target.clone()),
                    error,
                ),
                Err(_) => record_journal_error(
                    self.progress,
                    "Operation",
                    Some(self.terminal.target.clone()),
                    "operation journal finalization panicked",
                ),
            }
        }
        publish_terminal_progress(self.progress);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.notify)()));
    }
}

fn finalize_and_publish(
    terminal: &TerminalContext,
    progress: &TransferState,
    journal_enabled: bool,
    status: crate::operation_journal::OperationStatus,
) {
    if journal_enabled
        && let Err(error) = crate::operation_journal::finish(&terminal.operation_id, status)
    {
        record_journal_error(progress, "Operation", Some(terminal.target.clone()), error);
    }
    run_before_terminal_publish(terminal);
    publish_terminal_progress(progress);
}

#[cfg(test)]
fn run_before_terminal_publish(terminal: &TerminalContext) {
    if let Some(hook) = &terminal.before_publish {
        hook();
    }
}

#[cfg(not(test))]
const fn run_before_terminal_publish(_terminal: &TerminalContext) {}

pub fn spawn_transfer(
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    spawn_transfer_on(crate::workload::global_handle(), spec, progress, notify);
}

#[cfg(test)]
pub fn spawn_transfer_with_workload(
    workload: crate::workload::WorkloadHandle,
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    spawn_transfer_on(workload, spec, progress, notify);
}

fn spawn_transfer_on(
    workload: crate::workload::WorkloadHandle,
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    TransferExecutor::new(workload, spec, progress, notify).submit();
}

pub(super) struct TransferExecutor<N> {
    workload: crate::workload::WorkloadHandle,
    spec: TransferSpec,
    progress: TransferState,
    notify: N,
    backends: backend::BackendPorts,
}

impl<N: Fn() + Send + 'static> TransferExecutor<N> {
    fn new(
        workload: crate::workload::WorkloadHandle,
        spec: TransferSpec,
        progress: TransferState,
        notify: N,
    ) -> Self {
        Self {
            workload,
            spec,
            progress,
            notify,
            backends: backend::BackendPorts::production(),
        }
    }

    #[cfg(test)]
    pub(super) const fn with_backends(
        workload: crate::workload::WorkloadHandle,
        spec: TransferSpec,
        progress: TransferState,
        notify: N,
        backends: backend::BackendPorts,
    ) -> Self {
        Self {
            workload,
            spec,
            progress,
            notify,
            backends,
        }
    }

    pub(super) fn submit(self) {
        let Self {
            workload,
            spec,
            progress,
            notify,
            backends,
        } = self;
        static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let task_root = spec.target.clone();
        let worker_progress = Arc::clone(&progress);
        let notify = Arc::new(Mutex::new(notify));
        let worker_notify = Arc::clone(&notify);
        let abandoned_progress = Arc::clone(&progress);
        let abandoned_notify = Arc::clone(&notify);
        let abandoned_operation_id = spec.operation_id.clone();
        let abandoned_target = task_root.clone();
        let task_spec = crate::workload::TaskSpec::new(
            crate::workload::TaskKind::Transfer,
            task_root.clone(),
            generation,
        )
        .priority(crate::workload::Priority::Critical)
        .estimated_bytes(64 * 1024 * 1024);
        let worker = move |scheduler_cancel: crate::workload::CancellationToken| {
            let mut spec = spec;
            let progress = worker_progress;
            let backend_progress = backend::BackendProgress::new(Arc::clone(&progress));
            let notify = || (crate::lock_util::recover(&worker_notify))();
            let mount_wait_override: Option<Arc<dyn Fn() -> std::io::Result<()> + Send + Sync>> = {
                #[cfg(test)]
                {
                    spec.mount_wait_override.clone()
                }
                #[cfg(not(test))]
                {
                    None
                }
            };
            if scheduler_cancel.is_cancelled() {
                crate::lock_util::recover(&progress).cancelled = true;
                finish_progress(&progress, FinalizationOutcome::NotReached);
                notify();
                return;
            }
            let journal_enabled = journal_enabled(&spec);
            {
                let mut state = crate::lock_util::recover(&progress);
                state.operation_id = Some(spec.operation_id.clone());
                state.group_id = spec.group_id.clone();
                state.set_phase(crate::operation_view::OperationPhase::Scan);
            }
            let terminal = TerminalContext::capture(&spec);
            let mut terminal_guard =
                PanicTerminalGuard::new(terminal.clone(), &progress, &notify, journal_enabled);
            notify();
            let expectations_complete = spec.expectations.len() == spec.entries.len();
            if spec.preflight_bytes.is_some() && !expectations_complete {
                record_failure(
                    &progress,
                    "Operation",
                    ClassifiedFailure::message(
                        FailureClass::IntegrityUncertain,
                        None,
                        "Confirmed resource preflight snapshot is incomplete",
                    ),
                );
                finish_progress(&progress, FinalizationOutcome::NotReached);
                terminal_guard.disarm();
                notify();
                return;
            }
            let needs_worker_preflight = spec.preflight_bytes.is_none();
            let mut worker_preflight = needs_worker_preflight
                .then(|| crate::scan::transfer_preflight(&spec.entries, spec.symlink_policy));
            let total_bytes = match spec.preflight_bytes.map(Ok).unwrap_or_else(|| {
                worker_preflight
                    .as_ref()
                    .map(|scan| scan.need_bytes.clone())
                    .unwrap_or_else(|| {
                        Err(crate::ports::NativeFailure {
                            kind: crate::ports::NativeFailureKind::Unknown,
                            message: "Transfer resource preflight was unavailable".to_string(),
                        })
                    })
            }) {
                Ok(total_bytes) => total_bytes,
                Err(failure) => {
                    record_failure(
                        &progress,
                        "Operation",
                        ClassifiedFailure::message(FailureClass::Blocked, None, failure.message),
                    );
                    finish_progress(&progress, FinalizationOutcome::NotReached);
                    terminal_guard.disarm();
                    notify();
                    return;
                }
            };
            {
                let mut state = crate::lock_util::recover(&progress);
                state.total_bytes = total_bytes;
                state.total_known = true;
                state.set_phase(crate::operation_view::OperationPhase::Plan);
            }
            notify();
            if spec.expectations.len() != spec.entries.len() {
                let Some(scan) = worker_preflight.take() else {
                    record_failure(
                        &progress,
                        "Operation",
                        ClassifiedFailure::message(
                            FailureClass::Blocked,
                            None,
                            "Transfer source preflight was unavailable",
                        ),
                    );
                    finish_progress(&progress, FinalizationOutcome::NotReached);
                    terminal_guard.disarm();
                    notify();
                    return;
                };
                match expectations_from_source_identities(
                    &spec.entries,
                    &spec.target,
                    scan.source_identities,
                ) {
                    Ok(expectations) => spec.expectations = expectations,
                    Err(failure) => {
                        record_failure(
                            &progress,
                            "Operation",
                            ClassifiedFailure::message(
                                FailureClass::IntegrityUncertain,
                                None,
                                failure.message,
                            ),
                        );
                        finish_progress(&progress, FinalizationOutcome::NotReached);
                        terminal_guard.disarm();
                        notify();
                        return;
                    }
                }
            } else if let Some(scan) = worker_preflight.take()
                && let Err(failure) = rebind_expectations_to_worker_scan(
                    &mut spec.expectations,
                    scan.source_identities,
                )
            {
                record_failure(
                    &progress,
                    "Operation",
                    ClassifiedFailure::message(
                        FailureClass::IntegrityUncertain,
                        None,
                        failure.message,
                    ),
                );
                finish_progress(&progress, FinalizationOutcome::NotReached);
                terminal_guard.disarm();
                notify();
                return;
            }
            let target_profile = crate::volume_profile::profile(&spec.target);
            let target_mount = crate::mount_guard::MountGuard::capture(
                &spec.target,
                crate::mount_guard::ReconnectPolicy::default(),
            );
            let resource_rule = crate::transfer_tuning::rule_for(&target_profile);
            let tuning = crate::transfer_tuning::snapshot(&target_profile);
            {
                let mut state = crate::lock_util::recover(&progress);
                state.backend_label = target_profile.backend.label().to_string();
                state.backend_reason = target_profile.reason.clone();
                state.p95_latency_ms = tuning.p95_latency_ms;
                state.adaptive_concurrency = tuning.concurrency;
                state.bandwidth_limit = resource_rule.max_bytes_per_second;
            }
            if journal_enabled {
                if let Err(error) = crate::operation_journal::begin(&spec) {
                    record_journal_error(&progress, "Operation", Some(spec.target.clone()), error);
                    finish_progress(&progress, FinalizationOutcome::NotReached);
                    terminal_guard.disarm();
                    notify();
                    return;
                }
                terminal_guard.journal_started();
            }
            match wait_for_mount(
                &target_mount,
                "Destination volume",
                &progress,
                &scheduler_cancel,
                mount_wait_override.as_deref(),
                &notify,
            ) {
                Ok(()) => {}
                Err(MountWaitError::Interrupted) => {
                    prepare_terminal_progress(&progress, FinalizationOutcome::NotReached);
                    finalize_and_publish(
                        &terminal,
                        &progress,
                        journal_enabled,
                        crate::operation_journal::OperationStatus::Stopped,
                    );
                    terminal_guard.disarm();
                    notify();
                    return;
                }
                Err(MountWaitError::Unavailable(error)) => {
                    record_failure(
                        &progress,
                        "Operation",
                        ClassifiedFailure::io(
                            Some(spec.target.clone()),
                            "mount unavailable",
                            &error,
                        ),
                    );
                    prepare_terminal_progress(&progress, FinalizationOutcome::NotReached);
                    finalize_and_publish(
                        &terminal,
                        &progress,
                        journal_enabled,
                        crate::operation_journal::OperationStatus::Failed,
                    );
                    terminal_guard.disarm();
                    notify();
                    return;
                }
            }
            while resource_rule.is_quiet_now() {
                let should_stop = {
                    let mut state = crate::lock_util::recover(&progress);
                    let reason = crate::operation_view::PauseReason::QuietHours { resume_at: None };
                    state.waiting_reason = Some(reason.label());
                    state.pause_reason = Some(reason);
                    if scheduler_cancel.is_cancelled() {
                        state.cancelled = true;
                    }
                    state.cancelled || state.stop_requested
                };
                notify();
                if should_stop {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            {
                let mut state = crate::lock_util::recover(&progress);
                state.waiting_reason = None;
                state.pause_reason = None;
            }
            let is_move = spec.kind == TransferKind::Move;
            let mut base_bytes: u64 = 0;
            let expectations = spec.expectations;
            let mut work = spec
                .entries
                .into_iter()
                .zip(expectations)
                .enumerate()
                .map(|(index, (entry, expectation))| {
                    let destination = spec.target.join(&entry.name);
                    let key = expectation
                        .key
                        .clone()
                        .unwrap_or_else(|| spec.operation_id.step_key(index, &destination));
                    TransferWorkItem {
                        key,
                        entry,
                        expectation,
                        mount_retries: 0,
                    }
                })
                .collect::<VecDeque<_>>();

            crate::lock_util::recover(&progress)
                .set_phase(crate::operation_view::OperationPhase::Transfer);
            notify();

            'work: while let Some(mut work_item) = work.pop_front() {
                let this_size = work_item.expectation.source.as_ref().map_or_else(
                    |_| entry_size(&work_item.entry),
                    |source| source.logical_bytes,
                );
                let entry = work_item.entry.clone();
                let dest = spec.target.join(&entry.name);

                {
                    let mut s = crate::lock_util::recover(&progress);
                    s.current_file = entry.name.clone();
                    if scheduler_cancel.is_cancelled() {
                        s.cancelled = true;
                    }
                    if s.cancelled {
                        break; // fall through to the finished-setter below
                    }
                    if s.stop_requested {
                        s.stopped = true;
                        break;
                    }
                }

                match wait_for_mount(
                    &target_mount,
                    "Destination volume",
                    &progress,
                    &scheduler_cancel,
                    mount_wait_override.as_deref(),
                    &notify,
                ) {
                    Ok(()) => {}
                    Err(MountWaitError::Interrupted) => break 'work,
                    Err(MountWaitError::Unavailable(error)) => {
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            Some(ClassifiedFailure::io(
                                Some(spec.target.clone()),
                                "mount unavailable",
                                &error,
                            )),
                            Some((&spec.operation_id, &work_item.key, journal_enabled)),
                            &notify,
                        );
                        continue;
                    }
                }

                if journal_enabled {
                    match crate::operation_journal::step_is_settled(
                        &spec.operation_id,
                        &work_item.key,
                    ) {
                        Ok(true) => {
                            complete_without_copy(
                                &progress,
                                &mut base_bytes,
                                this_size,
                                &entry.name,
                                None,
                                None,
                                &notify,
                            );
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            complete_without_copy(
                                &progress,
                                &mut base_bytes,
                                this_size,
                                &entry.name,
                                Some(ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(dest.clone()),
                                    error,
                                )),
                                None,
                                &notify,
                            );
                            continue;
                        }
                    }
                }

                let expected_source = match work_item.expectation.source.clone() {
                    Ok(identity) => identity,
                    Err(message) => {
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            Some(ClassifiedFailure::message(
                                FailureClass::Blocked,
                                Some(entry.path.clone()),
                                message,
                            )),
                            Some((&spec.operation_id, &work_item.key, journal_enabled)),
                            &notify,
                        );
                        continue;
                    }
                };
                let source_before =
                    match crate::scan::capture_transfer_source(&entry.path, spec.symlink_policy) {
                        Ok(identity) => identity,
                        Err(failure) => {
                            complete_without_copy(
                                &progress,
                                &mut base_bytes,
                                this_size,
                                &entry.name,
                                Some(ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(entry.path.clone()),
                                    failure.message,
                                )),
                                Some((&spec.operation_id, &work_item.key, journal_enabled)),
                                &notify,
                            );
                            continue;
                        }
                    };
                if !expected_source.same_binding(&source_before) {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        Some(ClassifiedFailure::message(
                            FailureClass::IntegrityUncertain,
                            Some(entry.path.clone()),
                            "source changed after transfer preflight",
                        )),
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    );
                    continue;
                }
                let source_mount = crate::mount_guard::MountGuard::capture(
                    &entry.path,
                    crate::mount_guard::ReconnectPolicy::default(),
                );

                if spec.symlink_policy == crate::filesystem_policy::SymlinkPolicy::Skip
                    && source_before.lexical.kind == Some(crate::path_identity::PathKind::Symlink)
                {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        None,
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    );
                    continue;
                }

                let expected_destination = match work_item.expectation.destination.clone() {
                    Ok(identity) => identity,
                    Err(message) => {
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            Some(ClassifiedFailure::message(
                                FailureClass::Blocked,
                                Some(dest.clone()),
                                message,
                            )),
                            Some((&spec.operation_id, &work_item.key, journal_enabled)),
                            &notify,
                        );
                        continue;
                    }
                };

                // Reject destructive self-referential transfers (a directory into
                // itself or its own subtree, a file onto itself) before touching
                // anything, so neither source nor destination is harmed.
                if fs_util::is_within_or_equal(&dest, &entry.path) {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        Some(ClassifiedFailure::message(
                            FailureClass::UserDecision,
                            Some(entry.path.clone()),
                            "cannot copy a path into itself",
                        )),
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    );
                    continue;
                }

                // Check the destination LIVE, not the scan-time conflict list: the
                // confirmation dialog can sit open while the filesystem changes.
                // `path_is_taken` (not `exists`) so a broken symlink occupying the
                // name is honoured as a conflict, matching `find_conflicts`.
                let dest_present = fs_util::path_is_taken(&dest);
                if dest_present {
                    match spec.policy {
                        OverwritePolicy::SkipAll => {
                            complete_without_copy(
                                &progress,
                                &mut base_bytes,
                                this_size,
                                &entry.name,
                                None,
                                Some((&spec.operation_id, &work_item.key, journal_enabled)),
                                &notify,
                            );
                            continue;
                        }
                        OverwritePolicy::Ask => {
                            // No overwrite was confirmed (no conflict was shown, or
                            // the destination appeared after the scan): refuse
                            // rather than silently clobber it.
                            complete_without_copy(
                                &progress,
                                &mut base_bytes,
                                this_size,
                                &entry.name,
                                Some(ClassifiedFailure::message(
                                    FailureClass::UserDecision,
                                    Some(dest.clone()),
                                    "destination already exists",
                                )),
                                Some((&spec.operation_id, &work_item.key, journal_enabled)),
                                &notify,
                            );
                            continue;
                        }
                        // Fall through; KeepBoth/OverwriteAll handled below.
                        OverwritePolicy::OverwriteAll | OverwritePolicy::KeepBoth => {}
                    }
                }

                let (errors_before, failures_before) = {
                    let state = crate::lock_util::recover(&progress);
                    (state.errors.len(), state.failures.len())
                };

                // Every entry is staged beside its final landing. This gives new
                // directories the same no-merge guarantee as files and leaves one
                // commit point where destination identity can be revalidated.
                let landing = work_item.expectation.landing.clone().unwrap_or_else(|| {
                    if dest_present && spec.policy == OverwritePolicy::KeepBoth {
                        fs_util::available_copy_name(&dest)
                    } else {
                        dest.clone()
                    }
                });
                let expected_landing =
                    work_item
                        .expectation
                        .landing_before
                        .clone()
                        .unwrap_or_else(|| {
                            if landing == dest {
                                expected_destination
                            } else {
                                PathIdentity::missing(&landing)
                            }
                        });
                let replace_existing = expected_landing.exists;
                let copy_target = work_item.expectation.resume.as_ref().map_or_else(
                    || staging_path(&landing),
                    |checkpoint| checkpoint.staging.clone(),
                );
                if journal_enabled
                    && let Err(error) = crate::operation_journal::mark_running(
                        &spec.operation_id,
                        &work_item.key,
                        &copy_target,
                        &landing,
                        expected_landing.clone(),
                    )
                {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        Some(ClassifiedFailure::message(
                            FailureClass::IntegrityUncertain,
                            Some(landing.clone()),
                            format!("operation journal update failed: {error}"),
                        )),
                        None,
                        &notify,
                    );
                    continue;
                }

                let attempt_base = base_bytes;
                let attempt_started = std::time::Instant::now();
                let attempt_tuning = crate::transfer_tuning::snapshot(&target_profile);
                let mut checkpoint_sink = JournalCheckpointSink {
                    step: JournalStep {
                        operation_id: &spec.operation_id,
                        key: &work_item.key,
                        enabled: journal_enabled,
                    },
                };
                let mut copy_entry = || {
                    backends.stage(
                        spec.method,
                        backend::StageRequest {
                            source: &entry.path,
                            is_dir: entry.is_dir,
                            staging: &copy_target,
                            progress: &backend_progress,
                            base_bytes,
                            profile: &target_profile,
                            rule: resource_rule,
                            tuning: attempt_tuning,
                            basis: dest_present.then_some(dest.as_path()),
                            resume: work_item.expectation.resume.as_ref(),
                            symlink_policy: spec.symlink_policy,
                            durability: spec.durability,
                        },
                        &mut checkpoint_sink,
                    )
                };
                // A move first attempts the atomic rename itself. `EXDEV` is the
                // authoritative cross-volume result and falls back to the normal
                // copy pipeline without a separate device-id OS read.
                let try_rename = is_move
                    && spec.symlink_policy == crate::filesystem_policy::SymlinkPolicy::Preserve;
                let mut renamed = false;
                let mut result = if try_rename {
                    match rename_entry(
                        &entry.path,
                        &copy_target,
                        &entry,
                        this_size,
                        &progress,
                        base_bytes,
                    ) {
                        Ok(bytes) => {
                            renamed = true;
                            backend::receipt(
                                &copy_target,
                                bytes,
                                crate::transfer_tuning::FastPath::Rename,
                                spec.durability,
                            )
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                            copy_entry()
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    copy_entry()
                };
                crate::transfer_tuning::record(
                    &target_profile,
                    attempt_started.elapsed(),
                    result.is_ok(),
                );
                let tuning = crate::transfer_tuning::snapshot(&target_profile);
                {
                    let mut state = crate::lock_util::recover(&progress);
                    state.p95_latency_ms = tuning.p95_latency_ms;
                    state.adaptive_concurrency = tuning.concurrency;
                }

                #[cfg(test)]
                if result.is_ok()
                    && let Some(hook) = &spec.before_commit
                {
                    hook(&entry.path, &landing);
                }

                if result.is_ok() {
                    let cancelled = {
                        let mut state = crate::lock_util::recover(&progress);
                        if scheduler_cancel.is_cancelled() {
                            state.cancelled = true;
                        }
                        state.cancelled
                    };
                    if cancelled {
                        result = Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "transfer cancelled before commit",
                        ));
                    }
                }
                if result.is_ok()
                    && let Err(error) =
                        check_mount_fence(&target_mount, mount_wait_override.as_deref())
                {
                    result = Err(error);
                }
                if let Ok(receipt) = &result {
                    let current = if receipt.artifact.tree_fingerprint.is_some() {
                        PathIdentity::observe_deep(&copy_target)
                    } else {
                        PathIdentity::observe(&copy_target)
                    };
                    match current {
                        Ok(current) if receipt.artifact.same_binding(&current) => {}
                        Ok(_) => {
                            result = Err(std::io::Error::other(
                                "staging artifact changed before commit",
                            ));
                        }
                        Err(error) => result = Err(error),
                    }
                }
                if result
                    .as_ref()
                    .is_ok_and(|receipt| spec.durability.verifies() && !receipt.durable)
                {
                    result = Err(std::io::Error::other(
                        "staging backend returned no durability receipt",
                    ));
                }

                match &result {
                    Ok(outcome) => base_bytes = base_bytes.saturating_add(outcome.bytes),
                    Err(e) => {
                        let checkpoint = (!renamed && !entry.is_dir)
                            .then(|| {
                                valid_runtime_checkpoint(
                                    &spec.operation_id,
                                    &work_item.key,
                                    &entry.path,
                                    &copy_target,
                                    spec.symlink_policy,
                                    journal_enabled,
                                )
                            })
                            .flatten();
                        let resumable_partial = checkpoint.is_some();
                        if crate::lock_util::recover(&progress).cancelled {
                            // For a rename this is a no-op (a failed rename never
                            // created `copy_target`); for a copy it drops the
                            // partial. Either way the source is left intact.
                            if !resumable_partial
                                && let Some(msg) =
                                    undo_placement(&copy_target, &entry.path, renamed)
                            {
                                record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::message(
                                        FailureClass::IntegrityUncertain,
                                        Some(copy_target.clone()),
                                        msg,
                                    ),
                                );
                            }
                            break; // fall through to the finished-setter below
                        }
                        if is_disconnect_error(e)
                            && work_item.mount_retries < MAX_MOUNT_RETRIES
                            && !vanished_on_mounted_volumes(e, &source_mount, &target_mount)
                        {
                            let (reconnect_guard, reconnect_label) = if mount_wait_override
                                .is_none()
                                && source_mount.check_bounded()
                                    != crate::mount_guard::MountAvailability::Available
                            {
                                (&source_mount, "Source volume")
                            } else {
                                (&target_mount, "Destination volume")
                            };
                            match wait_for_mount(
                                reconnect_guard,
                                reconnect_label,
                                &progress,
                                &scheduler_cancel,
                                mount_wait_override.as_deref(),
                                &notify,
                            ) {
                                Ok(()) => {
                                    if checkpoint.is_none()
                                        && let Some(msg) =
                                            undo_placement(&copy_target, &entry.path, renamed)
                                    {
                                        record_failure(
                                            &progress,
                                            &entry.name,
                                            ClassifiedFailure::message(
                                                FailureClass::IntegrityUncertain,
                                                Some(copy_target.clone()),
                                                msg,
                                            ),
                                        );
                                    }
                                    work_item.expectation.resume = checkpoint;
                                    work_item.mount_retries += 1;
                                    base_bytes = attempt_base;
                                    {
                                        let mut state = crate::lock_util::recover(&progress);
                                        state.copied_bytes = attempt_base;
                                        state.current_file_copied = 0;
                                    }
                                    work.push_front(work_item);
                                    notify();
                                    continue 'work;
                                }
                                Err(MountWaitError::Interrupted) => {
                                    if !resumable_partial
                                        && let Some(msg) =
                                            undo_placement(&copy_target, &entry.path, renamed)
                                    {
                                        record_failure(
                                            &progress,
                                            &entry.name,
                                            ClassifiedFailure::message(
                                                FailureClass::IntegrityUncertain,
                                                Some(copy_target.clone()),
                                                msg,
                                            ),
                                        );
                                    }
                                    break 'work;
                                }
                                Err(MountWaitError::Unavailable(wait_error)) => record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::io(
                                        Some(entry.path.clone()),
                                        "mount reconnect failed",
                                        &wait_error,
                                    ),
                                ),
                            }
                        } else {
                            // A failed directory stage is kept (see below); name
                            // the hidden tree so the partial copy can be found.
                            let context =
                                if !renamed && entry.is_dir && fs_util::path_is_taken(&copy_target)
                                {
                                    format!(
                                        "copy failed; partial copy kept at {}",
                                        copy_target.display()
                                    )
                                } else {
                                    "copy failed".to_string()
                                };
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::io(Some(entry.path.clone()), &context, e),
                            );
                        }
                        crate::volume_profile::invalidate(target_profile.volume_id);
                        // The failed entry is finished: count its planned size,
                        // as a skipped entry does, so the next entry's progress
                        // does not restart below what was already reported.
                        base_bytes = attempt_base.saturating_add(this_size);
                        crate::lock_util::recover(&progress).copied_bytes = base_bytes;
                    }
                }

                if result.is_ok()
                    && crate::lock_util::recover(&progress).errors.len() == errors_before
                {
                    let observed_source = if renamed {
                        copy_target.as_path()
                    } else {
                        entry.path.as_path()
                    };
                    match crate::scan::capture_transfer_source(observed_source, spec.symlink_policy)
                    {
                        Ok(source_after)
                            if if renamed {
                                source_before.same_version(&source_after)
                            } else {
                                source_before.same_binding(&source_after)
                            } => {}
                        Ok(_) => {
                            let restore_error = undo_placement(&copy_target, &entry.path, renamed);
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(entry.path.clone()),
                                    "source changed while it was being copied",
                                ),
                            );
                            if let Some(message) = restore_error {
                                record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::message(
                                        FailureClass::IntegrityUncertain,
                                        Some(copy_target.clone()),
                                        message,
                                    ),
                                );
                            }
                        }
                        Err(failure) => {
                            let restore_error = undo_placement(&copy_target, &entry.path, renamed);
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(entry.path.clone()),
                                    failure.message,
                                ),
                            );
                            if let Some(message) = restore_error {
                                record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::message(
                                        FailureClass::IntegrityUncertain,
                                        Some(copy_target.clone()),
                                        message,
                                    ),
                                );
                            }
                        }
                    }
                }

                // "Clean" = Ok return AND no per-file errors recorded by the
                // native callback during this entry.
                let mut clean = result.is_ok()
                    && crate::lock_util::recover(&progress).errors.len() == errors_before;
                if clean
                    && spec.durability.verifies()
                    && !renamed
                    && !transfer_paths_equal(&entry.path, &copy_target, spec.symlink_policy)
                {
                    record_failure(
                        &progress,
                        &entry.name,
                        ClassifiedFailure::message(
                            FailureClass::IntegrityUncertain,
                            Some(copy_target.clone()),
                            "copy verification failed",
                        ),
                    );
                    clean = false;
                }
                if clean {
                    let current = if expected_landing.tree_fingerprint.is_some() {
                        PathIdentity::observe_deep(&landing)
                    } else {
                        PathIdentity::observe(&landing)
                    };
                    match current {
                        Ok(current) if expected_landing.same_version(&current) => {}
                        Ok(_) => {
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::message(
                                    FailureClass::UserDecision,
                                    Some(landing.clone()),
                                    "destination changed after conflict review",
                                ),
                            );
                            clean = false;
                        }
                        Err(error) => {
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::io(
                                    Some(landing.clone()),
                                    "destination identity check failed",
                                    &error,
                                ),
                            );
                            clean = false;
                        }
                    }
                }

                let resumable_partial = result.is_err()
                    && !renamed
                    && !entry.is_dir
                    && valid_runtime_checkpoint(
                        &spec.operation_id,
                        &work_item.key,
                        &entry.path,
                        &copy_target,
                        spec.symlink_policy,
                        journal_enabled,
                    )
                    .is_some();
                // A failed native (or other) directory stage may already hold
                // successfully copied siblings under the staging root. Wiping
                // that whole tree would discard recoverable work; leave it in
                // place and report the failure instead. A stage that completed
                // but was rejected afterwards (destination or source changed)
                // is a full duplicate of the source and is removed.
                let preserve_partial_dir = result.is_err()
                    && !renamed
                    && entry.is_dir
                    && fs_util::path_is_taken(&copy_target);
                let placed = if !clean {
                    // Undo our placement; a pre-existing dest is untouched. For a
                    // rename this restores the source rather than deleting its only
                    // copy.
                    if !resumable_partial
                        && !preserve_partial_dir
                        && let Some(msg) = undo_placement(&copy_target, &entry.path, renamed)
                    {
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::message(
                                FailureClass::IntegrityUncertain,
                                Some(copy_target.clone()),
                                msg,
                            ),
                        );
                    }
                    false
                } else {
                    let mut preserved_version = None;
                    let version_result = if replace_existing && spec.durability.keeps_versions() {
                        crate::version_store::preserve_expected_with_policy(
                            &landing,
                            &expected_landing,
                            &spec.operation_id,
                            work_item.key.clone(),
                            spec.version_retention,
                        )
                        .map(|record| {
                            preserved_version = record;
                        })
                        .map_err(|failure| std::io::Error::other(failure.message))
                    } else {
                        Ok(())
                    };
                    let placement = version_result.and_then(|()| {
                        if replace_existing {
                            swap_into_place(
                                &copy_target,
                                &landing,
                                &expected_landing,
                                JournalStep {
                                    operation_id: &spec.operation_id,
                                    key: &work_item.key,
                                    enabled: journal_enabled,
                                },
                            )
                        } else {
                            if journal_enabled {
                                crate::operation_journal::prepare_placement(
                                    &spec.operation_id,
                                    &work_item.key,
                                    &copy_target,
                                    &landing,
                                )
                                .map_err(std::io::Error::other)?;
                            }
                            crate::fs_at::rename_sibling(&copy_target, &landing, false)?;
                            fs_util::sync_parent_namespace(&landing)?;
                            if journal_enabled {
                                crate::operation_journal::mark_placement_placed(
                                    &spec.operation_id,
                                    &work_item.key,
                                )
                                .map_err(std::io::Error::other)?;
                            }
                            Ok(None)
                        }
                    });
                    match placement {
                        Ok(cleanup_warning) => {
                            if let Some(message) = cleanup_warning {
                                record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::message(
                                        FailureClass::IntegrityUncertain,
                                        Some(landing.clone()),
                                        message,
                                    ),
                                );
                            }
                            true
                        }
                        Err(e) => {
                            if let Some(record) = &preserved_version
                                && let Err(error) = crate::version_store::discard_record(record)
                            {
                                record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::message(
                                        FailureClass::IntegrityUncertain,
                                        Some(record.stored.clone()),
                                        format!(
                                            "failed placement left an uncommitted version record: {error}"
                                        ),
                                    ),
                                );
                            }
                            // The swap left the staged data at `copy_target`. For a
                            // rename that is the source's ONLY copy, so move it back
                            // to the source instead of deleting it (the previous
                            // unconditional cleanup here lost the source on a failed
                            // same-volume overwrite move).
                            let extra = undo_placement(&copy_target, &entry.path, renamed);
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::io(
                                    Some(landing.clone()),
                                    "final placement failed",
                                    &e,
                                ),
                            );
                            if let Some(msg) = extra {
                                record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::message(
                                        FailureClass::IntegrityUncertain,
                                        Some(copy_target.clone()),
                                        msg,
                                    ),
                                );
                            }
                            false
                        }
                    }
                };

                // Delete only the source object proven before copy. The final
                // identity fence narrows the pathname race; Skip preserves every
                // omitted symlink and the directories needed to contain it.
                if is_move && placed && !renamed {
                    let cleanup_result = match crate::scan::capture_transfer_source(
                        &entry.path,
                        spec.symlink_policy,
                    ) {
                        Ok(source_now) if source_before.same_binding(&source_now) => {
                            cleanup_moved_source(
                                &entry.path,
                                &source_before.lexical,
                                spec.symlink_policy,
                            )
                        }
                        Ok(_) => Err(std::io::Error::other(
                            "source changed before post-commit cleanup",
                        )),
                        Err(failure) => Err(std::io::Error::other(failure.message)),
                    };
                    if let Err(error) = cleanup_result {
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::message(
                                FailureClass::IntegrityUncertain,
                                Some(entry.path.clone()),
                                format!(
                                    "destination is complete but source cleanup failed: {error}"
                                ),
                            ),
                        );
                    }
                }
                if is_move
                    && placed
                    && let Err(error) = fs_util::sync_parent_namespace(&entry.path)
                {
                    record_failure(
                        &progress,
                        &entry.name,
                        ClassifiedFailure::message(
                            FailureClass::IntegrityUncertain,
                            Some(entry.path.clone()),
                            format!(
                                "destination is placed but source namespace could not be synced: {error}"
                            ),
                        ),
                    );
                }

                // Record where a successfully moved entry actually landed, so undo
                // reverses the real placement (a KeepBoth conflict lands at a "copy"
                // name, not the original).
                if is_move && placed {
                    crate::lock_util::recover(&progress)
                        .placements
                        .push((entry.path.clone(), landing.clone()));
                }
                if placed {
                    crate::lock_util::recover(&progress).fast_paths.push(
                        result
                            .as_ref()
                            .expect("placed transfer has an outcome")
                            .fast_path,
                    );
                }

                if journal_enabled {
                    let step_failure = {
                        let state = crate::lock_util::recover(&progress);
                        state
                            .failures
                            .iter()
                            .skip(failures_before)
                            .max_by_key(|failure| {
                                if failure.class == FailureClass::IntegrityUncertain {
                                    1
                                } else {
                                    0
                                }
                            })
                            .cloned()
                    };
                    if let Some(failure) = step_failure {
                        if let Err(error) = crate::operation_journal::mark_failed(
                            &spec.operation_id,
                            &work_item.key,
                            failure,
                        ) {
                            record_journal_error(
                                &progress,
                                &entry.name,
                                Some(landing.clone()),
                                error,
                            );
                        }
                    } else if placed
                        && let Err(error) = crate::operation_journal::mark_completed(
                            &spec.operation_id,
                            &work_item.key,
                            &landing,
                            result
                                .as_ref()
                                .expect("placed transfer has an outcome")
                                .fast_path,
                        )
                    {
                        record_journal_error(&progress, &entry.name, Some(landing.clone()), error);
                    }
                }

                {
                    let mut s = crate::lock_util::recover(&progress);
                    s.files_done += 1;
                    s.maybe_sample();
                }
                notify();
            }

            crate::lock_util::recover(&progress)
                .set_phase(crate::operation_view::OperationPhase::Verify);
            notify();
            crate::lock_util::recover(&progress)
                .set_phase(crate::operation_view::OperationPhase::Finalize);
            notify();
            let failure_rollback = run_failure_rollback(
                spec.rollback_cleanup.as_ref(),
                &spec.operation_id,
                journal_enabled,
                &progress,
            );
            #[cfg(test)]
            if let Some(hook) = &spec.before_post_success {
                hook();
            }
            let post_success = run_post_success(spec.post_success.as_ref(), &progress);
            let finalization = FinalizationOutcome::Reached(post_success);
            prepare_terminal_progress(&progress, finalization);
            let status = {
                let state = crate::lock_util::recover(&progress);
                if failure_rollback == FailureRollback::Complete {
                    crate::operation_journal::OperationStatus::RolledBack
                } else if state
                    .failures
                    .iter()
                    .any(|failure| failure.class == FailureClass::IntegrityUncertain)
                {
                    crate::operation_journal::OperationStatus::NeedsReview
                } else if state.cancelled || state.stopped {
                    crate::operation_journal::OperationStatus::Stopped
                } else if !state.errors.is_empty() {
                    crate::operation_journal::OperationStatus::Failed
                } else {
                    crate::operation_journal::OperationStatus::Completed
                }
            };
            finalize_and_publish(&terminal, &progress, journal_enabled, status);
            terminal_guard.disarm();
            notify();
        };
        let on_abandoned = move |reason: crate::workload::AbandonReason| {
            let record_abandonment = {
                let mut state = crate::lock_util::recover(&abandoned_progress);
                if state.finished {
                    return;
                }
                state.operation_id = Some(abandoned_operation_id);
                !state.cancelled
            };
            if record_abandonment {
                record_failure(
                    &abandoned_progress,
                    "Operation",
                    ClassifiedFailure::message(
                        FailureClass::Retryable,
                        Some(abandoned_target),
                        format!("workload abandoned transfer before execution: {reason}"),
                    ),
                );
            }
            finish_progress(&abandoned_progress, FinalizationOutcome::NotReached);
            (crate::lock_util::recover(&abandoned_notify))();
        };
        let task = workload.submit_with_abandonment(task_spec, worker, on_abandoned);
        match task {
            Ok(task) => {
                let mut state = crate::lock_util::recover(&progress);
                if !state.finished {
                    state.scheduler_task = Some(task);
                }
            }
            Err(error) => {
                record_failure(
                    &progress,
                    "Operation",
                    ClassifiedFailure::message(
                        FailureClass::Blocked,
                        Some(task_root),
                        format!("workload scheduler refused transfer: {error}"),
                    ),
                );
                finish_progress(&progress, FinalizationOutcome::NotReached);
                (crate::lock_util::recover(&notify))();
            }
        }
    }
}

/// `NotFound` and `UnexpectedEof` are what a dropped network mount looks like,
/// but also what a child deleted or truncated mid-copy looks like. While both
/// volumes are still mounted, the reconnect wait returns at once and the entry
/// would only be recopied, so the failure is reported instead.
fn vanished_on_mounted_volumes(
    error: &std::io::Error,
    source_mount: &crate::mount_guard::MountGuard,
    target_mount: &crate::mount_guard::MountGuard,
) -> bool {
    use crate::mount_guard::MountAvailability::Available;
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::UnexpectedEof
    ) && source_mount.check_bounded() == Available
        && target_mount.check_bounded() == Available
}

/// Run a transfer-owned follow-up only after every entry completed without an
/// error or cancellation. Failure is appended to the normal transfer error
/// surface, so the progress dialog remains open instead of hiding cleanup loss.
fn run_post_success(
    action: Option<&PostTransferAction>,
    progress: &TransferState,
) -> PostSuccessOutcome {
    let Some(action) = action else {
        return PostSuccessOutcome::NotRequired;
    };
    let ready = {
        let state = crate::lock_util::recover(progress);
        state.files_done == state.files_total
            && state.errors.is_empty()
            && !state.cancelled
            && !state.stop_requested
            && !state.stopped
    };
    if !ready {
        return PostSuccessOutcome::Skipped;
    }

    let PostTransferAction::RemoveEmptyDir(path) = action;
    let result = match std::fs::remove_dir(path) {
        Ok(()) => fs_util::sync_parent_namespace(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => PostSuccessOutcome::Succeeded,
        Err(error) => {
            record_failure(
                progress,
                &path.display().to_string(),
                ClassifiedFailure::io(Some(path.clone()), "Post-transfer cleanup failed", &error),
            );
            PostSuccessOutcome::Failed
        }
    }
}

/// Roll back placements made inside an operation-created container when the
/// enclosing transfer does not finish cleanly. This is used by Gather: a
/// partial move must put every completed entry back before removing the folder
/// it created. No-clobber restores fail closed if another process recreated a
/// source name, leaving the user's data at the reported landing instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailureRollback {
    NotRequested,
    Complete,
    Incomplete,
}

pub(super) fn run_failure_rollback(
    container: Option<&PathBuf>,
    operation_id: &OperationId,
    journal_enabled: bool,
    progress: &TransferState,
) -> FailureRollback {
    let Some(container) = container else {
        return FailureRollback::NotRequested;
    };
    let placements = {
        let state = crate::lock_util::recover(progress);
        let incomplete = state.files_done < state.files_total || !state.errors.is_empty();
        if !incomplete {
            return FailureRollback::NotRequested;
        }
        state.placements.clone()
    };
    if journal_enabled {
        return match crate::operation_journal::rollback(operation_id) {
            Ok(plan) if plan.remaining.is_empty() => {
                crate::lock_util::recover(progress).placements.clear();
                FailureRollback::Complete
            }
            Ok(plan) => {
                for item in plan.remaining {
                    record_failure(
                        progress,
                        &item.path.display().to_string(),
                        ClassifiedFailure::message(
                            FailureClass::IntegrityUncertain,
                            Some(item.path),
                            item.action,
                        ),
                    );
                }
                FailureRollback::Incomplete
            }
            Err(error) => {
                record_journal_error(progress, "Rollback", Some(container.clone()), error);
                FailureRollback::Incomplete
            }
        };
    }
    let journal_steps = journal_enabled
        .then(|| crate::operation_journal::operation(operation_id).ok())
        .flatten()
        .map(|operation| operation.steps);

    let mut failed = Vec::new();
    let mut complete = true;
    for (source, landing) in placements.into_iter().rev() {
        if !landing.starts_with(container) {
            complete = false;
            record_failure(
                progress,
                &landing.display().to_string(),
                ClassifiedFailure::message(
                    FailureClass::IntegrityUncertain,
                    Some(landing.clone()),
                    "rollback refused a placement outside its operation container",
                ),
            );
            failed.push((source, landing));
            continue;
        }

        if let Err(error) = crate::fs_at::rename_sibling(&landing, &source, false) {
            complete = false;
            record_failure(
                progress,
                &landing.display().to_string(),
                ClassifiedFailure::message(
                    FailureClass::IntegrityUncertain,
                    Some(landing.clone()),
                    format!(
                        "rollback could not restore {}; data preserved at {}: {error}",
                        source.display(),
                        landing.display()
                    ),
                ),
            );
            failed.push((source, landing));
            continue;
        }

        if journal_enabled {
            let key = journal_steps.as_ref().and_then(|steps| {
                steps
                    .iter()
                    .find(|step| {
                        step.source == source
                            && step.landing.as_deref().unwrap_or(&step.destination) == landing
                    })
                    .map(|step| step.key.clone())
            });
            let journal_result = key.map_or_else(
                || Err("completed rollback effect is missing from the journal".to_string()),
                |key| crate::operation_journal::mark_rolled_back(operation_id, &key),
            );
            if let Err(error) = journal_result {
                complete = false;
                record_journal_error(
                    progress,
                    &landing.display().to_string(),
                    Some(source),
                    error,
                );
            }
        }
    }

    crate::lock_util::recover(progress).placements = failed;
    if !crate::lock_util::recover(progress).placements.is_empty() {
        return FailureRollback::Incomplete;
    }

    let removal = std::fs::remove_dir(container).or_else(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        }
    });
    if let Err(error) = removal {
        complete = false;
        record_failure(
            progress,
            &container.display().to_string(),
            ClassifiedFailure::io(
                Some(container.clone()),
                "rollback restored entries but could not remove its container",
                &error,
            ),
        );
    }
    if complete {
        FailureRollback::Complete
    } else {
        FailureRollback::Incomplete
    }
}

/// Publish the terminal state. A cancel request that arrived after the final
/// entry and every mandatory finalization effect completed is too late to
/// cancel anything; treating that as a cancelled Move would discard its valid
/// undo action.
pub(super) fn finish_progress(progress: &TransferState, finalization: FinalizationOutcome) {
    prepare_terminal_progress(progress, finalization);
    publish_terminal_progress(progress);
}

/// Normalize late cancellation and stop requests before deriving the durable
/// journal status. This deliberately does not expose a terminal snapshot.
fn prepare_terminal_progress(progress: &TransferState, finalization: FinalizationOutcome) {
    let mut s = crate::lock_util::recover(progress);
    s.finalization = Some(finalization);
    if s.finalization_committed_success() {
        s.cancelled = false;
        s.stop_requested = false;
        s.stopped = false;
    } else if s.stop_requested {
        s.stopped = true;
    }
    s.set_phase(crate::operation_view::OperationPhase::Finalize);
}

/// The only terminal publication point. Durable finalization and any failure
/// recording must complete before this is called.
fn publish_terminal_progress(progress: &TransferState) {
    let mut s = crate::lock_util::recover(progress);
    s.set_phase(crate::operation_view::OperationPhase::Finalize);
    s.finished = true;
    s.scheduler_task = None;
    s.record_sample();
}
/// Move one entry to `dst` with a single atomic, no-clobber rename (the
/// same-volume move fast path). `dst` is always a path that should not exist
/// yet (an absent destination, a fresh "copy" name, or a staging sibling), so
/// the rename mirrors the copy path's `EXCL` no-clobber guarantee. The entry's
/// full size is reported as copied, since a rename transfers it whole; the size
/// is read before the move while the source still exists.
fn rename_entry(
    src: &Path,
    dst: &Path,
    entry: &FileEntry,
    size: u64,
    progress: &TransferState,
    base_bytes: u64,
) -> std::io::Result<u64> {
    {
        let mut s = crate::lock_util::recover(progress);
        s.current_file = entry.name.clone();
        s.current_file_size = size;
        s.current_file_copied = 0;
    }
    crate::fs_at::rename_sibling(src, dst, false)?;
    let mut s = crate::lock_util::recover(progress);
    s.current_file_copied = size;
    s.copied_bytes = base_bytes + size;
    s.maybe_sample();
    Ok(size)
}

/// A hidden sibling of `dest` that does not exist yet, used to stage an
/// overwrite copy before swapping it into place (same directory = same
/// volume, so the final rename is atomic and clone-friendly).
fn staging_path(dest: &Path) -> PathBuf {
    hidden_sibling(dest, "cmdr-tmp")
}

/// Longest leaf name accepted by the supported filesystems: 255 bytes on
/// Linux and macOS, 255 UTF-16 units on Windows (a UTF-8 byte bound is the
/// stricter of the two).
const MAX_LEAF_BYTES: usize = 255;

/// A free hidden sibling `.{name}.{tag}.{index}` of `path`. The journal
/// records the exact pathname, and recovery only matches the `.{tag}.`
/// infix, so the name part may be shortened to keep the leaf valid.
fn hidden_sibling(path: &Path, tag: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs_util::first_available(|index| parent.join(hidden_sibling_name(&name, tag, index)))
}

/// `.{name}.{tag}.{index}`, with `name` cut on a char boundary so the whole
/// leaf fits in [`MAX_LEAF_BYTES`]. Without the cut, a name only a dozen
/// bytes under the limit could never be staged (`ENAMETOOLONG`).
fn hidden_sibling_name(name: &str, tag: &str, index: usize) -> String {
    let suffix = format!(".{tag}.{index}");
    let budget = MAX_LEAF_BYTES.saturating_sub(1 + suffix.len());
    let mut end = name.len().min(budget);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!(".{}{suffix}", &name[..end])
}

/// Remove a file or directory tree, treating "not found" as success.
fn cleanup_path(path: &Path) -> std::io::Result<()> {
    let result = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path)
        }
        Ok(_) => std::fs::remove_file(path),
        Err(error) => {
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(error)
            };
        }
    };
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub(super) fn cleanup_moved_source(
    path: &Path,
    expected: &PathIdentity,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> std::io::Result<()> {
    let quarantine = source_cleanup_path(path);
    crate::fs_at::rename_sibling(path, &quarantine, false)?;
    let observed = if expected.tree_fingerprint.is_some() {
        PathIdentity::observe_deep(&quarantine)
    } else {
        PathIdentity::observe(&quarantine)
    };
    let observed = match observed {
        Ok(observed) if expected.same_version(&observed) => observed,
        Ok(_) => {
            return Err(restore_cleanup_quarantine(
                path,
                &quarantine,
                "source changed while it was quarantined for cleanup",
            ));
        }
        Err(error) => {
            return Err(restore_cleanup_quarantine(
                path,
                &quarantine,
                &format!("source cleanup proof failed: {error}"),
            ));
        }
    };
    debug_assert!(observed.exists);

    if symlink_policy != crate::filesystem_policy::SymlinkPolicy::Skip {
        cleanup_path(&quarantine)?;
        return fs_util::sync_parent_namespace(&quarantine);
    }
    match cleanup_transferred_tree(&quarantine) {
        Ok(true) => fs_util::sync_parent_namespace(&quarantine),
        Ok(false) => crate::fs_at::rename_sibling(&quarantine, path, false)
            .and_then(|()| fs_util::sync_parent_namespace(path)),
        Err(error) => Err(restore_cleanup_quarantine(
            path,
            &quarantine,
            &format!("selective source cleanup failed: {error}"),
        )),
    }
}

fn source_cleanup_path(path: &Path) -> PathBuf {
    hidden_sibling(path, "cmdr-source-cleanup")
}

fn restore_cleanup_quarantine(path: &Path, quarantine: &Path, reason: &str) -> std::io::Error {
    match crate::fs_at::rename_sibling(quarantine, path, false) {
        Ok(()) => {
            let _ = fs_util::sync_parent_namespace(path);
            std::io::Error::other(reason.to_string())
        }
        Err(error) => std::io::Error::other(format!(
            "{reason}; source object preserved at {} because its pathname could not be restored: {error}",
            quarantine.display()
        )),
    }
}

/// Remove the entries transferred under `Skip`, while leaving omitted
/// symlinks and any now-nonempty ancestor directory in place.
fn cleanup_transferred_tree(path: &Path) -> std::io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Ok(false);
    }
    if !metadata.is_dir() {
        std::fs::remove_file(path)?;
        return Ok(true);
    }

    let mut children = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        cleanup_transferred_tree(&child.path())?;
    }
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(error) => Err(error),
    }
}

/// Undo our own placement at `staged` after a failed transfer of one entry.
///
/// For a COPY, `staged` is a disposable duplicate (the original still sits at
/// `source`), so it is simply removed. For the same-volume rename fast path,
/// `staged` IS the source's only copy (the source was already moved into it),
/// so it must be moved back to `source` rather than deleted, or the user's data
/// would be lost. If the restore cannot complete, the data is left in place
/// (never deleted) and its location is returned so it can be recovered.
pub(super) fn undo_placement(staged: &Path, source: &Path, was_renamed: bool) -> Option<String> {
    if !was_renamed {
        return cleanup_path(staged).err().map(|error| {
            format!(
                "partial copy cleanup failed at {}: {error}",
                staged.display()
            )
        });
    }
    let error = crate::fs_at::rename_sibling(staged, source, false).err()?;
    staged
        .symlink_metadata()
        .is_ok()
        .then(|| format!("data preserved at {}: {error}", staged.display()))
}

/// Replace `dest` with the freshly-staged `staged`: move the existing `dest`
/// to a backup, rename `staged` into place, then drop the backup. Restores
/// the original on failure, so an interrupted overwrite never loses data.
fn swap_into_place(
    staged: &Path,
    dest: &Path,
    expected_dest: &PathIdentity,
    journal: JournalStep<'_>,
) -> std::io::Result<Option<String>> {
    let current = if expected_dest.tree_fingerprint.is_some() {
        PathIdentity::observe_deep(dest)?
    } else {
        PathIdentity::observe(dest)?
    };
    if !expected_dest.same_binding(&current) {
        return Err(std::io::Error::other(
            "destination changed after conflict review",
        ));
    }
    if !current.exists {
        if journal.enabled {
            crate::operation_journal::prepare_placement(
                journal.operation_id,
                journal.key,
                staged,
                dest,
            )
            .map_err(std::io::Error::other)?;
        }
        crate::fs_at::rename_sibling(staged, dest, false)?;
        fs_util::sync_parent_namespace(dest)?;
        if journal.enabled {
            crate::operation_journal::mark_placement_placed(journal.operation_id, journal.key)
                .map_err(std::io::Error::other)?;
        }
        return Ok(None);
    }
    let backup = staging_path(dest);
    if journal.enabled {
        let prepared = crate::operation_journal::prepare_replacement(
            journal.operation_id,
            journal.key,
            staged,
            dest,
            &backup,
            expected_dest,
        )
        .map_err(std::io::Error::other)?;
        if prepared.phase == crate::operation_journal::ReplacementPhase::Prepared {
            quarantine_expected_path(dest, &prepared.path, &prepared.original)?;
            fs_util::sync_parent_namespace(dest)?;
            if let Err(error) = crate::operation_journal::mark_replacement_backed_up(
                journal.operation_id,
                journal.key,
            ) {
                return Err(restore_overwrite_quarantine(
                    dest,
                    &prepared.path,
                    &format!("overwrite backup proof failed: {error}"),
                ));
            }
        }
        let current = crate::operation_journal::operation(journal.operation_id)
            .map_err(std::io::Error::other)?
            .steps
            .into_iter()
            .find(|step| step.key == *journal.key)
            .and_then(|step| step.replacement)
            .ok_or_else(|| std::io::Error::other("overwrite proof disappeared"))?;
        if current.phase == crate::operation_journal::ReplacementPhase::OriginalBackedUp {
            crate::fs_at::rename_sibling(staged, dest, false)?;
            fs_util::sync_parent_namespace(dest)?;
            crate::operation_journal::mark_replacement_placed(journal.operation_id, journal.key)
                .map_err(std::io::Error::other)?;
        }
        return Ok(None);
    }

    quarantine_expected_path(dest, &backup, &current)?;
    fs_util::sync_parent_namespace(dest)?;
    match crate::fs_at::rename_sibling(staged, dest, false) {
        Ok(()) => {
            fs_util::sync_parent_namespace(dest)?;
            Ok(cleanup_moved_source(
                &backup,
                &current,
                crate::filesystem_policy::SymlinkPolicy::Preserve,
            )
            .err()
            .map(|error| {
                format!(
                    "destination is complete but proven old destination cleanup failed at {}: {error}",
                    backup.display()
                )
            }))
        }
        Err(e) => {
            // Put the original back. If even that fails, the original now
            // lives only at the hidden backup path; name it in the error so
            // it can be recovered rather than vanishing silently.
            if let Err(restore) = crate::fs_at::rename_sibling(&backup, dest, false) {
                return Err(std::io::Error::other(format!(
                    "{e}; restore failed: {restore}; original preserved at {}",
                    backup.display()
                )));
            }
            Err(e)
        }
    }
}

pub(super) fn quarantine_expected_path(
    path: &Path,
    quarantine: &Path,
    expected: &PathIdentity,
) -> std::io::Result<()> {
    crate::fs_at::rename_sibling(path, quarantine, false)?;
    let observed = if expected.tree_fingerprint.is_some() {
        PathIdentity::observe_deep(quarantine)
    } else {
        PathIdentity::observe(quarantine)
    };
    match observed {
        Ok(observed) if expected.same_version(&observed) => Ok(()),
        Ok(_) => Err(restore_overwrite_quarantine(
            path,
            quarantine,
            "overwrite destination changed during the commit fence",
        )),
        Err(error) => Err(restore_overwrite_quarantine(
            path,
            quarantine,
            &format!("overwrite backup could not be verified: {error}"),
        )),
    }
}

fn restore_overwrite_quarantine(path: &Path, quarantine: &Path, reason: &str) -> std::io::Error {
    match crate::fs_at::rename_sibling(quarantine, path, false) {
        Ok(()) => {
            let _ = fs_util::sync_parent_namespace(path);
            std::io::Error::other(reason.to_string())
        }
        Err(error) => std::io::Error::other(format!(
            "{reason}; object preserved at {} because {} could not be restored: {error}",
            quarantine.display(),
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn progress_with_failed_tail(placements: Vec<(PathBuf, PathBuf)>) -> TransferState {
        let progress = Arc::new(Mutex::new(TransferProgress::new(2, 2)));
        {
            let mut state = crate::lock_util::recover(&progress);
            state.files_done = 1;
            state.errors.push("second entry failed".to_string());
            state.placements = placements;
        }
        progress
    }

    fn entry_for(path: &Path) -> FileEntry {
        let lexical = std::fs::symlink_metadata(path).unwrap();
        let display = std::fs::metadata(path).unwrap_or_else(|_| lexical.clone());
        let mut entry = FileEntry::from_meta(path.to_path_buf(), &display).unwrap();
        entry.identity = crate::panel::ListingIdentity::Captured(PathIdentity::from_metadata(
            path.to_path_buf(),
            &lexical,
        ));
        entry
    }

    fn spec(kind: TransferKind, entries: Vec<FileEntry>, target: &Path) -> TransferSpec {
        TransferSpec {
            expectations: capture_expectations(&entries, target),
            method: CopyMethod::Buffered,
            durability: DurabilityProfile::Fast,
            ..TransferSpec::new(kind, entries, target.to_path_buf())
        }
    }

    fn run_with_backends(spec: TransferSpec, backends: backend::BackendPorts) -> TransferProgress {
        let total = total_bytes(&spec.entries);
        let progress: TransferState =
            Arc::new(Mutex::new(TransferProgress::new(total, spec.entries.len())));
        TransferExecutor::with_backends(
            crate::workload::global_handle(),
            spec,
            Arc::clone(&progress),
            || {},
            backends,
        )
        .submit();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            {
                let state = crate::lock_util::recover(&progress);
                if state.finished {
                    return state.clone();
                }
            }
            assert!(std::time::Instant::now() < deadline, "transfer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn run(spec: TransferSpec) -> TransferProgress {
        run_with_backends(spec, backend::BackendPorts::production())
    }

    fn hidden_entries(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect()
    }

    type StageFn =
        dyn Fn(&backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt> + Send + Sync;

    /// One staging behavior behind every backend port.
    struct ScriptedBackend(Box<StageFn>);

    impl backend::NativeCloneBackend for ScriptedBackend {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
        ) -> std::io::Result<backend::StageReceipt> {
            (self.0)(&request)
        }
    }

    impl backend::DeltaBackend for ScriptedBackend {
        fn stage(
            &self,
            _mode: crate::delta_copy::DeltaMode,
            request: backend::StageRequest<'_>,
            _checkpoints: &mut dyn backend::CheckpointSink,
        ) -> std::io::Result<backend::StageReceipt> {
            (self.0)(&request)
        }
    }

    impl backend::SparseBackend for ScriptedBackend {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
        ) -> std::io::Result<backend::StageReceipt> {
            (self.0)(&request)
        }
    }

    impl backend::BufferedBackend for ScriptedBackend {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
            _checkpoints: &mut dyn backend::CheckpointSink,
        ) -> std::io::Result<backend::StageReceipt> {
            (self.0)(&request)
        }
    }

    fn scripted(
        stage: impl Fn(&backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt>
        + Send
        + Sync
        + 'static,
    ) -> backend::BackendPorts {
        let backend = Arc::new(ScriptedBackend(Box::new(stage)));
        backend::BackendPorts::new(backend.clone(), backend.clone(), backend.clone(), backend)
    }

    /// Stage a plain file copy the way a real backend reports it.
    fn copy_stage(request: &backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt> {
        let bytes = std::fs::copy(request.source, request.staging)?;
        request.progress.complete_file(request.base_bytes, bytes);
        backend::receipt(
            request.staging,
            bytes,
            crate::transfer_tuning::FastPath::Buffered,
            request.durability,
        )
    }

    #[test]
    fn reconnected_mount_requeues_the_entry_and_restarts_its_progress() {
        use std::sync::atomic::AtomicUsize;

        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("share.txt", "reconnected payload");
        let mut request = spec(TransferKind::Copy, vec![entry_for(&file)], target.path());
        request.mount_wait_override = Some(Arc::new(|| Ok(())));
        let calls = Arc::new(AtomicUsize::new(0));
        let stage_calls = Arc::clone(&calls);

        let state = run_with_backends(
            request,
            scripted(move |request| {
                if stage_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    std::fs::write(request.staging, "partial")?;
                    request.progress.complete_file(request.base_bytes, 7);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "fixture share dropped",
                    ));
                }
                copy_stage(request)
            }),
        );

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(
            std::fs::read_to_string(target.path().join("share.txt")).unwrap(),
            "reconnected payload"
        );
        assert_eq!(state.files_done, 1);
        assert_eq!(state.copied_bytes, state.total_bytes);
        assert!(hidden_entries(target.path()).is_empty());
    }

    #[test]
    fn local_not_found_mid_copy_fails_once_instead_of_waiting_for_a_mount() {
        use std::sync::atomic::AtomicUsize;

        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("vanishing.txt", "deleted mid-copy");
        let calls = Arc::new(AtomicUsize::new(0));
        let stage_calls = Arc::clone(&calls);

        let state = run_with_backends(
            spec(TransferKind::Copy, vec![entry_for(&file)], target.path()),
            scripted(move |_| {
                stage_calls.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "fixture child vanished",
                ))
            }),
        );

        assert_eq!(calls.load(Ordering::SeqCst), 1, "no reconnect retries");
        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("copy failed") && error.contains("vanished")),
            "{:?}",
            state.errors
        );
        assert!(!target.path().join("vanishing.txt").exists());
    }

    /// Buffered stage that publishes a proven prefix checkpoint, then fails.
    struct CheckpointThenFail;

    impl backend::BufferedBackend for CheckpointThenFail {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
            checkpoints: &mut dyn backend::CheckpointSink,
        ) -> std::io::Result<backend::StageReceipt> {
            std::fs::write(request.staging, &std::fs::read(request.source)?[..4])?;
            checkpoints.publish(ResumeCheckpoint::observe(
                request.source,
                request.staging,
                4,
                CheckpointLayout::Prefix,
                prefix_digest(request.source, 4)?,
            )?)?;
            Err(std::io::Error::other("fixture write failed"))
        }
    }

    #[test]
    fn failed_journaled_copy_keeps_its_proven_partial_for_resume() {
        let journal = TempDir::new();
        let _journal =
            crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("resumable.bin", "abcdefgh");
        let mut request = spec(TransferKind::Copy, vec![entry_for(&file)], target.path());
        request.journal_enabled = true;
        let unused = Arc::new(ScriptedBackend(Box::new(|_| {
            unreachable!("only the buffered port is selected")
        })));

        let state = run_with_backends(
            request,
            backend::BackendPorts::new(
                unused.clone(),
                unused.clone(),
                unused,
                Arc::new(CheckpointThenFail),
            ),
        );

        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("fixture write failed")),
            "{:?}",
            state.errors
        );
        assert!(!target.path().join("resumable.bin").exists());
        assert_eq!(
            std::fs::read(target.path().join(".resumable.bin.cmdr-tmp.0")).unwrap(),
            b"abcd",
            "a proven partial must survive for resume"
        );
    }

    #[test]
    fn a_failed_entry_still_counts_toward_the_byte_total() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let first = source.file("a-fails.txt", "first entry");
        let second = source.file("b-copies.txt", "second");

        let state = run_with_backends(
            spec(
                TransferKind::Copy,
                vec![entry_for(&first), entry_for(&second)],
                target.path(),
            ),
            scripted(|request| {
                if request.source.ends_with("a-fails.txt") {
                    request.progress.complete_file(request.base_bytes, 3);
                    return Err(std::io::Error::other("fixture write failed"));
                }
                copy_stage(request)
            }),
        );

        assert_eq!(state.errors.len(), 1, "{:?}", state.errors);
        assert_eq!(state.files_done, 2);
        assert_eq!(state.total_bytes, 17);
        assert_eq!(state.copied_bytes, state.total_bytes);
    }

    #[test]
    fn many_small_entries_do_not_flood_the_speed_window() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let entries = (0..200)
            .map(|index| entry_for(&source.file(&format!("{index}.txt"), "x")))
            .collect();

        let state = run_with_backends(
            spec(TransferKind::Copy, entries, target.path()),
            scripted(copy_stage),
        );

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(state.files_done, 200);
        // Entries are sampled at the same rate-limited cadence as bytes, so
        // the speed window keeps covering wall time, not the last 120 files.
        assert!(
            state.speed_samples.len() < 60,
            "{} samples",
            state.speed_samples.len()
        );
    }

    #[test]
    fn failed_dir_stage_is_kept_and_its_hidden_path_is_reported() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let dir = source.dir("folder");
        source.file("folder/good.txt", "kept sibling");

        let state = run_with_backends(
            spec(TransferKind::Copy, vec![entry_for(&dir)], target.path()),
            scripted(|request| {
                std::fs::create_dir(request.staging)?;
                std::fs::write(request.staging.join("good.txt"), "kept sibling")?;
                Err(std::io::Error::other("Failed to copy: bad.txt"))
            }),
        );

        let staging = target.path().join(".folder.cmdr-tmp.0");
        assert_eq!(
            std::fs::read_to_string(staging.join("good.txt")).unwrap(),
            "kept sibling"
        );
        assert!(!target.path().join("folder").exists());
        let kept = format!("partial copy kept at {}", staging.display());
        assert!(
            state.errors.iter().any(|error| error.contains(&kept)),
            "{:?}",
            state.errors
        );
    }

    #[test]
    fn complete_dir_stage_rejected_at_commit_is_removed() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let dir = source.dir("folder");
        source.file("folder/a.txt", "a");
        let landing = target.path().join("folder");
        let mut request = spec(TransferKind::Copy, vec![entry_for(&dir)], target.path());
        let created = landing.clone();
        request.before_commit = Some(Arc::new(move |_, _| {
            std::fs::create_dir(&created).unwrap();
        }));

        let state = run(request);

        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("destination changed after conflict review")),
            "{:?}",
            state.errors
        );
        assert!(
            hidden_entries(target.path()).is_empty(),
            "a complete staged duplicate must not be left behind"
        );
        assert_eq!(std::fs::read_dir(&landing).unwrap().count(), 0);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "a");
    }

    #[test]
    fn hidden_sibling_names_stay_within_the_leaf_limit() {
        assert_eq!(
            hidden_sibling_name("a.txt", "cmdr-tmp", 0),
            ".a.txt.cmdr-tmp.0"
        );
        let long = "я".repeat(127);
        for (tag, index) in [("cmdr-tmp", 0), ("cmdr-source-cleanup", 12)] {
            let leaf = hidden_sibling_name(&long, tag, index);
            assert!(leaf.len() <= MAX_LEAF_BYTES, "{} bytes", leaf.len());
            assert!(leaf.starts_with(".я"), "{leaf}");
            assert!(leaf.ends_with(&format!("я.{tag}.{index}")), "{leaf}");
        }
    }

    #[test]
    fn hidden_sibling_skips_taken_names() {
        let root = TempDir::new();
        let dest = root.path().join("a.txt");
        root.file(".a.txt.cmdr-tmp.0", "taken");
        assert_eq!(staging_path(&dest), root.path().join(".a.txt.cmdr-tmp.1"));
        assert_eq!(
            source_cleanup_path(&dest),
            root.path().join(".a.txt.cmdr-source-cleanup.0")
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_of_a_name_near_the_leaf_limit_is_staged_and_placed() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let name = "я".repeat(125);
        let file = source.file(&name, "long name");

        let state = run(spec(
            TransferKind::Copy,
            vec![entry_for(&file)],
            target.path(),
        ));

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(
            std::fs::read_to_string(target.path().join(&name)).unwrap(),
            "long name"
        );
        assert!(hidden_entries(target.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn moved_source_cleanup_handles_a_name_near_the_leaf_limit() {
        let root = TempDir::new();
        let path = root.file(&"s".repeat(240), "moved away");
        let expected = PathIdentity::observe(&path).unwrap();

        cleanup_moved_source(
            &path,
            &expected,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        )
        .unwrap();

        assert!(!path.exists());
        assert!(hidden_entries(root.path()).is_empty());
    }

    #[test]
    fn rollback_refuses_a_placement_outside_its_container() {
        let root = TempDir::new();
        let container = root.dir("Gathered");
        let landing = root.file("elsewhere.txt", "moved");
        let source = root.path().join("a.txt");
        let progress = progress_with_failed_tail(vec![(source.clone(), landing.clone())]);

        let outcome = run_failure_rollback(Some(&container), &OperationId::new(), false, &progress);

        assert_eq!(outcome, FailureRollback::Incomplete);
        assert!(
            !source.exists(),
            "an out-of-container landing is never moved"
        );
        assert_eq!(std::fs::read_to_string(&landing).unwrap(), "moved");
        assert!(
            container.exists(),
            "the container is kept while data is out"
        );
        let state = crate::lock_util::recover(&progress);
        assert_eq!(state.placements, [(source, landing.clone())]);
        assert!(state.failures.iter().any(|failure| {
            failure.class == FailureClass::IntegrityUncertain
                && failure.path.as_deref() == Some(landing.as_path())
                && failure.message.contains("outside its operation container")
        }));
    }

    #[test]
    fn non_journal_swap_restores_the_original_when_placement_fails() {
        let root = TempDir::new();
        let dest = root.file("report.txt", "original");
        // Not `.0`: that is the name the swap picks for its own backup.
        let staged = root.path().join(".report.txt.cmdr-tmp.staged");
        let expected = PathIdentity::observe(&dest).unwrap();
        let operation_id = OperationId::new();
        let key = operation_id.step_key(0, &dest);

        let error = swap_into_place(
            &staged,
            &dest,
            &expected,
            JournalStep {
                operation_id: &operation_id,
                key: &key,
                enabled: false,
            },
        )
        .expect_err("a missing staged artifact cannot be placed");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
        assert!(!error.to_string().contains("preserved at"), "{error}");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "original");
        let leftovers: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, [std::ffi::OsString::from("report.txt")]);
    }

    #[test]
    fn undo_of_a_copy_removes_the_staged_duplicate() {
        let root = TempDir::new();
        let source = root.file("a.txt", "source");
        let staged = root.file(".a.txt.cmdr-tmp.0", "copy");

        assert_eq!(undo_placement(&staged, &source, false), None);
        assert!(!staged.exists());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "source");
    }

    #[test]
    fn undo_of_a_rename_moves_the_only_copy_back() {
        let root = TempDir::new();
        let source = root.path().join("a.txt");
        let staged = root.file(".a.txt.cmdr-tmp.0", "only copy");

        assert_eq!(undo_placement(&staged, &source, true), None);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "only copy");
        assert!(!staged.exists());
    }

    #[test]
    fn undo_of_a_rename_keeps_data_when_the_source_name_is_taken() {
        let root = TempDir::new();
        let source = root.file("a.txt", "recreated");
        let staged = root.file(".a.txt.cmdr-tmp.0", "only copy");

        let message = undo_placement(&staged, &source, true).expect("restore must fail");

        assert!(message.contains(&staged.display().to_string()), "{message}");
        assert!(
            message.contains("os error"),
            "the failed restore must keep its OS error: {message}"
        );
        assert_eq!(std::fs::read_to_string(&staged).unwrap(), "only copy");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "recreated");
    }
}
