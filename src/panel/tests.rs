use super::*;
use crate::testutil::TempDir;

#[test]
fn selection_targets_restrict_both_mask_directions_and_name_list_counts() {
    let root = crate::testutil::TempDir::new();
    for name in ["keep-file.txt", "keep.bin", "outside.txt"] {
        root.file(name, "x");
    }
    root.dir("keep-folder.txt");
    for (target, count) in [
        (SelectionTarget::Both, 3),
        (SelectionTarget::Files, 2),
        (SelectionTarget::Folders, 1),
    ] {
        let mut panel = PanelState::new(root.path().into());
        panel.refresh();
        panel.toggle_select(root.path().join("outside.txt"));
        panel.set_search_query("keep");
        let names = "keep-file.txt\nkeep.bin\nkeep-folder.txt\noutside.txt";
        assert_eq!(
            panel.names_match_count_target(names, target).unwrap(),
            count
        );
        assert_eq!(panel.select_by_names_target(names, target).unwrap(), count);
        assert_eq!(panel.names_match_count_target(names, target).unwrap(), 0);
        assert_eq!(panel.mask_match_count_target("!keep*", target), count);
        assert_eq!(panel.select_by_mask_target("!keep*", target), 0);
        assert_eq!(panel.selected_count(), 1);
        assert!(panel.is_selected(&root.path().join("outside.txt")));
        assert_eq!(panel.mask_match_count_target("keep*", target), count);
        assert_eq!(panel.select_by_mask_target("keep*", target), count);
        assert_eq!(panel.mask_match_count_target("keep*", target), 0);
        panel.select_by_mask("keep*");
        assert_eq!(panel.selected_count(), 4);
        panel.select_by_mask_target("!keep*", target);
        assert_eq!(panel.selected_count(), 4 - count);
        for entry in panel.filtered_iter() {
            assert_eq!(
                panel.is_selected(&entry.path),
                !target.accepts(entry.is_dir)
            );
        }
        let before = panel.selected_count();
        assert!(
            panel
                .select_by_names_target("keep.bin\n../invalid", target)
                .is_err()
        );
        assert_eq!(panel.selected_count(), before);
    }
}

#[test]
fn name_list_preview_and_apply_are_literal_filtered_and_atomic_on_error() {
    let root = crate::testutil::TempDir::new();
    for name in ["exact.txt", "exact.TXT", "*.rs", "match.rs", " trailing "] {
        root.file(name, "x");
    }
    root.dir("folder");
    let mut panel = PanelState::new(root.path().into());
    panel.refresh();
    panel.toggle_select(root.path().join("match.rs"));
    panel.set_search_query("exact");
    let text = "exact.txt\nexact.txt\n*.rs\nfolder\n";
    assert_eq!(panel.names_match_count(text).unwrap(), 1);
    assert_eq!(panel.select_by_names(text).unwrap(), 1);
    assert_eq!(panel.names_match_count(text).unwrap(), 0);
    assert!(panel.is_selected(&root.path().join("match.rs")));
    assert!(!panel.is_selected(&root.path().join("exact.TXT")));
    assert!(!panel.is_selected(&root.path().join("*.rs")));
    panel.set_search_query("");
    assert_eq!(panel.names_match_count(text).unwrap(), 2);
    assert_eq!(panel.select_by_names(text).unwrap(), 2);
    assert!(panel.is_selected(&root.path().join("*.rs")));
    assert!(panel.is_selected(&root.path().join("folder")));
    let before = panel.selected_count();
    assert!(panel.select_by_names(" trailing \n../invalid").is_err());
    assert_eq!(panel.selected_count(), before);
    assert!(!panel.is_selected(&root.path().join(" trailing ")));
}

#[test]
fn selection_kind_and_extension_subtraction_respect_filtered_rows() {
    let root = crate::testutil::TempDir::new();
    for name in [
        "keep-a.txt",
        "keep-b.TXT",
        "keep.bin",
        "other.txt",
        "README",
    ] {
        root.file(name, "data");
    }
    let folder = root.dir("keep-folder.txt");
    let mut panel = PanelState::new(root.path().into());
    panel.refresh();
    panel.toggle_select(root.path().join("other.txt"));
    panel.set_search_query("keep");
    panel.focus_child("keep-a.txt".into());
    assert_eq!(panel.select_visible_kind(true), 1);
    assert_eq!(panel.select_visible_kind(false), 3);
    assert_eq!(panel.select_visible_kind(false), 0);
    assert_eq!(panel.selected_count(), 5);
    assert_eq!(panel.deselect_same_extension_as_cursor(), 2);
    assert_eq!(panel.deselect_same_extension_as_cursor(), 0);
    assert!(panel.is_selected(&folder));
    assert!(panel.is_selected(&root.path().join("other.txt")));
    assert!(panel.is_selected(&root.path().join("keep.bin")));
    assert_eq!(panel.selected_count(), 3);
    panel.set_search_query("");
    for name in ["README", "keep-folder.txt"] {
        panel.focus_child(name.into());
        assert_eq!(panel.deselect_same_extension_as_cursor(), 0);
    }
    panel.set_cursor(0);
    assert_eq!(panel.deselect_same_extension_as_cursor(), 0);
    assert_eq!(panel.selected_count(), 3);
}

#[test]
fn permission_targets_keep_explicit_parent_child_and_missing_foreign_picks() {
    let temp = TempDir::new();
    let folder = temp.dir("parent");
    let child = folder.join("child.txt");
    fs::write(&child, "bytes").unwrap();
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.toggle_select(folder.clone());
    panel.navigate_to(folder.clone());
    panel.refresh();
    panel.toggle_select(child.clone());
    let targets = panel.permission_targets().unwrap();
    assert_eq!(
        targets.len(),
        2,
        "non-recursive edits must retain nested picks"
    );
    let parent = targets.iter().find(|target| target.path == folder).unwrap();
    assert!(parent.capture_on_worker);
    let child_target = targets.iter().find(|target| target.path == child).unwrap();
    assert!(!child_target.capture_on_worker);
    assert!(matches!(
        child_target.identity,
        ListingIdentity::Captured(_)
    ));
    // A vanished foreign selection stays visible to the inspection worker.
    fs::remove_file(&child).unwrap();
    fs::remove_dir(&folder).unwrap();
    let targets = panel.permission_targets().unwrap();
    assert_eq!(targets.len(), 2);
    let plan = crate::permissions::inspect(targets, &|| false).unwrap();
    assert!(!plan.ready());
    assert_eq!(plan.rows().len(), 2);
    assert!(plan.rows().iter().all(|row| row.error.is_some()));
}

/// In-memory entry for pure sort/filter/selection tests.
fn entry(name: &str, is_dir: bool, size: u64) -> FileEntry {
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
        sort_key: diacritic_sort_key(&name.to_lowercase()),
        path: PathBuf::from(format!("/test/{name}")),
        identity: ListingIdentity::Unavailable,
        is_dir,
        size,
        extension: String::new(),
        modified: None,
        modified_str: "–".to_string(),
        size_str: if is_dir {
            "…".to_string()
        } else {
            format_size(size)
        },
    }
}

#[cfg(target_os = "linux")]
#[test]
fn child_vanishing_between_readdir_and_stat_is_not_a_partial_read() {
    let temp = TempDir::new();
    temp.file("kept.txt", "k");
    let gone = temp.file("gone.txt", "g");
    // The first `next` buffers this small directory's names; the file
    // then disappears before its metadata is read.
    let mut entries = fs::read_dir(temp.path()).unwrap().peekable();
    let _ = entries.peek();
    fs::remove_file(gone).unwrap();

    let DirectoryRead::Complete(rows) = PanelState::read_dir_entries(entries, true) else {
        panic!("a vanished child must not make the read partial");
    };
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, vec!["kept.txt"]);
}

#[test]
fn picked_entries_drop_rows_inside_a_picked_folder() {
    let tmp = TempDir::new();
    let folder = tmp.dir("folder");
    let inner = tmp.file("folder/inner.txt", "a");
    let other = tmp.file("other.txt", "b");

    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    p.toggle_select(folder.clone());
    p.toggle_select(other.clone());
    p.navigate_to(folder.clone());
    p.toggle_select(inner);

    let mut picked: Vec<PathBuf> = p
        .picked_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    picked.sort();
    assert_eq!(picked, vec![folder, other]);
}

#[cfg(unix)]
#[test]
fn tree_subdirs_include_folder_symlinks() {
    let temp = TempDir::new();
    let target = temp.dir("target");
    temp.file("file.txt", "f");
    let link = temp.path().join("link");
    std::os::unix::fs::symlink("target", &link).unwrap();
    std::os::unix::fs::symlink("file.txt", temp.path().join("file-link")).unwrap();

    assert_eq!(PanelState::subdirs(temp.path(), false), vec![link, target]);
}

#[test]
fn tree_subdirs_use_the_file_lists_natural_order() {
    let temp = TempDir::new();
    let dir10 = temp.dir("dir10");
    let dir2 = temp.dir("dir2");
    let alpha = temp.dir("Alpha");
    temp.dir(".hidden");
    temp.file("file.txt", "");

    assert_eq!(
        PanelState::subdirs(temp.path(), false),
        vec![alpha, dir2, dir10]
    );
}

#[cfg(unix)]
#[test]
fn selection_elsewhere_reads_folder_symlinks_like_the_listing() {
    let temp = TempDir::new();
    let target = temp.dir("target");
    let link = temp.path().join("folder-link");
    std::os::unix::fs::symlink("target", &link).unwrap();

    let mut p = PanelState::new(temp.path().to_path_buf());
    p.refresh();
    p.toggle_select(link.clone());
    p.navigate_to(target);

    let picked = p.picked_entries().unwrap();
    let entry = picked.iter().find(|entry| entry.path == link).unwrap();
    assert!(entry.is_dir, "a folder link is a folder row");
    assert!(entry.is_symlink(), "identity stays the link itself");
}

#[cfg(unix)]
#[test]
fn listing_keeps_symlinks_lexically_bound_without_losing_directory_ux() {
    let temp = TempDir::new();
    temp.dir("target");
    std::os::unix::fs::symlink("target", temp.path().join("folder-link")).unwrap();
    std::os::unix::fs::symlink("missing", temp.path().join("broken-link")).unwrap();

    let DirectoryRead::Complete(entries) = PanelState::read_dir(temp.path(), true) else {
        panic!("symlink listing should be complete");
    };
    let folder = entries
        .iter()
        .find(|entry| entry.name == "folder-link")
        .expect("directory symlink");
    assert!(folder.is_dir, "working directory links remain navigable");
    assert!(matches!(
        &folder.identity,
        ListingIdentity::Captured(identity)
            if identity.kind == Some(crate::path_identity::PathKind::Symlink)
    ));

    let broken = entries
        .iter()
        .find(|entry| entry.name == "broken-link")
        .expect("broken symlink");
    assert!(!broken.is_dir);
    assert!(matches!(
        &broken.identity,
        ListingIdentity::Captured(identity)
            if identity.kind == Some(crate::path_identity::PathKind::Symlink)
    ));
    // Not sized by the length of the link text ("missing" = 7 B).
    assert_eq!(broken.size, 0);
    assert_eq!(broken.size_display(), "–");

    // Get Info says what the rows are, and where a working link points.
    let card = make_info(folder, None, None);
    assert_eq!(card.kind, "Symlink to Folder");
    assert!(
        card.path.ends_with("folder-link -> target"),
        "{}",
        card.path
    );
    assert_eq!(make_info(broken, None, None).kind, "Broken symlink");
}

fn panel_with(entries: Vec<FileEntry>) -> PanelState {
    let mut p = PanelState::new(PathBuf::from("/test"));
    p.replace_entries_for_test(entries);
    p
}

#[test]
fn dragging_an_unselected_row_does_not_use_the_stale_selection() {
    let mut panel = panel_with(vec![
        entry("selected.txt", false, 1),
        entry("dragged.txt", false, 1),
    ]);
    panel.select_path(PathBuf::from("/test/selected.txt"));

    panel.begin_drag(PathBuf::from("/test/dragged.txt"));

    assert_eq!(panel.drag.entries(), [PathBuf::from("/test/dragged.txt")]);
}

#[test]
fn dragging_a_selected_row_uses_the_visible_selection() {
    let mut panel = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 1)]);
    panel.extend_selection([PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]);

    panel.begin_drag(PathBuf::from("/test/b.txt"));

    assert_eq!(
        panel.drag.entries(),
        [PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]
    );
}

#[test]
fn modified_dates_outside_chronos_range_show_as_unknown() {
    use std::time::Duration;
    let epoch = SystemTime::UNIX_EPOCH;
    assert!(format_modified(epoch + Duration::from_secs(86_400 * 365)).is_some());
    // Before the epoch, including a fractional second.
    assert!(format_modified(epoch - Duration::from_millis(1_500)).is_some());
    // A million years out: storable on Unix filesystems, beyond chrono.
    // (Windows' `SystemTime` cannot hold it, so there is nothing to test.)
    for far in [
        epoch.checked_add(Duration::from_secs(1 << 45)),
        epoch.checked_sub(Duration::from_secs(1 << 45)),
        epoch.checked_add(Duration::from_secs(i64::MAX as u64)),
    ]
    .into_iter()
    .flatten()
    {
        assert_eq!(format_modified(far), None);
    }
}

#[test]
fn format_size_units() {
    assert_eq!(format_size(500), "500 B");
    assert_eq!(format_size(1536), "1.5 KB");
    assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    // Just below a boundary rounds up into the next unit.
    assert_eq!(format_size(1024 * 1024 - 1), "1.0 MB");
    assert_eq!(format_size(1024 * 1024 * 1024 - 1), "1.00 GB");
    assert_eq!(format_size(1024 * 1024 - 52), "1023.9 KB");
    // Terabytes and beyond get their own units.
    let tib = 1024_u64.pow(4);
    assert_eq!(format_size(5 * tib), "5.00 TB");
    assert_eq!(format_size(tib - 1), "1.00 TB");
    assert_eq!(format_size(3 * 1024 * tib), "3.00 PB");
    assert_eq!(format_size(u64::MAX), "16384.00 PB");
}

#[cfg(unix)]
#[test]
fn hardlink_selection_preserves_hidden_picks_excludes_copies_and_links_and_uses_snapshot() {
    let dir = crate::testutil::TempDir::new();
    let original = dir.file("hard-original.txt", "bytes");
    let alias = dir.path().join("hard-alias.txt");
    std::fs::hard_link(&original, &alias).unwrap();
    dir.file("hard-copy.txt", "bytes");
    let hidden = dir.file("notes.md", "hidden");
    std::os::unix::fs::symlink("hard-original.txt", dir.path().join("hard-link.txt")).unwrap();
    let mut p = PanelState::new(dir.path().to_path_buf());
    p.refresh();
    p.set_search_query("hard-");
    p.focus_child("hard-original.txt".into());
    p.replace_selection([hidden.clone()]);
    assert_eq!(p.select_cursor_hardlink_group(), 2);
    assert_eq!(p.select_cursor_hardlink_group(), 0);
    assert_eq!(p.selected_count(), 3);
    assert!(p.is_selected(&original) && p.is_selected(&alias) && p.is_selected(&hidden));
    assert_eq!(p.cursor_entry().unwrap().path, original);
    assert!(!p.is_selected(&dir.path().join("hard-copy.txt")));
    assert!(!p.is_selected(&dir.path().join("hard-link.txt")));
    assert_eq!(p.clear_visible_selection(), 2);
    p.set_search_query("hard-original");
    assert_eq!(p.select_cursor_hardlink_group(), 0);
    assert_eq!(p.selected_count(), 1);
    p.set_search_query("hard-");
    p.focus_child("hard-original.txt".into());
    std::fs::remove_file(&original).unwrap();
    std::fs::remove_file(&alias).unwrap();
    assert_eq!(p.select_cursor_hardlink_group(), 2);
    p.set_cursor(0);
    assert_eq!(p.select_cursor_hardlink_group(), 0);
}

#[test]
fn hardlink_selection_requires_complete_regular_identity_and_same_device() {
    let mut a = entry("a", false, 1);
    let mut b = entry("b", false, 1);
    let identity = |path: PathBuf, device, file_id| {
        ListingIdentity::Captured(crate::path_identity::PathIdentity {
            path,
            exists: true,
            kind: Some(crate::path_identity::PathKind::File),
            volume: device,
            file_id,
            size: 1,
            modified_nanos: None,
            tree_fingerprint: None,
            observed_version: 0,
        })
    };
    for (device, file_id, expected) in [
        (Some(9), Some(u64::MAX), 2),
        (Some(8), Some(u64::MAX), 0),
        (None, Some(u64::MAX), 0),
        (Some(9), None, 0),
    ] {
        a.identity = identity(a.path.clone(), Some(9), Some(u64::MAX));
        b.identity = identity(b.path.clone(), device, file_id);
        let mut p = panel_with(vec![a.clone(), b.clone()]);
        p.focus_child("a".into());
        assert_eq!(p.select_cursor_hardlink_group(), expected);
    }
    for kind in [
        crate::path_identity::PathKind::Directory,
        crate::path_identity::PathKind::Symlink,
        crate::path_identity::PathKind::Other,
    ] {
        a.identity = identity(a.path.clone(), Some(9), Some(3));
        if let ListingIdentity::Captured(i) = &mut a.identity {
            i.kind = Some(kind);
        }
        assert_eq!(a.regular_file_id(), None);
    }
    a.identity = ListingIdentity::Unavailable;
    assert_eq!(a.regular_file_id(), None);
}

#[test]
fn metadata_change_time_signed_parts_preserve_fraction_and_reject_invalid_nanos() {
    use std::time::{Duration, UNIX_EPOCH};
    assert_eq!(unix_time_parts(0, 0), Some(UNIX_EPOCH));
    assert_eq!(
        unix_time_parts(-1, 999_999_999),
        UNIX_EPOCH.checked_sub(Duration::from_nanos(1))
    );
    assert_eq!(
        unix_time_parts(-1, 0),
        UNIX_EPOCH.checked_sub(Duration::from_secs(1))
    );
    assert_eq!(
        unix_time_parts(1, 123),
        UNIX_EPOCH.checked_add(Duration::new(1, 123))
    );
    for nanos in [-1, 1_000_000_000, i64::MAX] {
        assert_eq!(unix_time_parts(0, nanos), None);
    }
    let _ = unix_time_parts(i64::MIN, 0);
    let _ = unix_time_parts(i64::MAX, 999_999_999);
}

#[cfg(unix)]
#[test]
fn metadata_change_time_listing_info_and_links_keep_lexical_facts() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = crate::testutil::TempDir::new();
    let file = dir.file("file", "content");
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    std::os::unix::fs::symlink("file", dir.path().join("link")).unwrap();
    std::os::unix::fs::symlink("absent", dir.path().join("dangling")).unwrap();
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        before
    );
    let mut panel = PanelState::new(dir.path().to_path_buf());
    panel.refresh();
    for name in ["file", "folder", "link", "dangling"] {
        let path = dir.path().join(name);
        let raw = std::fs::symlink_metadata(&path).unwrap();
        let expected = SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::new(
                raw.ctime().try_into().unwrap(),
                raw.ctime_nsec().try_into().unwrap(),
            ))
            .unwrap();
        let observed = FileEntry::observe_lexical(path).unwrap();
        let listed = panel.entries().iter().find(|e| e.name == name).unwrap();
        for row in [&observed, listed] {
            assert_eq!(row.changed, Some(expected));
            let info = make_info(row, None, None);
            assert_eq!(
                info.changed,
                format!(
                    "{}{}",
                    if row.is_symlink() {
                        "Link object: "
                    } else {
                        ""
                    },
                    row.changed_str
                )
            );
        }
    }
    let entry = panel
        .entries()
        .iter()
        .find(|e| e.name == "link")
        .unwrap()
        .clone();
    let card = make_info(&entry, None, None);
    std::fs::remove_file(dir.path().join("link")).unwrap();
    std::fs::remove_file(file).unwrap();
    assert_eq!(make_info(&entry, None, None), card);
}

#[test]
fn sort_puts_dirs_first_and_is_case_insensitive() {
    let mut p = panel_with(vec![
        entry("zeta.txt", false, 1),
        entry("Apple", true, 0),
        entry("beta.txt", false, 1),
        entry("zoo", true, 0),
    ]);
    p.sort_entries();
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
}

