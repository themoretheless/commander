//! The search worker loop: candidate evaluation, batching and progress.

use super::*;

pub(super) enum ContentVerdict {
    Matched,
    NotMatched,
    Skipped,
}

pub(super) fn run_worker(
    request: ProviderRequest,
    query: CompiledQuery,
    cap: usize,
    sender: mpsc::Sender<SearchEvent>,
    notify: Notify,
    mut provider: Box<dyn SearchProvider>,
) {
    let generation = request.generation;
    let started = Instant::now();
    let now = SystemTime::now();
    let mut batch = Vec::with_capacity(BATCH_SIZE);
    let mut scanned = 0usize;
    let mut matched = 0usize;
    let mut content_skipped = 0usize;
    let mut truncated = false;
    let mut cancelled = false;
    let mut channel_open = true;
    let mut last_progress = Instant::now();
    let root = provider.root().to_path_buf();

    provider.visit(&request, &mut |record| {
        if request.cancelled() {
            cancelled = true;
            return false;
        }
        scanned += 1;
        let hit = record.and_then(|candidate| {
            evaluate_candidate(&query, candidate, &root, now, &mut content_skipped)
        });
        if let Some(hit) = hit {
            // Only a match beyond the cap proves the results are truncated; a
            // walk that ends with exactly `cap` matches is complete.
            if matched >= cap {
                truncated = true;
                return false;
            }
            batch.push(hit);
            matched += 1;
            if batch.len() >= BATCH_SIZE {
                sort_new_hits(&mut batch);
                let hits = std::mem::take(&mut batch);
                if sender
                    .send(SearchEvent::Batch {
                        generation,
                        hits,
                        scanned,
                    })
                    .is_err()
                {
                    channel_open = false;
                    return false;
                }
                notify();
            }
        }
        channel_open = maybe_send_progress(
            generation,
            scanned,
            matched,
            &mut last_progress,
            &sender,
            &notify,
        );
        channel_open
    });

    if !channel_open {
        return;
    }
    if request.cancelled() {
        cancelled = true;
    }
    content_skipped = content_skipped.saturating_add(request.unsearched.get());
    // Archives left unscanned mean the results are incomplete, like a hit cap.
    truncated |= request.archives_capped.get();

    if !batch.is_empty() && !cancelled {
        sort_new_hits(&mut batch);
        if sender
            .send(SearchEvent::Batch {
                generation,
                hits: batch,
                scanned,
            })
            .is_err()
        {
            return;
        }
        notify();
    }
    drop(request.archive_password);
    let summary = SearchSummary {
        scanned,
        matched,
        content_skipped,
        archive_error: request.archive_error.take(),
        elapsed: started.elapsed(),
        truncated,
        cancelled,
    };
    let _ = sender.send(SearchEvent::Complete {
        generation,
        summary,
    });
    notify();
}

/// The hit for one provider record, or `None` when its metadata or content
/// does not match. A candidate whose content could not be searched counts
/// in `content_skipped`.
fn evaluate_candidate(
    query: &CompiledQuery,
    candidate: SearchCandidate,
    root: &Path,
    now: SystemTime,
    content_skipped: &mut usize,
) -> Option<SearchHit> {
    let SearchCandidate {
        entry,
        is_symlink,
        is_executable,
        is_regular,
        unix_mode,
        owner_uid,
        owner_gid,
        device_id,
        inode,
        file_type_bits,
        is_empty,
        is_broken_symlink,
        identity,
        accessed,
        content,
        location,
    } = candidate;
    let mut entry = entry;
    // Archive pivots retain the container access time, but member predicates
    // must never borrow that observation as the member's own metadata.
    entry.accessed = if matches!(location, SearchLocation::Native(_)) {
        accessed
    } else {
        None
    };
    if matches!(location, SearchLocation::ArchiveMember { .. }) {
        entry.created = None;
        entry.changed = None;
    }
    let observed = ObservedMetadata {
        is_symlink,
        is_executable,
        is_regular,
        unix_mode,
        owner_uid,
        owner_gid,
        device_id,
        inode,
        file_type_bits,
        is_empty,
        is_broken_symlink,
    };
    if query.predicates.iter().any(|predicate| matches!(predicate, CompiledPredicate::Metadata(Predicate::DeviceId { .. }) if device_id.is_none()) || matches!(predicate, CompiledPredicate::Metadata(Predicate::Inode { .. }) if inode.is_none()))
        || query.predicates.iter().any(|predicate| matches!(predicate,
        CompiledPredicate::Metadata(Predicate::OwnerId { .. }) if owner_uid.is_none()) || matches!(predicate,
        CompiledPredicate::Metadata(Predicate::GroupId { .. }) if owner_gid.is_none()))
        || (query.requires_symlink_kind && is_symlink.is_none())
        || query.predicates.iter().any(|predicate| matches!(predicate,CompiledPredicate::Metadata(Predicate::BrokenSymlinks) if is_broken_symlink.is_none()))
        || (query.requires_executable_kind && is_executable.is_none())
        || (query.requires_regular_kind && is_regular.is_none())
        || (query.requires_unix_mode && unix_mode.is_none())
        || (query.requires_file_type_bits && file_type_bits.is_none())
        || (query.requires_links && entry.hard_link_count().is_none() && !entry.is_dir && is_regular!=Some(false) && is_symlink!=Some(true))
        || (query.requires_allocation && entry.allocated_bytes().is_none()
            && !entry.is_dir && is_regular!=Some(false) && is_symlink!=Some(true))
        || query.predicates.iter().any(|predicate| matches!(predicate,
            CompiledPredicate::Metadata(Predicate::Empty) if is_empty.is_none()))
        || query.predicates.iter().any(|predicate| matches!(predicate,
            CompiledPredicate::Metadata(Predicate::TypesAny(types)) if observed.matches_types(&entry, types).is_none()))
    {
        // Missing observed metadata cannot prove this filter. Keep partial coverage visible.
        *content_skipped = content_skipped.saturating_add(1);
        return None;
    }
    if query.predicates.iter().any(|p|matches!(p,CompiledPredicate::Timestamp{field,..} if !matches!(field,TimestampField::Modified) && field.observed(&entry).is_none())) {
        *content_skipped=content_skipped.saturating_add(1);return None;
    }
    let mut explanation = query.evaluate_metadata(&entry, root, now, observed)?;
    match query.evaluate_content(&entry, &content, &mut explanation) {
        ContentVerdict::NotMatched => None,
        ContentVerdict::Skipped => {
            *content_skipped += 1;
            None
        }
        ContentVerdict::Matched => Some(SearchHit {
            entry,
            identity,
            accessed,
            explanation,
            location,
        }),
    }
}

fn maybe_send_progress(
    generation: u64,
    scanned: usize,
    matched: usize,
    last_progress: &mut Instant,
    sender: &mpsc::Sender<SearchEvent>,
    notify: &Notify,
) -> bool {
    if last_progress.elapsed() < PROGRESS_INTERVAL {
        return true;
    }
    if sender
        .send(SearchEvent::Progress {
            generation,
            scanned,
            matched,
        })
        .is_err()
    {
        return false;
    }
    *last_progress = Instant::now();
    notify();
    true
}
