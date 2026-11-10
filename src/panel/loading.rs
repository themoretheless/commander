//! Directory loading: synchronous and background listings, watcher changes and size refresh.

use super::*;

impl PanelState {
    pub fn refresh(&mut self) {
        self.change_provenance
            .record(crate::change_provenance::ChangeProvenance::Commander);
        let focus = match &self.initial_focus_child {
            Some((path, name)) if path == &self.current_path => {
                PendingFocus::NamedChild(name.clone())
            }
            _ => {
                self.initial_focus_child = None;
                PendingFocus::None
            }
        };
        self.schedule_or_reload(focus);
    }

    pub(crate) fn queue_initial_focus_child(&mut self, name: String) {
        self.initial_focus_child = Some((self.current_path.clone(), name));
    }

    pub(super) fn can_async_list(&self) -> bool {
        self.workload.is_some() && self.watcher.notify().is_some()
    }

    pub(super) fn schedule_or_reload(&mut self, focus: PendingFocus) {
        if self.can_async_list() {
            self.schedule_listing(focus);
        } else {
            self.refresh_sync(focus);
        }
    }

    fn refresh_sync(&mut self, focus: PendingFocus) {
        let path = self.current_path.clone();
        self.sizes.bind(&path);
        // Subscribe before taking the snapshot. A callback racing with the
        // read stays queued with this exact binding and forces a later pass.
        self.watcher.ensure_binding(&path);
        let ticket = self.watcher.snapshot_ticket();
        if self.reload_entries() {
            self.commit_snapshot(ticket, true);
            self.apply_pending_focus(focus);
        } else {
            self.watcher.defer_snapshot(ticket.as_ref());
        }
    }

    fn schedule_listing(&mut self, focus: PendingFocus) {
        self.pending_view_config = None;
        self.view_apply_outcome = None;
        let path = self.current_path.clone();
        let show_hidden = self.view.show_hidden();
        let binding_changed = self.listing.binding() != path;
        self.sizes.bind(&path);
        self.watcher.ensure_binding(&path);
        let ticket = self.watcher.snapshot_ticket();
        if binding_changed {
            self.selection.bind(&path);
            self.drag.clear();
            self.listing.begin_loading(path.clone());
        }
        self.listing_job.request(path, show_hidden, ticket, focus);
        // Opportunistically admit/poll within this call so fast local disks
        // still settle before the next frame when the worker is free.
        let _ = self.poll_listing_results();
    }

    /// Drive in-flight listings and publish generation-checked results.
    /// Returns `true` when a listing was applied.
    pub fn poll_listing(&mut self) -> bool {
        self.poll_listing_results()
    }

    pub(super) fn poll_listing_results(&mut self) -> bool {
        let Some(workload) = self.workload.clone() else {
            return false;
        };
        let Some(notify) = self.watcher.notify() else {
            return false;
        };
        self.listing_job.drive(&workload, notify, Self::read_dir);
        let Some(ready) = self.listing_job.take_ready() else {
            return false;
        };
        let intended_config = self
            .pending_view_config
            .unwrap_or_else(|| self.view.config());
        if ready.binding.path != self.current_path
            || ready.binding.show_hidden != intended_config.show_hidden()
            || self
                .listing_job
                .desired_binding()
                .is_some_and(|desired| desired.generation > ready.binding.generation)
        {
            // Stale relative to panel intent; keep waiting for the current job.
            return false;
        }
        if let Some(candidate) = self.pending_view_config.take() {
            match ready.read {
                DirectoryRead::Complete(entries) => {
                    self.view.commit_config(candidate);
                    self.publish_complete_listing(entries, 0, candidate);
                    self.commit_snapshot(ready.ticket, ready.forced_sizes);
                    self.apply_pending_focus(ready.focus);
                    self.view_apply_outcome = Some(ViewApplyOutcome::Applied);
                    return true;
                }
                DirectoryRead::OnlyHidden(hidden) => {
                    self.view.commit_config(candidate);
                    self.publish_complete_listing(Vec::new(), hidden, candidate);
                    self.commit_snapshot(ready.ticket, ready.forced_sizes);
                    self.apply_pending_focus(ready.focus);
                    self.view_apply_outcome = Some(ViewApplyOutcome::Applied);
                    return true;
                }
                DirectoryRead::Incomplete(status) => {
                    self.watcher.defer_snapshot(ready.ticket.as_ref());
                    self.view_apply_outcome = Some(ViewApplyOutcome::ReadRejected(status));
                    return false;
                }
            }
        }
        let applied = self.apply_directory_read(ready.read);
        if applied {
            self.commit_snapshot(ready.ticket, ready.forced_sizes);
            self.apply_pending_focus(ready.focus);
            if let Some(recovered_gap) = ready.watcher_gap {
                self.change_provenance
                    .record(watcher_provenance(recovered_gap));
                crate::watcher_health::record_listing_reconciliation(recovered_gap);
            }
        } else if let Some(ticket) = ready.ticket.as_ref() {
            self.watcher.defer_snapshot(Some(ticket));
            let _ = ready.focus;
        } else {
            let _ = ready.focus;
        }
        applied
    }

