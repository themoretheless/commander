//! Pure directory-synchronisation planning: given the two panels' entry lists
//! and a policy, compute a per-row plan of what to copy and in which direction.
//! No I/O and no UI, so the whole diff is unit-tested on fabricated entries.
//! Names are matched case-insensitively, matching the default macOS FS.

use crate::panel::FileEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How to reconcile the two panels.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SyncPolicy {
    /// Make the right side match the left: copy left's new/changed files over,
    /// leave right-only files alone (copy-only, never deletes).
    MirrorLeftToRight,
    /// Make the left side match the right.
    MirrorRightToLeft,
    /// Each side receives the other's newer or unique files; genuine
    /// same-time-different-size conflicts are left for the user to resolve.
    TwoWay,
}

/// How an entry on one side relates to the same-named entry on the other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncStatus {
    /// Present only on the left.
    LeftOnly,
    /// Present only on the right.
    RightOnly,
    /// Present both sides; the left copy is newer (by mtime).
    LeftNewer,
    /// Present both sides; the right copy is newer (by mtime).
    RightNewer,
    /// Present both sides, different size but equal/unknown mtime.
    Differing,
    /// Present both sides with identical size and mtime.
    Identical,
    /// Same-named directories exist on both sides, but this shallow plan has
    /// not compared their recursive contents.
    DirectoryPair,
    /// The same name is a file on one side and a directory on the other.
    TypeConflict,
    /// Multiple entries fold to the same case-insensitive name, so choosing a
    /// source by display name would be ambiguous on a case-sensitive volume.
    CaseConflict,
}

/// Which way a row's file should flow when the plan is applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncDirection {
    ToLeft,
    ToRight,
    Skip,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyncAction {
    /// Display name (original case from whichever side holds the file).
    pub name: String,
    pub status: SyncStatus,
    /// The default direction for this status under the policy; the UI may flip
    /// it per row before applying.
    pub direction: SyncDirection,
    left_path: Option<PathBuf>,
    right_path: Option<PathBuf>,
}

impl SyncAction {
    /// Whether this row has a stable source for `direction`.
    pub fn allows(&self, direction: SyncDirection) -> bool {
        if matches!(
            self.status,
            SyncStatus::DirectoryPair | SyncStatus::TypeConflict
        ) {
            return direction == SyncDirection::Skip;
        }
        match direction {
            SyncDirection::ToLeft => self.right_path.is_some(),
            SyncDirection::ToRight => self.left_path.is_some(),
            SyncDirection::Skip => true,
        }
    }

    /// Stable source path for the row's currently selected direction.
    pub fn source_path(&self) -> Option<&Path> {
        match self.direction {
            SyncDirection::ToLeft => self.right_path.as_deref(),
            SyncDirection::ToRight => self.left_path.as_deref(),
            SyncDirection::Skip => None,
        }
    }
}

#[cfg(test)]
pub(crate) fn test_action(name: &str, status: SyncStatus, direction: SyncDirection) -> SyncAction {
    SyncAction {
        name: name.to_string(),
        status,
        direction,
        left_path: Some(PathBuf::from(format!("/left/{name}"))),
        right_path: Some(PathBuf::from(format!("/right/{name}"))),
    }
}

/// Compare two same-named entries by size and mtime.
pub(crate) fn compare(left: &FileEntry, right: &FileEntry) -> SyncStatus {
    if left.is_dir != right.is_dir {
        return SyncStatus::TypeConflict;
    }
    if left.is_dir {
        return SyncStatus::DirectoryPair;
    }
    if left.size == right.size && left.modified == right.modified {
        return SyncStatus::Identical;
    }
    match (left.modified, right.modified) {
        (Some(a), Some(b)) if a > b => SyncStatus::LeftNewer,
        (Some(a), Some(b)) if b > a => SyncStatus::RightNewer,
        // Equal or unknown mtime but the sizes differ: a real conflict.
        _ => SyncStatus::Differing,
    }
}

