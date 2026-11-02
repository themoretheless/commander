use crate::natural_sort::compare as natural_cmp;
use std::cmp::Ordering;

use super::{FileEntry, SizeSnapshot, SortColumn, SortOrder, ViewConfig};

/// Name order as the user configured it: natural ("file2" before "file10")
/// or plain A-Z. Every other column breaks its ties with this, always
/// ascending whatever the sort direction. Default folded ordering puts
/// "alpha" before "Zeta"; the case-sensitive option compares original names.
fn name_cmp(a: &FileEntry, b: &FileEntry, config: ViewConfig) -> Ordering {
    if config.case_sensitive_name_sort() {
        return if config.natural_name_sort() {
            natural_cmp(&a.name, &b.name)
        } else {
            a.name.cmp(&b.name)
        };
    }
    let folded = if config.natural_name_sort() {
        natural_cmp(&a.sort_key, &b.sort_key)
    } else {
        a.sort_key.cmp(&b.sort_key)
    };
    folded.then_with(|| {
        if config.natural_name_sort() {
            natural_cmp(&a.name_lower, &b.name_lower)
        } else {
            a.name_lower.cmp(&b.name_lower)
        }
    })
}

/// Borrow normalized name keys without allocating during comparison. Directory
/// names keep their dots; files lose only their final extension.
fn stem_key(key: &str, directory: bool) -> &str {
    if directory {
        key
    } else {
        std::path::Path::new(key)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(key)
    }
}

fn stem_cmp(a: &FileEntry, b: &FileEntry, config: ViewConfig) -> Ordering {
    let compare = |a: &str, b: &str| {
        if config.natural_name_sort() {
            natural_cmp(a, b)
        } else {
            a.cmp(b)
        }
    };
    if config.case_sensitive_name_sort() {
        return compare(stem_key(&a.name, a.is_dir), stem_key(&b.name, b.is_dir));
    }
    compare(
        stem_key(&a.sort_key, a.is_dir),
        stem_key(&b.sort_key, b.is_dir),
    )
    .then_with(|| {
        compare(
            stem_key(&a.name_lower, a.is_dir),
            stem_key(&b.name_lower, b.is_dir),
        )
    })
}

pub(super) fn sort_entries(entries: &mut [FileEntry], config: ViewConfig) {
    sort_entries_with_sizes(entries, config, None);
}