    pub fn take_view_apply_outcome(&mut self) -> Option<ViewApplyOutcome> {
        self.view_apply_outcome.take()
    }

    /// A complete listing was published: acknowledge the watcher snapshot it
    /// answers (against the binding the rows now belong to) and re-plan
    /// folder sizes for it. `forced_sizes` lets expensive folders re-walk.
    pub(super) fn commit_snapshot(
        &mut self,
        ticket: Option<ReconciliationTicket>,
        forced_sizes: bool,
    ) {
        let listing_binding = self.listing.binding().to_path_buf();
        self.watcher.acknowledge_snapshot(ticket, &listing_binding);
        self.refresh_sizes(forced_sizes);
    }

    fn apply_pending_focus(&mut self, focus: PendingFocus) {
        match focus {
            PendingFocus::None => {}
            PendingFocus::Remembered {
                cursor_path,
                scroll_anchor,
            } => {
                self.set_scroll_anchor(scroll_anchor.min(self.filtered_count().saturating_sub(1)));
                // No remembered path means the cursor was on the `..` row;
                // only a path that vanished falls back to the scroll anchor.
                let cursor = match cursor_path {
                    None => 0,
                    Some(path) => self
                        .filtered_position(|entry| entry.path == path)
                        .map_or_else(
                            || {
                                self.scroll_anchor()
                                    .saturating_add(1)
                                    .min(self.filtered_count())
                            },
                            |index| index + 1,
                        ),
                };
                self.set_cursor(cursor);
                self.set_scroll_to_cursor(self.cursor() > 0);
            }
            PendingFocus::NamedChild(name) => {
                self.initial_focus_child = None;
                if let Some(idx) = self.named_child_position(&name) {
                    self.reveal_cursor(idx + 1);
                }
            }
        }
    }

    /// Re-read the directory, preserving selection and cursor position
    /// by path (entries may have been added, removed or re-sorted).
    pub(super) fn reload_entries(&mut self) -> bool {
        let read = Self::read_dir(&self.current_path, self.view.show_hidden());
        self.apply_directory_read(read)
    }

    pub(super) fn apply_directory_read(&mut self, read: DirectoryRead) -> bool {
        let binding = self.current_path.clone();
        let binding_changed = self.listing.binding() != binding;
        if self.pending_visit.as_ref() == Some(&binding) {
            self.pending_visit = None;
            if !matches!(read, DirectoryRead::Incomplete(DirStatus::Gone)) {
                record_visit(&binding);
            }
        }
        match read {
            DirectoryRead::Complete(entries) => {
                self.publish_complete_listing(entries, 0, self.view.config());
                true
            }
            DirectoryRead::OnlyHidden(hidden) => {
                self.publish_complete_listing(Vec::new(), hidden, self.view.config());
                true
            }
            DirectoryRead::Incomplete(status) => {
                if binding_changed {
                    self.selection.bind(&binding);
                    self.sizes.bind(&binding);
                    self.drag.clear();
                }
                let changed = self.listing.mark_incomplete(binding, status);
                debug_assert_eq!(changed, binding_changed);
                false
            }
        }
    }