/// The default direction for `status` under `policy`. Mirrors force the target
/// side to match the source (even overwriting a newer target); two-way moves
/// each newer/unique file toward the side that lacks it and skips ambiguous
/// same-time conflicts.
fn default_direction(status: SyncStatus, policy: SyncPolicy) -> SyncDirection {
    use SyncDirection::{Skip, ToLeft, ToRight};
    use SyncStatus::*;
    match policy {
        SyncPolicy::MirrorLeftToRight => match status {
            LeftOnly | LeftNewer | RightNewer | Differing => ToRight,
            RightOnly | Identical | DirectoryPair | TypeConflict | CaseConflict => Skip,
        },
        SyncPolicy::MirrorRightToLeft => match status {
            RightOnly | RightNewer | LeftNewer | Differing => ToLeft,
            LeftOnly | Identical | DirectoryPair | TypeConflict | CaseConflict => Skip,
        },
        SyncPolicy::TwoWay => match status {
            LeftOnly | LeftNewer => ToRight,
            RightOnly | RightNewer => ToLeft,
            Differing | Identical | DirectoryPair | TypeConflict | CaseConflict => Skip,
        },
    }
}

/// Build the synchronisation plan between the two panels' entry lists. Left
/// entries are walked first (stable order), then right-only entries.
pub fn sync_diff(left: &[FileEntry], right: &[FileEntry], policy: SyncPolicy) -> Vec<SyncAction> {
    let mut left_counts: HashMap<&str, usize> = HashMap::new();
    let mut right_counts: HashMap<&str, usize> = HashMap::new();
    for entry in left {
        *left_counts.entry(entry.name_lower.as_str()).or_default() += 1;
    }
    for entry in right {
        *right_counts.entry(entry.name_lower.as_str()).or_default() += 1;
    }

    let mut actions = Vec::new();
    let mut used_right = vec![false; right.len()];

    for l in left {
        let exact = right
            .iter()
            .enumerate()
            .find(|(i, r)| !used_right[*i] && r.name == l.name)
            .map(|(i, _)| i);
        let unique_fold = left_counts.get(l.name_lower.as_str()) == Some(&1)
            && right_counts.get(l.name_lower.as_str()) == Some(&1);
        let matched = exact.or_else(|| {
            if !unique_fold {
                return None;
            }
            right
                .iter()
                .enumerate()
                .find(|(i, r)| !used_right[*i] && r.name_lower == l.name_lower)
                .map(|(i, _)| i)
        });

        let ambiguous = left_counts.get(l.name_lower.as_str()).copied().unwrap_or(0) > 1
            || right_counts
                .get(l.name_lower.as_str())
                .copied()
                .unwrap_or(0)
                > 1;
        let (status, right_path) = match matched {
            Some(i) => {
                used_right[i] = true;
                (compare(l, &right[i]), Some(right[i].path.clone()))
            }
            None if ambiguous => (SyncStatus::CaseConflict, None),
            None => (SyncStatus::LeftOnly, None),
        };
        actions.push(SyncAction {
            name: l.name.clone(),
            status,
            direction: default_direction(status, policy),
            left_path: Some(l.path.clone()),
            right_path,
        });
    }

    for (i, r) in right.iter().enumerate() {
        if used_right[i] {
            continue;
        }
        let ambiguous = left_counts.get(r.name_lower.as_str()).copied().unwrap_or(0) > 1
            || right_counts
                .get(r.name_lower.as_str())
                .copied()
                .unwrap_or(0)
                > 1;
        let status = if ambiguous {
            SyncStatus::CaseConflict
        } else {
            SyncStatus::RightOnly
        };
        actions.push(SyncAction {
            name: r.name.clone(),
            status,
            direction: default_direction(status, policy),
            left_path: None,
            right_path: Some(r.path.clone()),
        });
    }

    actions
}

/// How the active panel's entries relate to the other panel, as index sets
/// into the active list. Drives "select only-here / differing / identical".
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct PaneRelation {
    /// Present here but not in the other panel (no same-named entry there).
    pub only_here: Vec<usize>,
    /// Present in both with identical size and mtime.
    pub identical: Vec<usize>,
    /// Present in both but differing in size or mtime.
    pub differing: Vec<usize>,
    /// Same-named directories whose recursive contents are not part of this
    /// shallow relation.
    pub directories: Vec<usize>,
}

