//! Pure duplicate-file grouping. Given files with a precomputed size and
//! content hash, group the byte-identical ones and pick a keeper by policy.
//! Hashing and IO live in the caller; everything here is pure and tested.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

/// A file participating in duplicate detection. `hash` is a content hash
/// computed by the caller; `modified` drives the keep-oldest policy.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileKey {
    pub path: PathBuf,
    pub identity: crate::panel::ListingIdentity,
    pub size: u64,
    pub hash: u64,
    pub modified: Option<SystemTime>,
}

/// A cluster of byte-identical files (always >= 2 members).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DupGroup {
    pub size: u64,
    pub files: Vec<FileKey>,
}

/// Which file in a group to keep by default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeepPolicy {
    /// Keep the oldest by mtime (unknown mtime sorts last).
    KeepOldest,
    /// Keep the one with the shortest path string.
    KeepShortestPath,
}

/// The on-disk object behind `file` (volume, file id) when the listing
/// captured it. Hard links to one file share it.
fn object_key(file: &FileKey) -> Option<(u64, u64)> {
    match &file.identity {
        crate::panel::ListingIdentity::Captured(identity) => {
            Some((identity.volume?, identity.file_id?))
        }
        _ => None,
    }
}

/// Group byte-identical files (same size AND same hash). Hard links to one
/// file collapse to their first listed path, since trashing another link
/// reclaims no space. Singletons are dropped; only groups with >= 2 members
/// are returned. Group order and in-group order follow first appearance in
/// `files` (stable).
pub fn group_duplicates(files: &[FileKey]) -> Vec<DupGroup> {
    let mut order: Vec<(u64, u64)> = Vec::new();
    let mut buckets: HashMap<(u64, u64), Vec<FileKey>> = HashMap::new();
    let mut seen_objects: HashSet<(u64, u64)> = HashSet::new();
    for f in files {
        if object_key(f).is_some_and(|object| !seen_objects.insert(object)) {
            continue;
        }
        let key = (f.size, f.hash);
        let bucket = buckets.entry(key).or_default();
        if bucket.is_empty() {
            order.push(key);
        }
        bucket.push(f.clone());
    }
    order
        .into_iter()
        .filter_map(|key| {
            let files = buckets.remove(&key)?;
            (files.len() >= 2).then_some(DupGroup { size: key.0, files })
        })
        .collect()
}

/// Index of the file to keep in `group` under `policy`. A non-empty group
/// always yields a valid index (the "never an empty keep set" invariant);
/// ties resolve to the earliest file in input order.
pub fn default_keep(group: &DupGroup, policy: KeepPolicy) -> usize {
    match policy {
        KeepPolicy::KeepOldest => group
            .files
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| match (a.modified, b.modified) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less, // known mtime wins
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .map(|(i, _)| i)
            .unwrap_or(0),
        KeepPolicy::KeepShortestPath => group
            .files
            .iter()
            .enumerate()
            .min_by_key(|(_, f)| f.path.as_os_str().len())
            .map(|(i, _)| i)
            .unwrap_or(0),
    }
}

/// Total number of files the plan would delete: every non-kept member across
/// all groups.
pub fn delete_count(groups: &[DupGroup]) -> usize {
    groups.iter().map(|g| g.files.len().saturating_sub(1)).sum()
}

/// Bytes the plan moves to the Trash: every non-kept member's size.
pub fn bytes_reclaimed(groups: &[DupGroup]) -> u64 {
    groups
        .iter()
        .map(|g| {
            g.size
                .saturating_mul(g.files.len().saturating_sub(1) as u64)
        })
        .fold(0, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn key(path: &str, size: u64, hash: u64, secs: Option<u64>) -> FileKey {
        FileKey {
            path: PathBuf::from(path),
            identity: crate::panel::ListingIdentity::Unavailable,
            size,
            hash,
            modified: secs.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        }
    }

    #[test]
    fn groups_byte_identical_drops_singletons() {
        let files = vec![
            key("/a.txt", 10, 1, None),
            key("/b.txt", 10, 1, None), // dup of a
            key("/c.txt", 10, 2, None), // same size, different hash -> unique
            key("/d.txt", 99, 1, None), // same hash, different size -> unique
        ];
        let groups = group_duplicates(&files);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].size, 10);
        let names: Vec<&str> = groups[0]
            .files
            .iter()
            .map(|f| f.path.to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["/a.txt", "/b.txt"]); // stable order
    }

    #[test]
    fn hard_links_to_one_file_are_not_duplicates() {
        let linked = |path: &str, file_id| {
            let mut file = key(path, 10, 1, None);
            let mut identity = crate::path_identity::PathIdentity::missing(&file.path);
            identity.volume = Some(7);
            identity.file_id = Some(file_id);
            file.identity = crate::panel::ListingIdentity::Captured(identity);
            file
        };
        // Two names for inode 42 alone are no duplicate group.
        assert!(group_duplicates(&[linked("/a", 42), linked("/b", 42)]).is_empty());
        // With a real copy, the group keeps one link plus the copy.
        let groups = group_duplicates(&[linked("/a", 42), linked("/b", 42), linked("/c", 43)]);
        assert_eq!(groups.len(), 1);
        let names: Vec<&str> = groups[0]
            .files
            .iter()
            .map(|f| f.path.to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["/a", "/c"]);
    }

    #[test]
    fn bytes_reclaimed_counts_every_copy_but_one() {
        let group = |size, copies| DupGroup {
            size,
            files: (0..copies)
                .map(|i| key(&format!("/{i}"), size, 1, None))
                .collect(),
        };
        assert_eq!(bytes_reclaimed(&[]), 0);
        assert_eq!(bytes_reclaimed(&[group(10, 3), group(7, 2)]), 27);
        assert_eq!(bytes_reclaimed(&[group(u64::MAX, 3)]), u64::MAX);
    }

    #[test]
    fn no_duplicates_yields_no_groups() {
        let files = vec![key("/a", 1, 1, None), key("/b", 2, 2, None)];
        assert!(group_duplicates(&files).is_empty());
    }

    #[test]
    fn keep_oldest_picks_smallest_mtime_unknown_last() {
        let g = DupGroup {
            size: 10,
            files: vec![
                key("/new.txt", 10, 1, Some(300)),
                key("/old.txt", 10, 1, Some(100)),
                key("/mid.txt", 10, 1, Some(200)),
                key("/unknown.txt", 10, 1, None),
            ],
        };
        assert_eq!(default_keep(&g, KeepPolicy::KeepOldest), 1); // /old.txt
    }

    #[test]
    fn keep_shortest_path_and_tie_breaks_to_first() {
        let g = DupGroup {
            size: 10,
            files: vec![
                key("/deep/nested/file.txt", 10, 1, None),
                key("/a.txt", 10, 1, None),
                key("/b.txt", 10, 1, None), // same length as /a.txt -> tie
            ],
        };
        // Shortest path wins; the tie between /a.txt and /b.txt keeps the first.
        assert_eq!(default_keep(&g, KeepPolicy::KeepShortestPath), 1);
    }

    #[test]
    fn delete_count_is_members_minus_one_per_group() {
        let g = |n: usize| DupGroup {
            size: 1,
            files: (0..n).map(|i| key(&format!("/f{i}"), 1, 1, None)).collect(),
        };
        assert_eq!(delete_count(&[g(2), g(3)]), 1 + 2);
        assert_eq!(delete_count(&[]), 0);
    }
}
