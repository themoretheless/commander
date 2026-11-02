//! Pure duplicate-file grouping. Given files with a precomputed size and
//! content hash, group the byte-identical ones and pick a keeper by policy.
//! Hashing and IO live in the caller; everything here is pure and tested.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

/// A file participating in duplicate detection. `hash` is a content hash
/// computed by the caller; `modified` drives the chronological keep policies.
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
    Oldest,
    /// Keep the newest known mtime; unknown times still sort last.
    Newest,
    /// Keep the one with the shortest path string.
    ShortestPath,
    FirstName,
    LastName,
    ShortestName,
    LongestName,
    ShallowestPath,
    DeepestPath,
}
impl KeepPolicy {
    pub const ALL: [Self; 9] = [
        Self::ShortestPath,
        Self::Oldest,
        Self::Newest,
        Self::FirstName,
        Self::LastName,
        Self::ShortestName,
        Self::LongestName,
        Self::ShallowestPath,
        Self::DeepestPath,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::ShortestPath => "shortest path",
            Self::Oldest => "oldest",
            Self::Newest => "newest",
            Self::FirstName => "name A–Z",
            Self::LastName => "name Z–A",
            Self::ShortestName => "shortest name",
            Self::LongestName => "longest name",
            Self::ShallowestPath => "shallowest path",
            Self::DeepestPath => "deepest path",
        }
    }
    pub const fn report_key(self) -> &'static str {
        match self {
            Self::ShortestPath => "shortest_path",
            Self::Oldest => "oldest",
            Self::Newest => "newest",
            Self::FirstName => "first_name",
            Self::LastName => "last_name",
            Self::ShortestName => "shortest_name",
            Self::LongestName => "longest_name",
            Self::ShallowestPath => "shallowest_path",
            Self::DeepestPath => "deepest_path",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupSort {
    #[default]
    Scan,
    Savings,
    Size,
    Copies,
}
impl GroupSort {
    pub const ALL: [Self; 4] = [Self::Scan, Self::Savings, Self::Size, Self::Copies];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Scan => "Scan order",
            Self::Savings => "Potential savings",
            Self::Size => "File size",
            Self::Copies => "Copies",
        }
    }
}