pub(super) fn sort_entries_with_sizes(
    entries: &mut [FileEntry],
    config: ViewConfig,
    sizes: Option<&SizeSnapshot>,
) {
    entries.sort_by(|a, b| {
        if config.folders_first() {
            match (a.is_dir, b.is_dir) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (true, true) if config.folders_by_name() => {
                    return name_cmp(a, b, config).then_with(|| a.path.cmp(&b.path));
                }
                _ => {}
            }
        }

        if config.sort_column() == SortColumn::HardLinks {
            match (a.hard_link_count(), b.hard_link_count()) {
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                _ => {}
            }
        }
        if config.sort_column() == SortColumn::Inode {
            match (a.inode_sort_key(), b.inode_sort_key()) {
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                _ => {}
            }
        }
        if config.sort_column() == SortColumn::Allocated {
            match (a.allocated_bytes(), b.allocated_bytes()) {
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                _ => {}
            }
        }
        if matches!(
            config.sort_column(),
            SortColumn::Accessed | SortColumn::Created | SortColumn::Changed
        ) {
            match (
                a.listing_time(config.sort_column()).0,
                b.listing_time(config.sort_column()).0,
            ) {
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                _ => {}
            }
        }
        let primary = match config.sort_column() {
            SortColumn::HardLinks => a.hard_link_count().cmp(&b.hard_link_count()),
            SortColumn::Inode => a.inode_sort_key().cmp(&b.inode_sort_key()),
            SortColumn::Allocated => a.allocated_bytes().cmp(&b.allocated_bytes()),
            SortColumn::Name => name_cmp(a, b, config),
            SortColumn::Stem => stem_cmp(a, b, config),
            SortColumn::Size => sizes
                .map_or(a.size, |sizes| sizes.display_size(a))
                .cmp(&sizes.map_or(b.size, |sizes| sizes.display_size(b))),
            SortColumn::Modified => a.modified.cmp(&b.modified),
            SortColumn::Accessed | SortColumn::Created | SortColumn::Changed => a
                .listing_time(config.sort_column())
                .0
                .cmp(&b.listing_time(config.sort_column()).0),
            SortColumn::Extension => a.extension.cmp(&b.extension),
            SortColumn::Kind => {
                crate::selection_summary::kind_of(a).cmp(&crate::selection_summary::kind_of(b))
            }
        };
        let ordered = match config.sort_order() {
            SortOrder::Asc => primary,
            SortOrder::Desc => primary.reverse(),
        };
        // Name is the primary key of its own column; elsewhere it is an
        // ascending tie-break that does not flip with the direction.
        ordered
            .then_with(|| name_cmp(a, b, config))
            .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_link_count_sort_distinguishes_zero_unknown_full_width_and_name_ties() {
        let mut rows = vec![
            named("file10", false, 1),
            named("file2", false, 1),
            named("zero", false, 1),
            named("maximum", false, 1),
            named("unknown", false, 1),
            named("folder", true, 0),
        ];
        for (i, n) in [2, 2, 0, u64::MAX].into_iter().enumerate() {
            rows[i].hard_links = Some(n);
        }
        rows[5].hard_links = Some(u64::MAX);
        for (order, expected) in [
            (
                SortOrder::Asc,
                vec!["zero", "file2", "file10", "maximum", "folder", "unknown"],
            ),
            (
                SortOrder::Desc,
                vec!["maximum", "file2", "file10", "zero", "folder", "unknown"],
            ),
        ] {
            let config = ViewConfig::default()
                .with_folders_first(false)
                .with_sort(SortColumn::HardLinks, order);
            let restored: ViewConfig =
                serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
            assert_eq!(restored.sort_column(), SortColumn::HardLinks);
            let mut values = rows.clone();
            sort_entries(&mut values, restored);
            assert_eq!(
                values.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                expected
            );
        }
        sort_entries(
            &mut rows,
            ViewConfig::default().with_sort(SortColumn::HardLinks, SortOrder::Desc),
        );
        assert_eq!(rows[0].name, "folder");
        assert_eq!(rows.last().unwrap().name, "unknown");
    }

    #[test]
    fn inode_sort_preserves_full_width_keys_unknown_last_and_name_ties() {
        let mut rows = vec![
            named("file10", false, 1),
            named("file2", false, 1),
            named("zero", false, 1),
            named("other-device", false, 1),
            named("maximum", false, 1),
            named("unknown", false, 1),
            named("folder", true, 0),
        ];
        for (index, (inode, device)) in [(7, 4), (7, 4), (0, 4), (7, 5), (u64::MAX, u64::MAX)]
            .into_iter()
            .enumerate()
        {
            rows[index].identity =
                super::super::ListingIdentity::Captured(crate::path_identity::PathIdentity {
                    path: rows[index].path.clone(),
                    exists: true,
                    kind: Some(crate::path_identity::PathKind::File),
                    volume: Some(device),
                    file_id: Some(inode),
                    size: 1,
                    modified_nanos: None,
                    tree_fingerprint: None,
                    observed_version: 0,
                });
        }
        for (order, expected) in [
            (
                SortOrder::Asc,
                vec![
                    "zero",
                    "file2",
                    "file10",
                    "other-device",
                    "maximum",
                    "folder",
                    "unknown",
                ],
            ),
            (
                SortOrder::Desc,
                vec![
                    "maximum",
                    "other-device",
                    "file2",
                    "file10",
                    "zero",
                    "folder",
                    "unknown",
                ],
            ),
        ] {
            let config = ViewConfig::default()
                .with_folders_first(false)
                .with_sort(SortColumn::Inode, order);
            let restored: ViewConfig =
                serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
            assert_eq!(restored.sort_column(), SortColumn::Inode);
            let mut entries = rows.clone();
            sort_entries(&mut entries, restored);
            assert_eq!(
                entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                expected
            );
        }
        sort_entries(
            &mut rows,
            ViewConfig::default().with_sort(SortColumn::Inode, SortOrder::Desc),
        );
        assert_eq!(rows[0].name, "folder");
        assert_eq!(rows.last().unwrap().name, "unknown");
    }

    #[test]
    fn allocated_sort_orders_known_zero_and_keeps_unknowns_last_in_both_directions() {
        let mut rows = vec![
            named("file10", false, 1),
            named("file2", false, 1),
            named("sparse", false, 999999),
            named("unknown", false, 1),
            named("folder", true, 0),
        ];
        rows[0].allocated_size = Some(4096);
        rows[1].allocated_size = Some(4096);
        rows[2].allocated_size = Some(0);
        rows[4].allocated_size = Some(12345);
        for (order, expected) in [
            (
                SortOrder::Asc,
                vec!["sparse", "file2", "file10", "folder", "unknown"],
            ),
            (
                SortOrder::Desc,
                vec!["file2", "file10", "sparse", "folder", "unknown"],
            ),
        ] {
            let config = ViewConfig::default()
                .with_folders_first(false)
                .with_sort(SortColumn::Allocated, order);
            let mut entries = rows.clone();
            sort_entries(&mut entries, config);
            assert_eq!(
                entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                expected
            );
        }
        sort_entries(
            &mut rows,
            ViewConfig::default().with_sort(SortColumn::Allocated, SortOrder::Desc),
        );
        assert_eq!(rows[0].name, "folder");
        assert_eq!(rows.last().unwrap().name, "unknown");
    }

    #[test]
    fn alternate_dates_order_exact_times_unknowns_and_stable_ties() {
        let epoch = std::time::SystemTime::UNIX_EPOCH;
        let mut rows = vec![
            named("file10", false, 1),
            named("file2", false, 1),
            named("old", false, 1),
            named("unknown", false, 1),
            named("folder", true, 0),
        ];
        for row in &mut rows[..2] {
            row.accessed = Some(epoch + std::time::Duration::from_nanos(1));
            row.created = row.accessed;
            row.changed = row.accessed;
        }
        rows[2].accessed = Some(epoch - std::time::Duration::from_nanos(1));
        rows[2].created = rows[2].accessed;
        rows[2].changed = rows[2].accessed;
        for column in [
            SortColumn::Accessed,
            SortColumn::Created,
            SortColumn::Changed,
        ] {
            for (direction, expected) in [
                (
                    SortOrder::Asc,
                    vec!["old", "file2", "file10", "folder", "unknown"],
                ),
                (
                    SortOrder::Desc,
                    vec!["file2", "file10", "old", "folder", "unknown"],
                ),
            ] {
                let config = ViewConfig::default()
                    .with_folders_first(false)
                    .with_sort(column, direction);
                assert_eq!(sorted(rows.clone(), config), expected);
                let restored: ViewConfig =
                    serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
                assert_eq!(restored, config);
            }
            let result = sorted(
                rows.clone(),
                ViewConfig::default().with_sort(column, SortOrder::Desc),
            );
            assert_eq!(result[0], "folder");
            assert_eq!(result.last().unwrap(), "unknown");
        }
        rows[0].created = Some(epoch - std::time::Duration::from_secs(1));
        assert_ne!(
            sorted(
                rows.clone(),
                ViewConfig::default()
                    .with_folders_first(false)
                    .with_sort(SortColumn::Accessed, SortOrder::Asc)
            ),
            sorted(
                rows,
                ViewConfig::default()
                    .with_folders_first(false)
                    .with_sort(SortColumn::Created, SortOrder::Asc)
            )
        );
    }

    fn named(name: &str, is_dir: bool, size: u64) -> FileEntry {
        FileEntry {
            symlink: None,
            permissions: None,
            allocated_size: None,
            hard_links: None,
            owner_uid: None,
            owner_gid: None,
            accessed: None,
            created: None,
            accessed_str: "Unavailable".into(),
            created_str: "Unavailable".into(),
            changed: None,
            changed_str: "Unavailable".into(),
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            sort_key: super::super::diacritic_sort_key(&name.to_lowercase()),
            path: std::path::PathBuf::from(format!("/t/{name}")),
            identity: super::super::ListingIdentity::Unavailable,
            is_dir,
            size,
            extension: name
                .rsplit_once('.')
                .map(|(_, e)| e.to_string())
                .unwrap_or_default(),
            modified: None,
            modified_str: String::new(),
            size_str: String::new(),
        }
    }

    fn sorted(mut entries: Vec<FileEntry>, config: ViewConfig) -> Vec<String> {
        sort_entries(&mut entries, config);
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn independent_folder_names_ignore_file_column_and_direction() {
        let dirs = vec![named("folder10.z", true, 1), named("folder2.a", true, 99)];
        let files = vec![named("large.txt", false, 10), named("small.rs", false, 2)];
        for column in [
            SortColumn::Name,
            SortColumn::Stem,
            SortColumn::Allocated,
            SortColumn::Size,
            SortColumn::Modified,
            SortColumn::Accessed,
            SortColumn::Created,
            SortColumn::Extension,
            SortColumn::Kind,
        ] {
            for order in [SortOrder::Asc, SortOrder::Desc] {
                for natural in [false, true] {
                    let config = ViewConfig::default()
                        .with_sort(column, order)
                        .with_natural_name_sort(natural)
                        .with_folders_by_name(true);
                    let expected_dirs = if natural {
                        ["folder2.a", "folder10.z"]
                    } else {
                        ["folder10.z", "folder2.a"]
                    };
                    let mut entries = files.clone();
                    entries.extend(dirs.clone());
                    let result = sorted(entries, config);
                    assert_eq!(result[..2], expected_dirs);
                    assert_eq!(
                        result[2..],
                        sorted(files.clone(), config.with_folders_by_name(false))
                    );
                }
            }
        }
        let config = ViewConfig::default()
            .with_sort(SortColumn::Size, SortOrder::Desc)
            .with_folders_by_name(true);
        let mut entries = files;
        entries.extend(dirs);
        assert_eq!(
            sorted(entries.clone(), config),
            ["folder2.a", "folder10.z", "large.txt", "small.rs"]
        );
        assert_eq!(
            sorted(entries, config.with_folders_first(false)),
            ["folder2.a", "large.txt", "small.rs", "folder10.z"]
        );
    }

    #[test]
    fn case_sensitive_names_use_original_case_for_primary_and_tied_order() {
        let entries = vec![
            named("alpha", false, 1),
            named("Beta", false, 1),
            named("Zulu", false, 1),
            named("beta", false, 1),
        ];
        for natural in [false, true] {
            let config = ViewConfig::default()
                .with_natural_name_sort(natural)
                .with_case_sensitive_name_sort(true);
            assert_eq!(
                sorted(entries.clone(), config),
                ["Beta", "Zulu", "alpha", "beta"]
            );
            assert_eq!(
                sorted(
                    entries.clone(),
                    config.with_sort(SortColumn::Name, SortOrder::Desc)
                ),
                ["beta", "alpha", "Zulu", "Beta"]
            );
            assert_eq!(
                sorted(
                    entries.clone(),
                    config.with_sort(SortColumn::Size, SortOrder::Desc)
                ),
                ["Beta", "Zulu", "alpha", "beta"]
            );
            assert_eq!(
                sorted(entries.clone(), config.with_case_sensitive_name_sort(false)),
                ["alpha", "Beta", "beta", "Zulu"]
            );
        }
    }

    #[test]
    fn sensitive_stems_keep_numeric_sort_and_unicode_code_point_order() {
        let config = ViewConfig::default()
            .with_sort(SortColumn::Stem, SortOrder::Asc)
            .with_case_sensitive_name_sort(true);
        let entries = vec![
            named("file2.zip", false, 1),
            named("File10.txt", false, 1),
            named("File2.rs", false, 1),
            named("File2.txt", false, 1),
            named("zebra", false, 1),
            named("Éclair", false, 1),
        ];
        assert_eq!(
            sorted(entries.clone(), config),
            [
                "File2.rs",
                "File2.txt",
                "File10.txt",
                "file2.zip",
                "zebra",
                "Éclair"
            ]
        );
        assert_eq!(
            sorted(entries, config.with_natural_name_sort(false)),
            [
                "File10.txt",
                "File2.rs",
                "File2.txt",
                "file2.zip",
                "zebra",
                "Éclair"
            ]
        );
    }

    #[test]
    fn stem_sort_strips_final_extension_and_keeps_ties_ascending() {
        let entries = vec![
            named("a-z.txt", false, 1),
            named("a.txt", false, 1),
            named("a.rs", false, 1),
            named("archive.tar.gz", false, 1),
            named("archive.zip", false, 1),
            named(".env", false, 1),
            named(".env.local", false, 1),
        ];
        let config = ViewConfig::default().with_sort(SortColumn::Stem, SortOrder::Asc);
        assert_eq!(
            sorted(entries.clone(), config),
            [
                ".env",
                ".env.local",
                "a.rs",
                "a.txt",
                "a-z.txt",
                "archive.zip",
                "archive.tar.gz"
            ]
        );
        assert_eq!(
            sorted(entries, config.with_sort(SortColumn::Stem, SortOrder::Desc)),
            [
                "archive.tar.gz",
                "archive.zip",
                "a-z.txt",
                "a.rs",
                "a.txt",
                ".env",
                ".env.local"
            ]
        );
    }

    #[test]
    fn stem_sort_respects_natural_order_and_directory_dots() {
        let entries = vec![
            named("file10.zip", false, 1),
            named("file2.txt", false, 1),
            named("a.z", true, 0),
            named("a-b", true, 0),
        ];
        let config = ViewConfig::default().with_sort(SortColumn::Stem, SortOrder::Asc);
        assert_eq!(
            sorted(entries.clone(), config),
            ["a-b", "a.z", "file2.txt", "file10.zip"]
        );
        assert_eq!(
            sorted(entries, config.with_natural_name_sort(false)),
            ["a-b", "a.z", "file10.zip", "file2.txt"]
        );
    }

    #[test]
    fn equal_keys_break_ties_by_name_not_byte_path() {
        let config = ViewConfig::default().with_sort(SortColumn::Size, SortOrder::Asc);
        let names = sorted(
            vec![
                named("Zeta", true, 0),
                named("alpha", true, 0),
                named("beta", true, 0),
            ],
            config,
        );
        assert_eq!(names, ["alpha", "beta", "Zeta"]);
    }

    #[test]
    fn descending_sorts_keep_ties_in_ascending_name_order() {
        let config = ViewConfig::default().with_sort(SortColumn::Size, SortOrder::Desc);
        let names = sorted(
            vec![
                named("Zeta", false, 1),
                named("alpha", false, 1),
                named("big", false, 9),
            ],
            config,
        );
        assert_eq!(names, ["big", "alpha", "Zeta"]);

        let by_name = ViewConfig::default().with_sort(SortColumn::Name, SortOrder::Desc);
        let names = sorted(
            vec![named("alpha", false, 1), named("Zeta", false, 1)],
            by_name,
        );
        assert_eq!(names, ["Zeta", "alpha"]);
    }

    #[test]
    fn extension_ties_follow_the_natural_sort_setting() {
        let entries = || vec![named("f10.txt", false, 1), named("f2.txt", false, 1)];
        let natural = ViewConfig::default().with_sort(SortColumn::Extension, SortOrder::Asc);
        assert!(natural.natural_name_sort());
        assert_eq!(sorted(entries(), natural), ["f2.txt", "f10.txt"]);
        let plain = natural.with_natural_name_sort(false);
        assert_eq!(sorted(entries(), plain), ["f10.txt", "f2.txt"]);
    }

    #[test]
    fn name_sort_folds_diacritics_before_codepoint_tiebreaks() {
        let entries = || {
            vec![
                named("zebra", false, 1),
                named("École", false, 1),
                named("Ärger", false, 1),
            ]
        };
        let natural = ViewConfig::default().with_sort(SortColumn::Name, SortOrder::Asc);
        assert_eq!(sorted(entries(), natural), ["Ärger", "École", "zebra"]);
        assert_eq!(
            sorted(entries(), natural.with_natural_name_sort(false)),
            ["Ärger", "École", "zebra"]
        );
    }

    #[test]
    fn size_sort_uses_measured_directory_sizes() {
        let entries = vec![named("small", true, 0), named("large", true, 0)];
        let sizes = SizeSnapshot::from_sizes([
            (std::path::PathBuf::from("/t/small"), 10),
            (std::path::PathBuf::from("/t/large"), 100),
        ]);
        let config = ViewConfig::default().with_sort(SortColumn::Size, SortOrder::Desc);
        let mut entries = entries;

        sort_entries_with_sizes(&mut entries, config, Some(&sizes));

        assert_eq!(
            entries
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            ["large", "small"]
        );
    }

    #[test]
    fn natural_numbers_are_ordered_by_value_then_leading_zero_count() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("img9", "img09"), Ordering::Less);
        assert_eq!(natural_cmp("v1.2", "v1.10"), Ordering::Less);
        // The zero-count tie-break never overrides a later difference.
        assert_eq!(natural_cmp("a01b", "a1c"), Ordering::Less);
        assert_eq!(natural_cmp("a1c", "a01b"), Ordering::Greater);
        assert_eq!(natural_cmp("a01b", "a1b"), Ordering::Greater);
    }

    #[test]
    fn natural_order_of_non_ascii_names_follows_code_points() {
        // Byte-wise comparison must agree with comparing characters.
        assert_eq!(natural_cmp("é", "z"), Ordering::Greater);
        assert_eq!(natural_cmp("z", "é"), Ordering::Less);
        assert_eq!(natural_cmp("ä", "é"), Ordering::Less);
        assert_eq!(natural_cmp("日本2", "日本10"), Ordering::Less);
        assert_eq!(natural_cmp("é2", "é"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
        assert_eq!(natural_cmp("1é", "1a"), Ordering::Greater);
        let mut names = vec!["ü10", "ü9", "u10", "日1", "a"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["a", "u10", "ü9", "ü10", "日1"]);
    }

    #[test]
    fn natural_digit_runs_longer_than_u64_and_all_zero_runs() {
        assert_eq!(
            natural_cmp("f99999999999999999999", "f100000000000000000000"),
            Ordering::Less
        );
        assert_eq!(
            natural_cmp("f100000000000000000001", "f100000000000000000000"),
            Ordering::Greater
        );
        // All-zero runs are the value 0; the shorter run sorts first.
        assert_eq!(natural_cmp("a0", "a00"), Ordering::Less);
        assert_eq!(natural_cmp("a00", "a0"), Ordering::Greater);
        assert_eq!(natural_cmp("a00", "a1"), Ordering::Less);
    }

    #[test]
    fn natural_prefixes_and_digits_before_letters() {
        assert_eq!(natural_cmp("file", "file1"), Ordering::Less);
        assert_eq!(natural_cmp("a1b", "a"), Ordering::Greater);
        assert_eq!(natural_cmp("1a", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a", "1a"), Ordering::Greater);
    }

    #[test]
    fn modified_sort_puts_unknown_dates_first_ascending_and_last_descending() {
        use std::time::{Duration, UNIX_EPOCH};
        let dated = |name: &str, secs: Option<u64>| {
            let mut entry = named(name, false, 1);
            entry.modified = secs.map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
            entry
        };
        let entries = || {
            vec![
                dated("new", Some(300)),
                dated("unknown", None),
                dated("old", Some(100)),
            ]
        };
        let asc = ViewConfig::default().with_sort(SortColumn::Modified, SortOrder::Asc);
        assert_eq!(sorted(entries(), asc), ["unknown", "old", "new"]);
        let desc = ViewConfig::default().with_sort(SortColumn::Modified, SortOrder::Desc);
        assert_eq!(sorted(entries(), desc), ["new", "old", "unknown"]);
    }
}
