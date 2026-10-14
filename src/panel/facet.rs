//! Quick-filter facets: kind, size and age chips `ANDed` with the text filter.
//!
//! The age rule lives in one place, [`age_deadline`], which both the
//! predicate and the listing's filter-cache expiry use.

use std::time::{Duration, SystemTime};

use super::FileEntry;
use crate::selection_summary::{Kind, kind_of};

const DAY_SECS: u64 = 24 * 60 * 60;

/// The instant an entry modified at `modified` turns `days` old, or `None`
/// if that lies beyond what `SystemTime` can represent.
fn age_deadline(modified: SystemTime, days: u64) -> Option<SystemTime> {
    modified.checked_add(Duration::from_secs(days.saturating_mul(DAY_SECS)))
}

/// A category facet for the quick-filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindFacet {
    Folders,
    Images,
    Docs,
    Archives,
    Code,
}

/// Active quick-filter facets, `ANDed` with the substring filter. Default is
/// "no facets" (everything passes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FacetSet {
    pub kind: Option<KindFacet>,
    /// Minimum file size in bytes (folders are not filtered by size).
    pub min_size: Option<u64>,
    /// Maximum age in days by mtime (entries with unknown mtime fail this).
    pub max_age_days: Option<u64>,
    /// Minimum age in days by mtime: the entry must be at least this old
    /// (entries with unknown mtime fail this). Drives the "Older" chip.
    pub min_age_days: Option<u64>,
}

impl FacetSet {
    /// Whether an entry modified at `modified` passes the age facets at
    /// `now`. A future mtime counts as fresh: it passes "max age" and fails
    /// "min age".
    pub(super) fn age_passes(&self, modified: SystemTime, now: SystemTime) -> bool {
        let within_max = self
            .max_age_days
            .is_none_or(|days| age_deadline(modified, days).is_none_or(|deadline| now <= deadline));
        let old_enough = self.min_age_days.is_none_or(|days| {
            age_deadline(modified, days).is_some_and(|deadline| now >= deadline)
        });
        within_max && old_enough
    }

    /// The next instant after `now` at which `age_passes` flips for an entry
    /// modified at `modified`, so a cached filter result can expire on time.
    fn age_transition(&self, modified: SystemTime, now: SystemTime) -> Option<SystemTime> {
        let max = self
            .max_age_days
            .and_then(|days| age_deadline(modified, days)?.checked_add(Duration::from_nanos(1)));
        let min = self
            .min_age_days
            .and_then(|days| age_deadline(modified, days));
        [max, min]
            .into_iter()
            .flatten()
            .filter(|deadline| *deadline > now)
            .min()
    }

    pub const fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.min_size.is_none()
            && self.max_age_days.is_none()
            && self.min_age_days.is_none()
    }

    pub fn active_count(&self) -> usize {
        usize::from(self.kind.is_some())
            + usize::from(self.min_size.is_some())
            + usize::from(self.max_age_days.is_some())
            + usize::from(self.min_age_days.is_some())
    }
}

pub fn filter_is_active(search_query: &str, facets: &FacetSet) -> bool {
    !search_query.trim().is_empty() || !facets.is_empty()
}

/// The [`Kind`] a chip selects.
const fn chip_kind(kind: KindFacet) -> Kind {
    match kind {
        KindFacet::Folders => Kind::Folder,
        KindFacet::Images => Kind::Image,
        KindFacet::Docs => Kind::Document,
        KindFacet::Archives => Kind::Archive,
        KindFacet::Code => Kind::Code,
    }
}