/// View permutation only; keeper/member indices remain bound to original groups.
pub fn group_order(groups: &[DupGroup], sort: GroupSort) -> Vec<usize> {
    let mut order: Vec<_> = (0..groups.len()).collect();
    if sort != GroupSort::Scan {
        order.sort_by(|&a, &b| {
            let savings = |group: &DupGroup| {
                u128::from(group.size) * group.files.len().saturating_sub(1) as u128
            };
            let comparison = match sort {
                GroupSort::Savings => savings(&groups[b]).cmp(&savings(&groups[a])),
                GroupSort::Size => groups[b].size.cmp(&groups[a].size),
                GroupSort::Copies => groups[b].files.len().cmp(&groups[a].files.len()),
                GroupSort::Scan => std::cmp::Ordering::Equal,
            };
            comparison.then_with(|| a.cmp(&b))
        });
    }
    order
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

fn compare_known<T: Ord>(a: Option<T>, b: Option<T>, reverse: bool) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => {
            if reverse {
                b.cmp(&a)
            } else {
                a.cmp(&b)
            }
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
fn parent_depth(path: &std::path::Path) -> Option<usize> {
    path.parent().map(|parent| {
        parent
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .count()
    })
}

/// Index of the file to keep in `group` under `policy`. A non-empty group
/// always yields a valid index (the "never an empty keep set" invariant);
/// ties resolve to the earliest file in input order.
pub fn default_keep(group: &DupGroup, policy: KeepPolicy) -> usize {
    match policy {
        KeepPolicy::Oldest | KeepPolicy::Newest => group
            .files
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| match (a.modified, b.modified) {
                (Some(x), Some(y)) => {
                    if policy == KeepPolicy::Newest {
                        y.cmp(&x)
                    } else {
                        x.cmp(&y)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less, // known mtime wins
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .map(|(i, _)| i)
            .unwrap_or(0),
        KeepPolicy::FirstName
        | KeepPolicy::LastName
        | KeepPolicy::ShortestName
        | KeepPolicy::LongestName
        | KeepPolicy::ShallowestPath
        | KeepPolicy::DeepestPath => group
            .files
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| match policy {
                KeepPolicy::FirstName | KeepPolicy::LastName => compare_known(
                    a.path.file_name(),
                    b.path.file_name(),
                    policy == KeepPolicy::LastName,
                ),
                KeepPolicy::ShortestName | KeepPolicy::LongestName => compare_known(
                    a.path.file_name().map(|n| n.len()),
                    b.path.file_name().map(|n| n.len()),
                    policy == KeepPolicy::LongestName,
                ),
                _ => compare_known(
                    parent_depth(&a.path),
                    parent_depth(&b.path),
                    policy == KeepPolicy::DeepestPath,
                ),
            })
            .map(|(i, _)| i)
            .unwrap_or(0),
        KeepPolicy::ShortestPath => group
            .files
            .iter()
            .enumerate()
            .min_by_key(|(_, f)| f.path.as_os_str().len())
            .map(|(i, _)| i)
            .unwrap_or(0),
    }
}

/// Replace manual choices with a complete policy plan, keeping input identities.
pub fn keep_plan(groups: &[DupGroup], policy: KeepPolicy) -> Vec<Option<usize>> {
    groups
        .iter()
        .map(|group| (!group.files.is_empty()).then(|| default_keep(group, policy)))
        .collect()
}

fn valid_keeper(group: &DupGroup, keep: Option<usize>) -> Option<usize> {
    keep.filter(|&index| index < group.files.len())
}

/// Shared action owner for report formats; invalid keep/member indices skip safely.
pub fn plan_action(group: &DupGroup, keep: Option<usize>, member: usize) -> &'static str {
    if member >= group.files.len() {
        return "skip";
    }
    match valid_keeper(group, keep) {
        None => "skip",
        Some(index) if index == member => "keep",
        Some(_) => "trash",
    }
}
pub fn validate_group_order(group_count: usize, order: &[usize]) -> Result<(), String> {
    let mut seen = vec![false; group_count];
    if order.len() != group_count
        || order.iter().any(|&index| {
            if let Some(value) = seen.get_mut(index) {
                std::mem::replace(value, true)
            } else {
                true
            }
        })
    {
        return Err("Duplicate group order is not a complete permutation".into());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathList {
    Keepers,
    ProposedTrash,
}

/// Plain newline-delimited paths from the proposal. Never shells/quotes or reads files.
/// Reject ambiguous/non-lossless paths as a whole before a caller publishes output.
pub fn plan_paths(
    groups: &[DupGroup],
    keep: &[Option<usize>],
    order: &[usize],
    list: PathList,
) -> Result<String, String> {
    validate_group_order(groups.len(), order)?;
    let wanted = match list {
        PathList::Keepers => "keep",
        PathList::ProposedTrash => "trash",
    };
    let mut output = String::new();
    for &index in order {
        let group = &groups[index];
        for (member, file) in group.files.iter().enumerate() {
            if plan_action(group, keep.get(index).copied().flatten(), member) != wanted {
                continue;
            }
            let path = file
                .path
                .to_str()
                .ok_or("Path lists require lossless UTF-8; use a different report format")?;
            if !file.path.is_absolute() || path.contains(['\0', '\r', '\n']) {
                return Err("Path lists require absolute paths without NUL or line breaks".into());
            }
            if path.len().saturating_add(1) > (1024usize * 1024).saturating_sub(output.len()) {
                return Err("Duplicate path lists are limited to 1 MiB".into());
            }
            output.push_str(path);
            output.push('\n');
        }
    }
    Ok(output)
}

/// Snapshot the current proposal, including kept and skipped files. No file I/O.
pub fn plan_csv(groups: &[DupGroup], keep: &[Option<usize>]) -> Result<String, String> {
    plan_csv_in_order(groups, keep, &(0..groups.len()).collect::<Vec<_>>())
}

pub fn plan_csv_in_order(
    groups: &[DupGroup],
    keep: &[Option<usize>],
    order: &[usize],
) -> Result<String, String> {
    validate_group_order(groups.len(), order)?;
    const MAX_BYTES: usize = 1024 * 1024;
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record(["group", "action", "path", "size_bytes"])
        .map_err(|error| error.to_string())?;
    let mut records = 0;
    for (position, &group_index) in order.iter().enumerate() {
        let group = &groups[group_index];
        let group_number = (position + 1).to_string();
        let size = group.size.to_string();
        for (file_index, file) in group.files.iter().enumerate() {
            let path = file
                .path
                .to_str()
                .ok_or("CSV plan needs lossless UTF-8 paths")?;
            if !file.path.is_absolute() || path.contains('\0') {
                return Err("CSV plan needs absolute paths without NUL".into());
            }
            let action = plan_action(group, keep.get(group_index).copied().flatten(), file_index);
            writer
                .write_record([group_number.as_str(), action, path, size.as_str()])
                .map_err(|error| error.to_string())?;
            records += 1;
            if writer.get_ref().len() > MAX_BYTES {
                return Err("Duplicate CSV plan exceeds 1 MiB".into());
            }
        }
    }
    if records == 0 {
        return Err("No duplicate records to copy".into());
    }
    let bytes = writer.into_inner().map_err(|error| error.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("Duplicate CSV plan exceeds 1 MiB".into());
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

/// Members the plan moves to the Trash: in each group every file except the
/// kept one (`keep[i]` indexes group `i`'s files). A group whose keep is
/// `None` ("Keep all") or an invalid keeper index is skipped entirely.
pub fn trashed_members<'a>(
    groups: &'a [DupGroup],
    keep: &'a [Option<usize>],
) -> impl Iterator<Item = (&'a DupGroup, &'a FileKey)> + 'a {
    groups
        .iter()
        .zip(keep)
        .filter_map(|(group, keep)| valid_keeper(group, *keep).map(|keep| (group, keep)))
        .flat_map(|(group, keep)| {
            group
                .files
                .iter()
                .enumerate()
                .filter(move |(index, _)| *index != keep)
                .map(move |(_, file)| (group, file))
        })
}

/// Total number of files the plan would delete.
pub fn delete_count(groups: &[DupGroup], keep: &[Option<usize>]) -> usize {
    trashed_members(groups, keep).count()
}

/// Bytes the plan moves to the Trash: every trashed member's size.
pub fn bytes_reclaimed(groups: &[DupGroup], keep: &[Option<usize>]) -> u64 {
    trashed_members(groups, keep)
        .map(|(group, _)| group.size)
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
    fn path_lists_follow_actual_plan_and_order_and_reject_ambiguous_or_oversized_paths() {
        let mut groups = vec![
            DupGroup {
                size: 1,
                files: vec![
                    key("/a/keep space", 1, 1, None),
                    key("/a/trash", 1, 1, None),
                ],
            },
            DupGroup {
                size: 2,
                files: vec![key("/b/trash", 2, 2, None), key("/b/keep", 2, 2, None)],
            },
        ];
        let keep = vec![Some(0), Some(1)];
        assert_eq!(
            plan_paths(&groups, &keep, &[1, 0], PathList::Keepers).unwrap(),
            "/b/keep\n/a/keep space\n"
        );
        assert_eq!(
            plan_paths(&groups, &keep, &[1, 0], PathList::ProposedTrash).unwrap(),
            "/b/trash\n/a/trash\n"
        );
        assert_eq!(
            plan_paths(&groups, &[None, Some(99)], &[0, 1], PathList::Keepers).unwrap(),
            ""
        );
        assert_eq!(
            plan_paths(&groups, &[], &[0, 1], PathList::ProposedTrash).unwrap(),
            ""
        );
        assert!(plan_paths(&groups, &keep, &[0, 0], PathList::Keepers).is_err());
        for path in ["relative", "/bad\nname", "/bad\rname", "/bad\0name"] {
            groups[1].files[1].path = path.into();
            assert!(plan_paths(&groups, &keep, &[0, 1], PathList::Keepers).is_err());
            assert!(plan_paths(&groups, &keep, &[0, 1], PathList::ProposedTrash).is_ok());
        }
        groups[1].files[1].path = format!("/{}", "x".repeat(1024 * 1024)).into();
        assert!(plan_paths(&groups, &keep, &[0, 1], PathList::Keepers).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            groups[1].files[1].path = std::ffi::OsString::from_vec(b"/bad/\xff".to_vec()).into();
            assert!(plan_paths(&groups, &keep, &[0, 1], PathList::Keepers).is_err());
        }
    }
    #[test]
    fn name_and_depth_policies_keep_distinct_stable_literal_winners_without_io() {
        let paths = [
            "/root/root-copy.bin",
            "/root/first/alpha.bin",
            "/root/middle/nested/zulu.bin",
            "/root/deep/a/b/mid.bin",
            "/root/wide-folder/x",
            "/root/long-copy/the-very-very-longest-basename-copy.bin",
        ];
        let group = DupGroup {
            size: 10,
            files: paths.into_iter().map(|p| key(p, 10, 1, None)).collect(),
        };
        for (policy, winner) in [
            (KeepPolicy::FirstName, 1),
            (KeepPolicy::LastName, 2),
            (KeepPolicy::ShortestName, 4),
            (KeepPolicy::LongestName, 5),
            (KeepPolicy::ShallowestPath, 0),
            (KeepPolicy::DeepestPath, 3),
        ] {
            assert_eq!(default_keep(&group, policy), winner, "{policy:?}");
            let keep = keep_plan(std::slice::from_ref(&group), policy);
            assert_eq!(keep, vec![Some(winner)]);
            assert_eq!(delete_count(std::slice::from_ref(&group), &keep), 5);
            assert_eq!(bytes_reclaimed(std::slice::from_ref(&group), &keep), 50);
            let mut tied = DupGroup {
                size: 10,
                files: vec![key("/a/same", 10, 1, None), key("/b/same", 10, 1, None)],
            };
            assert_eq!(default_keep(&tied, policy), 0, "stable {policy:?}");
            tied.files.insert(0, key("", 10, 1, None));
            assert_eq!(default_keep(&tied, policy), 1, "unknown last {policy:?}");
            tied.files.clear();
            assert_eq!(default_keep(&tied, policy), 0);
            assert_eq!(keep_plan(&[tied], policy), vec![None]);
        }
        // Literal native ordering, not natural ordering, case folding or full-path sorting.
        let group = DupGroup {
            size: 10,
            files: ["/z/a10", "/a/a2", "/m/A", "/x/é"]
                .into_iter()
                .map(|p| key(p, 10, 1, None))
                .collect(),
        };
        assert_eq!(default_keep(&group, KeepPolicy::FirstName), 2);
        assert_eq!(default_keep(&group, KeepPolicy::LastName), 3);
        assert_eq!(default_keep(&group, KeepPolicy::ShortestName), 2);
        let group = DupGroup {
            size: 10,
            files: vec![key("/a/a10", 10, 1, None), key("/b/a2", 10, 1, None)],
        };
        assert_eq!(default_keep(&group, KeepPolicy::FirstName), 0);
    }
    #[cfg(unix)]
    #[test]
    fn name_policies_compare_raw_names_not_lossy_labels() {
        use std::os::unix::ffi::OsStringExt;
        let mut group = DupGroup {
            size: 10,
            files: vec![key("/a/�", 10, 1, None), key("/b/�", 10, 1, None)],
        };
        group.files[0].path = std::ffi::OsString::from_vec(b"/a/\xff".to_vec()).into();
        assert_eq!(default_keep(&group, KeepPolicy::FirstName), 1);
        assert_eq!(default_keep(&group, KeepPolicy::LastName), 0);
        assert_eq!(default_keep(&group, KeepPolicy::ShortestName), 0);
        assert_eq!(default_keep(&group, KeepPolicy::LongestName), 1);
    }

    #[test]
    fn newest_policy_prefers_known_times_stably_and_handles_pre_epoch_dates() {
        let mut group = DupGroup {
            size: 10,
            files: vec![
                key("unknown", 10, 1, None),
                key("newest", 10, 1, Some(300)),
                key("tied", 10, 1, Some(300)),
                key("old", 10, 1, Some(100)),
            ],
        };
        assert_eq!(default_keep(&group, KeepPolicy::Newest), 1);
        assert_eq!(default_keep(&group, KeepPolicy::Oldest), 3);
        for file in &mut group.files {
            file.modified = None;
        }
        assert_eq!(default_keep(&group, KeepPolicy::Newest), 0);
        group.files[2].modified = Some(UNIX_EPOCH - Duration::from_secs(5));
        assert_eq!(default_keep(&group, KeepPolicy::Newest), 2);
        group.files[3].modified = Some(UNIX_EPOCH);
        assert_eq!(default_keep(&group, KeepPolicy::Newest), 3);
    }

    #[test]
    fn policy_reapply_keep_all_and_invalid_keepers_never_trash_an_entire_group() {
        let groups = vec![
            DupGroup {
                size: 10,
                files: vec![
                    key("unknown", 10, 1, None),
                    key("newest", 10, 1, Some(300)),
                    key("tied", 10, 1, Some(300)),
                    key("old", 10, 1, Some(100)),
                ],
            },
            DupGroup {
                size: 20,
                files: vec![key("first", 20, 2, None), key("second", 20, 2, None)],
            },
        ];
        let mut keep = keep_plan(&groups, KeepPolicy::Newest);
        assert_eq!(keep, vec![Some(1), Some(0)]);
        let paths: Vec<_> = trashed_members(&groups, &keep)
            .map(|(_, file)| file.path.to_str().unwrap())
            .collect();
        assert_eq!(paths, ["unknown", "tied", "old", "second"]);
        assert_eq!(bytes_reclaimed(&groups, &keep), 50);
        keep.fill(None);
        assert_eq!(delete_count(&groups, &keep), 0);
        assert_eq!(bytes_reclaimed(&groups, &keep), 0);
        keep = keep_plan(&groups, KeepPolicy::Newest);
        assert_eq!(delete_count(&groups, &keep), 4);
        assert_eq!(delete_count(&groups, &[Some(usize::MAX), Some(99)]), 0);
        assert_eq!(
            keep_plan(
                &[DupGroup {
                    size: 0,
                    files: vec![]
                }],
                KeepPolicy::Newest
            ),
            vec![None]
        );
    }

    #[test]
    fn csv_plan_preserves_order_exact_paths_and_authoritative_keep_trash_skip_actions() {
        let groups = vec![
            DupGroup {
                size: 10,
                files: vec![
                    key("/a\"quote\nline", 10, 1, None),
                    key("/b,comma", 10, 1, None),
                ],
            },
            DupGroup {
                size: 20,
                files: vec![key("/ёж", 20, 2, None), key("/back\\slash", 20, 2, None)],
            },
            DupGroup {
                size: 30,
                files: vec![
                    key("/invalid-keep-a", 30, 3, None),
                    key("/invalid-keep-b", 30, 3, None),
                ],
            },
        ];
        let keep = vec![Some(0), None, Some(99)];
        let text = plan_csv(&groups, &keep).unwrap();
        let rows: Vec<_> = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(text.as_bytes())
            .records()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(&rows[0], &["group", "action", "path", "size_bytes"][..]);
        let mut position = 1;
        for (index, group) in groups.iter().enumerate() {
            for file in &group.files {
                assert_eq!(&rows[position][0], (index + 1).to_string());
                assert_eq!(&rows[position][2], file.path.to_str().unwrap());
                assert_eq!(&rows[position][3], group.size.to_string());
                position += 1;
            }
        }
        let actions: Vec<_> = rows.iter().skip(1).map(|row| &row[1]).collect();
        assert_eq!(actions, ["keep", "trash", "skip", "skip", "skip", "skip"]);
        let exported_trash: Vec<_> = rows
            .iter()
            .skip(1)
            .filter(|row| &row[1] == "trash")
            .map(|row| &row[2])
            .collect();
        let planned_trash: Vec<_> = trashed_members(&groups, &keep)
            .map(|(_, file)| file.path.to_str().unwrap())
            .collect();
        assert_eq!(exported_trash, planned_trash);
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn csv_plan_refuses_empty_lossy_relative_nul_and_oversized_exports() {
        assert!(plan_csv(&[], &[]).is_err());
        for path in ["relative", "/nul\0name"] {
            assert!(
                plan_csv(
                    &[DupGroup {
                        size: 1,
                        files: vec![key(path, 1, 0, None)]
                    }],
                    &[Some(0)]
                )
                .is_err()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut file = key("/valid", 1, 0, None);
            file.path = std::ffi::OsString::from_vec(b"/bad\xff".to_vec()).into();
            assert!(
                plan_csv(
                    &[DupGroup {
                        size: 1,
                        files: vec![file]
                    }],
                    &[Some(0)]
                )
                .is_err()
            );
        }
        let path = format!("/{}", "\"".repeat(1024 * 1024));
        assert!(
            plan_csv(
                &[DupGroup {
                    size: 1,
                    files: vec![key(&path, 1, 0, None)]
                }],
                &[Some(0)]
            )
            .unwrap_err()
            .contains("1 MiB")
        );
    }

    #[test]
    fn group_sort_uses_wide_potential_keys_stable_ties_and_scan_restore() {
        let group = |name: &str, size, copies| DupGroup {
            size,
            files: (0..copies)
                .map(|index| key(&format!("/{name}{index}"), size, size, None))
                .collect(),
        };
        let groups = vec![group("a", 50, 4), group("b", 100, 2), group("c", 50, 4)];
        assert_eq!(group_order(&groups, GroupSort::Savings), vec![0, 2, 1]);
        assert_eq!(group_order(&groups, GroupSort::Size), vec![1, 0, 2]);
        assert_eq!(group_order(&groups, GroupSort::Copies), vec![0, 2, 1]);
        assert_eq!(group_order(&groups, GroupSort::Scan), vec![0, 1, 2]);
        let large = vec![group("wide", u64::MAX, 3), group("narrow", u64::MAX, 2)];
        assert_eq!(group_order(&large, GroupSort::Savings), vec![0, 1]);
        assert!(group_order(&[], GroupSort::Savings).is_empty());
    }

    #[test]
    fn group_order_preserves_keeper_skip_bindings_and_csv_follows_the_view() {
        let groups = vec![
            DupGroup {
                size: 10,
                files: vec![
                    key("/small1", 10, 1, None),
                    key("/small2", 10, 1, None),
                    key("/small3", 10, 1, None),
                ],
            },
            DupGroup {
                size: 64,
                files: vec![key("/large1", 64, 2, None), key("/large2", 64, 2, None)],
            },
        ];
        let keep = vec![None, Some(1)];
        let before = groups.clone();
        for sort in GroupSort::ALL {
            let order = group_order(&groups, sort);
            let csv = plan_csv_in_order(&groups, &keep, &order).unwrap();
            let rows: Vec<_> = csv::ReaderBuilder::new()
                .has_headers(false)
                .from_reader(csv.as_bytes())
                .records()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(
                &rows[1][2],
                groups[order[0]].files[0].path.to_str().unwrap()
            );
            let trash: Vec<_> = rows
                .iter()
                .skip(1)
                .filter(|row| &row[1] == "trash")
                .map(|row| &row[2])
                .collect();
            assert_eq!(trash, ["/large1"]);
            assert_eq!(
                rows.iter().skip(1).filter(|row| &row[1] == "skip").count(),
                3
            );
            assert_eq!(delete_count(&groups, &keep), 1);
            assert_eq!(bytes_reclaimed(&groups, &keep), 64);
        }
        assert_eq!(groups, before);
        assert_eq!(keep, vec![None, Some(1)]);
        for order in [vec![], vec![0], vec![0, 0], vec![1, 2]] {
            assert!(plan_csv_in_order(&groups, &keep, &order).is_err());
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
        assert_eq!(bytes_reclaimed(&[], &[]), 0);
        assert_eq!(
            bytes_reclaimed(&[group(10, 3), group(7, 2)], &[Some(0), Some(1)]),
            27
        );
        assert_eq!(bytes_reclaimed(&[group(u64::MAX, 3)], &[Some(2)]), u64::MAX);
        // "Keep all" leaves a group out of the plan.
        assert_eq!(
            bytes_reclaimed(&[group(10, 3), group(7, 2)], &[None, Some(0)]),
            7
        );
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
        assert_eq!(default_keep(&g, KeepPolicy::Oldest), 1); // /old.txt
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
        assert_eq!(default_keep(&g, KeepPolicy::ShortestPath), 1);
    }

    #[test]
    fn delete_count_is_members_minus_one_per_group() {
        let g = |n: usize| DupGroup {
            size: 1,
            files: (0..n).map(|i| key(&format!("/f{i}"), 1, 1, None)).collect(),
        };
        assert_eq!(delete_count(&[g(2), g(3)], &[Some(0), Some(2)]), 1 + 2);
        assert_eq!(delete_count(&[], &[]), 0);
        assert_eq!(delete_count(&[g(2), g(3)], &[Some(1), None]), 1);
        assert_eq!(delete_count(&[g(2), g(3)], &[None, None]), 0);
    }

    #[test]
    fn trashed_members_skip_the_kept_file_and_kept_groups() {
        let g = |n: usize, tag: &str| DupGroup {
            size: 1,
            files: (0..n)
                .map(|i| key(&format!("/{tag}{i}"), 1, 1, None))
                .collect(),
        };
        let groups = [g(3, "a"), g(2, "b")];
        let trashed: Vec<String> = trashed_members(&groups, &[Some(1), None])
            .map(|(_, file)| file.path.display().to_string())
            .collect();
        assert_eq!(trashed, ["/a0", "/a2"]);
    }

    #[test]
    fn keep_oldest_ties_and_all_unknown_keep_the_first_file() {
        let tied = DupGroup {
            size: 10,
            files: vec![
                key("/new.txt", 10, 1, Some(300)),
                key("/first-old.txt", 10, 1, Some(100)),
                key("/unknown.txt", 10, 1, None),
                key("/second-old.txt", 10, 1, Some(100)),
            ],
        };
        assert_eq!(default_keep(&tied, KeepPolicy::Oldest), 1);
        let unknown = DupGroup {
            size: 10,
            files: vec![
                key("/b.txt", 10, 1, None),
                key("/a.txt", 10, 1, None),
                key("/c.txt", 10, 1, None),
            ],
        };
        assert_eq!(default_keep(&unknown, KeepPolicy::Oldest), 0);
        // A known mtime beats every unknown one, wherever it sits.
        let last_known = DupGroup {
            size: 10,
            files: vec![key("/b.txt", 10, 1, None), key("/a.txt", 10, 1, Some(5))],
        };
        assert_eq!(default_keep(&last_known, KeepPolicy::Oldest), 1);
    }
}
