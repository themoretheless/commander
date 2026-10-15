//! Cross-pane folder comparison: how an entry relates to the same-named entry
//! in the other panel, and the pure set logic for turning a comparison into a
//! selection. UI-independent so it is unit-tested without a panel or GUI; the
//! `app` compare-mode tinting and the workspace selection commands call in here.

use crate::panel::FileEntry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Filesystems keep mtimes at different precision (FAT/exFAT 2 s, SMB and
/// other network shares 1-2 s), so a faithful copy can differ from its source
/// by up to this much without either side having changed.
pub const MTIME_TOLERANCE: Duration = Duration::from_secs(2);

/// Whether both mtimes are known and equal within [`MTIME_TOLERANCE`]. An
/// unknown mtime never matches: it cannot vouch for an unchanged file.
pub fn mtimes_match(a: Option<SystemTime>, b: Option<SystemTime>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.duration_since(b).unwrap_or_else(|e| e.duration()) <= MTIME_TOLERANCE
        }
        _ => false,
    }
}

/// Whether `a` is known to be newer than `b` by more than [`MTIME_TOLERANCE`].
pub fn mtime_newer(a: Option<SystemTime>, b: Option<SystemTime>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.duration_since(b).is_ok_and(|d| d > MTIME_TOLERANCE),
        _ => false,
    }
}

/// Which attributes of two same-named files disagree. The one place compare
/// mode decides "changed", so the mtime tolerance applies to every caller.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FileDiff {
    size: bool,
    /// Mtimes differ beyond [`MTIME_TOLERANCE`] or are unknown on a side.
    time: bool,
}

impl FileDiff {
    fn of(fingerprint: &CompareFingerprint, entry: &FileEntry) -> Self {
        Self {
            size: fingerprint.size != entry.size,
            time: !mtimes_match(fingerprint.modified, entry.modified),
        }
    }

    const fn any(self) -> bool {
        self.size || self.time
    }
}

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

/// Other-panel entries, built once per frame. An entry pairs with the exact
/// same name first and falls back to a case-insensitive match only when that
/// folded name is unique, so `Readme` and `README` side by side on a
/// case-sensitive volume each compare against the right file.
#[derive(Clone, Default, Debug)]
pub struct CompareMap {
    exact: HashMap<String, CompareFingerprint>,
    /// Lowercase name -> fingerprint, or `None` when several names fold to it.
    folded: HashMap<String, Option<CompareFingerprint>>,
}

impl CompareMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: String, fingerprint: CompareFingerprint) {
        self.folded
            .entry(name.to_lowercase())
            .and_modify(|slot| *slot = None)
            .or_insert(Some(fingerprint));
        self.exact.insert(name, fingerprint);
    }

    /// Whether some entry folds to the lowercase `name`.
    #[cfg(test)]
    pub fn contains_key(&self, name: &str) -> bool {
        self.folded.contains_key(name)
    }

    /// The other-panel fingerprint `entry` pairs with, if any.
    fn get(&self, entry: &FileEntry) -> Option<&CompareFingerprint> {
        self.exact
            .get(&entry.name)
            .or_else(|| self.folded.get(&entry.name_lower)?.as_ref())
    }
}

#[cfg(test)]
impl std::ops::Index<&str> for CompareMap {
    type Output = CompareFingerprint;

    fn index(&self, name: &str) -> &CompareFingerprint {
        self.folded[name]
            .as_ref()
            .expect("folded name is ambiguous")
    }
}