#[cfg(unix)]
#[test]
fn inode_sort_uses_retained_lexical_identity_and_preserves_selection_and_cursor() {
    let dir = crate::testutil::TempDir::new();
    let original = dir.file("original.txt", "bytes");
    let alias = dir.path().join("alias.txt");
    std::fs::hard_link(&original, &alias).unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink("original.txt", &link).unwrap();
    let mut panel = PanelState::new(dir.path().to_path_buf());
    panel.refresh();
    panel.focus_child("original.txt".into());
    panel.replace_selection([original.clone(), alias.clone()]);
    let keys: Vec<_> = panel
        .entries()
        .iter()
        .map(|e| (e.path.clone(), e.inode_sort_key()))
        .collect();
    assert_eq!(
        keys.iter().find(|(p, _)| p == &original).unwrap().1,
        keys.iter().find(|(p, _)| p == &alias).unwrap().1
    );
    assert_ne!(
        keys.iter().find(|(p, _)| p == &link).unwrap().1,
        keys.iter().find(|(p, _)| p == &original).unwrap().1
    );
    std::fs::remove_file(&original).unwrap();
    for _ in 0..2 {
        panel.set_sort(SortColumn::Inode);
        assert_eq!(panel.cursor_entry().unwrap().path, original);
        assert_eq!(panel.selected_count(), 2);
        assert!(panel.is_selected(&alias));
        for row in panel.entries() {
            assert_eq!(
                row.inode_sort_key(),
                keys.iter().find(|(p, _)| p == &row.path).unwrap().1
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn hard_link_sort_uses_retained_counts_and_preserves_selection_and_cursor() {
    let dir = crate::testutil::TempDir::new();
    let original = dir.file("original.txt", "bytes");
    let alias = dir.path().join("alias.txt");
    std::fs::hard_link(&original, &alias).unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink("original.txt", &link).unwrap();
    let mut panel = PanelState::new(dir.path().to_path_buf());
    panel.refresh();
    panel.focus_child("original.txt".into());
    panel.replace_selection([original.clone(), alias.clone()]);
    let keys: Vec<_> = panel
        .entries()
        .iter()
        .map(|e| (e.path.clone(), e.hard_link_count()))
        .collect();
    assert_eq!(
        keys.iter().find(|(p, _)| p == &original).unwrap().1,
        keys.iter().find(|(p, _)| p == &alias).unwrap().1
    );
    assert_ne!(
        keys.iter().find(|(p, _)| p == &link).unwrap().1,
        keys.iter().find(|(p, _)| p == &original).unwrap().1
    );
    std::fs::remove_file(&original).unwrap();
    for _ in 0..2 {
        panel.set_sort(SortColumn::HardLinks);
        assert_eq!(panel.cursor_entry().unwrap().path, original);
        assert_eq!(panel.selected_count(), 2);
        assert!(panel.is_selected(&alias));
        for row in panel.entries() {
            assert_eq!(
                row.hard_link_count(),
                keys.iter().find(|(p, _)| p == &row.path).unwrap().1
            );
        }
    }
}

#[test]
fn sort_desc_reverses_within_groups() {
    let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
    p.set_sort(SortColumn::Size); // asc
    p.set_sort(SortColumn::Size); // same column again -> desc
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["b.txt", "a.txt"]);
}

#[test]
fn descending_sort_keeps_folders_first_and_reverses_within_each_group() {
    let mut p = panel_with(vec![
        entry("small-dir", true, 1),
        entry("large-file", false, 20),
        entry("large-dir", true, 10),
        entry("small-file", false, 2),
    ]);

    p.set_sort(SortColumn::Size);
    p.set_sort(SortColumn::Size);

    let names: Vec<&str> = p
        .entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["large-dir", "small-dir", "large-file", "small-file"]
    );
}

#[test]
fn folders_first_off_sorts_dirs_inline() {
    let mut p = panel_with(vec![
        entry("zeta.txt", false, 1),
        entry("Apple", true, 0),
        entry("beta.txt", false, 1),
        entry("zoo", true, 0),
    ]);
    // Off: folders are no longer pinned, names sort as one stream.
    p.toggle_folders_first();
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Apple", "beta.txt", "zeta.txt", "zoo"]);
    // Back on: folders return to the top.
    p.toggle_folders_first();
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
}

#[test]
fn natural_sort_toggle_switches_to_ascii_order() {
    let mut p = panel_with(vec![
        entry("file10.txt", false, 1),
        entry("file2.txt", false, 1),
    ]);
    // Natural (default): file2 before file10.
    p.sort_entries();
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["file2.txt", "file10.txt"]);
    // ASCII: "file10" sorts before "file2" lexicographically.
    p.toggle_natural_sort();
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["file10.txt", "file2.txt"]);
}

#[test]
fn sort_by_extension_groups_by_ext_then_name() {
    let mut p = panel_with(vec![
        entry("b.txt", false, 1),
        entry("a.rs", false, 1),
        entry("c.txt", false, 1),
        entry("z.rs", false, 1),
    ]);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    p.set_sort(SortColumn::Extension);
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["a.rs", "z.rs", "b.txt", "c.txt"]);
}

#[test]
fn sort_by_kind_orders_image_before_doc_before_code() {
    let mut p = panel_with(vec![
        entry("main.rs", false, 1),
        entry("pic.jpg", false, 1),
        entry("doc.pdf", false, 1),
    ]);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    p.set_sort(SortColumn::Kind);
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    // Declaration order of Kind: Image, Document, Code.
    assert_eq!(names, vec!["pic.jpg", "doc.pdf", "main.rs"]);
}

#[test]
fn select_junk_picks_known_clutter_only() {
    let mut p = panel_with(vec![
        entry(".DS_Store", false, 6),
        entry("photo.jpg", false, 100),
        entry("Thumbs.db", false, 10),
        entry("notes.txt", false, 20),
    ]);
    let n = p.select_junk();
    assert_eq!(n, 2);
    let names: std::collections::HashSet<String> = p
        .selected_paths()
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    assert!(names.contains(".DS_Store"));
    assert!(names.contains("Thumbs.db"));
    assert!(!names.contains("photo.jpg"));
}

#[test]
fn select_junk_matches_case_variants() {
    let mut p = panel_with(vec![
        entry("thumbs.db", false, 10),
        entry("Desktop.ini", false, 10),
        entry("notes.txt", false, 20),
    ]);
    assert_eq!(p.select_junk(), 2);
    let names = selected_names(&p);
    assert!(names.contains("thumbs.db"));
    assert!(names.contains("Desktop.ini"));
}

fn selected_names(p: &PanelState) -> std::collections::HashSet<String> {
    p.selected_paths()
        .iter()
        .filter_map(|x| x.file_name().map(|s| s.to_string_lossy().to_string()))
        .collect()
}

#[test]
fn allocated_selection_uses_visible_known_metric_and_preserves_existing_picks() {
    let mut sparse = entry("sparse.bin", false, 1 << 30);
    sparse.allocated_size = Some(0);
    let mut dense = entry("dense.bin", false, 1);
    dense.allocated_size = Some(8192);
    let unknown = entry("unknown.bin", false, u64::MAX);
    let mut hidden = entry("hidden.txt", false, 1);
    hidden.allocated_size = Some(u64::MAX);
    let mut folder = entry("folder.bin", true, 0);
    folder.allocated_size = Some(u64::MAX);
    let mut p = panel_with(vec![sparse, dense, unknown, hidden, folder]);
    p.replace_selection([PathBuf::from("/test/hidden.txt")]);
    p.set_search_query(".bin");
    p.focus_child("sparse.bin".into());
    let cursor = p.cursor_entry().unwrap().path.clone();
    assert_eq!(p.select_largest_allocated(0), 0);
    assert_eq!(p.select_largest_allocated(1), 1);
    assert_eq!(
        selected_names(&p),
        ["dense.bin".to_string(), "hidden.txt".to_string()].into()
    );
    assert_eq!(p.select_largest_allocated(1), 0);
    assert_eq!(p.select_largest_allocated(usize::MAX), 1);
    assert!(p.is_selected(&PathBuf::from("/test/sparse.bin")));
    assert!(!p.is_selected(&PathBuf::from("/test/unknown.bin")));
    assert_eq!(p.cursor_entry().unwrap().path, cursor);
}

#[test]
fn allocated_selection_caps_at_ten_with_stable_path_ties_independent_of_view_order() {
    for order in [SortOrder::Asc, SortOrder::Desc] {
        let rows = (0..15)
            .map(|i| {
                let mut e = entry(&format!("file{i:02}"), false, 1);
                e.allocated_size = Some(4096);
                e
            })
            .collect();
        let mut p = panel_with(rows);
        if order == SortOrder::Desc {
            p.set_sort(SortColumn::Name);
        }
        assert_eq!(p.select_largest_allocated(10), 10);
        assert_eq!(
            selected_names(&p),
            (5..15).map(|i| format!("file{i:02}")).collect()
        );
    }
}

#[test]
fn select_largest_picks_top_n_by_size() {
    let mut p = panel_with(vec![
        entry("a", false, 10),
        entry("b", false, 50),
        entry("c", false, 30),
        entry("dir", true, 0),
    ]);
    assert_eq!(p.select_largest(2), 2);
    let names = selected_names(&p);
    assert!(names.contains("b")); // 50
    assert!(names.contains("c")); // 30
    assert!(!names.contains("a"));
    assert!(!names.contains("dir"));
}

#[test]
fn select_like_cursor_matches_extension() {
    let mut p = panel_with(vec![
        entry("a.rs", false, 1),
        entry("b.txt", false, 1),
        entry("c.rs", false, 1),
    ]);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    p.set_cursor(1); // first filtered entry: a.rs
    assert_eq!(p.select_same_extension_as_cursor(), 2);
    let names = selected_names(&p);
    assert!(names.contains("a.rs"));
    assert!(names.contains("c.rs"));
    assert!(!names.contains("b.txt"));
}

#[test]
fn select_empty_files_picks_zero_byte_only() {
    let mut p = panel_with(vec![
        entry("empty", false, 0),
        entry("full", false, 100),
        entry("dir", true, 0),
    ]);
    assert_eq!(p.select_empty_files(), 1);
    let names = selected_names(&p);
    assert!(names.contains("empty"));
    assert!(!names.contains("full"));
    assert!(!names.contains("dir")); // a folder is never "empty file"
}

#[test]
fn reverse_sort_flips_order() {
    let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
    p.sort_entries(); // Name asc: a, b
    p.reverse_sort(); // -> desc: b, a
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["b.txt", "a.txt"]);
}

#[test]
fn listing_entries_uses_selection_when_present() {
    let mut p = panel_with(vec![
        entry("a", false, 1),
        entry("b", false, 2),
        entry("c", false, 3),
    ]);
    // Nothing selected: the whole filtered view.
    assert_eq!(p.listing_entries().len(), 3);
    // With a selection: only the selected rows, in display order.
    p.select_path(PathBuf::from("/test/b"));
    let names: Vec<String> = p.listing_entries().iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["b"]);
}

#[test]
fn filter_matches_case_insensitively() {
    let mut p = panel_with(vec![
        entry("Cargo.toml", false, 1),
        entry("main.rs", false, 1),
    ]);
    p.set_search_query("CARGO");
    let names: Vec<&str> = p
        .filtered_entries()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, vec!["Cargo.toml"]);
}

#[test]
fn whitespace_only_filter_is_inactive_and_matches_everything() {
    let mut p = panel_with(vec![
        entry("Cargo.toml", false, 1),
        entry("main.rs", false, 1),
    ]);
    p.set_search_query("   ");
    assert!(!filter_is_active(p.search_query(), &p.facets()));
    assert_eq!(p.filtered_count(), 2);
}

#[test]
fn filter_is_a_fuzzy_subsequence() {
    let mut p = panel_with(vec![
        entry("scanner.rs", false, 1),
        entry("main.rs", false, 1),
        entry("notes.txt", false, 1),
    ]);
    // "scn" is a subsequence of scanner.rs only (substring would miss it).
    p.set_search_query("scn");
    let names: Vec<&str> = p
        .filtered_entries()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, vec!["scanner.rs"]);

    // Empty query shows everything.
    p.set_search_query("");
    assert_eq!(p.filtered_count(), 3);

    // A non-subsequence excludes every row.
    p.set_search_query("zzz");
    assert_eq!(p.filtered_count(), 0);
}

#[test]
fn selection_view_key_changes_only_with_what_the_hud_shows() {
    let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
    let path = p.entries()[0].path.clone();
    let key = p.selection_view_key();
    assert_eq!(p.selection_view_key(), key, "idle frames reuse the key");

    p.toggle_select(path.clone());
    let selected = p.selection_view_key();
    assert_ne!(selected, key);
    p.set_search_query("b");
    let filtered = p.selection_view_key();
    assert_ne!(filtered, selected);
    p.set_search_query("");
    p.clear_selection();
    assert_ne!(p.selection_view_key(), filtered);
    let cleared = p.selection_view_key();
    p.replace_entries_for_test(vec![entry("c", false, 1)]);
    assert_ne!(p.selection_view_key(), cleared);
}

#[test]
fn toggle_select_adds_then_removes() {
    let mut p = panel_with(vec![entry("a", false, 1)]);
    let path = p.entries()[0].path.clone();
    p.toggle_select(path.clone());
    assert!(p.is_selected(&path));
    p.toggle_select(path.clone());
    assert!(!p.is_selected(&path));
}

#[test]
fn select_all_toggles_between_all_and_none() {
    let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
    p.select_all();
    assert_eq!(p.selected_count(), 2);
    p.select_all();
    assert!(p.selection_is_empty());
}

#[test]
fn select_all_preserves_selection_hidden_by_filter() {
    let mut p = panel_with(vec![
        entry("alpha", false, 1),
        entry("album", false, 1),
        entry("zebra", false, 1),
    ]);
    let alpha = p.entries()[0].path.clone();
    let album = p.entries()[1].path.clone();
    let zebra = p.entries()[2].path.clone();
    p.select_path(zebra.clone());
    p.set_search_query("al");

    p.select_all();
    assert!(p.is_selected(&alpha));
    assert!(p.is_selected(&album));
    assert!(p.is_selected(&zebra));

    p.select_all();
    assert!(!p.is_selected(&alpha));
    assert!(!p.is_selected(&album));
    assert!(p.is_selected(&zebra));
}

#[test]
fn conjunctive_filters_keep_excluded_picks_and_combine_with_facets() {
    let temp = crate::testutil::TempDir::new();
    temp.file("main.rs", "x");
    let excluded = temp.file("test.rs", "x");
    temp.file("Cargo.toml", "x");
    let folder = temp.dir("source.rs");
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.replace_selection([excluded.clone()]);
    panel.set_search_query(".rs$ && !^test");
    assert_eq!(panel.filtered_count(), 2);
    assert!(panel.is_selected(&excluded));
    panel.set_facets(FacetSet {
        kind: Some(KindFacet::Folders),
        ..Default::default()
    });
    assert_eq!(panel.filtered_count(), 1);
    assert_eq!(panel.filtered_get(0).unwrap().path, folder);
    panel.set_search_query(".rs$ &&");
    assert_eq!(panel.filtered_count(), 0);
    assert!(panel.is_selected(&excluded));
}

#[test]
fn alternative_filters_compose_with_facets_without_losing_excluded_picks() {
    let temp = crate::testutil::TempDir::new();
    temp.file("main.rs", "x");
    temp.file("Cargo.toml", "x");
    let excluded = temp.file("notes.txt", "x");
    let folder = temp.dir("source.rs");
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.replace_selection([excluded.clone()]);
    panel.set_search_query(".rs$ | .toml$");
    assert_eq!(panel.filtered_count(), 3);
    assert!(panel.is_selected(&excluded));
    panel.set_facets(FacetSet {
        kind: Some(KindFacet::Folders),
        ..Default::default()
    });
    assert_eq!(panel.filtered_count(), 1);
    assert_eq!(panel.filtered_get(0).unwrap().path, folder);
    panel.set_search_query(".rs$ |");
    assert_eq!(panel.filtered_count(), 0);
    assert!(panel.is_selected(&excluded));
}

#[test]
fn negated_filter_keeps_excluded_selection_and_composes_with_facets_and_export() {
    let temp = crate::testutil::TempDir::new();
    let excluded = temp.file("skip.tmp", "x");
    temp.file("keep.rs", "x");
    temp.file("other.tmp.bak", "x");
    let folder = temp.dir("tmp-dir");
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.replace_selection([excluded.clone()]);
    panel.set_search_query("!.tmp$");
    assert_eq!(panel.filtered_count(), 3);
    assert!(panel.is_selected(&excluded));
    let query = crate::query::from_panel_filter(panel.search_query(), &panel.facets());
    for entry in panel.entries() {
        assert_eq!(
            crate::search::query_matches_metadata(&query, entry, std::time::SystemTime::UNIX_EPOCH),
            entry.path != excluded
        );
    }
    panel.set_facets(FacetSet {
        kind: Some(KindFacet::Folders),
        ..Default::default()
    });
    assert_eq!(panel.filtered_count(), 1);
    assert_eq!(panel.filtered_get(0).unwrap().path, folder);
    panel.set_search_query("!^tmp");
    assert_eq!(panel.filtered_count(), 0);
    assert!(panel.is_selected(&excluded));
}

#[test]
fn pane_filter_export_agrees_with_unicode_and_fuzzy_candidates() {
    for text in [
        "^i.txt$", "^σ.txt$", "^ς.txt$", "'i", "ab", "'a b ", "!^i.txt$", "!σ$", "!ab",
    ] {
        let query = crate::query::from_panel_filter(text, &FacetSet::default());
        for name in ["İ.txt", "σ.txt", "ς.txt", "a-b.txt", "ab.txt", "a b .txt"] {
            let candidate = entry(name, false, 1);
            assert_eq!(
                crate::search::query_matches_metadata(
                    &query,
                    &candidate,
                    std::time::SystemTime::UNIX_EPOCH
                ),
                crate::name_filter::is_match(text, name),
                "query {text:?}, name {name:?}"
            );
        }
    }
}

#[test]
fn anchored_filter_and_exported_query_agree_for_live_listing() {
    let temp = crate::testutil::TempDir::new();
    let picked = temp.file("xab.txt", "x");
    temp.file("ab.txt", "x");
    temp.file("ab.txt.bak", "x");
    temp.file("a-b.txt", "x");
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.replace_selection([picked.clone()]);
    for text in ["^ab", ".txt$", "^ab.txt$", "^$", "'^ab$"] {
        panel.set_search_query(text);
        let query = crate::query::from_panel_filter(text, &panel.facets());
        let expected: Vec<_> = panel
            .entries()
            .iter()
            .filter(|entry| {
                crate::search::query_matches_metadata(&query, entry, std::time::SystemTime::now())
            })
            .map(|entry| entry.path.clone())
            .collect();
        let actual: Vec<_> = panel
            .filtered_iter()
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(actual, expected, "query {text}");
        assert!(panel.is_selected(&picked));
    }
    panel.set_search_query("^ab.txt$");
    assert_eq!(panel.filtered_count(), 1);
    assert_eq!(panel.filtered_get(0).unwrap().name, "ab.txt");
}

#[test]
fn literal_name_filter_composes_with_facets_and_keeps_excluded_picks() {
    let temp = crate::testutil::TempDir::new();
    let separated = temp.file("a-b.txt", "x");
    temp.file("ab.txt", "x");
    let folder = temp.dir("ab-folder");
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.replace_selection([separated.clone()]);
    panel.set_search_query("ab");
    assert_eq!(panel.filtered_count(), 3);
    panel.set_search_query("'ab");
    assert_eq!(panel.filtered_count(), 2);
    assert!(panel.is_selected(&separated));
    panel.set_facets(FacetSet {
        kind: Some(KindFacet::Folders),
        ..Default::default()
    });
    assert_eq!(panel.filtered_count(), 1);
    assert_eq!(panel.filtered_get(0).unwrap().path, folder);
    panel.set_facets(FacetSet::default());
    panel.set_search_query("'a-b");
    assert_eq!(panel.filtered_count(), 1);
    assert_eq!(panel.filtered_get(0).unwrap().path, separated);
}

#[test]
fn visible_clear_keeps_nonmatches_elsewhere_and_saved_memory() {
    let temp = crate::testutil::TempDir::new();
    let visible = temp.file("keep.txt", "x");
    let visible_dir = temp.dir("keep-dir");
    let hidden = temp.file("hidden.txt", "x");
    let other = temp.dir("other");
    let elsewhere = temp.file("other/selected.txt", "x");
    let mut panel = PanelState::new(other);
    panel.refresh();
    panel.replace_selection([elsewhere]);
    panel.navigate_to(temp.path().into());
    panel.refresh();
    let picks = HashSet::from([visible, visible_dir, hidden.clone()]);
    panel.replace_selection(picks.iter().cloned());
    panel.save_selection();
    panel.set_search_query("keep");
    let cursor = panel.cursor();
    assert_eq!(panel.clear_visible_selection(), 2);
    assert_eq!(panel.clear_visible_selection(), 0);
    assert_eq!(panel.selected_paths(), &HashSet::from([hidden]));
    assert_eq!(panel.selected_elsewhere_count(), 1);
    assert_eq!(panel.cursor(), cursor);
    assert_eq!(panel.search_query(), "keep");
    assert!(panel.restore_saved_selection());
    assert_eq!(panel.selected_paths(), &picks);
    assert_eq!(panel.selected_elsewhere_count(), 1);
    panel.set_search_query("no match");
    assert_eq!(panel.clear_visible_selection(), 0);
    assert_eq!(panel.selected_paths(), &picks);
}

