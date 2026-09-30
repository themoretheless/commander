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
    fn new(
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

    fn journal_started(&mut self) {
        self.journal_started = true;
    }

    fn disarm(&mut self) {
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
fn run_before_terminal_publish(_terminal: &TerminalContext) {}

pub fn spawn_transfer(
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    spawn_transfer_on(crate::workload::global_handle(), spec, progress, notify);
}

#[cfg(test)]
pub(crate) fn spawn_transfer_with_workload(
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
    pub(super) fn with_backends(
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
                    Ok(outcome) => base_bytes += outcome.bytes,
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
                        if is_disconnect_error(e) && work_item.mount_retries < MAX_MOUNT_RETRIES {
                            let (reconnect_guard, reconnect_label) = if mount_wait_override
                                .is_none()
                                && source_mount.check()
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
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::io(Some(entry.path.clone()), "copy failed", e),
                            );
                        }
                        crate::volume_profile::invalidate(target_profile.volume_id);
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
                // place and report the failure instead.
                let preserve_partial_dir =
                    !clean && !renamed && entry.is_dir && fs_util::path_is_taken(&copy_target);
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
                    s.record_sample();
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
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs_util::first_available(|i| parent.join(format!(".{}.cmdr-tmp.{}", name, i)))
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
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs_util::first_available(|index| parent.join(format!(".{name}.cmdr-source-cleanup.{index}")))
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
    if crate::fs_at::rename_sibling(staged, source, false).is_ok() {
        return None;
    }
    if staged.symlink_metadata().is_ok() {
        Some(format!("data preserved at {}", staged.display()))
    } else {
        None
    }
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
            if crate::fs_at::rename_sibling(&backup, dest, false).is_err() {
                return Err(std::io::Error::other(format!(
                    "{e}; original preserved at {}",
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
