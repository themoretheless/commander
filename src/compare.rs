//! Cross-pane folder comparison: how an entry relates to the same-named entry
//! in the other panel, and the pure set logic for turning a comparison into a
//! selection. UI-independent so it is unit-tested without a panel or GUI; the
//! `app` compare-mode tinting and the workspace selection commands call in here.

use crate::panel::FileEntry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

/// How an entry relates to the same-named entry in the other panel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompareStatus {
    /// Same name, size and mtime as the other panel's entry.
    Identical,
    /// Same name but a different size or mtime.
    Differs,
    /// Both entries are directories. Their recursive contents have not been
    /// compared, so the row must not claim byte-level identity.
    DirectoryPair,
    /// The same name identifies a file on one side and a directory on the other.
    TypeConflict,
    /// No entry of this name in the other panel.
    Unique,
}

/// Minimal same-name fingerprint used by compare mode. Entry type is part of
/// identity so a zero-byte file can never be mistaken for a directory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CompareFingerprint {
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub is_dir: bool,
}

/// Other-panel entries indexed by lowercase name, built once per frame.
pub type CompareMap = HashMap<String, CompareFingerprint>;

/// Index a panel's entries for comparison against the other panel.
pub fn build_compare_map(entries: &[FileEntry]) -> CompareMap {
    entries
        .iter()
        .map(|e| {
            (
                e.name_lower.clone(),
                CompareFingerprint {
                    size: e.size,
                    modified: e.modified,
                    is_dir: e.is_dir,
                },
            )
        })
        .collect()
}

/// Which entries to select when turning a folder comparison into a selection.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompareCriterion {
    /// Present in the other panel but newer here (by mtime).
    Newer,
    /// Present in the other panel but differing in size or mtime.
    Differing,
    /// Absent from the other panel.
    Unique,
}

/// Collect the paths of `entries` matching `criterion` against the other
/// panel's [`CompareMap`]. Pure, so it can feed the selection set directly.
pub fn select_by_compare<'a>(
    entries: impl Iterator<Item = &'a FileEntry>,
    other: &CompareMap,
    criterion: CompareCriterion,
) -> HashSet<PathBuf> {
    entries
        .filter(|e| match other.get(&e.name_lower) {
            None => criterion == CompareCriterion::Unique,
            Some(fingerprint) => match criterion {
                CompareCriterion::Unique => false,
                CompareCriterion::Differing => {
                    fingerprint.is_dir != e.is_dir
                        || (!e.is_dir
                            && (fingerprint.size != e.size || fingerprint.modified != e.modified))
                }
                CompareCriterion::Newer if fingerprint.is_dir == e.is_dir && !e.is_dir => {
                    match (e.modified, fingerprint.modified) {
                        (Some(a), Some(b)) => a > b,
                        _ => false,
                    }
                }
                CompareCriterion::Newer => false,
            },
        })
        .map(|e| e.path.clone())
        .collect()
}

/// Paths among `entries` whose lowercased name appears in `names`.
/// Pure set logic so "select files also present in the other panel" can be
/// unit-tested without a panel or filesystem. The complement of the
/// [`CompareCriterion::Unique`] set: name-matched regardless of size/mtime.
pub fn matching_name_paths<'a>(
    entries: impl Iterator<Item = &'a FileEntry>,
    names: &HashSet<String>,
) -> HashSet<PathBuf> {
    entries
        .filter(|e| names.contains(&e.name_lower))
        .map(|e| e.path.clone())
        .collect()
}

/// Classify `entry` against the other panel's [`CompareMap`].
pub fn classify_entry(entry: &FileEntry, other: &CompareMap) -> CompareStatus {
    match other.get(&entry.name_lower) {
        None => CompareStatus::Unique,
        Some(fingerprint) if fingerprint.is_dir != entry.is_dir => CompareStatus::TypeConflict,
        Some(_) if entry.is_dir => CompareStatus::DirectoryPair,
        Some(fingerprint) => {
            if fingerprint.size == entry.size && fingerprint.modified == entry.modified {
                CompareStatus::Identical
            } else {
                CompareStatus::Differs
            }
        }
    }
}