#[test]
fn selected_navigation_is_directional_filtered_and_keeps_picks() {
    let temp = crate::testutil::TempDir::new();
    temp.file("a-visible", "x");
    let b = temp.file("b-visible", "x");
    let hidden = temp.file("c-hidden", "x");
    temp.file("d-visible", "x");
    let e = temp.dir("e-visible");
    let mut panel = PanelState::new(temp.path().into());
    panel.refresh();
    panel.toggle_folders_first();
    panel.replace_selection([b.clone(), hidden, e.clone()]);
    panel.set_search_query("visible");
    let picks = panel.selected_paths().clone();
    for (forward, expected) in [
        (true, [Some(2), Some(2), Some(4), Some(4), None]),
        (false, [None, None, None, Some(2), Some(2)]),
    ] {
        for (cursor, destination) in expected.into_iter().enumerate() {
            panel.set_cursor(cursor);
            panel.set_scroll_to_cursor(false);
            assert_eq!(panel.jump_to_selected(forward), destination.is_some());
            assert_eq!(panel.cursor(), destination.unwrap_or(cursor));
            assert_eq!(panel.scroll_to_cursor(), destination.is_some());
            assert_eq!(panel.selected_paths(), &picks);
        }
    }
    panel.set_sort(SortColumn::Name);
    panel.set_cursor(0);
    assert!(panel.jump_to_selected(true));
    assert_eq!(panel.cursor_entry().unwrap().path, e);
    assert!(panel.jump_to_selected(true));
    assert_eq!(panel.cursor_entry().unwrap().path, b);
    assert!(!panel.jump_to_selected(true));
    panel.set_search_query("no match");
    assert!(!panel.jump_to_selected(true));
    assert!(!panel.jump_to_selected(false));
    assert_eq!(panel.selected_paths(), &picks);
}

#[test]
fn kind_deselection_keeps_filtered_and_other_folder_picks() {
    let temp = crate::testutil::TempDir::new();
    let file = temp.file("keep.txt", "x");
    let hidden_file = temp.file("hidden.txt", "x");
    let folder = temp.dir("keep-dir");
    let hidden_folder = temp.dir("hidden-dir");
    let elsewhere = temp.file("hidden-dir/elsewhere.txt", "x");
    let mut panel = PanelState::new(hidden_folder.clone());
    panel.refresh();
    panel.replace_selection([elsewhere]);
    panel.navigate_to(temp.path().into());
    panel.refresh();
    panel.replace_selection([
        file.clone(),
        hidden_file.clone(),
        folder.clone(),
        hidden_folder.clone(),
    ]);
    panel.set_search_query("keep");
    let cursor = panel.cursor();
    assert_eq!(panel.deselect_visible_kind(false), 1);
    assert_eq!(panel.deselect_visible_kind(false), 0);
    assert_eq!(
        panel.selected_paths(),
        &HashSet::from([hidden_file.clone(), folder, hidden_folder.clone()])
    );
    assert_eq!(panel.deselect_visible_kind(true), 1);
    assert_eq!(panel.deselect_visible_kind(true), 0);
    assert_eq!(
        panel.selected_paths(),
        &HashSet::from([hidden_file, hidden_folder])
    );
    assert_eq!(panel.selected_elsewhere_count(), 1);
    assert_eq!(panel.cursor(), cursor);
    assert_eq!(panel.search_query(), "keep");
}

#[test]
fn mask_remove_invert_preserve_exclusions_filtered_picks_and_folder_memory() {
    use crate::name_list::Action;
    let temp = crate::testutil::TempDir::new();
    let a = temp.file("a.rs", "a");
    let b = temp.file("b.rs", "b");
    let protected = temp.file("test.rs", "t");
    let other = temp.file("other.txt", "o");
    let folder = temp.dir("folder.rs");
    let elsewhere = temp.file("folder.rs/elsewhere", "e");
    let mut p = PanelState::new(folder.clone());
    p.refresh();
    p.select_path(elsewhere);
    p.navigate_to(temp.path().into());
    p.refresh();
    p.replace_selection([a.clone(), protected.clone(), other.clone(), folder.clone()]);
    let options = SelectionMaskOptions {
        target: SelectionTarget::Files,
        ..Default::default()
    };
    for action in [Action::Remove, Action::Invert, Action::Invert] {
        let preview = p.mask_action_count("*.rs;!test*", options, action);
        assert_eq!(p.apply_mask_action("*.rs;!test*", options, action), preview);
        assert!(p.is_selected(&protected) && p.is_selected(&other) && p.is_selected(&folder));
        assert_eq!(p.selected_elsewhere_count(), 1);
    }
    assert!(!p.is_selected(&a) && !p.is_selected(&b));
    p.set_search_query("a.rs");
    assert_eq!(p.apply_mask_action("*.rs", options, Action::Invert), 1);
    assert!(p.is_selected(&a) && !p.is_selected(&b));
    for mask in ["", "!*.rs", "   "] {
        assert_eq!(p.apply_mask_action(mask, options, Action::Invert), 0);
        assert_eq!(p.mask_action_count(mask, options, Action::Remove), 0);
    }
}

#[test]
fn mask_actions_share_case_accents_and_extensionless_options() {
    use crate::name_list::Action;
    let temp = crate::testutil::TempDir::new();
    let accented = temp.file("École.TXT", "x");
    let plain = temp.file("README", "x");
    let folder = temp.dir("École-folder");
    let mut p = PanelState::new(temp.path().into());
    p.refresh();
    let mut options = SelectionMaskOptions {
        target: SelectionTarget::Files,
        case_sensitive: true,
        ..Default::default()
    };
    assert_eq!(p.apply_mask_action("école.txt", options, Action::Invert), 0);
    options.case_sensitive = false;
    options.ignore_accents = true;
    assert_eq!(p.mask_action_count("ecole.txt", options, Action::Invert), 1);
    assert_eq!(p.apply_mask_action("ecole.txt", options, Action::Invert), 1);
    assert!(p.is_selected(&accented));
    assert_eq!(p.apply_mask_action("ecole.txt", options, Action::Remove), 1);
    options.extensionless_star = true;
    assert_eq!(p.apply_mask_action("*.*", options, Action::Invert), 2);
    assert!(p.is_selected(&plain) && p.is_selected(&accented) && !p.is_selected(&folder));
    options.target = SelectionTarget::Folders;
    assert_eq!(p.apply_mask_action("ecole*", options, Action::Invert), 1);
    assert!(p.is_selected(&folder));
}

#[test]
fn names_remove_and_invert_counts_preserve_filtered_kinds_and_other_folders() {
    use crate::name_list::Action;
    let temp = crate::testutil::TempDir::new();
    let a = temp.file("a.txt", "a");
    let b = temp.file("b.txt", "b");
    let hidden = temp.file("hidden.txt", "h");
    let folder = temp.dir("folder");
    let elsewhere = temp.file("folder/elsewhere", "x");
    let mut p = PanelState::new(folder.clone());
    p.refresh();
    p.select_path(elsewhere.clone());
    p.navigate_to(temp.path().into());
    p.refresh();
    p.replace_selection([a.clone(), hidden.clone(), folder.clone()]);
    p.set_search_query(".txt");
    let names = "a.txt\nb.txt\nfolder\nmissing\na.txt";
    for action in [Action::Remove, Action::Invert, Action::Add] {
        let preview = p
            .names_action_count(names, SelectionTarget::Files, action)
            .unwrap();
        let changed = p
            .apply_names(names, SelectionTarget::Files, action)
            .unwrap();
        assert_eq!(preview, changed);
        assert!(p.is_selected(&folder));
        assert!(p.is_selected(&hidden));
        assert_eq!(p.selected_elsewhere_count(), 1);
    }
    assert!(p.is_selected(&a) && p.is_selected(&b));
    let before = p.selected_paths().clone();
    for action in [Action::Remove, Action::Invert] {
        assert!(
            p.apply_names("a.txt\n../bad", SelectionTarget::Both, action)
                .is_err()
        );
        assert_eq!(p.selected_paths(), &before);
    }
    p.set_search_query("a.txt");
    assert_eq!(
        p.apply_names("a.txt\nb.txt", SelectionTarget::Both, Action::Invert)
            .unwrap(),
        1
    );
    assert!(!p.is_selected(&a) && p.is_selected(&b));
    p.set_search_query("");
    assert_eq!(
        p.apply_names("folder", SelectionTarget::Folders, Action::Remove)
            .unwrap(),
        1
    );
    assert!(!p.is_selected(&folder));
    assert_eq!(p.selected_elsewhere_count(), 1);
}

#[test]
fn tab_selection_is_independent_folder_bound_and_rejects_over_budget_replacement() {
    let temp = crate::testutil::TempDir::new();
    let a = temp.file("a.txt", "a");
    let other = temp.dir("other");
    let mut p = PanelState::new(temp.path().into());
    p.refresh();
    p.select_path(a.clone());
    p.save_tab_selection().unwrap();
    let old = p.tab_selection.clone();
    p.clear_selection();
    p.save_selection();
    assert!(p.restore_tab_selection());
    assert!(p.is_selected(&a));
    p.clear_selection();
    assert!(p.restore_saved_selection());
    assert_eq!(p.selected_count(), 0);
    for i in 0..1025 {
        p.select_path(temp.path().join(format!("synthetic{i}")));
    }
    assert!(p.save_tab_selection().is_err());
    assert_eq!(p.tab_selection, old);
    p.navigate_to(other);
    assert!(!p.can_restore_tab_selection());
    assert!(!p.restore_tab_selection());
}
#[test]
fn saved_selection_restores_filtered_picks_and_prunes_missing_entries() {
    let root = crate::testutil::TempDir::new();
    let a = root.file("alpha.txt", "a");
    let b = root.file("beta.txt", "b");
    let c = root.file("gamma.txt", "c");
    let mut panel = PanelState::new(root.path().into());
    panel.refresh();
    assert!(!panel.can_restore_selection());
    panel.replace_selection([a.clone(), b.clone()]);
    panel.set_search_query("alpha");
    assert!(panel.save_selection());
    panel.replace_selection([c.clone()]);
    assert!(panel.restore_saved_selection());
    assert_eq!(
        panel.selected_paths(),
        &HashSet::from([a.clone(), b.clone()])
    );
    std::fs::remove_file(&b).unwrap();
    panel.refresh();
    panel.replace_selection([c]);
    assert!(panel.restore_saved_selection());
    assert_eq!(panel.selected_paths(), &HashSet::from([a]));
    panel.clear_selection();
    assert!(panel.save_selection());
    panel.select_all();
    assert!(panel.restore_saved_selection());
    assert!(panel.selection_is_empty());
}

#[test]
fn saved_selection_is_folder_bound_preserves_elsewhere_and_rejects_incomplete_reads() {
    let root = crate::testutil::TempDir::new();
    let a = root.file("alpha.txt", "a");
    let child = root.dir("child");
    let b = root.file("child/beta.txt", "b");
    let mut panel = PanelState::new(root.path().into());
    panel.refresh();
    panel.replace_selection([a.clone()]);
    assert!(panel.save_selection());
    panel.navigate_to(child);
    panel.refresh();
    panel.replace_selection([b]);
    assert!(!panel.can_restore_selection());
    assert!(!panel.restore_saved_selection());
    panel.navigate_to(root.path().into());
    panel.refresh();
    panel.clear_selection();
    let elsewhere = panel.selected_elsewhere_count();
    assert_eq!(elsewhere, 1);
    assert!(panel.restore_saved_selection());
    assert!(panel.is_selected(&a));
    assert_eq!(panel.selected_elsewhere_count(), elsewhere);
    for status in [DirStatus::Partial, DirStatus::Denied, DirStatus::Gone] {
        panel.apply_directory_read(DirectoryRead::Incomplete(status));
        assert!(!panel.can_save_selection());
        assert!(!panel.save_selection());
        assert!(!panel.restore_saved_selection());
        assert!(panel.is_selected(&a));
    }
    panel.refresh();
    panel.clear_selection();
    assert!(panel.restore_saved_selection());
    assert!(panel.is_selected(&a));
}

#[test]
fn visible_selection_metrics_do_not_count_filtered_out_items() {
    let mut panel = panel_with(vec![entry("alpha", false, 10), entry("zebra", false, 90)]);
    panel.extend_selection([PathBuf::from("/test/alpha"), PathBuf::from("/test/zebra")]);
    panel.set_search_query("alpha");

    assert_eq!(panel.selected_count(), 2);
    assert_eq!(panel.visible_selected_count(), 1);
    assert_eq!(panel.visible_selected_size(), 10);
    assert_eq!(
        panel.selected_or_cursor().unwrap()[0].path,
        PathBuf::from("/test/alpha")
    );
}

#[test]
fn incomplete_snapshot_preserves_selection_and_reconciliation() {
    let mut panel = panel_with(vec![entry("keep.txt", false, 10)]);
    let path = PathBuf::from("/test/keep.txt");
    panel.select_path(path.clone());
    panel.watcher.activate_test_binding(Path::new("/test"));
    let ticket = panel.watcher.snapshot_ticket();

    for status in [DirStatus::Partial, DirStatus::Denied, DirStatus::Gone] {
        assert!(!panel.apply_directory_read(DirectoryRead::Incomplete(status)));
        assert_eq!(panel.entries().len(), 1);
        assert!(panel.is_selected(&path));
        assert_eq!(panel.dir_status(), status);
    }
    panel.watcher.defer_snapshot(ticket.as_ref());
    assert!(panel.watcher.has_pending_reconciliation_for_test());
}

fn assert_incomplete_navigation_hides_old_binding(status: DirStatus) {
    let old_entry = entry("keep.txt", false, 10);
    let old_path = old_entry.path.clone();
    let mut panel = panel_with(vec![old_entry.clone()]);
    panel.select_path(old_path.clone());
    panel.set_cursor(1);
    panel.begin_drag(old_path.clone());
    panel.drag.set_drop_target(PathBuf::from("/test"));

    let old_listing_revision = panel.entries_gen();
    let old_size_revision = panel.size_snapshot().revision();
    let old_filter = panel.filtered_indices();
    assert_eq!(panel.folder_overview().total, Some(10));

    panel.current_path = PathBuf::from("/unavailable");
    assert!(!panel.apply_directory_read(DirectoryRead::Incomplete(status)));

    assert_eq!(panel.listing.binding(), Path::new("/unavailable"));
    assert!(panel.entries().is_empty());
    assert!(panel.filtered_indices().is_empty());
    assert!(!Arc::ptr_eq(&old_filter, &panel.filtered_indices()));
    assert!(panel.entries_gen() > old_listing_revision);
    assert!(panel.size_snapshot().revision() > old_size_revision);
    assert_eq!(
        panel.folder_overview(),
        FolderOverview {
            total: Some(0),
            partial: false,
            largest: None,
            oldest: None,
        }
    );
    assert!(panel.selected_paths().is_empty());
    assert!(panel.selected_or_cursor().unwrap().is_empty());
    assert_eq!(panel.cursor(), 0);
    assert!(panel.cursor_entry().is_none());
    assert!(panel.drag.is_empty());
    assert!(!panel.drag.has_drop_target());

    panel.current_path = PathBuf::from("/test");
    assert!(panel.apply_directory_read(DirectoryRead::Complete(vec![old_entry])));
    assert!(panel.is_selected(&old_path));
    let actionable = panel.selected_or_cursor().unwrap();
    assert_eq!(actionable.len(), 1);
    assert_eq!(actionable[0].path, old_path);
}

#[test]
fn gone_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
    assert_incomplete_navigation_hides_old_binding(DirStatus::Gone);
}

#[test]
fn denied_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
    assert_incomplete_navigation_hides_old_binding(DirStatus::Denied);
}

#[test]
fn partial_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
    assert_incomplete_navigation_hides_old_binding(DirStatus::Partial);
}

#[test]
fn invert_selection_flips_only_filtered_rows() {
    let mut p = panel_with(vec![
        entry("alpha", false, 1),
        entry("album", false, 1),
        entry("zebra", false, 1),
    ]);
    let alpha = p.entries()[0].path.clone();
    let album = p.entries()[1].path.clone();
    let zebra = p.entries()[2].path.clone();
    // Pre-select one visible (alpha) and one that the filter will hide (zebra).
    p.extend_selection([alpha.clone(), zebra.clone()]);
    // Filter to the "al" rows; zebra is now hidden from the view.
    p.set_search_query("al");
    p.invert_selection();
    assert!(!p.is_selected(&alpha), "visible+selected -> cleared");
    assert!(p.is_selected(&album), "visible+unselected -> selected");
    assert!(p.is_selected(&zebra), "filtered-out row keeps its state");
}

#[test]
fn selected_or_cursor_falls_back_to_cursor_row() {
    let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
    p.set_cursor(0); // ".." row
    assert!(p.selected_or_cursor().unwrap().is_empty());
    p.set_cursor(2); // second file
    let picked = p.selected_or_cursor().unwrap();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].name, "b");
}

#[test]
fn sorting_reclamps_a_cursor_after_the_filter_changes() {
    let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
    p.set_cursor(2);
    p.set_search_query("alpha");

    p.sort_entries();

    assert_eq!(p.cursor(), 1);
    assert_eq!(p.filtered_get(0).unwrap().name, "alpha");
}

#[test]
fn filtering_out_the_focused_row_lands_on_the_first_match() {
    let mut p = panel_with(vec![
        entry("alpha.txt", false, 1),
        entry("beta.txt", false, 1),
        entry("gamma.rs", false, 1),
        entry("delta.rs", false, 1),
        entry("omega.txt", false, 1),
    ]);
    p.sort_entries();
    // alpha, beta, delta, gamma, omega: focus omega (row 5).
    p.set_cursor(5);
    assert_eq!(p.cursor_entry().unwrap().name, "omega.txt");

    p.set_search_query(".rs");

    assert_eq!(p.cursor(), 1);
    assert_eq!(p.cursor_entry().unwrap().name, "delta.rs");
    assert_eq!(filtered_cursor(0, None, 3), 0, "the .. row stays put");
    assert_eq!(filtered_cursor(4, Some(2), 3), 2);
    assert_eq!(filtered_cursor(4, None, 0), 0);
}

#[test]
fn sort_and_filter_keep_focus_on_the_same_path() {
    let mut p = panel_with(vec![
        entry("beta.txt", false, 20),
        entry("alpha.txt", false, 10),
        entry("notes.md", false, 30),
    ]);
    p.set_cursor(1);
    let focused = p.focused_path().unwrap();

    p.set_search_query("t");
    assert_eq!(p.focused_path(), Some(focused.clone()));
    p.set_sort(SortColumn::Size);
    assert_eq!(p.focused_path(), Some(focused));
}

#[test]
fn equal_sort_keys_use_path_as_a_stable_tie_breaker() {
    let mut z = entry("same.txt", false, 10);
    z.path = PathBuf::from("/z/same.txt");
    let mut a = entry("same.txt", false, 10);
    a.path = PathBuf::from("/a/same.txt");
    let mut p = panel_with(vec![z, a]);

    p.set_sort(SortColumn::Size);

    assert_eq!(p.entries()[0].path, PathBuf::from("/a/same.txt"));
    assert_eq!(p.entries()[1].path, PathBuf::from("/z/same.txt"));

    p.set_sort(SortColumn::Size);
    assert_eq!(p.sort_order(), SortOrder::Desc);
    assert_eq!(p.entries()[0].path, PathBuf::from("/a/same.txt"));
    assert_eq!(p.entries()[1].path, PathBuf::from("/z/same.txt"));
}

#[test]
fn stale_filter_indices_are_bounded_and_cursor_miss_is_explicit() {
    let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
    p.set_cursor(2);
    assert_eq!(p.filtered_count(), 2); // warm the index cache
    p.clear_entries_without_revision_for_test();

    assert!(p.filtered_entries().is_empty());
    assert!(p.filtered_indices().is_empty());
    assert_eq!(
        p.selected_or_cursor().unwrap_err(),
        StaleCursor {
            cursor: 2,
            visible_entries: 0,
        }
    );
}

#[test]
fn reload_preserves_cursor_by_path_and_prunes_selection() {
    let tmp = TempDir::new();
    tmp.file("a.txt", "1");
    tmp.file("b.txt", "2");
    let doomed = tmp.file("c.txt", "3");

    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    assert_eq!(p.entries().len(), 3);

    // Cursor on "b.txt" (row 2), select "c.txt".
    p.set_cursor(2);
    p.toggle_select(doomed.clone());

    // A new file shifts sort order; a selected file disappears.
    tmp.file("0-first.txt", "0");
    std::fs::remove_file(&doomed).unwrap();
    p.refresh();

    let under_cursor = p.filtered_entries()[p.cursor() - 1].path.clone();
    assert!(under_cursor.ends_with("b.txt"), "cursor follows the path");
    assert!(p.selection_is_empty(), "selection drops deleted paths");
}