/// Index a panel's entries for comparison against the other panel.
pub fn build_compare_map(entries: &[FileEntry]) -> CompareMap {
    let mut map = CompareMap::new();
    for e in entries {
        map.insert(
            e.name.clone(),
            CompareFingerprint {
                size: e.size,
                modified: e.modified,
                is_dir: e.is_dir,
            },
        );
    }
    map
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
        .filter(|e| match other.get(e) {
            None => criterion == CompareCriterion::Unique,
            Some(fingerprint) => match criterion {
                CompareCriterion::Unique => false,
                CompareCriterion::Differing => {
                    fingerprint.is_dir != e.is_dir
                        || (!e.is_dir && FileDiff::of(fingerprint, e).any())
                }
                CompareCriterion::Newer if fingerprint.is_dir == e.is_dir && !e.is_dir => {
                    mtime_newer(e.modified, fingerprint.modified)
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
    match other.get(entry) {
        None => CompareStatus::Unique,
        Some(fingerprint) if fingerprint.is_dir != entry.is_dir => CompareStatus::TypeConflict,
        Some(_) if entry.is_dir => CompareStatus::DirectoryPair,
        Some(fingerprint) if FileDiff::of(fingerprint, entry).any() => CompareStatus::Differs,
        Some(_) => CompareStatus::Identical,
    }
}

/// Human explanation for compare-mode row tinting. Identical rows intentionally
/// return `None` so quiet rows stay quiet.
pub fn compare_hint(entry: &FileEntry, other: &CompareMap) -> Option<String> {
    match other.get(entry) {
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
            let diff = FileDiff::of(fingerprint, entry);
            let time_unknown = fingerprint.modified.is_none() || entry.modified.is_none();
            match (diff.size, diff.time) {
                (false, false) => None,
                (false, true) if time_unknown => {
                    Some("Same name and size, modified time unknown".to_string())
                }
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
    use std::time::Duration;

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

    fn fake(name: &str, size: u64, modified: Option<SystemTime>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/x/{name}")),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size,
            extension: String::new(),
            modified,
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    #[test]
    fn coarse_mtime_copies_still_compare_identical() {
        use std::time::UNIX_EPOCH;
        let source = UNIX_EPOCH + Duration::new(1_000_000_123, 456_789_000);
        // FAT rounds to an even second.
        let fat_copy = UNIX_EPOCH + Duration::from_secs(1_000_000_124);
        let here = fake("a.txt", 10, Some(source));
        let other = build_compare_map(&[fake("a.txt", 10, Some(fat_copy))]);
        assert_eq!(classify_entry(&here, &other), CompareStatus::Identical);
        assert_eq!(compare_hint(&here, &other), None);
        for criterion in [CompareCriterion::Differing, CompareCriterion::Newer] {
            assert!(select_by_compare([&here].into_iter(), &other, criterion).is_empty());
        }

        let later = fake("a.txt", 10, Some(fat_copy + Duration::from_secs(3)));
        assert_eq!(classify_entry(&later, &other), CompareStatus::Differs);
        let newer = select_by_compare([&later].into_iter(), &other, CompareCriterion::Newer);
        assert_eq!(newer.len(), 1);
    }

    #[test]
    fn unknown_mtimes_never_compare_identical() {
        let here = fake("a.txt", 10, None);
        let other = build_compare_map(&[fake("a.txt", 10, None)]);
        assert_eq!(classify_entry(&here, &other), CompareStatus::Differs);
        assert!(compare_hint(&here, &other).unwrap().contains("unknown"));
    }

    #[test]
    fn case_variant_names_pair_exactly_before_folding() {
        use std::time::UNIX_EPOCH;
        let t = Some(UNIX_EPOCH + Duration::from_secs(100));
        let other = build_compare_map(&[fake("Readme", 1, t), fake("README", 2, t)]);
        assert_eq!(
            classify_entry(&fake("Readme", 1, t), &other),
            CompareStatus::Identical
        );
        assert_eq!(
            classify_entry(&fake("README", 2, t), &other),
            CompareStatus::Identical
        );
        // An ambiguous fold pairs with neither.
        assert_eq!(
            classify_entry(&fake("readme", 1, t), &other),
            CompareStatus::Unique
        );
        // A unique fold still pairs case-insensitively.
        let other = build_compare_map(&[fake("Photo.JPG", 1, t)]);
        assert_eq!(
            classify_entry(&fake("photo.jpg", 1, t), &other),
            CompareStatus::Identical
        );
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

    fn fake_dir(name: &str) -> FileEntry {
        FileEntry {
            is_dir: true,
            ..fake(name, 0, None)
        }
    }

    #[test]
    fn differing_selects_type_conflicts_but_not_folder_pairs() {
        use std::time::UNIX_EPOCH;
        let t = Some(UNIX_EPOCH + Duration::from_secs(100));
        let entries = [
            fake("mixed", 0, t),
            fake_dir("folders"),
            fake_dir("other-mixed"),
            fake("same.txt", 4, t),
        ];
        let other = build_compare_map(&[
            fake_dir("mixed"),
            // A folder pair with different sizes and mtimes is still not
            // known to differ: its contents were never compared.
            FileEntry {
                size: 4096,
                modified: t,
                ..fake_dir("folders")
            },
            fake("other-mixed", 0, None),
            fake("same.txt", 4, t),
        ]);
        let picks = select_by_compare(entries.iter(), &other, CompareCriterion::Differing);
        let expected: HashSet<PathBuf> = [entries[0].path.clone(), entries[2].path.clone()]
            .into_iter()
            .collect();
        assert_eq!(picks, expected);
        // Neither kind of pair is ever "newer".
        assert!(select_by_compare(entries.iter(), &other, CompareCriterion::Newer).is_empty());
    }

    #[test]
    fn newer_never_selects_an_unknown_mtime() {
        use std::time::UNIX_EPOCH;
        let t = Some(UNIX_EPOCH + Duration::from_secs(10_000));
        let here_unknown = fake("a.txt", 1, None);
        let there_unknown = fake("b.txt", 1, t);
        let both_unknown = fake("c.txt", 1, None);
        let other = build_compare_map(&[
            fake("a.txt", 1, Some(UNIX_EPOCH)),
            fake("b.txt", 1, None),
            fake("c.txt", 1, None),
        ]);
        let entries = [here_unknown, there_unknown, both_unknown];
        assert!(select_by_compare(entries.iter(), &other, CompareCriterion::Newer).is_empty());
        // The same pairs do count as differing: unknown cannot vouch for equal.
        assert_eq!(
            select_by_compare(entries.iter(), &other, CompareCriterion::Differing).len(),
            3
        );
    }
}