    /// Publish a complete read. `hidden` counts entries skipped as hidden;
    /// it only matters when no row is left to show.
    pub(super) fn publish_complete_listing(
        &mut self,
        mut entries: Vec<FileEntry>,
        hidden: usize,
        config: ViewConfig,
    ) {
        let binding = self.current_path.clone();
        let binding_changed = self.listing.binding() != binding;
        let cursor_path = (!binding_changed).then(|| self.focused_path()).flatten();
        let old_cursor = if binding_changed { 0 } else { self.cursor() };

        if binding_changed {
            self.selection.bind(&binding);
            self.sizes.bind(&binding);
            self.drag.clear();
        }

        let status = match (entries.is_empty(), hidden) {
            (false, _) => DirStatus::Listed,
            (true, 0) => DirStatus::Empty,
            (true, hidden) => DirStatus::HiddenOnly(hidden),
        };
        sort_entries(&mut entries, config);
        self.selection.publish_complete(&binding);
        self.listing.replace(binding, entries, status);

        self.selection
            .retain_present(self.listing.entries().iter().map(|entry| &entry.path));

        if !binding_changed {
            self.restore_cursor_focus(cursor_path, old_cursor);
        }
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    /// Returns `true` when the directory listing was re-read.
    /// Deep events (below the watched dir) only recompute directory
    /// sizes, debounced so event floods during transfers don't thrash.
    pub fn poll_fs_changes(&mut self) -> bool {
        // Polled every frame: the filtered view can change with time alone
        // (age facets), so keep the cursor on its entry first.
        self.ensure_cursor_valid();
        let path = self.current_path.clone();
        let outcome = self.watcher.poll(&path);
        if outcome.sizes_dirty {
            self.sizes.mark_dirty();
        }
        if let Some(ticket) = outcome.ticket {
            let source = watcher_provenance(outcome.recovered_gap);
            if self.can_async_list() {
                let show_hidden = self.view.show_hidden();
                self.listing_job
                    .reconcile(path, show_hidden, ticket, outcome.recovered_gap);
                // The result usually lands on a later poll; whichever poll
                // applies it records the watcher provenance it carries.
                return self.poll_listing_results();
            }
            if self.reload_entries() {
                // Watcher-driven: expensive folders are not re-walked.
                self.commit_snapshot(Some(ticket), false);
                self.change_provenance.record(source);
                crate::watcher_health::record_listing_reconciliation(outcome.recovered_gap);
                return true;
            }
            self.watcher.defer_snapshot(Some(&ticket));
        }
        self.poll_sizes();
        false
    }

    /// Schedule background recomputation of subdirectory sizes and counts.
    /// `forced` marks user-driven refreshes: they may re-walk expensive
    /// dirs, background (watcher-noise) recomputes may not.
    /// Returns `true` when some dir was skipped because of the walk
    /// cooldown and the caller should retry later.
    fn refresh_sizes(&mut self, forced: bool) {
        if self.miller_context_only {
            return;
        }
        let filtered = self.filtered_indices();
        let path = self.current_path.clone();
        let scroll_anchor = self.scroll_anchor();
        let page_rows = self.page_rows();
        let notify = self.watcher.notify();
        self.sizes.refresh(
            SizeScanInput {
                path: &path,
                entries: self.listing.entries(),
                filtered,
                scroll_anchor,
                page_rows,
                notify,
                show_hidden: self.view.show_hidden(),
            },
            forced,
        );
    }

    fn poll_sizes(&mut self) {
        if self.miller_context_only {
            return;
        }
        let filtered = self.filtered_indices();
        let path = self.current_path.clone();
        let input = SizeScanInput {
            path: &path,
            entries: self.listing.entries(),
            filtered,
            scroll_anchor: self.scroll_anchor(),
            page_rows: self.page_rows(),
            notify: self.watcher.notify(),
            show_hidden: self.view.show_hidden(),
        };
        self.sizes.poll(input);
        self.resort_when_directory_sizes_settle();
    }
    pub(super) fn read_dir(path: &Path, show_hidden: bool) -> DirectoryRead {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => return DirectoryRead::Incomplete(status_for_read_error(&error)),
        };
        Self::read_dir_entries(entries, show_hidden)
    }

    /// Build rows from a directory iterator. A child that vanished between
    /// readdir and stat is simply not listed: it is gone, not unreadable,
    /// so it must not turn the whole read into a partial one.
    fn read_dir_entries(
        entries: impl Iterator<Item = std::io::Result<fs::DirEntry>>,
        show_hidden: bool,
    ) -> DirectoryRead {
        let vanished = |error: &std::io::Error| error.kind() == std::io::ErrorKind::NotFound;
        let mut result = Vec::new();
        let mut hidden = 0_usize;
        let mut complete = true;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let path = entry.path();
            if !show_hidden && path.file_name().is_some_and(is_hidden_name) {
                hidden += 1;
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    complete &= vanished(&error);
                    continue;
                }
            };
            let lexical = match entry.metadata() {
                Ok(metadata) => Ok(metadata),
                Err(error) if vanished(&error) => continue,
                Err(error) => Err(crate::ports::NativeFailure::from_io(&error)),
            };
            let followed = file_type.is_symlink().then(|| fs::metadata(&path));
            let link_info = followed
                .as_ref()
                .map(|result| Arc::new(LinkInfo::observe(&path, result)));
            let dangling = followed.as_ref().is_some_and(Result::is_err);
            let display_metadata = followed
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .or_else(|| lexical.as_ref().ok());
            let Some(display_metadata) = display_metadata else {
                complete = false;
                continue;
            };
            match FileEntry::from_meta(path.clone(), display_metadata) {
                Some(mut file_entry) => {
                    file_entry.symlink = link_info;
                    if dangling {
                        // The lstat size is the link text's length, not
                        // content: a broken link has no size to show.
                        file_entry.size = 0;
                        file_entry.size_str = "–".to_string();
                        // Nor a target mode: there is no target.
                        file_entry.permissions = None;
                    }
                    // Display kind/permissions may follow a link, but ownership
                    // and link counts belong to the lexical object itself.
                    file_entry.owner_uid = lexical.as_ref().ok().and_then(crate::query::native_uid);
                    file_entry.owner_gid = lexical.as_ref().ok().and_then(crate::query::native_gid);
                    file_entry.hard_links = lexical.as_ref().ok().and_then(hard_link_count);
                    file_entry.set_lexical_dates(lexical.as_ref().ok());
                    file_entry.identity = match lexical {
                        Ok(metadata) => ListingIdentity::Captured(
                            crate::path_identity::PathIdentity::from_metadata(path, &metadata),
                        ),
                        Err(failure) => ListingIdentity::CaptureFailed(failure),
                    };
                    result.push(file_entry);
                }
                None => complete = false,
            }
        }
        if !complete {
            DirectoryRead::Incomplete(DirStatus::Partial)
        } else if result.is_empty() && hidden > 0 {
            DirectoryRead::OnlyHidden(hidden)
        } else {
            DirectoryRead::Complete(result)
        }
    }
}