#[test]
fn seeded_view_config_drives_the_first_complete_listing() {
    let root = TempDir::new();
    root.file("small.txt", "1");
    root.file("large.txt", "12345");
    root.file(".hidden.txt", "123");
    let config = ViewConfig::default()
        .with_sort(SortColumn::Size, SortOrder::Desc)
        .with_show_hidden(true)
        .with_density(crate::density::Density::Compact);
    let mut panel = PanelState::new_with_view(root.path().to_path_buf(), config);

    panel.refresh();

    assert_eq!(panel.view_config(), config);
    assert_eq!(
        panel
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["large.txt", ".hidden.txt", "small.txt"]
    );
}

#[test]
fn hidden_toggle_commits_config_and_listing_together() {
    let root = TempDir::new();
    root.file("visible.txt", "v");
    root.file(".hidden.txt", "h");
    let mut panel = PanelState::new(root.path().to_path_buf());
    panel.refresh();
    let before_revision = panel.entries_gen();
    assert!(!panel.show_hidden());
    assert_eq!(panel.entries().len(), 1);
    panel.set_cursor(1);
    let focused = panel.cursor_entry().unwrap().path.clone();

    assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);

    assert!(panel.show_hidden());
    assert_eq!(panel.entries().len(), 2);
    assert_eq!(panel.entries_gen(), before_revision + 1);
    assert_eq!(panel.cursor_entry().unwrap().path, focused);
}

#[test]
fn async_hidden_toggle_keeps_the_old_view_until_the_listing_lands() {
    let root = TempDir::new();
    root.file("visible.txt", "v");
    root.file(".hidden.txt", "h");
    let mut panel = PanelState::new(root.path().to_path_buf());
    panel.refresh();
    let workload = deterministic_listings(&mut panel);

    assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Pending);
    assert!(!panel.show_hidden());
    assert_eq!(panel.entries().len(), 1);

    assert!(workload.run_next());
    assert!(panel.poll_listing());
    assert!(panel.show_hidden());
    assert_eq!(panel.entries().len(), 2);
    assert_eq!(
        panel.take_view_apply_outcome(),
        Some(ViewApplyOutcome::Applied)
    );
}

#[test]
fn rejected_hidden_toggle_preserves_complete_view_and_rows() {
    let root = TempDir::new();
    let folder = root.dir("folder");
    root.file("folder/visible.txt", "v");
    root.file("folder/second.txt", "s");
    let mut panel = PanelState::new(folder.clone());
    panel.refresh();
    let selected = panel.entries()[0].path.clone();
    panel.select_path(selected.clone());
    panel.set_cursor(1);
    panel.set_scroll_anchor(1);
    panel.set_search_query("txt");
    panel.watcher.activate_test_binding(&folder);

    let config = panel.view_config();
    let paths = panel
        .entries()
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let revision = panel.entries_gen();
    let status = panel.dir_status();
    let cursor = panel.cursor();
    let scroll_anchor = panel.scroll_anchor();
    let selected = panel.selected_paths().clone();
    let filter = panel.filtered_indices();
    let size_revision = panel.size_snapshot().revision();
    let size_binding = panel.sizes.binding_for_test().to_path_buf();
    let watcher_state = panel.watcher.reconciliation_state_for_test();
    std::fs::remove_dir_all(&folder).unwrap();

    assert_eq!(
        panel.toggle_hidden(),
        ViewApplyOutcome::ReadRejected(DirStatus::Gone)
    );

    assert_eq!(panel.view_config(), config);
    assert_eq!(panel.entries_gen(), revision);
    assert_eq!(panel.dir_status(), status);
    assert_eq!(panel.cursor(), cursor);
    assert_eq!(panel.scroll_anchor(), scroll_anchor);
    assert_eq!(panel.selected_paths(), &selected);
    assert!(Arc::ptr_eq(&filter, &panel.filtered_indices()));
    assert_eq!(panel.size_snapshot().revision(), size_revision);
    assert_eq!(panel.sizes.binding_for_test(), size_binding);
    assert_eq!(panel.watcher.reconciliation_state_for_test(), watcher_state);
    assert_eq!(
        panel
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
}

#[test]
fn watcher_event_racing_hidden_snapshot_remains_pending_for_reconciliation() {
    let root = TempDir::new();
    root.file("before.txt", "before");
    let mut panel = PanelState::new(root.path().to_path_buf());
    panel.refresh();
    panel.watcher.activate_test_binding(root.path());

    let raced = root.file("raced.txt", "raced");
    panel.watcher.inject_current_change_for_test(raced.clone());

    assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);
    assert!(panel.poll_fs_changes());
    assert!(panel.entries().iter().any(|entry| entry.path == raced));
    assert!(!panel.watcher.has_pending_reconciliation_for_test());
}

#[test]
fn captured_history_jumps_restore_focus_keep_forward_and_report_missing_targets() {
    let root = TempDir::new();
    let a = root.dir("a");
    let b = root.dir("b");
    let file = root.file("a/keep.txt", "kept");
    let mut p = PanelState::new(root.path().into());
    p.refresh();
    p.navigate_to(a.clone());
    p.focus_child("keep.txt".into());
    p.navigate_to(b.clone());
    let captured = p.history.snapshot();
    assert!(p.go_to_history(&captured, 1));
    assert_eq!(p.current_path, a);
    assert_eq!(p.cursor_entry().unwrap().path, file);
    assert!(p.can_go_forward());
    assert_eq!(p.history.snapshot().paths, captured.paths);
    assert!(!p.go_to_history(&captured, 0));
    let current = p.history.snapshot();
    assert!(p.go_to_history(&current, 2));
    assert_eq!(p.current_path, b);
    std::fs::remove_file(&file).unwrap();
    std::fs::remove_dir(&a).unwrap();
    let current = p.history.snapshot();
    assert!(p.go_to_history(&current, 1));
    assert_eq!(p.current_path, a);
    assert!(p.entries().is_empty());
    assert!(p.can_go_forward());
    let current = p.history.snapshot();
    assert!(p.go_to_history(&current, 2));
    // Repeated path slots move the trail cursor without reloading that folder.
    p.navigate_to(root.path().into());
    p.navigate_to(b.clone());
    let current = p.history.snapshot();
    let generation = p.entries_gen();
    assert!(p.go_to_history(&current, 2));
    assert_eq!(p.current_path, b);
    assert_eq!(p.entries_gen(), generation);
}

#[test]
fn history_navigation_walks_back_and_forward() {
    let tmp = TempDir::new();
    let sub = tmp.dir("sub");

    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    p.navigate_to(sub.clone());
    assert_eq!(p.current_path, sub);
    assert!(p.can_go_back());

    p.go_back();
    assert_eq!(p.current_path, tmp.path());
    assert!(p.can_go_forward());

    p.go_forward();
    assert_eq!(p.current_path, sub);
}

#[test]
fn history_navigation_prunes_directories_that_disappeared() {
    let temp = TempDir::new();
    let first = temp.dir("first");
    let removed = temp.dir("removed");
    let last = temp.dir("last");

    let mut panel = PanelState::new(temp.path().to_path_buf());
    panel.refresh();
    panel.navigate_to(first.clone());
    panel.navigate_to(removed.clone());
    panel.navigate_to(last.clone());
    std::fs::remove_dir(&removed).unwrap();

    panel.go_back();
    assert_eq!(panel.current_path, first);
    panel.go_forward();
    assert_eq!(panel.current_path, last);
}

#[test]
fn history_back_skips_a_repeat_of_the_current_directory() {
    let temp = TempDir::new();
    let a = temp.dir("a");
    let removed = temp.dir("removed");

    let mut panel = PanelState::new(temp.path().to_path_buf());
    panel.refresh();
    panel.navigate_to(a.clone()); // [root, a]
    panel.navigate_to(removed.clone()); // [root, a, removed]
    panel.navigate_to(a.clone()); // [root, a, removed, a]
    std::fs::remove_dir(&removed).unwrap();

    // Pruning `removed` leaves `a` next to itself; Back moves on to root
    // instead of "reloading" the directory already shown.
    panel.go_back();
    assert_eq!(panel.current_path, temp.path());
    panel.go_forward();
    assert_eq!(panel.current_path, a);
    assert!(!panel.can_go_forward());
}

#[test]
fn navigating_to_a_missing_directory_is_not_a_recent_visit() {
    let temp = TempDir::new();
    let missing = temp.path().join("never-created");
    let mut panel = PanelState::new(temp.path().to_path_buf());
    panel.refresh();
    panel.navigate_to(missing.clone());
    assert!(!visited_paths().contains(&missing));

    let file = temp.file("plain.txt", "x");
    panel.navigate_to(file.clone());
    assert!(!visited_paths().contains(&file));
}

#[test]
fn async_navigation_records_the_visit_when_the_listing_lands() {
    let temp = TempDir::new();
    let target = temp.dir("visit-after-listing");
    let mut panel = PanelState::new(temp.path().to_path_buf());
    panel.refresh();

    // The deterministic backend holds the listing until explicitly driven.
    // A real scheduler clamps a zero per-kind limit to one worker.
    let _workload = deterministic_listings(&mut panel);
    panel.navigate_to(target.clone());
    assert_eq!(panel.listing.status(), DirStatus::Loading);
    assert!(!visited_paths().contains(&target));

    // Once the read resolves, the visit lands in Recent.
    panel.workload = None;
    panel.refresh();
    assert!(visited_paths().contains(&target));
    assert_eq!(panel.pending_visit, None);
}

#[test]
fn history_jump_back_twice_then_forward_and_truncate() {
    let tmp = TempDir::new();
    let a = tmp.dir("a");
    let b = tmp.dir("b");
    let c = tmp.dir("c");
    let d = tmp.dir("d");

    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    p.navigate_to(a.clone()); // trail: [root, a]
    p.navigate_to(b.clone()); // [root, a, b]
    p.navigate_to(c.clone()); // [root, a, b, c]

    // Back twice lands on `a`; forward returns to `b`.
    p.go_back();
    p.go_back();
    assert_eq!(p.current_path, a);
    p.go_forward();
    assert_eq!(p.current_path, b);

    // A fresh navigation from `b` truncates the forward tail (drops c).
    p.navigate_to(d.clone());
    assert_eq!(p.current_path, d);
    assert!(!p.can_go_forward(), "forward tail truncated by a new jump");
    // Back now steps to `b`, not the discarded `c`.
    p.go_back();
    assert_eq!(p.current_path, b);
}

#[test]
fn go_up_lands_cursor_on_the_left_directory() {
    let tmp = TempDir::new();
    tmp.dir("aaa");
    let mid = tmp.dir("mmm");
    tmp.dir("zzz");

    let mut p = PanelState::new(mid.clone());
    p.refresh();
    p.go_up();

    assert_eq!(p.current_path, tmp.path());
    // Cursor should sit on "mmm" (the dir we came from), not row 0.
    let under = p.filtered_get(p.cursor() - 1).unwrap();
    assert_eq!(under.name, "mmm");
}

#[test]
fn focus_child_falls_back_to_a_case_insensitive_name() {
    let mut p = panel_with(vec![
        entry("Desktop", true, 0),
        entry("Downloads", true, 0),
        entry("downloads.txt", false, 1),
    ]);
    p.sort_entries();
    // A path typed in another case on a case-insensitive volume.
    p.focus_child("downloads".to_string());
    assert_eq!(p.cursor_entry().unwrap().name, "Downloads");
    // An exact match still wins over a case variant.
    p.focus_child("downloads.txt".to_string());
    assert_eq!(p.cursor_entry().unwrap().name, "downloads.txt");
}

#[test]
fn navigate_to_restores_the_view_remembered_for_that_directory() {
    let tmp = TempDir::new();
    let downloads = tmp.dir("downloads");
    let docs = tmp.dir("docs");

    let mut p = PanelState::new(downloads.clone());
    p.refresh();
    p.set_sort(SortColumn::Size);
    p.reverse_sort();
    assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
    p.set_density(crate::density::Density::Compact);
    p.set_search_query("download-filter");

    // "docs" has never been visited: like before per-folder memory
    // existed, its view carries over from wherever we came from.
    p.navigate_to(docs.clone());
    assert_eq!(p.sort_column(), SortColumn::Size);
    assert_eq!(p.sort_order(), SortOrder::Desc);
    assert!(p.show_hidden());
    assert_eq!(p.density(), crate::density::Density::Compact);
    assert_eq!(p.search_query(), "");

    // Now give "docs" its own, different view.
    p.set_sort(SortColumn::Extension);
    assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
    p.set_density(crate::density::Density::Spacious);
    p.set_search_query("docs-filter");

    // Back to "downloads": its own remembered view returns, not "docs"'s.
    p.navigate_to(downloads.clone());
    assert_eq!(p.sort_column(), SortColumn::Size);
    assert_eq!(p.sort_order(), SortOrder::Desc);
    assert!(p.show_hidden());
    assert_eq!(p.density(), crate::density::Density::Compact);
    assert_eq!(p.search_query(), "download-filter");

    // And "docs" kept its own distinct view too.
    p.navigate_to(docs);
    assert_eq!(p.sort_column(), SortColumn::Extension);
    assert_eq!(p.sort_order(), SortOrder::Asc);
    assert!(!p.show_hidden());
    assert_eq!(p.density(), crate::density::Density::Spacious);
    assert_eq!(p.search_query(), "docs-filter");
}

#[test]
fn navigate_to_restores_cursor_path_and_scroll_anchor() {
    let tmp = TempDir::new();
    let downloads = tmp.dir("downloads");
    let docs = tmp.dir("docs");
    tmp.file("downloads/a.txt", "a");
    let focused = tmp.file("downloads/b.txt", "b");
    tmp.file("downloads/c.txt", "c");

    let mut p = PanelState::new(downloads.clone());
    p.refresh();
    let cursor = p
        .filtered_entries()
        .iter()
        .position(|entry| entry.path == focused)
        .unwrap()
        + 1;
    p.set_cursor(cursor);
    p.set_scroll_anchor(1);

    p.navigate_to(docs);
    p.navigate_to(downloads);

    assert_eq!(p.filtered_get(p.cursor() - 1).unwrap().path, focused);
    assert_eq!(p.scroll_anchor(), 1);
    assert!(p.scroll_to_cursor());
}

#[test]
fn cursor_follows_its_entry_when_an_age_facet_expires_rows_above_it() {
    let now = SystemTime::now();
    let mut p = panel_with(vec![
        entry("a.txt", false, 1),
        entry("b.txt", false, 1),
        entry("c.txt", false, 1),
    ]);
    let window = std::time::Duration::from_secs(1);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.modified = Some(if e.name == "a.txt" {
                // Turns one day old (and leaves the facet) in `window`.
                now - std::time::Duration::from_secs(24 * 60 * 60) + window
            } else {
                now
            });
        }
    });
    p.set_facets(FacetSet {
        max_age_days: Some(1),
        ..FacetSet::default()
    });
    assert_eq!(p.filtered_count(), 3, "a.txt is still young enough");
    p.set_cursor(2);
    assert_eq!(p.cursor_entry().unwrap().name, "b.txt");

    std::thread::sleep(window + std::time::Duration::from_millis(100));
    assert_eq!(p.filtered_count(), 2);
    // The per-frame hook re-resolves the cursor with no fs event at all.
    let _ = p.poll_fs_changes();
    assert_eq!(p.cursor(), 1);
    assert_eq!(p.cursor_entry().unwrap().name, "b.txt");
}

#[test]
fn remembered_parent_row_focus_is_restored_on_the_parent_row() {
    let tmp = TempDir::new();
    let downloads = tmp.dir("downloads");
    let docs = tmp.dir("docs");
    tmp.file("downloads/a.txt", "a");
    tmp.file("downloads/b.txt", "b");

    let mut p = PanelState::new(downloads.clone());
    p.refresh();
    p.set_cursor(0);
    p.navigate_to(docs);
    p.navigate_to(downloads);

    assert_eq!(p.cursor(), 0, "the `..` row stays focused");
}

#[test]
fn leaving_a_folder_before_its_listing_lands_keeps_its_remembered_cursor() {
    let tmp = TempDir::new();
    let downloads = tmp.dir("downloads");
    tmp.file("downloads/a.txt", "a");
    let focused = tmp.file("downloads/b.txt", "b");
    tmp.file("downloads/c.txt", "c");

    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    p.navigate_to(downloads.clone());
    let cursor = p.filtered_position(|entry| entry.path == focused).unwrap() + 1;
    p.set_cursor(cursor);
    p.navigate_to(tmp.path().to_path_buf());

    // Keep the listing queued while leaving; no worker-speed assumption.
    let _workload = deterministic_listings(&mut p);
    p.navigate_to(downloads.clone());
    assert_eq!(p.listing.status(), DirStatus::Loading);
    p.navigate_to(tmp.path().to_path_buf());

    // Back on synchronous listing, the original focus is still there.
    p.workload = None;
    p.navigate_to(downloads);
    assert_eq!(
        p.cursor_entry().map(|entry| entry.path.clone()),
        Some(focused)
    );
}

#[test]
fn missing_remembered_cursor_falls_back_to_scroll_anchor() {
    let tmp = TempDir::new();
    let downloads = tmp.dir("downloads");
    let docs = tmp.dir("docs");
    tmp.file("downloads/a.txt", "a");
    let focused = tmp.file("downloads/b.txt", "b");
    tmp.file("downloads/c.txt", "c");

    let mut p = PanelState::new(downloads.clone());
    p.refresh();
    p.set_cursor(2);
    p.set_scroll_anchor(1);
    p.navigate_to(docs);
    std::fs::remove_file(focused).unwrap();
    p.navigate_to(downloads);

    assert_eq!(p.cursor(), 2.min(p.filtered_count()));
    assert!(p.cursor() <= p.filtered_count());
}

#[test]
fn natural_cmp_orders_numbers_by_value() {
    assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
    assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
    assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
    // Equal numeric value: the shorter run (fewer leading zeros) sorts first.
    assert_eq!(natural_cmp("img9", "img09"), Ordering::Less);
    assert_eq!(natural_cmp("v1.2", "v1.10"), Ordering::Less);
}

#[test]
fn natural_sort_applies_to_listing() {
    let mut p = panel_with(vec![
        entry("file10.txt", false, 1),
        entry("file2.txt", false, 1),
        entry("file1.txt", false, 1),
    ]);
    p.sort_entries();
    let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["file1.txt", "file2.txt", "file10.txt"]);
}

#[test]
fn read_errors_map_to_gone_or_denied() {
    use std::io::{Error, ErrorKind};
    assert_eq!(
        status_for_read_error(&Error::from(ErrorKind::NotFound)),
        DirStatus::Gone
    );
    assert_eq!(
        status_for_read_error(&Error::from(ErrorKind::NotADirectory)),
        DirStatus::Gone
    );
    assert_eq!(
        status_for_read_error(&Error::from(ErrorKind::PermissionDenied)),
        DirStatus::Denied
    );
    // Anything else (an I/O error on a dying disk, too many open files)
    // is not a permission problem.
    assert_eq!(
        status_for_read_error(&Error::other("io")),
        DirStatus::Unavailable
    );
    assert_eq!(
        status_for_read_error(&Error::from(ErrorKind::TimedOut)),
        DirStatus::Unavailable
    );
}

#[test]
fn folder_of_only_dotfiles_reports_its_hidden_count() {
    let tmp = TempDir::new();
    let dir = tmp.dir("dots");
    tmp.file("dots/.a", "a");
    tmp.file("dots/.b", "b");
    let mut panel = PanelState::new(dir);
    panel.refresh();
    assert!(!panel.show_hidden());
    assert_eq!(panel.dir_status(), DirStatus::HiddenOnly(2));
    assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);
    assert_eq!(panel.dir_status(), DirStatus::Listed);
    assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);
    assert_eq!(panel.dir_status(), DirStatus::HiddenOnly(2));

    // A truly empty folder stays "Empty".
    let mut empty = PanelState::new(tmp.dir("empty"));
    empty.refresh();
    assert_eq!(empty.dir_status(), DirStatus::Empty);
}

#[cfg(unix)]
#[test]
fn read_dir_distinguishes_empty_denied_gone() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new();
    let status = |path: &Path| {
        let mut panel = PanelState::new(path.to_path_buf());
        panel.refresh();
        panel.dir_status()
    };

    assert_eq!(status(&tmp.dir("empty")), DirStatus::Empty);

    let full = tmp.dir("full");
    tmp.file("full/x.txt", "x");
    assert_eq!(status(&full), DirStatus::Listed);

    assert_eq!(status(&tmp.path().join("nope")), DirStatus::Gone);
    assert_eq!(status(&tmp.file("file.txt", "f")), DirStatus::Gone);

    let denied = tmp.dir("denied");
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&denied).is_ok(); // root ignores modes
    let result = status(&denied);
    let _ = std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o755));
    if !readable {
        assert_eq!(result, DirStatus::Denied);
    }
}

