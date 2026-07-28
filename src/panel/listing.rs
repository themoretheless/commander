use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::{DirStatus, FacetSet, FileEntry, facet_matches};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ListingRevision(u64);

impl ListingRevision {
    pub(super) fn value(self) -> u64 {
        self.0
    }
}

struct FilterCache {
    revision: ListingRevision,
    query: String,
    facets: FacetSet,
    valid_until: Option<SystemTime>,
    indices: Arc<[usize]>,
}

impl FilterCache {
    fn stale() -> Self {
        Self {
            revision: ListingRevision(u64::MAX),
            query: String::new(),
            facets: FacetSet::default(),
            valid_until: None,
            indices: Arc::from([]),
        }
    }
}

pub(super) struct ListingState {
    entries: Vec<FileEntry>,
    status: DirStatus,
    revision: ListingRevision,
    filter_cache: RefCell<FilterCache>,
}

impl Default for ListingState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            status: DirStatus::Empty,
            revision: ListingRevision(0),
            filter_cache: RefCell::new(FilterCache::stale()),
        }
    }
}

impl ListingState {
    pub(super) fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    pub(super) fn status(&self) -> DirStatus {
        self.status
    }

    pub(super) fn revision(&self) -> ListingRevision {
        self.revision
    }

    pub(super) fn replace(&mut self, entries: Vec<FileEntry>, status: DirStatus) {
        self.entries = entries;
        self.status = status;
        self.bump_revision();
    }

    pub(super) fn mark_incomplete(&mut self, status: DirStatus) {
        debug_assert!(matches!(
            status,
            DirStatus::Denied | DirStatus::Gone | DirStatus::Partial
        ));
        if self.status != status {
            self.status = status;
            self.bump_revision();
        }
    }

    pub(super) fn resort(&mut self, sort: impl FnOnce(&mut [FileEntry])) {
        sort(&mut self.entries);
        self.bump_revision();
    }

    fn bump_revision(&mut self) {
        self.revision = ListingRevision(self.revision.0.wrapping_add(1));
    }

    pub(super) fn filtered_snapshot(&self, query: &str, facets: FacetSet) -> Arc<[usize]> {
        self.filtered_snapshot_at(query, facets, SystemTime::now())
    }

    pub(super) fn filtered_snapshot_at(
        &self,
        query: &str,
        facets: FacetSet,
        now: SystemTime,
    ) -> Arc<[usize]> {
        let query = query.trim();
        {
            let cache = self.filter_cache.borrow();
            let time_valid = cache.valid_until.is_none_or(|deadline| now < deadline);
            let indices_valid = cache
                .indices
                .last()
                .is_none_or(|index| *index < self.entries.len());
            if cache.revision == self.revision
                && cache.query == query
                && cache.facets == facets
                && time_valid
                && indices_valid
            {
                return Arc::clone(&cache.indices);
            }
        }

        let _latency =
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::FilterResponse);
        let no_facets = facets.is_empty();
        let indices: Arc<[usize]> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| crate::fuzzy::is_match(query, &entry.name))
            .filter(|(_, entry)| no_facets || facet_matches(entry, &facets, now))
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
            .into();
        let valid_until = next_age_transition(&self.entries, facets, now);
        *self.filter_cache.borrow_mut() = FilterCache {
            revision: self.revision,
            query: query.to_string(),
            facets,
            valid_until,
            indices: Arc::clone(&indices),
        };
        indices
    }

    #[cfg(test)]
    pub(super) fn replace_for_test(&mut self, entries: Vec<FileEntry>) {
        let status = if entries.is_empty() {
            DirStatus::Empty
        } else {
            DirStatus::Listed
        };
        self.replace(entries, status);
    }

    #[cfg(test)]
    pub(super) fn mutate_for_test(&mut self, mutate: impl FnOnce(&mut Vec<FileEntry>)) {
        mutate(&mut self.entries);
        self.bump_revision();
    }

    #[cfg(test)]
    pub(super) fn push_for_test(&mut self, entry: FileEntry) {
        self.mutate_for_test(|entries| entries.push(entry));
    }

    #[cfg(test)]
    pub(super) fn mutate_entries_without_revision_for_test(
        &mut self,
        mutate: impl FnOnce(&mut Vec<FileEntry>),
    ) {
        mutate(&mut self.entries);
    }
}

fn next_age_transition(
    entries: &[FileEntry],
    facets: FacetSet,
    now: SystemTime,
) -> Option<SystemTime> {
    if facets.max_age_days.is_none() && facets.min_age_days.is_none() {
        return None;
    }

    entries
        .iter()
        .filter_map(|entry| entry.modified)
        .filter_map(|modified| {
            let max_transition = facets.max_age_days.and_then(|days| {
                modified
                    .checked_add(Duration::from_secs(days.saturating_mul(24 * 60 * 60)))
                    .and_then(|deadline| deadline.checked_add(Duration::from_nanos(1)))
                    .filter(|deadline| *deadline > now)
            });
            let min_transition = facets.min_age_days.and_then(|days| {
                modified
                    .checked_add(Duration::from_secs(days.saturating_mul(24 * 60 * 60)))
                    .filter(|deadline| *deadline > now)
            });
            match (max_transition, min_transition) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
                (None, None) => None,
            }
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    fn entry(name: &str, modified: SystemTime) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from("/test").join(name),
            is_dir: false,
            size: 1,
            extension: String::new(),
            modified: Some(modified),
            modified_str: String::new(),
            size_str: String::new(),
        }
    }

    #[test]
    fn warm_filter_returns_the_same_shared_snapshot() {
        let mut listing = ListingState::default();
        listing.replace_for_test(vec![entry("alpha", UNIX_EPOCH)]);

        let first = listing.filtered_snapshot_at("al", FacetSet::default(), UNIX_EPOCH);
        let second = listing.filtered_snapshot_at("al", FacetSet::default(), UNIX_EPOCH);
        assert!(Arc::ptr_eq(&first, &second));

        listing.push_for_test(entry("alps", UNIX_EPOCH));
        let changed = listing.filtered_snapshot_at("al", FacetSet::default(), UNIX_EPOCH);
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(&*changed, &[0, 1]);
    }

    #[test]
    fn age_filter_invalidates_at_its_real_transition() {
        let modified = UNIX_EPOCH + Duration::from_secs(100);
        let mut listing = ListingState::default();
        listing.replace_for_test(vec![entry("report", modified)]);
        let facets = FacetSet {
            min_age_days: Some(1),
            ..FacetSet::default()
        };
        let before = modified + Duration::from_secs(24 * 60 * 60 - 1);
        let at_boundary = modified + Duration::from_secs(24 * 60 * 60);

        let fresh = listing.filtered_snapshot_at("", facets, before);
        assert!(fresh.is_empty());
        let old = listing.filtered_snapshot_at("", facets, at_boundary);
        assert_eq!(&*old, &[0]);
        assert!(!Arc::ptr_eq(&fresh, &old));
    }
}