/// Human explanation for compare-mode row tinting. Identical rows intentionally
/// return `None` so quiet rows stay quiet.
pub fn compare_hint(entry: &FileEntry, other: &CompareMap) -> Option<String> {
    match other.get(&entry.name_lower) {
        None => Some("Only in this panel".to_string()),
        Some(fingerprint) if fingerprint.is_dir != entry.is_dir => {
            let here = if entry.is_dir { "folder" } else { "file" };
            let there = if fingerprint.is_dir { "folder" } else { "file" };
            Some(format!(
                "Same name, but this pane has a {here} and the other has a {there}"
            ))
        }
        Some(_) if entry.is_dir => {
            Some("Folder exists in both panes; recursive contents not compared".to_string())
        }
        Some(fingerprint) => {
            let size_differs = fingerprint.size != entry.size;
            let time_differs = fingerprint.modified != entry.modified;
            match (size_differs, time_differs) {
                (false, false) => None,
                (true, true) => Some("Same name, different size and modified time".to_string()),
                (true, false) => Some("Same name, different size".to_string()),
                (false, true) => Some("Same name, different modified time".to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn classify_entry_against_other_panel() {
        use std::time::{Duration, UNIX_EPOCH};
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        let t1 = UNIX_EPOCH + Duration::from_secs(2000);

        let (l, r) = (TempDir::new(), TempDir::new());
        let same = l.file("same.txt", "abc");
        let diff = l.file("diff.txt", "abc");
        let only = l.file("only.txt", "abc");
        let mk = |p: &std::path::Path, time| {
            let meta = std::fs::metadata(p).unwrap();
            let mut e = FileEntry::from_meta(p.to_path_buf(), &meta).unwrap();
            e.modified = Some(time);
            e
        };
        let _ = &r;

        // Other panel has "same" (identical) and "diff" (different size/mtime).
        let mut other = CompareMap::new();
        other.insert(
            "same.txt".to_string(),
            CompareFingerprint {
                size: 3,
                modified: Some(t0),
                is_dir: false,
            },
        );
        other.insert(
            "diff.txt".to_string(),
            CompareFingerprint {
                size: 999,
                modified: Some(t1),
                is_dir: false,
            },
        );

        assert_eq!(
            classify_entry(&mk(&same, t0), &other),
            CompareStatus::Identical
        );
        assert_eq!(
            classify_entry(&mk(&diff, t0), &other),
            CompareStatus::Differs
        );
        assert_eq!(
            classify_entry(&mk(&only, t0), &other),
            CompareStatus::Unique
        );
        assert_eq!(
            compare_hint(&mk(&same, t0), &other),
            None,
            "identical rows should stay visually quiet"
        );
        assert_eq!(
            compare_hint(&mk(&diff, t0), &other).as_deref(),
            Some("Same name, different size and modified time")
        );
        assert_eq!(
            compare_hint(&mk(&only, t0), &other).as_deref(),
            Some("Only in this panel")
        );
    }

    #[test]
    fn select_by_compare_picks_newer_differing_unique() {
        use std::time::{Duration, UNIX_EPOCH};
        let older = UNIX_EPOCH + Duration::from_secs(1000);
        let newer = UNIX_EPOCH + Duration::from_secs(2000);

        let tmp = TempDir::new();
        let mk = |name: &str, size: u64, time| {
            let p = tmp.file(name, "");
            let meta = std::fs::metadata(&p).unwrap();
            let mut e = FileEntry::from_meta(p, &meta).unwrap();
            e.size = size;
            e.modified = Some(time);
            e
        };
        let a = mk("a.txt", 10, newer); // exists in other, newer here
        let b = mk("b.txt", 99, older); // exists in other, differs (size)
        let c = mk("c.txt", 10, older); // unique here
        let entries = [a.clone(), b.clone(), c.clone()];

        let mut other = CompareMap::new();
        other.insert(
            "a.txt".to_string(),
            CompareFingerprint {
                size: 10,
                modified: Some(older),
                is_dir: false,
            },
        );
        other.insert(
            "b.txt".to_string(),
            CompareFingerprint {
                size: 10,
                modified: Some(older),
                is_dir: false,
            },
        );

        let newer_sel = select_by_compare(entries.iter(), &other, CompareCriterion::Newer);
        assert!(newer_sel.contains(&a.path) && newer_sel.len() == 1);

        let diff_sel = select_by_compare(entries.iter(), &other, CompareCriterion::Differing);
        assert!(diff_sel.contains(&b.path) && diff_sel.contains(&a.path) && diff_sel.len() == 2);

        let uniq_sel = select_by_compare(entries.iter(), &other, CompareCriterion::Unique);
        assert!(uniq_sel.contains(&c.path) && uniq_sel.len() == 1);
    }

    #[test]
    fn matching_name_paths_picks_name_matches_only() {
        let tmp = TempDir::new();
        let mk = |name: &str| {
            let p = tmp.file(name, "");
            let meta = std::fs::metadata(&p).unwrap();
            FileEntry::from_meta(p, &meta).unwrap()
        };
        let shared = mk("Shared.txt");
        let local = mk("local.txt");
        let entries = [shared.clone(), local.clone()];
        // Name set is lowercased, mirroring build_compare_map's key space.
        let names: HashSet<String> = ["shared.txt".to_string()].into_iter().collect();
        let picks = matching_name_paths(entries.iter(), &names);
        assert!(picks.contains(&shared.path), "name match selected");
        assert!(!picks.contains(&local.path), "unmatched name skipped");
        assert_eq!(picks.len(), 1);
    }

    #[test]
    fn build_compare_map_indexes_by_lowercase_name() {
        let tmp = TempDir::new();
        let f = tmp.file("Photo.JPG", "xy");
        let meta = std::fs::metadata(&f).unwrap();
        let e = FileEntry::from_meta(f, &meta).unwrap();
        let map = build_compare_map(std::slice::from_ref(&e));
        assert!(map.contains_key("photo.jpg"));
        assert_eq!(map["photo.jpg"].size, 2);
    }

    #[test]
    fn directories_are_unverified_and_type_mismatches_are_explicit() {
        let left = TempDir::new();
        let right = TempDir::new();
        let left_dir = left.dir("shared");
        let right_dir = right.dir("shared");
        let left_file = left.file("mixed", "");
        let right_mixed_dir = right.dir("mixed");
        let entry = |path: &std::path::Path| {
            let metadata = std::fs::metadata(path).unwrap();
            FileEntry::from_meta(path.to_path_buf(), &metadata).unwrap()
        };
        let other = build_compare_map(&[entry(&right_dir), entry(&right_mixed_dir)]);

        assert_eq!(
            classify_entry(&entry(&left_dir), &other),
            CompareStatus::DirectoryPair
        );
        assert_eq!(
            classify_entry(&entry(&left_file), &other),
            CompareStatus::TypeConflict
        );
        assert!(
            compare_hint(&entry(&left_dir), &other)
                .unwrap()
                .contains("not compared")
        );
        assert!(
            compare_hint(&entry(&left_file), &other)
                .unwrap()
                .contains("file")
        );
    }
}