#[test]
fn push_visit_dedupes_caps_and_orders_recent_first() {
    let mut v: Vec<PathBuf> = Vec::new();
    push_visit(&mut v, Path::new("/a"), 3);
    push_visit(&mut v, Path::new("/b"), 3);
    push_visit(&mut v, Path::new("/a"), 3); // revisit /a -> front, no dup
    assert_eq!(v, vec![PathBuf::from("/a"), PathBuf::from("/b")]);

    push_visit(&mut v, Path::new("/c"), 3);
    push_visit(&mut v, Path::new("/d"), 3); // cap 3 drops oldest
    assert_eq!(
        v,
        vec![
            PathBuf::from("/d"),
            PathBuf::from("/c"),
            PathBuf::from("/a"),
        ]
    );
}

#[test]
fn recording_visits_prunes_stats_for_paths_off_the_list() {
    let mut v: Vec<PathBuf> = Vec::new();
    let mut stats = VisitStats::default();
    for name in ["/a", "/b", "/c", "/d"] {
        visit::record_visit_into(&mut v, &mut stats, Path::new(name), 3);
    }
    assert_eq!(v.len(), 3);
    assert_eq!(stats.uses.len(), 3);
    assert!(!stats.uses.contains_key(Path::new("/a")));
    assert_eq!(stats.uses[Path::new("/d")].count, 1);
}

#[test]
fn recent_visit_policy_keeps_two_hundred_paths() {
    let mut v: Vec<PathBuf> = Vec::new();
    for i in 0..205 {
        push_visit(&mut v, Path::new(&format!("/recent/{i}")), VISITED_CAP);
    }

    assert_eq!(v.len(), 200);
    assert_eq!(v.first(), Some(&PathBuf::from("/recent/204")));
    assert_eq!(v.last(), Some(&PathBuf::from("/recent/5")));
}

#[test]
fn recent_destinations_can_switch_between_frecency_and_chronology() {
    let a = PathBuf::from("/work/frequent");
    let b = PathBuf::from("/work/middle");
    let c = PathBuf::from("/work/latest");
    let paths = vec![c.clone(), b.clone(), a.clone()];
    let mut stats = VisitStats::default();
    stats.record(&a);
    stats.record(&a);
    stats.record(&a);
    stats.record(&b);
    stats.record(&c);

    let chronological = rank_visited(&paths, "", RecentOrder::Chronological, &stats);
    assert_eq!(chronological[0].path, c);

    let frecency = rank_visited(&paths, "", RecentOrder::Frecency, &stats);
    assert_eq!(frecency[0].path, a);
    assert_eq!(frecency[0].count, 3);
}

#[test]
fn recent_destination_query_uses_fuzzy_path_matching() {
    let paths = vec![
        PathBuf::from("/Users/me/Documents"),
        PathBuf::from("/Users/me/Downloads"),
    ];
    let matches = rank_visited(
        &paths,
        "dwn",
        RecentOrder::Chronological,
        &VisitStats::default(),
    );
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].path, PathBuf::from("/Users/me/Downloads"));
}

#[test]
fn format_mode_renders_rwx() {
    assert_eq!(format_mode(0o755), "rwxr-xr-x");
    assert_eq!(format_mode(0o644), "rw-r--r--");
    assert_eq!(format_mode(0o600), "rw-------");
    assert_eq!(format_mode(0o000), "---------");
    assert_eq!(format_mode(0o777), "rwxrwxrwx");
    // Special bits, as `ls -l` shows them.
    assert_eq!(format_mode(0o1777), "rwxrwxrwt");
    assert_eq!(format_mode(0o4755), "rwsr-xr-x");
    assert_eq!(format_mode(0o2755), "rwxr-sr-x");
    assert_eq!(format_mode(0o7644), "rwSr-Sr-T");
}

#[test]
fn make_info_describes_file_and_folder() {
    let tmp = TempDir::new();
    let f = tmp.file("notes.txt", "hello");
    let meta = std::fs::metadata(&f).unwrap();
    let fe = FileEntry::from_meta(f, &meta).unwrap();
    let card = make_info(&fe, None, None);
    assert_eq!(card.kind, "TXT file");
    assert_eq!(card.size, "5 B");
    assert!(card.children.is_none());
    assert_eq!(card.permissions.len(), 9);

    let d = tmp.dir("box");
    let meta = std::fs::metadata(&d).unwrap();
    let de = FileEntry::from_meta(d, &meta).unwrap();
    let card = make_info(&de, Some(4096), Some(3));
    assert_eq!(card.kind, "Folder");
    assert_eq!(card.size, "4.0 KB");
    assert_eq!(card.children, Some(3));
}

#[test]
fn make_info_says_when_permissions_cannot_be_read() {
    let tmp = TempDir::new();
    let f = tmp.file("gone.txt", "hello");
    let meta = std::fs::metadata(&f).unwrap();
    let mut fe = FileEntry::from_meta(f, &meta).unwrap();
    fe.permissions = None;
    let card = make_info(&fe, None, None);
    // Not a fake mode string indistinguishable from a real `000` file.
    assert_eq!(card.permissions, "unavailable");
}

#[test]
fn make_info_reads_permissions_from_the_listing_not_the_disk() {
    let tmp = TempDir::new();
    let f = tmp.file("listed.txt", "hello");
    let meta = std::fs::metadata(&f).unwrap();
    let fe = FileEntry::from_meta(f.clone(), &meta).unwrap();
    let listed = make_info(&fe, None, None).permissions;
    // Gone (or on a hung mount) by the time Get Info opens: the card
    // still shows what the listing saw, without touching the disk.
    std::fs::remove_file(&f).unwrap();
    assert_eq!(make_info(&fe, None, None).permissions, listed);
    assert_ne!(listed, "unavailable");
}

#[test]
fn declined_image_preview_message_names_the_file_and_the_switch() {
    let mut img = entry("photo.jpg", false, 5_000);
    img.extension = "jpg".into();
    let message = image_preview_unavailable_message(&img);
    assert!(message.contains("photo.jpg"), "{message}");
    assert!(message.contains("Runtime controls"), "{message}");
}

#[test]
fn facet_matches_kind_size_age() {
    use std::time::{Duration, UNIX_EPOCH};
    let now = UNIX_EPOCH + Duration::from_secs(100 * 24 * 60 * 60); // day 100
    let mut img = entry("photo.jpg", false, 5_000_000);
    img.extension = "jpg".into();
    img.modified = Some(UNIX_EPOCH + Duration::from_secs(99 * 24 * 60 * 60)); // 1 day old
    let mut code = entry("main.rs", false, 100);
    code.extension = "rs".into();
    code.modified = Some(UNIX_EPOCH + Duration::from_secs(50 * 24 * 60 * 60)); // 50 days old
    let dir = entry("box", true, 0);

    let images = FacetSet {
        kind: Some(KindFacet::Images),
        ..Default::default()
    };
    assert!(facet_matches(&img, &images, now));
    assert!(!facet_matches(&code, &images, now));

    let folders = FacetSet {
        kind: Some(KindFacet::Folders),
        ..Default::default()
    };
    assert!(facet_matches(&dir, &folders, now));
    assert!(!facet_matches(&img, &folders, now));

    let big = FacetSet {
        min_size: Some(1_000_000),
        ..Default::default()
    };
    assert!(facet_matches(&img, &big, now)); // 5MB
    assert!(!facet_matches(&code, &big, now)); // 100B
    assert!(facet_matches(&dir, &big, now)); // folders ignore size

    let recent = FacetSet {
        max_age_days: Some(7),
        ..Default::default()
    };
    assert!(facet_matches(&img, &recent, now)); // 1 day
    assert!(!facet_matches(&code, &recent, now)); // 50 days

    let stale = FacetSet {
        min_age_days: Some(30),
        ..Default::default()
    };
    assert!(!facet_matches(&img, &stale, now)); // 1 day: too fresh
    assert!(facet_matches(&code, &stale, now)); // 50 days: old enough
    assert!(!facet_matches(&dir, &stale, now)); // unknown mtime: fails

    // Empty facets pass everything.
    assert!(facet_matches(&code, &FacetSet::default(), now));
}

#[test]
fn folder_overview_reports_largest_and_oldest() {
    use std::time::{Duration, UNIX_EPOCH};
    let mut a = entry("a.txt", false, 100);
    a.modified = Some(UNIX_EPOCH + Duration::from_secs(300));
    let mut big = entry("big.bin", false, 900);
    big.modified = Some(UNIX_EPOCH + Duration::from_secs(200));
    let mut old = entry("old.log", false, 50);
    old.modified = Some(UNIX_EPOCH + Duration::from_secs(100));
    let p = panel_with(vec![a, big, old]);
    let o = p.folder_overview();
    assert_eq!(o.total, Some(1050));
    assert_eq!(o.largest, Some(("big.bin".to_string(), 900)));
    assert_eq!(
        o.oldest,
        Some(("old.log".to_string(), UNIX_EPOCH + Duration::from_secs(100)))
    );
}

#[test]
fn facets_filter_the_listing() {
    let mut p = panel_with(vec![
        entry("a.jpg", false, 1),
        entry("b.rs", false, 1),
        entry("c.jpg", false, 1),
    ]);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    p.set_facets(FacetSet {
        kind: Some(KindFacet::Images),
        ..Default::default()
    });
    let names: Vec<&str> = p
        .filtered_entries()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, vec!["a.jpg", "c.jpg"]);
}

#[test]
fn glob_match_handles_star_and_question() {
    assert!(glob_match("*.rs", "main.rs"));
    assert!(glob_match("img_*.jpg", "img_2024.jpg"));
    assert!(glob_match("a?c", "abc"));
    assert!(!glob_match("a?c", "ac"));
    assert!(!glob_match("*.rs", "main.txt"));
    assert!(glob_match("*", "anything"));
    // Multi-byte names: `?` is one character, and a failed match after a
    // star re-anchors on a character boundary.
    assert!(glob_match("caf?.txt", "café.txt"));
    assert!(glob_match("*é*", "résumé.pdf"));
    assert!(glob_match("*ü.txt", "üüü.txt"));
    assert!(!glob_match("?", "éé"));
    assert!(glob_match("**a", "a"));
    assert!(glob_match("", ""));
    assert!(!glob_match("", "a"));
}

#[test]
fn parse_mask_splits_and_marks_subtraction() {
    let terms = parse_mask("*.JPG, !*raw*, -tmp");
    assert_eq!(
        terms,
        vec![
            ("*.jpg".to_string(), false),
            ("*raw*".to_string(), true),
            ("tmp".to_string(), true),
        ]
    );
    // A bare subtraction marker is not an (empty) term.
    assert!(parse_mask("-").is_empty());
    assert_eq!(parse_mask("*.jpg, !"), vec![("*.jpg".to_string(), false)]);
}

#[test]
fn kind_inversion_is_reversible_and_preserves_hidden_picks_and_other_kind() {
    for folders in [false, true] {
        let mut panel = panel_with(vec![
            entry("a.txt", false, 1),
            entry("b.txt", false, 1),
            entry("dir-a", true, 0),
            entry("dir-b", true, 0),
        ]);
        panel.select_by_names("a.txt\ndir-a").unwrap();
        let before = selected_names(&panel);
        assert_eq!(panel.invert_visible_kind(folders), 2);
        let expected = if folders {
            HashSet::from(["a.txt".to_string(), "dir-b".to_string()])
        } else {
            HashSet::from(["b.txt".to_string(), "dir-a".to_string()])
        };
        assert_eq!(selected_names(&panel), expected);
        assert_eq!(panel.invert_visible_kind(folders), 2);
        assert_eq!(selected_names(&panel), before);
        panel.select_by_names("b.txt\ndir-b").unwrap();
        panel.set_search_query("a");
        assert_eq!(panel.invert_visible_kind(folders), 1);
        let after = selected_names(&panel);
        assert_eq!(after.len(), 3);
        assert!(after.contains("b.txt") && after.contains("dir-b"));
        assert!(after.contains(if folders { "a.txt" } else { "dir-a" }));
        panel.set_search_query("nothing-matches");
        assert_eq!(panel.invert_visible_kind(folders), 0);
        assert_eq!(selected_names(&panel), after);
    }
}

#[test]
fn file_edge_navigation_skips_directories_and_respects_sort_filter_and_selection() {
    for folders_first in [true, false] {
        for reversed in [false, true] {
            let mut panel = panel_with(vec![
                entry("a.txt", false, 1),
                entry("bdir", true, 0),
                entry("z.txt", false, 1),
            ]);
            if !folders_first {
                panel.toggle_folders_first();
            }
            if reversed {
                panel.reverse_sort();
            }
            panel.select_by_names("z.txt").unwrap();
            assert!(panel.jump_to_file_edge(false));
            assert_eq!(
                panel.cursor_entry().unwrap().name,
                if reversed { "z.txt" } else { "a.txt" }
            );
            assert!(panel.scroll_to_cursor());
            assert!(panel.jump_to_file_edge(true));
            assert_eq!(
                panel.cursor_entry().unwrap().name,
                if reversed { "a.txt" } else { "z.txt" }
            );
            assert_eq!(selected_names(&panel), HashSet::from(["z.txt".to_string()]));
            panel.set_search_query("a.txt");
            assert!(panel.jump_to_file_edge(true));
            assert_eq!(panel.cursor_entry().unwrap().name, "a.txt");
            panel.set_search_query("bdir");
            let before = panel.cursor();
            assert!(!panel.jump_to_file_edge(false));
            assert!(!panel.jump_to_file_edge(true));
            assert_eq!(panel.cursor(), before);
            assert_eq!(panel.selected_count(), 1);
        }
    }
}

#[test]
fn same_stem_actions_keep_unrelated_and_filtered_out_picks() {
    let mut panel = panel_with(vec![
        entry("report.pdf", false, 1),
        entry("report.docx", false, 1),
        entry("report", false, 1),
        entry("Report.TXT", false, 1),
        entry("report.dir", true, 0),
        entry("other.txt", false, 1),
    ]);
    panel.select_by_names("other.txt").unwrap();
    let row = panel
        .filtered_iter()
        .position(|entry| entry.name == "report.pdf")
        .unwrap()
        + 1;
    panel.set_cursor(row);
    assert_eq!(panel.change_same_stem_as_cursor(true), 4);
    assert_eq!(panel.change_same_stem_as_cursor(true), 0);
    assert_eq!(panel.selected_count(), 5);
    panel.set_search_query("pdf");
    assert_eq!(panel.change_same_stem_as_cursor(false), 1);
    assert_eq!(panel.selected_count(), 4);
    panel.set_search_query("");
    assert!(selected_names(&panel).contains("other.txt"));
    panel.set_cursor(0);
    assert_eq!(panel.change_same_stem_as_cursor(true), 0);
    let folder_row = panel
        .filtered_iter()
        .position(|entry| entry.is_dir)
        .unwrap()
        + 1;
    panel.set_cursor(folder_row);
    assert_eq!(panel.change_same_stem_as_cursor(false), 0);
}

#[test]
fn same_stem_uses_only_the_last_extension() {
    let mut panel = panel_with(vec![
        entry("archive.tar.gz", false, 1),
        entry("archive.tar.xz", false, 1),
        entry("archive.zip", false, 1),
    ]);
    let row = panel
        .filtered_iter()
        .position(|entry| entry.name == "archive.tar.gz")
        .unwrap()
        + 1;
    panel.set_cursor(row);
    assert_eq!(panel.change_same_stem_as_cursor(true), 2);
    assert!(!selected_names(&panel).contains("archive.zip"));
}

#[test]
fn extensionless_star_option_changes_only_star_dot_star_terms() {
    for (target, expected) in [
        (SelectionTarget::Both, 3),
        (SelectionTarget::Files, 2),
        (SelectionTarget::Folders, 1),
    ] {
        let mut panel = panel_with(vec![
            entry("README", false, 1),
            entry("doc.txt", false, 1),
            entry("folder", true, 0),
        ]);
        assert_eq!(
            panel.mask_match_count_target("*.*", target),
            usize::from(target != SelectionTarget::Folders)
        );
        assert_eq!(
            panel.mask_match_count_compatibility("*.*", target, true, true, true),
            expected
        );
        assert_eq!(
            panel.select_by_mask_compatibility("*.*", target, true, true, true),
            expected
        );
        assert_eq!(
            panel.mask_match_count_compatibility("*.*", target, true, true, true),
            0
        );
        assert_eq!(
            panel.mask_match_count_compatibility("!*.*", target, false, false, true),
            expected
        );
        panel.select_by_mask_compatibility("!*.*", target, false, false, true);
        assert_eq!(panel.selected_count(), 0);
        assert_eq!(
            panel.select_by_mask_compatibility("*.txt", target, false, false, true),
            usize::from(target != SelectionTarget::Folders)
        );
    }
    let mut panel = panel_with(vec![entry("README", false, 1), entry("Café", false, 1)]);
    assert_eq!(
        panel.select_by_mask_compatibility(
            "*.*;!*cafe*",
            SelectionTarget::Files,
            false,
            true,
            true
        ),
        1
    );
}

#[test]
fn accent_ignoring_masks_fold_both_sides_but_keep_case_and_exclusions() {
    for (mask, expected) in [
        ("*cafe*", 3),
        ("*coeur*", 1),
        ("*café*", 3),
        ("*strasse*", 1),
    ] {
        let mut panel = panel_with(vec![
            entry("café.txt", false, 1),
            entry("cafe\u{301}.txt", false, 1),
            entry("CAFÉ.txt", false, 1),
            entry("cœur.txt", false, 1),
            entry("Straße.txt", false, 1),
        ]);
        assert_eq!(
            panel.mask_match_count_folding(mask, SelectionTarget::Files, false, true),
            expected
        );
        assert_eq!(
            panel.select_by_mask_folding(mask, SelectionTarget::Files, false, true),
            expected
        );
        assert_eq!(
            panel.mask_match_count_folding(mask, SelectionTarget::Files, false, true),
            0
        );
    }
    let mut panel = panel_with(vec![
        entry("café.txt", false, 1),
        entry("CAFÉ.txt", false, 1),
        entry("cafe-dir", true, 0),
    ]);
    assert_eq!(
        panel.select_by_mask_folding("*cafe*;!*dir*", SelectionTarget::Both, true, true),
        1
    );
    assert_eq!(
        panel.mask_match_count_folding("!*café*", SelectionTarget::Both, true, true),
        1
    );
    assert_eq!(
        panel.select_by_mask_folding("*CAFE*", SelectionTarget::Files, true, true),
        1
    );
    let mut strict = panel_with(vec![entry("café.txt", false, 1)]);
    assert_eq!(strict.select_by_mask("*cafe*"), 0);
    assert!(parse_mask_folding("\u{301}", false, true).is_empty());
}

#[test]
fn sensitive_masks_preserve_case_for_globs_extensions_and_subtraction() {
    for mask in ["*.JPG", "JPG", ".JPG"] {
        let mut panel = panel_with(vec![
            entry("a.JPG", false, 1),
            entry("b.jpg", false, 1),
            entry("RAW.JPG", false, 1),
            entry("é.JPG", false, 1),
        ]);
        panel.mutate_entries_for_test(|entries| {
            for entry in entries {
                entry.extension = "jpg".into();
            }
        });
        assert_eq!(
            panel.mask_match_count_options(mask, SelectionTarget::Files, true),
            3
        );
        assert_eq!(
            panel.select_by_mask_options(mask, SelectionTarget::Files, true),
            3
        );
        assert_eq!(
            panel.mask_match_count_options(mask, SelectionTarget::Files, true),
            0
        );
        assert_eq!(
            panel.mask_match_count_options("!*raw*", SelectionTarget::Files, true),
            0
        );
        assert_eq!(
            panel.mask_match_count_options("!*RAW*", SelectionTarget::Files, true),
            1
        );
        panel.select_by_mask_options("!*RAW*", SelectionTarget::Files, true);
        assert_eq!(panel.selected_count(), 2);
        assert_eq!(
            panel.mask_match_count_target(mask, SelectionTarget::Files),
            2
        );
    }
    let mut panel = panel_with(vec![entry("Étage", true, 0), entry("étage", true, 0)]);
    assert_eq!(
        panel.select_by_mask_options("É*", SelectionTarget::Folders, true),
        1
    );
    assert_eq!(
        panel.mask_match_count_options("*", SelectionTarget::Files, true),
        0
    );
    assert_eq!(
        panel.select_by_mask_options("é*", SelectionTarget::Folders, true),
        1
    );
}

