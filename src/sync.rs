//! Pure directory-synchronisation planning: given the two panels' entry lists
//! and a policy, compute a per-row plan of what to copy and in which direction.
//! No I/O and no UI, so the whole diff is unit-tested on fabricated entries.
//! Names are matched case-insensitively, matching the default macOS FS.

use crate::panel::FileEntry;
use std::collections::{HashMap, HashSet};

/// How to reconcile the two panels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
}

/// Compare two same-named entries by size and mtime.
fn compare(left: &FileEntry, right: &FileEntry) -> SyncStatus {
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
            RightOnly | Identical => Skip,
        },
        SyncPolicy::MirrorRightToLeft => match status {
            RightOnly | RightNewer | LeftNewer | Differing => ToLeft,
            LeftOnly | Identical => Skip,
        },
        SyncPolicy::TwoWay => match status {
            LeftOnly | LeftNewer => ToRight,
            RightOnly | RightNewer => ToLeft,
            Differing | Identical => Skip,
        },
    }
}

/// Build the synchronisation plan between the two panels' entry lists. Left
/// entries are walked first (stable order), then right-only entries.
pub fn sync_diff(left: &[FileEntry], right: &[FileEntry], policy: SyncPolicy) -> Vec<SyncAction> {
    let rmap: HashMap<&str, &FileEntry> =
        right.iter().map(|e| (e.name_lower.as_str(), e)).collect();

    let mut actions = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    for l in left {
        seen.insert(l.name_lower.as_str());
        let status = match rmap.get(l.name_lower.as_str()) {
            None => SyncStatus::LeftOnly,
            Some(r) => compare(l, r),
        };
        actions.push(SyncAction {
            name: l.name.clone(),
            status,
            direction: default_direction(status, policy),
        });
    }

    for r in right {
        if seen.contains(r.name_lower.as_str()) {
            continue;
        }
        actions.push(SyncAction {
            name: r.name.clone(),
            status: SyncStatus::RightOnly,
            direction: default_direction(SyncStatus::RightOnly, policy),
        });
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, size: u64, secs: Option<u64>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: std::path::PathBuf::from(format!("/x/{name}")),
            is_dir: false,
            size,
            extension: String::new(),
            modified: secs.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn status_of<'a>(actions: &'a [SyncAction], name: &str) -> &'a SyncAction {
        actions
            .iter()
            .find(|a| a.name == name)
            .expect("row present")
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
    fn diff_pairs_by_case_insensitive_name() {
        let left = [entry("Photo.JPG", 10, Some(100))];
        let right = [entry("photo.jpg", 10, Some(100))];
        let actions = sync_diff(&left, &right, SyncPolicy::TwoWay);
        assert_eq!(actions.len(), 1, "matched as the same file");
        assert_eq!(actions[0].status, SyncStatus::Identical);
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