/// Whether `entry` passes all active facets, relative to `now`. Pure.
pub fn facet_matches(entry: &FileEntry, facets: &FacetSet, now: SystemTime) -> bool {
    if let Some(kind) = facets.kind {
        // Classified by the same table saved queries use (`type:` predicates
        // built by `query::from_panel_filter`), so a saved filter matches
        // exactly what the chip shows. Folders are only ever `Folder`, so a
        // folder named `backup.zip` never passes the Archives chip.
        let ok = kind_of(entry) == chip_kind(kind);
        if !ok {
            return false;
        }
    }
    if let Some(min) = facets.min_size {
        // Folders are not filtered by size (their byte size is 0 here).
        if !entry.is_dir && entry.size < min {
            return false;
        }
    }
    // Unknown mtime fails any active age facet.
    let no_age_facet = facets.max_age_days.is_none() && facets.min_age_days.is_none();
    no_age_facet
        || entry
            .modified
            .is_some_and(|modified| facets.age_passes(modified, now))
}

/// The earliest instant after `now` at which any entry's age-facet verdict
/// changes. `None` when no age facet is active or nothing will change.
pub(super) fn next_age_transition(
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
        .filter_map(|modified| facets.age_transition(modified, now))
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn age_verdict_flips_exactly_at_the_reported_transition() {
        let modified = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let now = modified + Duration::from_secs(60);
        for facets in [
            FacetSet {
                max_age_days: Some(1),
                ..FacetSet::default()
            },
            FacetSet {
                min_age_days: Some(1),
                ..FacetSet::default()
            },
        ] {
            let at = facets.age_transition(modified, now).expect("a future flip");
            let before = at - Duration::from_nanos(1);
            assert_eq!(
                facets.age_passes(modified, now),
                facets.age_passes(modified, before)
            );
            assert_ne!(
                facets.age_passes(modified, before),
                facets.age_passes(modified, at)
            );
        }
    }

    fn entry(name: &str, is_dir: bool) -> FileEntry {
        let extension = if is_dir {
            String::new()
        } else {
            std::path::Path::new(name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: std::path::PathBuf::from(format!("/t/{name}")),
            identity: super::super::ListingIdentity::Unavailable,
            is_dir,
            size: 1,
            extension,
            modified: None,
            modified_str: String::new(),
            size_str: String::new(),
        }
    }

    fn chip(kind: KindFacet) -> FacetSet {
        FacetSet {
            kind: Some(kind),
            ..FacetSet::default()
        }
    }

    #[test]
    fn kind_chips_agree_with_saved_query_kinds() {
        let now = UNIX_EPOCH;
        for (name, kind, expected) in [
            ("sheet.xlsx", KindFacet::Docs, true),
            ("data.csv", KindFacet::Docs, true),
            ("deck.pptx", KindFacet::Docs, true),
            ("page.html", KindFacet::Code, true),
            ("disk.dmg", KindFacet::Archives, true),
            ("clip.mp4", KindFacet::Images, false),
            ("pic.png", KindFacet::Images, true),
        ] {
            let e = entry(name, false);
            assert_eq!(facet_matches(&e, &chip(kind), now), expected, "{name}");
            let saved = crate::query::from_panel_filter("", &chip(kind));
            assert_eq!(saved.matches(&e, now), expected, "{name} (saved)");
        }
    }

    #[test]
    fn folders_named_like_files_fail_file_kind_chips() {
        let now = UNIX_EPOCH;
        for (name, kind) in [
            ("backup.zip", KindFacet::Archives),
            ("photos.jpg", KindFacet::Images),
            ("three.js", KindFacet::Code),
            ("notes.md", KindFacet::Docs),
        ] {
            let mut e = entry(name, true);
            // Even with an extension recorded, a folder is a folder.
            e.extension = name.rsplit('.').next().unwrap().to_string();
            assert!(!facet_matches(&e, &chip(kind), now), "{name}");
            assert!(facet_matches(&e, &chip(KindFacet::Folders), now));
        }
    }

    #[test]
    fn future_mtime_is_fresh() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let future = now + Duration::from_secs(3600);
        let max = FacetSet {
            max_age_days: Some(1),
            ..FacetSet::default()
        };
        let min = FacetSet {
            min_age_days: Some(1),
            ..FacetSet::default()
        };
        assert!(max.age_passes(future, now));
        assert!(!min.age_passes(future, now));
    }
}