#[test]
fn semicolon_masks_mix_delimiters_and_keep_preview_application_in_sync() {
    assert_eq!(
        parse_mask("*.JPG; *.PNG, !*raw*; -tmp"),
        parse_mask("*.JPG, *.PNG, !*raw*, -tmp")
    );
    assert!(parse_mask(";;, ! ; -").is_empty());
    for target in [
        SelectionTarget::Both,
        SelectionTarget::Files,
        SelectionTarget::Folders,
    ] {
        let mut panel = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("b.png", false, 1),
            entry("raw.jpg", false, 1),
            entry("folder.png", true, 0),
            entry("notes.txt", false, 1),
        ]);
        panel.select_by_mask("*raw*");
        let before: HashSet<_> = panel.selected_paths().iter().cloned().collect();
        let expected_changes = panel.mask_match_count_target("*.jpg;*.png,!*raw*", target);
        panel.select_by_mask_target("*.jpg;*.png,!*raw*", target);
        let after: HashSet<_> = panel.selected_paths().iter().cloned().collect();
        assert_eq!(
            before.symmetric_difference(&after).count(),
            expected_changes
        );
        assert_eq!(
            after.len(),
            match target {
                SelectionTarget::Both => 3,
                SelectionTarget::Files => 2,
                SelectionTarget::Folders => 2,
            }
        );
        assert_eq!(
            panel.mask_match_count_target("*.jpg;*.png,!*raw*", target),
            0
        );
    }
}

#[test]
fn exact_names_still_select_literal_mask_delimiters() {
    let mut panel = panel_with(vec![
        entry("semi;colon.txt", false, 1),
        entry("comma,name.txt", false, 1),
        entry("other.txt", false, 1),
    ]);
    assert_eq!(
        panel
            .select_by_names("semi;colon.txt\ncomma,name.txt")
            .unwrap(),
        2
    );
    assert_eq!(panel.selected_count(), 2);
}

#[test]
fn dotted_mask_terms_match_name_suffixes() {
    // A leading dot or a multi-part extension still selects by suffix.
    assert!(term_matches(".jpg", "photo.jpg", "jpg"));
    assert!(term_matches("tar.gz", "backup.tar.gz", "gz"));
    assert!(term_matches(".tar.gz", "backup.tar.gz", "gz"));
    assert!(!term_matches("tar.gz", "backup.gz", "gz"));
    assert!(!term_matches("tar.gz", "backuptar.gz", "gz"));
    // Whole names still match exactly, and wildcards stay pure globs.
    assert!(term_matches(".gitignore", ".gitignore", ""));
    assert!(term_matches("readme.md", "readme.md", "md"));
    assert!(!term_matches("*.tar", "backup.tar.gz", "gz"));
    assert!(!term_matches(".", "a.", ""));
}

#[test]
fn select_by_mask_adds_then_subtracts() {
    let mut p = panel_with(vec![
        entry("a.jpg", false, 1),
        entry("b.jpg", false, 1),
        entry("c.png", false, 1),
        entry("raw.jpg", false, 1),
    ]);
    // bare ext for files needs the extension field populated:
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    p.sort_entries();

    // Add all jpgs, then subtract anything containing "raw".
    // raw.jpg matches the add but the subtract wins, so it is not added.
    let added = p.select_by_mask("*.jpg, !*raw*");
    assert_eq!(added, 2);
    let names: Vec<String> = p
        .selected_paths()
        .iter()
        .filter_map(|pth| pth.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    assert!(names.contains(&"a.jpg".to_string()));
    assert!(names.contains(&"b.jpg".to_string()));
    assert!(!names.contains(&"raw.jpg".to_string()));
    assert!(!names.contains(&"c.png".to_string()));
    assert_eq!(p.selected_count(), 2);
}

#[test]
fn mask_match_count_reports_selection_changes() {
    let mut p = panel_with(vec![
        entry("a.jpg", false, 1),
        entry("raw.jpg", false, 1),
        entry("note.txt", false, 1),
    ]);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    let raw = p
        .entries()
        .iter()
        .find(|entry| entry.name == "raw.jpg")
        .unwrap()
        .path
        .clone();

    assert_eq!(p.mask_match_count("!*raw*"), 0);
    p.select_path(raw);
    assert_eq!(p.mask_match_count("!*raw*"), 1);
    assert_eq!(p.mask_match_count("*.jpg, !*raw*"), 2);
}

#[test]
fn bare_extension_term_matches_by_extension() {
    let mut p = panel_with(vec![
        entry("doc.pdf", false, 1),
        entry("note.txt", false, 1),
    ]);
    p.mutate_entries_for_test(|entries| {
        for e in entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
    });
    assert_eq!(p.mask_match_count("pdf"), 1);
    p.select_by_mask("pdf");
    assert_eq!(p.selected_count(), 1);
}

#[test]
fn type_ahead_jumps_by_prefix_then_substring() {
    let mut p = panel_with(vec![
        entry("apple.txt", false, 1),
        entry("banana.txt", false, 1),
        entry("cherry-banana.txt", false, 1),
    ]);
    p.sort_entries();

    assert!(p.type_ahead("ban"));
    assert_eq!(p.filtered_get(p.cursor() - 1).unwrap().name, "banana.txt");

    // No prefix hit -> substring fallback finds "cherry-banana".
    assert!(p.type_ahead("cherry"));
    assert_eq!(
        p.filtered_get(p.cursor() - 1).unwrap().name,
        "cherry-banana.txt"
    );

    assert!(!p.type_ahead("zzz"));
}

#[test]
fn select_cursor_adds_current_row() {
    let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
    p.set_cursor(2); // second file
    p.select_cursor();
    assert!(p.is_selected(&p.entries()[1].path));
    assert_eq!(p.selected_count(), 1);
}

#[test]
fn filter_cache_tracks_query_and_entry_changes() {
    let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
    assert_eq!(p.filtered_count(), 2);

    // Query change invalidates the cache.
    p.set_search_query("al");
    assert_eq!(p.filtered_count(), 1);
    assert_eq!(p.filtered_get(0).unwrap().name, "alpha");

    // Entry change (generation bump via sort) invalidates it too.
    p.push_entry_for_test(entry("alps", false, 1));
    p.sort_entries();
    assert_eq!(p.filtered_count(), 2);
    let names: Vec<&str> = p
        .filtered_entries()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, vec!["alpha", "alps"]);
}

#[test]
fn filtered_indices_point_into_entries() {
    let mut p = panel_with(vec![
        entry("keep.txt", false, 1),
        entry("skip.rs", false, 1),
        entry("keeper.txt", false, 1),
    ]);
    p.set_search_query("keep");
    let idx = p.filtered_indices();
    assert_eq!(idx.len(), 2);
    for &i in idx.iter() {
        assert!(p.entries()[i].name.contains("keep"));
    }
}

#[test]
fn size_cache_invalidation_cancels_epoch_and_removes_ancestors_only() {
    let tmp = TempDir::new();
    let a = tmp.dir("a");
    let other = tmp.dir("other");
    let a_key = VolumePathKey::observe(&a);
    let other_key = VolumePathKey::observe(&other);
    let active_epoch = capture_path_epoch(&a_key);
    let now = SystemTime::now();
    {
        let _boundary = lock_recover(scan_commit_boundary());
        let mut cache = lock_recover(dir_size_cache());
        cache.insert(a_key.clone(), CachedDirSize::new(now, 100, 1));
        cache.insert(other_key.clone(), CachedDirSize::new(now, 5, 1));
    }

    invalidate_size_cache(&a.join("b/c/file.txt"));

    assert!(active_epoch.is_cancelled());
    let cache = lock_recover(dir_size_cache());
    assert!(!cache.contains_key(&a_key));
    assert_eq!(cache[&other_key].modified, now, "unrelated dirs stay valid");
}

#[test]
fn batched_invalidation_matches_per_path_invalidation() {
    let tmp = TempDir::new();
    let keys: Vec<VolumePathKey> = ["x", "x/deep", "x/deep/below", "y", "y/sub", "z"]
        .into_iter()
        .map(|name| VolumePathKey::observe(&tmp.dir(name)))
        .collect();
    let now = SystemTime::now();
    {
        let _boundary = lock_recover(scan_commit_boundary());
        let mut cache = lock_recover(dir_size_cache());
        for key in &keys {
            cache.insert(key.clone(), CachedDirSize::new(now, 1, 1));
        }
    }

    let changed_x = tmp.path().join("x/deep/file.txt");
    let changed_y = tmp.path().join("y");
    invalidate_size_cache_many([changed_x.as_path(), changed_y.as_path()]);

    let cache = lock_recover(dir_size_cache());
    let kept: Vec<bool> = keys.iter().map(|key| cache.contains_key(key)).collect();
    // Ancestors of each change go; a folder below a change and an
    // unrelated sibling stay.
    assert_eq!(kept, [false, false, true, false, true, true]);
}

#[test]
fn directory_cache_key_rejects_an_old_mount_generation() {
    let path = PathBuf::from("/fixture/folder");
    let first = VolumePathKey {
        path: path.clone(),
        volume_id: 7,
        generation: 1,
    };
    let remounted = VolumePathKey {
        path,
        volume_id: 7,
        generation: 2,
    };
    assert_ne!(first, remounted);
}

#[test]
fn partial_sizes_are_reused_in_memory_but_never_cached_as_exact() {
    let tmp = TempDir::new();
    let directory = tmp.dir("partial");
    let cache_key = VolumePathKey::observe(&directory);
    let path_epoch = capture_path_epoch(&cache_key);
    let current = Arc::new(Mutex::new(Arc::new(ScanEpoch::new())));
    let sizes = Arc::new(Mutex::new(HashMap::new()));
    let counts = Arc::new(Mutex::new(HashMap::new()));
    let revision = Arc::new(AtomicU64::new(0));
    let scan_epoch = begin_panel_scan(
        &current,
        &sizes,
        &counts,
        &HashSet::from([directory.clone()]),
        &revision,
    );
    let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
    let measurement = DirMeasurement {
        path: directory.clone(),
        modified: Some(mtime),
        complete: false,
        cache_key: cache_key.clone(),
        path_epoch,
        size: 7,
        completed_at: std::time::Instant::now(),
        elapsed: std::time::Duration::from_millis(1),
    };
    let outcome = publish_dir_measurements_if_current(
        &current,
        &scan_epoch,
        &sizes,
        &[measurement],
        &revision,
    );
    assert_eq!(outcome.published, 1);
    assert!(!outcome.cache_changed, "a partial size is not persisted");
    assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));

    // A later recompute with the same folder mtime reuses it: no re-walk.
    lock_recover(&sizes).clear();
    assert!(publish_cached_size_if_current(
        &current,
        &scan_epoch,
        &sizes,
        &directory,
        &cache_key,
        Some(mtime),
        &revision,
    ));
    assert_eq!(lock_recover(&sizes).get(&directory), Some(&7));

    // A change inside the folder drops it like the exact cache.
    invalidate_size_cache(&directory.join("file.bin"));
    assert!(!publish_cached_size_if_current(
        &current,
        &scan_epoch,
        &sizes,
        &directory,
        &cache_key,
        Some(mtime),
        &revision,
    ));
}

#[test]
fn watcher_invalidation_wins_against_worker_paused_before_commit() {
    let tmp = TempDir::new();
    let directory = tmp.dir("subject");
    let changed = directory.join("nested/file.bin");
    let cache_key = VolumePathKey::observe(&directory);
    let path_epoch = capture_path_epoch(&cache_key);
    let current = Arc::new(Mutex::new(Arc::new(ScanEpoch::new())));
    let sizes = Arc::new(Mutex::new(HashMap::new()));
    let counts = Arc::new(Mutex::new(HashMap::new()));
    let revision = Arc::new(AtomicU64::new(0));
    let scan_epoch = begin_panel_scan(
        &current,
        &sizes,
        &counts,
        &HashSet::from([directory.clone()]),
        &revision,
    );

    {
        let _boundary = lock_recover(scan_commit_boundary());
        lock_recover(dir_size_cache()).remove(&cache_key);
        lock_recover(walk_log()).remove(&cache_key);
    }

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let worker_current = Arc::clone(&current);
    let worker_sizes = Arc::clone(&sizes);
    let worker_revision = Arc::clone(&revision);
    let worker_scan_epoch = Arc::clone(&scan_epoch);
    let worker_path_epoch = Arc::clone(&path_epoch);
    let worker_key = cache_key.clone();
    let worker_path = directory.clone();
    let worker = std::thread::spawn(move || {
        let measurement = DirMeasurement {
            path: worker_path,
            modified: Some(SystemTime::now()),
            complete: true,
            cache_key: worker_key,
            path_epoch: worker_path_epoch,
            size: 42,
            completed_at: std::time::Instant::now(),
            elapsed: std::time::Duration::from_millis(1),
        };
        worker_barrier.wait();
        worker_barrier.wait();
        publish_dir_measurements_if_current(
            &worker_current,
            &worker_scan_epoch,
            &worker_sizes,
            &[measurement],
            &worker_revision,
        )
    });

    barrier.wait();
    invalidate_size_cache(&changed);
    assert!(path_epoch.is_cancelled());
    assert!(scan_is_current(&current, &scan_epoch));
    barrier.wait();

    assert_eq!(worker.join().unwrap(), PublishOutcome::default());
    assert!(!lock_recover(&sizes).contains_key(&directory));
    let _boundary = lock_recover(scan_commit_boundary());
    assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));
    assert!(!lock_recover(walk_log()).contains_key(&cache_key));
}

#[test]
fn recursive_size_walk_stops_between_entries_when_cancelled() {
    let tmp = TempDir::new();
    tmp.file("a.bin", "123");
    tmp.file("b.bin", "456");
    tmp.file("c.bin", "789");
    let checks = std::sync::atomic::AtomicUsize::new(0);

    let result = dir_size_recursive_until(tmp.path(), false, || {
        checks.fetch_add(1, AtomicOrdering::SeqCst) >= 2
    });

    assert_eq!(result, None);
    assert_eq!(checks.load(AtomicOrdering::SeqCst), 3);
}

#[cfg(unix)]
#[test]
fn recursive_size_counts_file_symlinks_not_their_targets() {
    let outside = TempDir::new();
    let target = outside.file("big.bin", &"x".repeat(4000));
    let tmp = TempDir::new();
    tmp.file("small.bin", "12345");
    std::os::unix::fs::symlink(&target, tmp.path().join("link.bin")).unwrap();
    let link_len = std::fs::symlink_metadata(tmp.path().join("link.bin"))
        .unwrap()
        .len();

    let expected = Some(5 + link_len);
    assert_eq!(
        dir_size_recursive_until(tmp.path(), false, || false),
        expected
    );
    // A background walk measures the same tree.
    assert_eq!(
        dir_size_recursive_until(tmp.path(), true, || false),
        expected
    );
}

#[cfg(unix)]
#[test]
fn recursive_size_counts_hard_linked_files_once() {
    let tmp = TempDir::new();
    let data = tmp.file("data.bin", &"x".repeat(1000));
    tmp.dir("sub");
    std::fs::hard_link(&data, tmp.path().join("link.bin")).unwrap();
    std::fs::hard_link(&data, tmp.path().join("sub/link.bin")).unwrap();
    tmp.file("other.bin", "12345");

    assert_eq!(
        dir_size_recursive_until(tmp.path(), false, || false),
        Some(1005)
    );
}

#[test]
fn size_walk_reports_whether_every_part_was_read() {
    let tmp = TempDir::new();
    tmp.dir("sub");
    tmp.file("sub/a.bin", "1234");
    assert_eq!(
        dir_size_walk_until(tmp.path(), false, || false),
        Some(DirWalk {
            size: 4,
            complete: true
        })
    );
    // A vanished child is gone, an unreadable one is a gap in the total.
    let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(!walk_error_leaves_gap(&gone));
    assert!(walk_error_leaves_gap(&denied));
}

#[cfg(unix)]
#[test]
fn size_walk_with_unreadable_subfolder_is_incomplete() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new();
    tmp.file("a.bin", "12");
    let locked = tmp.dir("locked");
    tmp.file("locked/b.bin", "345");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok(); // root ignores modes
    let walk = dir_size_walk_until(tmp.path(), false, || false);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(
        walk,
        Some(DirWalk {
            size: 2,
            complete: false
        })
    );
}

#[test]
fn cache_persistence_failures_warn_once() {
    let warned = AtomicBool::new(false);
    assert!(first_cache_warning(&warned));
    assert!(!first_cache_warning(&warned));
}