/// Classify each `active` entry against `other` (matched case-insensitively by
/// name), reusing [`compare`] for the size/mtime test so the relation stays
/// consistent with the sync diff. Pure; indices point into `active`.
pub fn pane_relation(active: &[FileEntry], other: &[FileEntry]) -> PaneRelation {
    let omap: HashMap<&str, &FileEntry> =
        other.iter().map(|e| (e.name_lower.as_str(), e)).collect();
    let mut rel = PaneRelation::default();
    for (i, a) in active.iter().enumerate() {
        match omap.get(a.name_lower.as_str()) {
            None => rel.only_here.push(i),
            Some(o) => match compare(a, o) {
                SyncStatus::Identical => rel.identical.push(i),
                SyncStatus::DirectoryPair => rel.directories.push(i),
                _ => rel.differing.push(i),
            },
        }
    }
    rel
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, size: u64, secs: Option<u64>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: std::path::PathBuf::from(format!("/x/{name}")),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size,
            extension: String::new(),
            modified: secs.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn directory(name: &str, secs: Option<u64>) -> FileEntry {
        let mut entry = entry(name, 0, secs);
        entry.is_dir = true;
        entry
    }

    fn status_of<'a>(actions: &'a [SyncAction], name: &str) -> &'a SyncAction {
        actions
            .iter()
            .find(|a| a.name == name)
            .expect("row present")
    }

    #[test]
    fn pane_relation_classifies_only_here_identical_and_differing() {
        // Active: a (only here), b (identical to other), c (differs), D (matches
        // other's "d" case-insensitively, identical).
        let active = vec![
            entry("a.txt", 1, Some(10)),
            entry("b.txt", 2, Some(20)),
            entry("c.txt", 3, Some(30)),
            entry("D.txt", 4, Some(40)),
        ];
        let other = vec![
            entry("b.txt", 2, Some(20)),  // identical to active b
            entry("c.txt", 99, Some(30)), // same name, different size -> differing
            entry("d.txt", 4, Some(40)),  // identical to active D (case-insensitive)
            entry("z.txt", 5, Some(50)),  // only in other (ignored)
        ];
        let rel = pane_relation(&active, &other);
        assert_eq!(rel.only_here, vec![0]); // a.txt
        assert_eq!(rel.identical, vec![1, 3]); // b.txt, D.txt
        assert_eq!(rel.differing, vec![2]); // c.txt
    }

    #[test]
    fn pane_relation_empty_other_is_all_only_here() {
        let active = vec![entry("a", 1, Some(1)), entry("b", 2, Some(2))];
        let rel = pane_relation(&active, &[]);
        assert_eq!(rel.only_here, vec![0, 1]);
        assert!(rel.identical.is_empty() && rel.differing.is_empty());
    }

    #[test]
    fn compare_classifies_size_and_mtime() {
        assert_eq!(
            compare(&entry("a", 10, Some(100)), &entry("a", 10, Some(100))),
            SyncStatus::Identical
        );
        assert_eq!(
            compare(&entry("a", 10, Some(200)), &entry("a", 10, Some(100))),
            SyncStatus::LeftNewer
        );
        assert_eq!(
            compare(&entry("a", 10, Some(100)), &entry("a", 10, Some(200))),
            SyncStatus::RightNewer
        );
        // Same mtime, different size -> a genuine conflict.
        assert_eq!(
            compare(&entry("a", 10, Some(100)), &entry("a", 20, Some(100))),
            SyncStatus::Differing
        );
    }

    #[test]
    fn compare_never_claims_directory_identity_or_auto_resolves_type_conflicts() {
        assert_eq!(
            compare(
                &directory("shared", Some(100)),
                &directory("shared", Some(100))
            ),
            SyncStatus::DirectoryPair
        );
        assert_eq!(
            compare(
                &entry("mixed", 0, Some(100)),
                &directory("mixed", Some(100))
            ),
            SyncStatus::TypeConflict
        );

        let actions = sync_diff(
            &[entry("mixed", 0, Some(100))],
            &[directory("mixed", Some(100))],
            SyncPolicy::MirrorLeftToRight,
        );
        assert_eq!(actions[0].direction, SyncDirection::Skip);
        assert!(!actions[0].allows(SyncDirection::ToRight));
    }

    #[test]
    fn pane_relation_keeps_unverified_directories_out_of_identical_selection() {
        let relation = pane_relation(
            &[directory("shared", Some(100))],
            &[directory("shared", Some(100))],
        );
        assert!(relation.identical.is_empty());
        assert!(relation.differing.is_empty());
        assert_eq!(relation.directories, vec![0]);
    }

    #[test]
    fn diff_pairs_by_case_insensitive_name() {
        let left = [entry("Photo.JPG", 10, Some(100))];
        let right = [entry("photo.jpg", 10, Some(100))];
        let actions = sync_diff(&left, &right, SyncPolicy::TwoWay);
        assert_eq!(actions.len(), 1, "matched as the same file");
        assert_eq!(actions[0].status, SyncStatus::Identical);
    }

    #[test]
    fn case_collisions_keep_distinct_sources_and_default_to_skip() {
        let left = [
            entry("Photo.JPG", 10, Some(100)),
            entry("photo.jpg", 20, Some(200)),
        ];
        let actions = sync_diff(&left, &[], SyncPolicy::MirrorLeftToRight);

        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|a| a.status == SyncStatus::CaseConflict));
        assert!(actions.iter().all(|a| a.direction == SyncDirection::Skip));
        let paths: HashSet<_> = actions
            .iter()
            .map(|a| {
                let mut action = a.clone();
                action.direction = SyncDirection::ToRight;
                action.source_path().unwrap().to_path_buf()
            })
            .collect();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn two_way_routes_newer_and_unique_each_way() {
        let left = [
            entry("only_left.txt", 1, Some(100)),
            entry("newer_left.txt", 1, Some(300)),
            entry("conflict.txt", 1, Some(100)),
            entry("same.txt", 5, Some(100)),
        ];
        let right = [
            entry("only_right.txt", 1, Some(100)),
            entry("newer_left.txt", 1, Some(100)), // left is newer
            entry("conflict.txt", 9, Some(100)),   // same mtime, diff size
            entry("same.txt", 5, Some(100)),
        ];
        let a = sync_diff(&left, &right, SyncPolicy::TwoWay);
        assert_eq!(
            status_of(&a, "only_left.txt").direction,
            SyncDirection::ToRight
        );
        assert_eq!(
            status_of(&a, "only_right.txt").direction,
            SyncDirection::ToLeft
        );
        assert_eq!(
            status_of(&a, "newer_left.txt").direction,
            SyncDirection::ToRight
        );
        assert_eq!(status_of(&a, "conflict.txt").status, SyncStatus::Differing);
        assert_eq!(status_of(&a, "conflict.txt").direction, SyncDirection::Skip);
        assert_eq!(status_of(&a, "same.txt").direction, SyncDirection::Skip);
    }

    #[test]
    fn mirror_left_to_right_overwrites_and_ignores_right_only() {
        let left = [
            entry("a.txt", 1, Some(100)), // left only
            entry("b.txt", 1, Some(100)), // right is newer, still overwrite
        ];
        let right = [
            entry("b.txt", 1, Some(500)), // newer on the right
            entry("c.txt", 1, Some(100)), // right only
        ];
        let a = sync_diff(&left, &right, SyncPolicy::MirrorLeftToRight);
        assert_eq!(status_of(&a, "a.txt").direction, SyncDirection::ToRight);
        // Mirror forces right to match left even though right is newer.
        assert_eq!(status_of(&a, "b.txt").status, SyncStatus::RightNewer);
        assert_eq!(status_of(&a, "b.txt").direction, SyncDirection::ToRight);
        // Right-only files are left untouched (copy-only mirror).
        assert_eq!(status_of(&a, "c.txt").direction, SyncDirection::Skip);
    }
}
