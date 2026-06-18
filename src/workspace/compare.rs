//! Pure comparison logic between the two panels (for tinting rows, "select same named", etc.).
//! SRP: all the "how does this entry relate to the one in the other pane" rules in one small file.
//! Easy to test in isolation, no UI or side effects.
//! Extracted from workspace.rs per SOLID split plan.

use crate::panel::FileEntry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// How an entry relates to the same-named entry in the other panel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompareStatus {
    /// Same name, size and mtime as the other panel's entry.
    Identical,
    /// Same name but a different size or mtime.
    Differs,
    /// No entry of this name in the other panel.
    Unique,
}

/// Other-panel entries indexed by lowercase name → (size, mtime), for folder
/// comparison. Built once per frame from a panel's loaded entries.
pub type CompareMap = HashMap<String, (u64, Option<std::time::SystemTime>)>;

/// Index a panel's entries for comparison against the other panel.
pub fn build_compare_map(entries: &[FileEntry]) -> CompareMap {
    entries
        .iter()
        .map(|e| (e.name_lower.clone(), (e.size, e.modified)))
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
            Some(&(size, mtime)) => match criterion {
                CompareCriterion::Unique => false,
                CompareCriterion::Differing => size != e.size || mtime != e.modified,
                CompareCriterion::Newer => match (e.modified, mtime) {
                    (Some(a), Some(b)) => a > b,
                    _ => false,
                },
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
        Some(&(size, mtime)) => {
            if size == entry.size && mtime == entry.modified {
                CompareStatus::Identical
            } else {
                CompareStatus::Differs
            }
        }
    }
}