#[cfg(target_os = "linux")]
#[test]
fn recursive_size_does_not_walk_into_another_filesystem() {
    use std::os::unix::fs::MetadataExt;
    let proc_dev = std::fs::metadata("/proc").map(|m| m.dev());
    let root_dev = std::fs::metadata("/").map(|m| m.dev());
    if !matches!((proc_dev, root_dev), (Ok(p), Ok(r)) if p != r) {
        return; // no separate procfs mount to test against
    }
    // `/proc` listed in `/` is a mount point: sizing it must not walk
    // procfs (whose `kcore` alone reports ~128 TB).
    assert_eq!(
        dir_size_recursive_until(Path::new("/proc"), false, || false),
        Some(0)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn recursive_size_measures_a_local_mount_point_root() {
    use std::os::unix::fs::MetadataExt;
    let shm = Path::new("/dev/shm");
    let (Ok(shm_meta), Ok(dev_meta)) = (std::fs::metadata(shm), std::fs::metadata("/dev")) else {
        return; // no /dev/shm here
    };
    if shm_meta.dev() == dev_meta.dev() {
        return; // /dev/shm is not its own mount
    }
    let file = shm.join(format!("commander-size-test-{}", std::process::id()));
    if std::fs::write(&file, vec![0u8; 4096]).is_err() {
        return; // not writable in this sandbox
    }
    // `/dev/shm` is a tmpfs mount on another device than `/dev`: a local
    // mount-point row is measured in full, not reported as 0 B.
    let size = dir_size_recursive_until(shm, false, || false);
    let _ = std::fs::remove_file(&file);
    assert!(size.is_some_and(|size| size >= 4096), "got {size:?}");
}

#[test]
fn shuffled_duplicate_batch_deduplicates_before_top_k() {
    let key = |name: &str| VolumePathKey {
        path: PathBuf::from(format!("/batch/{name}")),
        volume_id: 1,
        generation: 1,
    };
    let a = key("a");
    let b = key("b");
    let started = std::time::Instant::now();
    let completed = |seconds| started + std::time::Duration::from_secs(seconds);
    let modified = |seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
    let entries = [
        (
            a.clone(),
            RankedDirSize {
                modified: modified(1),
                completed_at: Some(completed(100)),
                measured_at_millis: 100,
                size: 100,
            },
        ),
        (
            a.clone(),
            RankedDirSize {
                modified: modified(200),
                completed_at: Some(completed(99)),
                measured_at_millis: 99,
                size: 99,
            },
        ),
        (
            b.clone(),
            RankedDirSize {
                modified: modified(98),
                completed_at: Some(completed(98)),
                measured_at_millis: 98,
                size: 98,
            },
        ),
    ];
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    for order in permutations {
        let mut ordered = order.into_iter().map(|index| entries[index].clone());
        let mut cache = HashMap::new();
        let (first_key, first_value) = ordered.next().unwrap();
        cache.insert(first_key, first_value);

        extend_bounded_by(&mut cache, ordered, 2, 2, compare_ranked_dir_sizes);

        assert_eq!(cache.len(), 2, "order {order:?}");
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(2));
        assert_eq!(
            cache[&a].completed_at,
            Some(completed(100)),
            "order {order:?}"
        );
        assert_eq!(cache[&a].size, 100, "order {order:?}");
        assert_eq!(
            cache[&b].completed_at,
            Some(completed(98)),
            "order {order:?}"
        );
    }
}

#[test]
fn advisory_lock_state_is_recovered_after_poisoning() {
    let state = Mutex::new(1usize);
    let panic = std::panic::catch_unwind(|| {
        let mut state = state.lock().unwrap();
        *state = 2;
        panic!("poison advisory state");
    });

    assert!(panic.is_err());
    assert!(state.is_poisoned());
    assert_eq!(*lock_recover(&state), 2);
}

#[test]
fn dir_size_cache_pruning_is_bounded_and_deterministic() {
    let key = |name: &str| VolumePathKey {
        path: PathBuf::from(format!("/cache/{name}")),
        volume_id: 1,
        generation: 1,
    };
    let oldest = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    let mut cache = HashMap::from([
        (key("a"), CachedDirSize::new(oldest, 1, 1)),
        (key("b"), CachedDirSize::new(oldest, 2, 1)),
        (key("c"), CachedDirSize::new(oldest, 3, 2)),
        (key("d"), CachedDirSize::new(oldest, 4, 3)),
        (key("e"), CachedDirSize::new(oldest, 5, 4)),
    ]);

    prune_dir_size_cache_to(&mut cache, 4, 3);

    assert_eq!(cache.len(), 3);
    assert!(cache.capacity() <= hash_map_capacity_upper_bound(4));
    assert_eq!(
        cache
            .keys()
            .map(|key| key.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([key("c").path, key("d").path, key("e").path])
    );
}

#[test]
fn walk_log_pruning_is_bounded_and_deterministic() {
    let key = |name: &str| VolumePathKey {
        path: PathBuf::from(format!("/walk/{name}")),
        volume_id: 1,
        generation: 1,
    };
    let oldest = std::time::Instant::now();
    let cost = std::time::Duration::from_millis(1);
    let mut log = HashMap::from([
        (key("a"), (oldest, cost)),
        (key("b"), (oldest, cost)),
        (key("c"), (oldest + std::time::Duration::from_secs(1), cost)),
        (key("d"), (oldest + std::time::Duration::from_secs(2), cost)),
        (key("e"), (oldest + std::time::Duration::from_secs(3), cost)),
    ]);

    prune_walk_log_to(&mut log, 4, 3);

    assert_eq!(log.len(), 3);
    assert!(log.capacity() <= hash_map_capacity_upper_bound(4));
    assert_eq!(
        log.keys()
            .map(|key| key.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([key("c").path, key("d").path, key("e").path])
    );
}

#[test]
fn oversized_batch_is_selected_before_extend_and_shrinks_capacity() {
    let key = |prefix: &str, index: usize| VolumePathKey {
        path: PathBuf::from(format!("/{prefix}/{index:03}")),
        volume_id: 1,
        generation: 1,
    };
    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    let mut cache = HashMap::with_capacity(1_024);
    cache.extend((0..4).map(|index| {
        (
            key("old", index),
            CachedDirSize::new(base, index as u64, index as u64),
        )
    }));
    let incoming: Vec<_> = (0..100)
        .map(|index| {
            (
                key("incoming", index),
                CachedDirSize::new(base, index as u64, index as u64 + 4),
            )
        })
        .collect();

    extend_bounded_by(&mut cache, incoming, 8, 6, compare_cache_entries);

    assert_eq!(cache.len(), 6);
    assert!(cache.capacity() <= hash_map_capacity_upper_bound(8));
    assert_eq!(
        cache
            .keys()
            .map(|key| key.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        (94..100).map(|index| key("incoming", index).path).collect()
    );
}

#[test]
fn corrupt_persisted_mtime_is_dropped_without_panicking() {
    let entry = |path: &str, mtime_secs: u64, mtime_nanos: u32| PersistedCacheEntry {
        key: VolumePathKey {
            path: PathBuf::from(path),
            volume_id: 1,
            generation: 1,
        },
        mtime_secs,
        mtime_nanos,
        size: 7,
        measured_at_millis: 0,
    };
    let persisted = PersistedCache {
        schema: 1,
        entries: vec![
            // Carrying the nanos into the seconds would overflow them.
            entry("/corrupt/carry", u64::MAX, 1_000_000_000),
            entry("/corrupt/nanos", 5, u32::MAX),
            entry("/valid", 5, 999_999_999),
        ],
    };
    let json = serde_json::to_vec(&persisted).unwrap();

    let cache = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();

    assert_eq!(
        cache.keys().map(|key| key.path.clone()).collect::<Vec<_>>(),
        vec![PathBuf::from("/valid")]
    );
}

#[test]
fn oversized_persisted_load_is_streamed_and_capacity_bounded() {
    let persisted = PersistedCache {
        schema: 1,
        entries: (0..50)
            .map(|index| PersistedCacheEntry {
                key: VolumePathKey {
                    path: PathBuf::from(format!("/load/{index:03}")),
                    volume_id: 1,
                    generation: 1,
                },
                mtime_secs: index as u64 + 1,
                mtime_nanos: 0,
                size: index as u64,
                measured_at_millis: index as u64,
            })
            .collect(),
    };
    let json = serde_json::to_vec(&persisted).unwrap();

    let cache = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();

    assert_eq!(cache.len(), 6);
    assert!(cache.capacity() <= hash_map_capacity_upper_bound(8));
    assert_eq!(
        cache
            .keys()
            .map(|key| key.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        (44..50)
            .map(|index| PathBuf::from(format!("/load/{index:03}")))
            .collect()
    );
}

#[test]
fn shuffled_persisted_duplicates_are_deduplicated_before_top_k() {
    let key = |name: &str| VolumePathKey {
        path: PathBuf::from(format!("/load/{name}")),
        volume_id: 1,
        generation: 1,
    };
    let a = key("a");
    let b = key("b");
    let entries = [
        PersistedCacheEntry {
            key: a.clone(),
            mtime_secs: 100,
            mtime_nanos: 0,
            size: 100,
            measured_at_millis: 100,
        },
        PersistedCacheEntry {
            key: a.clone(),
            mtime_secs: 99,
            mtime_nanos: 0,
            size: 99,
            measured_at_millis: 99,
        },
        PersistedCacheEntry {
            key: b.clone(),
            mtime_secs: 98,
            mtime_nanos: 0,
            size: 98,
            measured_at_millis: 98,
        },
    ];
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    for order in permutations {
        let persisted = PersistedCache {
            schema: 1,
            entries: order
                .into_iter()
                .map(|index| PersistedCacheEntry {
                    key: entries[index].key.clone(),
                    mtime_secs: entries[index].mtime_secs,
                    mtime_nanos: entries[index].mtime_nanos,
                    size: entries[index].size,
                    measured_at_millis: entries[index].measured_at_millis,
                })
                .collect(),
        };
        let json = serde_json::to_vec(&persisted).unwrap();

        let cache = load_cache_from_reader_with_bounds(json.as_slice(), 2, 2).unwrap();

        assert_eq!(cache.len(), 2, "order {order:?}");
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(2));
        assert_eq!(
            cache[&a],
            CachedDirSize::new(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(100),
                100,
                100,
            ),
            "order {order:?}"
        );
        assert_eq!(
            cache[&b],
            CachedDirSize::new(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(98),
                98,
                98,
            ),
            "order {order:?}"
        );
    }
}

#[test]
fn batch_between_retain_and_limit_is_not_pruned() {
    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    let incoming = (0..9_500).map(|index| {
        (
            VolumePathKey {
                path: PathBuf::from(format!("/batch-threshold/{index:05}")),
                volume_id: 1,
                generation: 1,
            },
            CachedDirSize::new(base, index as u64, index as u64),
        )
    });
    let mut cache = HashMap::new();

    extend_bounded_by(
        &mut cache,
        incoming,
        DIR_SIZE_CACHE_LIMIT,
        DIR_SIZE_CACHE_RETAIN,
        compare_cache_entries,
    );

    assert_eq!(cache.len(), 9_500);
    assert!(cache.capacity() <= hash_map_capacity_upper_bound(DIR_SIZE_CACHE_LIMIT));
}

#[test]
fn persisted_load_between_retain_and_limit_is_not_pruned() {
    let persisted = PersistedCache {
        schema: 1,
        entries: (0..9_500)
            .map(|index| PersistedCacheEntry {
                key: VolumePathKey {
                    path: PathBuf::from(format!("/load-threshold/{index:05}")),
                    volume_id: 1,
                    generation: 1,
                },
                mtime_secs: index as u64 + 1,
                mtime_nanos: 0,
                size: index as u64,
                measured_at_millis: index as u64,
            })
            .collect(),
    };
    let json = serde_json::to_vec(&persisted).unwrap();

    let cache = load_cache_from_reader_with_bounds(
        json.as_slice(),
        DIR_SIZE_CACHE_LIMIT,
        DIR_SIZE_CACHE_RETAIN,
    )
    .unwrap();

    assert_eq!(cache.len(), 9_500);
    assert!(cache.capacity() <= hash_map_capacity_upper_bound(DIR_SIZE_CACHE_LIMIT));
}

#[test]
fn deep_change_recomputes_dir_size_without_reloading_listing() {
    fn wait_for_size(p: &PanelState, dir: &Path, expected: u64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if p.size_snapshot().size_of(dir) == Some(expected) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "size {expected} for {dir:?} not observed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let tmp = TempDir::new();
    tmp.dir("sub/deep");
    tmp.file("sub/deep/a.bin", "12345"); // 5 bytes
    let sub = tmp.path().join("sub");

    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    wait_for_size(&p, &sub, 5);

    // A deep change: sub's own mtime does not move, so the mtime
    // cache alone would keep serving the stale 5 bytes.
    let new_file = tmp.file("sub/deep/b.bin", "1234567"); // +7 bytes

    // What the recursive watcher does on such an event:
    invalidate_size_cache(&new_file);
    p.sizes.mark_dirty_immediately_for_test();
    reset_walk_log(); // bypass the walk cooldown in the test

    let reloaded = p.poll_fs_changes();
    assert!(!reloaded, "sizes-only events must not reload the listing");
    wait_for_size(&p, &sub, 12);
}

#[test]
fn mutation_during_subscription_snapshot_handoff_forces_reconciliation() {
    let tmp = TempDir::new();
    tmp.file("before.txt", "before");
    let mut panel = PanelState::new(tmp.path().to_path_buf());
    panel.refresh();

    panel.watcher.activate_test_binding(tmp.path());
    let subscription_ticket = panel.watcher.snapshot_ticket();

    // Deterministic handoff barrier: the subscription is active, but its
    // first complete listing has not yet been acknowledged.
    let created = tmp.file("during.txt", "during");
    panel
        .watcher
        .inject_current_change_for_test(created.clone());
    assert!(panel.reload_entries());
    let listing_binding = panel.listing.binding().to_path_buf();
    panel
        .watcher
        .acknowledge_snapshot(subscription_ticket.clone(), &listing_binding);

    assert!(panel.poll_fs_changes());
    assert!(panel.entries().iter().any(|entry| entry.path == created));
    assert!(!panel.watcher.has_pending_reconciliation_for_test());
}

#[test]
fn watcher_gap_replaces_the_listing_and_applies_its_generation() {
    let health_before = crate::watcher_health::snapshot();
    let tmp = TempDir::new();
    tmp.file("before.txt", "before");
    let mut panel = PanelState::new(tmp.path().to_path_buf());
    panel.refresh();
    assert!(
        panel
            .entries()
            .iter()
            .any(|entry| entry.name == "before.txt")
    );

    std::fs::remove_file(tmp.path().join("before.txt")).unwrap();
    tmp.file("after.txt", "after");
    panel.watcher.activate_test_binding(tmp.path());
    panel.watcher.inject_gap_for_test();

    assert!(panel.poll_fs_changes());
    assert!(
        panel
            .entries()
            .iter()
            .any(|entry| entry.name == "after.txt")
    );
    assert!(
        !panel
            .entries()
            .iter()
            .any(|entry| entry.name == "before.txt")
    );
    assert!(!panel.watcher.has_pending_reconciliation_for_test());
    let health_after = crate::watcher_health::snapshot();
    assert!(health_after.listing_reconciliations > health_before.listing_reconciliations);
    assert!(health_after.gap_reconciliations > health_before.gap_reconciliations);
}

/// Profiling harness, not a test: drives a real watcher + poll loop
/// at ~60 "fps" against `COMMANDER_PROFILE_ROOT` for
/// `COMMANDER_PROFILE_SECS` seconds while an external driver generates
/// fs activity and samples this process's CPU.
/// Run: cargo test --release `watcher_profile_harness` -- --ignored --nocapture
#[test]
#[ignore = "profiling harness, run manually"]
fn watcher_profile_harness() {
    let root = std::env::var("COMMANDER_PROFILE_ROOT").expect("set COMMANDER_PROFILE_ROOT");
    let secs: u64 = std::env::var("COMMANDER_PROFILE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let mut p = PanelState::new(PathBuf::from(root));
    p.refresh();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut reloads = 0u32;
    while std::time::Instant::now() < deadline {
        if p.poll_fs_changes() {
            reloads += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    println!("harness done: {} listing reloads", reloads);
}

#[test]
fn make_preview_marks_text_pending_without_reading_and_skips_dirs() {
    let tmp = TempDir::new();
    let file = tmp.file("note.txt", "hello");
    let meta = std::fs::metadata(&file).unwrap();
    let fe = FileEntry::from_meta(file.clone(), &meta).unwrap();
    std::fs::remove_file(file).unwrap();
    match make_preview(&fe) {
        Some(PreviewContent::Pending(identity)) => assert_eq!(identity.path, fe.path),
        other => panic!("expected pending preview, got {other:?}"),
    }

    let dir = tmp.dir("d");
    let meta = std::fs::metadata(&dir).unwrap();
    let de = FileEntry::from_meta(dir, &meta).unwrap();
    assert!(make_preview(&de).is_none());
}

#[test]
fn visible_window_clamps_anchor_and_has_a_cold_start_default() {
    assert_eq!(visible_window(100, 40, 10), 40..72);
    assert_eq!(visible_window(8, 99, 20), 0..8);
    assert_eq!(visible_window(20, 0, 0), 0..20);
}

/// `entry` rows whose extension is filled in from the name, as a real
/// listing does.
fn with_extensions(mut entries: Vec<FileEntry>) -> Vec<FileEntry> {
    for e in &mut entries {
        e.extension = Path::new(&e.name)
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
    }
    entries
}

#[test]
fn subtracting_mask_deselects_an_already_selected_entry() {
    let mut p = panel_with(with_extensions(vec![
        entry("a.jpg", false, 1),
        entry("raw.jpg", false, 1),
    ]));
    p.select_path(PathBuf::from("/test/raw.jpg"));
    p.select_path(PathBuf::from("/test/a.jpg"));

    // Nothing was added, but the selected match is removed.
    assert_eq!(p.select_by_mask("!*raw*"), 0);
    assert!(!p.is_selected(Path::new("/test/raw.jpg")));
    assert!(p.is_selected(Path::new("/test/a.jpg")));
}

#[test]
fn parse_mask_trims_space_after_the_subtract_marker() {
    assert_eq!(
        parse_mask("! *.TMP, -  Raw"),
        vec![("*.tmp".to_string(), true), ("raw".to_string(), true)]
    );
}

#[test]
fn select_largest_takes_every_file_when_n_exceeds_the_count() {
    let mut p = panel_with(vec![
        entry("a", false, 10),
        entry("b", false, 20),
        entry("c", false, 30),
        entry("dir", true, 0),
    ]);
    assert_eq!(p.select_largest(10), 3);
    assert_eq!(selected_names(&p), ["a", "b", "c"].map(String::from).into());
}

#[test]
fn select_largest_breaks_size_ties_by_path_whatever_the_order() {
    for names in [
        ["x", "y", "small"],
        ["y", "x", "small"],
        ["small", "y", "x"],
    ] {
        let size = |name: &str| if name == "small" { 1 } else { 50 };
        let mut p = panel_with(names.map(|n| entry(n, false, size(n))).into());
        assert_eq!(p.select_largest(1), 1);
        // The bounded heap evicts the smaller (size, path) first.
        assert_eq!(selected_names(&p), ["y".to_string()].into(), "{names:?}");
    }
}

#[test]
fn select_largest_ignores_files_hidden_by_the_filter() {
    let mut p = panel_with(vec![
        entry("huge.iso", false, 9_000),
        entry("notes-big.txt", false, 500),
        entry("notes-small.txt", false, 5),
    ]);
    p.set_search_query("notes");
    assert_eq!(p.select_largest(1), 1);
    assert_eq!(selected_names(&p), ["notes-big.txt".to_string()].into());
}

#[test]
fn select_same_extension_does_nothing_off_a_file_with_an_extension() {
    let mut p = panel_with(with_extensions(vec![
        entry("box", true, 0),
        entry("Makefile", false, 1),
        entry("a.rs", false, 1),
        entry("b.rs", false, 1),
    ]));
    p.sort_entries(); // box, a.rs, b.rs, Makefile
    for (row, what) in [(0, ".."), (1, "a folder"), (4, "an extension-less file")] {
        p.set_cursor(row);
        assert_eq!(p.select_same_extension_as_cursor(), 0, "cursor on {what}");
        assert!(p.selection_is_empty(), "cursor on {what}");
    }
}

#[test]
fn type_ahead_matches_typed_capitals_case_insensitively() {
    let mut p = panel_with(vec![
        entry("apple.txt", false, 1),
        entry("Banana.txt", false, 1),
    ]);
    p.sort_entries();
    assert!(p.type_ahead("BAN"));
    assert_eq!(p.cursor_entry().unwrap().name, "Banana.txt");
}

#[test]
fn cursor_entry_is_none_when_another_entry_moved_under_the_cursor() {
    let mut p = panel_with(vec![
        entry("a", false, 1),
        entry("b", false, 1),
        entry("c", false, 1),
    ]);
    p.set_cursor(1);
    assert_eq!(p.cursor_entry().unwrap().name, "a");

    // Row 1 now holds "c", which the cursor never focused.
    p.mutate_entries_for_test(|entries| entries.reverse());
    assert!(p.cursor_entry().is_none());
    assert_eq!(
        p.selected_or_cursor().unwrap_err(),
        StaleCursor {
            cursor: 1,
            visible_entries: 3,
        }
    );
}

#[test]
fn ensure_cursor_valid_clamps_a_cursor_past_the_end_and_is_otherwise_a_no_op() {
    let mut p = panel_with(vec![
        entry("a", false, 1),
        entry("b", false, 1),
        entry("c", false, 1),
    ]);
    p.set_cursor(1);
    p.set_scroll_to_cursor(false);
    p.ensure_cursor_valid();
    assert_eq!(p.cursor(), 1);
    assert!(!p.scroll_to_cursor(), "a valid cursor is left alone");

    p.set_cursor(3);
    p.set_scroll_to_cursor(false);
    p.mutate_entries_for_test(|entries| {
        entries.pop();
    });
    p.ensure_cursor_valid();
    assert_eq!(p.cursor(), 2);
    assert_eq!(p.cursor_entry().unwrap().name, "b");
    assert!(p.scroll_to_cursor());
}

#[test]
fn parent_row_cursor_stays_put_across_filter_and_sort_changes() {
    let mut p = panel_with(vec![entry("a", false, 2), entry("b", false, 1)]);
    p.set_cursor(0);
    p.set_scroll_to_cursor(false);

    p.set_search_query("a");
    assert_eq!(p.cursor(), 0);
    p.set_sort(SortColumn::Size);
    assert_eq!(p.cursor(), 0);
    p.clear_filters();
    assert_eq!(p.cursor(), 0);
    assert!(p.cursor_entry().is_none());
    assert!(!p.scroll_to_cursor(), "the .. row is not scrolled to");
}

#[test]
fn clear_filters_drops_query_and_facets_and_keeps_focus() {
    let mut p = panel_with(with_extensions(vec![
        entry("alpha.jpg", false, 1),
        entry("beta.jpg", false, 1),
        entry("notes.txt", false, 1),
    ]));
    p.sort_entries();
    p.set_facets(FacetSet {
        kind: Some(KindFacet::Images),
        ..FacetSet::default()
    });
    p.set_search_query("beta");
    assert_eq!(p.filtered_count(), 1);
    p.set_cursor(1);
    let focused = p.cursor_entry().unwrap().path.clone();

    p.clear_filters();

    assert_eq!(p.search_query(), "");
    assert_eq!(p.facets(), FacetSet::default());
    assert_eq!(p.filtered_count(), 3);
    assert_eq!(p.cursor_entry().map(|e| e.path.clone()), Some(focused));
    assert_eq!(p.cursor(), 2);
}

#[test]
fn clear_selection_keeps_picks_remembered_in_other_folders() {
    let tmp = TempDir::new();
    let a = tmp.dir("a");
    let b = tmp.dir("b");
    let in_a = tmp.file("a/x.txt", "x");
    let in_b = tmp.file("b/y.txt", "y");

    let mut p = PanelState::new(a);
    p.refresh();
    p.toggle_select(in_a);
    p.navigate_to(b);
    p.toggle_select(in_b);

    p.clear_selection();
    assert!(p.selection_is_empty());
    assert_eq!(p.selected_elsewhere_count(), 1, "only this folder clears");

    p.clear_selection_everywhere();
    assert_eq!(p.selected_elsewhere_count(), 0);
}

#[test]
fn picks_only_in_other_folders_win_over_the_cursor_here() {
    let tmp = TempDir::new();
    let a = tmp.dir("a");
    let b = tmp.dir("b");
    let in_a = tmp.file("a/x.txt", "x");
    tmp.file("b/y.txt", "y");

    let mut p = PanelState::new(a);
    p.refresh();
    p.toggle_select(in_a.clone());
    p.navigate_to(b);
    p.set_cursor(1);
    assert_eq!(p.cursor_entry().unwrap().name, "y.txt");

    let picked: Vec<PathBuf> = p
        .picked_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    assert_eq!(picked, vec![in_a]);
}

#[test]
fn remembered_scroll_anchor_is_clamped_when_the_folder_shrank() {
    let tmp = TempDir::new();
    let downloads = tmp.dir("downloads");
    let docs = tmp.dir("docs");
    let files: Vec<PathBuf> = (0..12)
        .map(|i| tmp.file(&format!("downloads/f{i:02}.txt"), "x"))
        .collect();

    let mut p = PanelState::new(downloads.clone());
    p.refresh();
    p.set_cursor(11); // f10
    p.set_scroll_anchor(10);
    p.navigate_to(docs);
    for file in &files[1..11] {
        std::fs::remove_file(file).unwrap();
    }
    p.navigate_to(downloads);

    assert_eq!(p.filtered_count(), 2);
    assert!(
        p.scroll_anchor() < p.filtered_count(),
        "{}",
        p.scroll_anchor()
    );
    assert_eq!(p.cursor(), 2);
    assert_eq!(p.cursor_entry().unwrap().path, files[11]);
}

#[test]
fn navigating_away_clears_an_in_progress_drag() {
    let tmp = TempDir::new();
    let file = tmp.file("x.txt", "x");
    let sub = tmp.dir("sub");
    let other = tmp.dir("other");
    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();

    p.begin_drag(file.clone());
    p.drag.set_drop_target(tmp.path().to_path_buf());
    p.navigate_to(sub.clone());
    assert!(p.drag.is_empty(), "synchronous listing");
    assert!(!p.drag.has_drop_target());

    // A background listing the test never runs: the drag is dropped as
    // soon as the breadcrumb changes, not when rows land.
    let _workload = deterministic_listings(&mut p);
    p.begin_drag(sub.join("y"));
    p.drag.set_drop_target(sub);
    p.navigate_to(other);
    assert_eq!(p.listing.status(), DirStatus::Loading);
    assert!(p.drag.is_empty(), "asynchronous listing");
    assert!(!p.drag.has_drop_target());
}

#[test]
fn from_meta_derives_a_lowercase_last_extension() {
    let tmp = TempDir::new();
    let extension = |name: &str| {
        let path = tmp.file(name, "x");
        let meta = std::fs::metadata(&path).unwrap();
        FileEntry::from_meta(path, &meta).unwrap().extension
    };
    assert_eq!(extension("PHOTO.JPG"), "jpg");
    assert_eq!(extension(".bashrc"), "");
    assert_eq!(extension("a.tar.gz"), "gz");
    assert_eq!(extension("Makefile"), "");
    let dir = tmp.dir("Folder.D");
    let meta = std::fs::metadata(&dir).unwrap();
    let folder = FileEntry::from_meta(dir, &meta).unwrap();
    assert_eq!(folder.extension, "d");
    assert!(folder.is_dir);
    assert_eq!(folder.size, 0);
}

#[test]
fn videos_count_as_images_but_not_as_static_images() {
    let [video, still, text] = with_extensions(vec![
        entry("clip.MP4", false, 1),
        entry("pic.png", false, 1),
        entry("a.txt", false, 1),
    ])
    .try_into()
    .unwrap();
    assert!(video.is_video() && video.is_image() && !video.is_static_image());
    assert!(!still.is_video() && still.is_image() && still.is_static_image());
    assert!(!text.is_video() && !text.is_image());
}

/// Hand-driven background listings for `p`: the worker runs only when
/// the test says so.
fn deterministic_listings(p: &mut PanelState) -> crate::workload::DeterministicWorkload {
    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    p.set_workload(workload.handle());
    p.set_notify(Arc::new(|| {}));
    workload
}

#[test]
fn async_tab_activation_restores_focus_after_loading_frame_clamps_cursor() {
    let tmp = TempDir::new();
    tmp.file("aaa.txt", "a");
    let target = tmp.file("zzz.txt", "z");
    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    p.focus_child("zzz.txt".into());
    p.select_path(target.clone());
    p.set_scroll_anchor(1);
    p.suspend_for_tab();
    p.prepare_tab_activation();
    let workload = deterministic_listings(&mut p);
    p.refresh();
    assert_eq!(p.dir_status(), DirStatus::Loading);
    // The UI polls filesystem changes and validates cursor rows before
    // the asynchronous listing finishes, while no rows are exposed.
    p.ensure_cursor_valid();
    assert!(p.cursor_entry().is_none());
    assert!(workload.run_next());
    assert!(p.poll_listing());
    assert_eq!(p.cursor_entry().unwrap().path, target);
    assert!(p.is_selected(&target));
    assert_eq!(p.scroll_anchor(), 1);
}

#[test]
fn async_go_up_lands_on_the_child_once_the_listing_publishes() {
    let tmp = TempDir::new();
    tmp.dir("aaa");
    let mid = tmp.dir("mmm");
    tmp.dir("zzz");
    let mut p = PanelState::new(mid);
    p.refresh();
    let workload = deterministic_listings(&mut p);

    p.go_up();
    assert_eq!(p.listing.status(), DirStatus::Loading);
    assert_eq!(p.cursor(), 0, "nothing to land on yet");

    assert!(workload.run_next());
    assert!(p.poll_listing());
    assert_eq!(p.cursor_entry().unwrap().name, "mmm");
    assert!(p.scroll_to_cursor());
}

#[test]
fn first_async_listing_lands_on_a_launch_file() {
    // App startup: the panel is built unlisted, then the first frame
    // wires the workload, refreshes and focuses the launch file.
    let tmp = TempDir::new();
    tmp.file("aaa.txt", "a");
    tmp.file("notes.txt", "n");
    tmp.file("zzz.txt", "z");
    let mut p = PanelState::new(tmp.path().to_path_buf());
    let workload = deterministic_listings(&mut p);
    p.refresh();
    p.focus_child("notes.txt".to_string());

    assert!(workload.run_next());
    assert!(p.poll_listing());
    assert_eq!(p.cursor_entry().unwrap().name, "notes.txt");
}

#[test]
fn listing_read_under_the_old_hidden_setting_is_not_applied() {
    let tmp = TempDir::new();
    tmp.file("visible.txt", "v");
    let hidden = tmp.file(".hidden.txt", "h");
    let mut p = PanelState::new(tmp.path().to_path_buf());
    p.refresh();
    let workload = deterministic_listings(&mut p);

    // A background re-read starts with hidden files off, then the user
    // requests hidden files before it lands. Neither result may publish
    // under the other's view configuration.
    p.refresh();
    assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Pending);
    assert!(!p.entries().iter().any(|entry| entry.path == hidden));
    let revision = p.entries_gen();

    assert!(workload.run_next());
    assert!(
        p.poll_listing(),
        "the cancelled stale read is skipped and the current hidden read is applied"
    );
    assert!(p.entries().iter().any(|entry| entry.path == hidden));
    assert_eq!(p.entries_gen(), revision + 1);
}

#[test]
fn preview_identity_goes_stale_when_size_or_date_changes() {
    let mut file = entry("notes.txt", false, 5);
    file.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100));
    let identity = PreviewIdentity::from_entry(&file);
    assert!(identity.matches_entry(&file));

    let mut grown = file.clone();
    grown.size = 6;
    assert!(!identity.matches_entry(&grown));

    let mut touched = file.clone();
    touched.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(101));
    assert!(!identity.matches_entry(&touched));
    touched.modified = None;
    assert!(!identity.matches_entry(&touched));

    let mut moved = file;
    moved.path = PathBuf::from("/test/other.txt");
    assert!(!identity.matches_entry(&moved));
}

#[cfg(unix)]
#[test]
fn recursive_size_does_not_follow_folder_symlinks_or_loop() {
    let outside = TempDir::new();
    outside.file("big.bin", &"x".repeat(4000));
    let tmp = TempDir::new();
    tmp.file("small.bin", "12345");
    tmp.dir("sub");
    // A link back to the walked folder and one to a folder elsewhere.
    std::os::unix::fs::symlink(tmp.path(), tmp.path().join("sub/loop")).unwrap();
    std::os::unix::fs::symlink(outside.path(), tmp.path().join("away")).unwrap();
    let link_len = |name: &str| {
        std::fs::symlink_metadata(tmp.path().join(name))
            .unwrap()
            .len()
    };

    assert_eq!(
        dir_size_walk_until(tmp.path(), false, || false),
        Some(DirWalk {
            size: 5 + link_len("sub/loop") + link_len("away"),
            complete: true,
        })
    );
}

#[test]
fn cache_files_from_another_schema_load_empty() {
    let entry = r#"{"key":{"path":"/s","volume_id":1,"generation":1},"mtime_secs":5,"mtime_nanos":0,"size":7}"#;
    for json in [
        format!(r#"{{"schema":3,"entries":[{entry}]}}"#),
        format!(r#"{{"entries":[{entry}]}}"#),
        format!(r#"{{"schema":0,"entries":[{entry}]}}"#),
    ] {
        let cache = load_cache_from_reader_with_bounds(json.as_bytes(), 8, 6).unwrap();
        assert!(cache.is_empty(), "{json}");
    }
    for schema in [1, 2] {
        let current = format!(r#"{{"schema":{schema},"entries":[{entry}]}}"#);
        let cache = load_cache_from_reader_with_bounds(current.as_bytes(), 8, 6).unwrap();
        assert_eq!(cache.len(), 1);
    }
}

#[test]
fn persisted_snapshot_round_trips_and_drops_pre_epoch_dates() {
    let key = |name: &str| VolumePathKey {
        path: PathBuf::from(format!("/persist/{name}")),
        volume_id: 3,
        generation: 4,
    };
    let at = |secs: u64, nanos: u32| SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, nanos);
    let cache = HashMap::from([
        (key("b"), CachedDirSize::new(at(200, 5), 20, 2)),
        (key("a"), CachedDirSize::new(at(100, 999_999_999), 10, 1)),
        (
            key("ancient"),
            CachedDirSize::new(
                SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1),
                30,
                0,
            ),
        ),
    ]);

    let persisted = persisted_snapshot(&cache);
    assert_eq!(persisted.schema, 2);
    let order: Vec<_> = persisted.entries.iter().map(|e| e.key.clone()).collect();
    assert_eq!(order, [key("a"), key("b")], "sorted, pre-1970 dropped");

    let json = serde_json::to_vec(&persisted).unwrap();
    let loaded = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();
    let mut expected = cache;
    expected.remove(&key("ancient"));
    assert_eq!(loaded, expected);
}

#[test]
fn path_epoch_is_shared_while_live_and_replaced_once_cancelled() {
    let tmp = TempDir::new();
    let key = VolumePathKey::observe(&tmp.dir("epoch"));
    let first = capture_path_epoch(&key);
    let second = capture_path_epoch(&key);
    assert!(Arc::ptr_eq(&first, &second), "concurrent walkers share it");

    first.cancel();
    let fresh = capture_path_epoch(&key);
    assert!(!Arc::ptr_eq(&first, &fresh));
    assert!(!fresh.is_cancelled());
    assert!(Arc::ptr_eq(&fresh, &capture_path_epoch(&key)));
}

#[test]
fn invalidation_keeps_a_sibling_that_shares_a_name_prefix() {
    let tmp = TempDir::new();
    let b = VolumePathKey::observe(&tmp.dir("b"));
    let bc = VolumePathKey::observe(&tmp.dir("bc"));
    let now = SystemTime::now();
    {
        let _boundary = lock_recover(scan_commit_boundary());
        let mut cache = lock_recover(dir_size_cache());
        cache.insert(b.clone(), CachedDirSize::new(now, 1, 1));
        cache.insert(bc.clone(), CachedDirSize::new(now, 2, 1));
    }

    invalidate_size_cache(&tmp.path().join("bc/file.txt"));

    let cache = lock_recover(dir_size_cache());
    assert!(!cache.contains_key(&bc));
    assert_eq!(
        cache.get(&b),
        Some(&CachedDirSize::new(now, 1, 1)),
        "b is not an ancestor of bc"
    );
}

/// A fresh panel scan over one folder, with its own size map.
struct TestScan {
    current: Mutex<Arc<ScanEpoch>>,
    epoch: Arc<ScanEpoch>,
    sizes: Mutex<HashMap<PathBuf, u64>>,
    revision: AtomicU64,
}

fn scan_for(directory: &Path) -> TestScan {
    let current = Mutex::new(Arc::new(ScanEpoch::new()));
    let sizes = Mutex::new(HashMap::new());
    let revision = AtomicU64::new(0);
    let epoch = begin_panel_scan(
        &current,
        &sizes,
        &Mutex::new(HashMap::new()),
        &HashSet::from([directory.to_path_buf()]),
        &revision,
    );
    TestScan {
        current,
        epoch,
        sizes,
        revision,
    }
}

#[test]
fn cached_size_is_reused_only_for_the_same_folder_mtime() {
    let tmp = TempDir::new();
    let directory = tmp.dir("cached");
    let key = VolumePathKey::observe(&directory);
    let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5_000);
    {
        let _boundary = lock_recover(scan_commit_boundary());
        lock_recover(dir_size_cache()).insert(key.clone(), CachedDirSize::new(mtime, 77, 1));
    }
    let scan = scan_for(&directory);
    let sizes = &scan.sizes;
    let publish = |expected: Option<SystemTime>| {
        publish_cached_size_if_current(
            &scan.current,
            &scan.epoch,
            sizes,
            &directory,
            &key,
            expected,
            &scan.revision,
        )
    };

    let later = mtime + std::time::Duration::from_secs(1);
    assert!(!publish(Some(later)), "the folder changed since");
    assert_eq!(lock_recover(sizes).get(&directory), None);

    assert!(publish(Some(mtime)));
    assert_eq!(lock_recover(sizes).get(&directory), Some(&77));
    // No expected mtime (a walk held back by its cooldown) takes it as is.
    lock_recover(sizes).clear();
    assert!(publish(None));
    assert_eq!(lock_recover(sizes).get(&directory), Some(&77));
}

#[test]
fn measurement_without_an_mtime_is_shown_but_not_cached() {
    let tmp = TempDir::new();
    let directory = tmp.dir("undated");
    let cache_key = VolumePathKey::observe(&directory);
    let path_epoch = capture_path_epoch(&cache_key);
    let scan = scan_for(&directory);
    let measurement = DirMeasurement {
        path: directory.clone(),
        modified: None,
        complete: true,
        cache_key: cache_key.clone(),
        path_epoch,
        size: 9,
        completed_at: std::time::Instant::now(),
        elapsed: std::time::Duration::from_millis(1),
    };

    let outcome = publish_dir_measurements_if_current(
        &scan.current,
        &scan.epoch,
        &scan.sizes,
        &[measurement],
        &scan.revision,
    );

    assert_eq!(outcome.published, 1);
    assert!(!outcome.cache_changed);
    assert_eq!(lock_recover(&scan.sizes).get(&directory), Some(&9));
    let _boundary = lock_recover(scan_commit_boundary());
    assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));
    assert!(!lock_recover(partial_size_cache()).contains_key(&cache_key));
}

#[test]
fn merging_a_measurement_replaces_the_folders_old_mount_generation() {
    let key = |name: &str, generation: u64| VolumePathKey {
        path: PathBuf::from(format!("/merge/{name}")),
        volume_id: 1,
        generation,
    };
    let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let measured = DirMeasurement {
        path: PathBuf::from("/merge/a"),
        modified: Some(at(50)),
        complete: true,
        cache_key: key("a", 2),
        path_epoch: Arc::new(ScanEpoch::new()),
        size: 42,
        completed_at: std::time::Instant::now(),
        elapsed: std::time::Duration::from_millis(1),
    };
    let previous = HashMap::from([
        (key("a", 1), CachedDirSize::new(at(40), 1, 1)),
        (key("other", 1), CachedDirSize::new(at(30), 3, 2)),
    ]);

    let merged = merge_measurements_into_cache(previous, &[&measured], 8, 6);

    assert_eq!(
        merged,
        HashMap::from([
            (
                key("a", 2),
                CachedDirSize::new(at(50), 42, merged[&key("a", 2)].measured_at_millis)
            ),
            (key("other", 1), CachedDirSize::new(at(30), 3, 2)),
        ])
    );
}

#[test]
fn merging_past_the_limit_keeps_the_newest_entries() {
    let key = |index: u64| VolumePathKey {
        path: PathBuf::from(format!("/merge-bound/{index}")),
        volume_id: 1,
        generation: 1,
    };
    let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let previous: HashMap<_, _> = (0..4)
        .map(|i| (key(i), CachedDirSize::new(at(10 + i), i, i)))
        .collect();
    let measured = DirMeasurement {
        path: key(9).path,
        modified: Some(at(5)),
        complete: true,
        cache_key: key(9),
        path_epoch: Arc::new(ScanEpoch::new()),
        size: 9,
        completed_at: std::time::Instant::now(),
        elapsed: std::time::Duration::from_millis(1),
    };

    let merged = merge_measurements_into_cache(previous, &[&measured], 4, 3);

    // Five entries pass the limit of four; the freshly measured folder
    // outranks older cache rows even though its directory mtime is old.
    let mut kept: Vec<_> = merged.keys().map(|key| key.path.clone()).collect();
    kept.sort();
    assert_eq!(kept, [key(2).path, key(3).path, key(9).path]);
    assert!(merged.capacity() <= hash_map_capacity_upper_bound(4));
}

#[test]
fn tree_subdirs_show_dot_folders_on_request_and_nothing_for_a_missing_path() {
    let temp = TempDir::new();
    let hidden = temp.dir(".hidden");
    let visible = temp.dir("visible");
    temp.file(".dotfile", "");

    assert_eq!(
        PanelState::subdirs(temp.path(), false),
        vec![visible.clone()]
    );
    assert_eq!(
        PanelState::subdirs(temp.path(), true),
        vec![hidden, visible]
    );
    assert!(PanelState::subdirs(&temp.path().join("missing"), true).is_empty());
    let file = temp.file("plain.txt", "x");
    assert!(PanelState::subdirs(&file, true).is_empty());
}
#[test]
fn browser_mode_follows_folder_memory_without_losing_selection() {
    let temp = crate::testutil::TempDir::new();
    let child = temp.dir("child");
    let file = temp.file("selected.txt", "text");
    let mut panel = PanelState::new(temp.path().to_path_buf());
    panel.refresh();
    panel.toggle_select(file.clone());
    panel.set_browser_mode(BrowserMode::Icons);
    assert!(panel.is_selected(&file));
    panel.navigate_to(child);
    panel.set_browser_mode(BrowserMode::List);
    panel.go_up();
    assert_eq!(panel.browser_mode(), BrowserMode::Icons);
    assert!(panel.is_selected(&file));
}

#[test]
#[ignore = "manual benchmark: cargo test --release large_listing_hot_paths -- --ignored --nocapture"]
fn large_listing_hot_paths() {
    const ROWS: usize = 100_000;
    let extensions = ["txt", "rs", "jpg", "md", "tar.gz"];
    let entries = (0..ROWS)
        .map(|index| {
            let extension = extensions[index % extensions.len()];
            let mut entry = entry(
                &format!("file-{index:06}.{extension}"),
                index % 10 == 0,
                index as u64,
            );
            entry.extension = extension.rsplit('.').next().unwrap_or_default().into();
            entry
        })
        .collect();
    let mut panel = panel_with(entries);
    fn report(panel: &mut PanelState, label: &str, f: &mut dyn FnMut(&mut PanelState)) {
        let started = std::time::Instant::now();
        for _ in 0..5 {
            f(panel);
        }
        println!("{label:<34} {:>10.3?}", started.elapsed() / 5);
    }
    report(&mut panel, "filter cold `file-0`", &mut |p| {
        p.set_search_query("");
        p.filtered_count();
        p.set_search_query("file-0");
        std::hint::black_box(p.filtered_count());
    });
    report(&mut panel, "filter typing f/fi/fil/file/file-0", &mut |p| {
        for query in ["f", "fi", "fil", "file", "file-0"] {
            p.set_search_query(query);
            std::hint::black_box(p.filtered_count());
        }
    });
    report(&mut panel, "filter `txt || rs`", &mut |p| {
        p.set_search_query("txt || rs");
        std::hint::black_box(p.filtered_count());
        p.set_search_query("");
    });
    for column in [
        SortColumn::Name,
        SortColumn::Kind,
        SortColumn::Size,
        SortColumn::Extension,
    ] {
        report(&mut panel, &format!("sort {column:?}"), &mut |p| {
            p.set_sort(column)
        });
    }
    panel.set_search_query("");
    panel.select_all();
    report(&mut panel, "visible selected count+size", &mut |p| {
        std::hint::black_box((p.visible_selected_count(), p.visible_selected_size()));
    });
    report(&mut panel, "footer summary (first, then cached)", &mut |p| {
        std::hint::black_box(p.visible_selection_summary());
    });
    report(&mut panel, "is_selected over every row", &mut |p| {
        let selected = p
            .filtered_iter()
            .filter(|entry| p.is_selected(&entry.path))
            .count();
        std::hint::black_box(selected);
    });
}
