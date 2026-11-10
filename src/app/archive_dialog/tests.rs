fn sort_group(size: u64, copies: usize) -> crate::archive_check::ContentGroup {
    crate::archive_check::ContentGroup {
        size,
        sha256: format!("{size}-{copies}"),
        members: (0..copies)
            .map(|index| crate::archive_check::ContentMember {
                index,
                path: format!("member-{index}").into(),
                size,
            })
            .collect(),
    }
}
#[test]
fn archive_group_totals_track_filters_errors_and_cache_invalidation() {
    use super::ContentGroupView;
    let mut groups = vec![sort_group(10, 5), sort_group(0, 2)];
    let mut view = ContentGroupView {
        min_copies: "3".into(),
        ..Default::default()
    };
    assert_eq!(view.indices(&groups), [0]);
    let all = view.scanned_totals.unwrap();
    let shown = view.shown_totals.unwrap();
    assert_eq!(
        (
            all.groups,
            all.members,
            all.payload_bytes,
            all.extra_members,
            all.extra_payload_bytes
        ),
        (2, 7, 50, 5, 40)
    );
    assert_eq!(
        (
            shown.groups,
            shown.members,
            shown.payload_bytes,
            shown.extra_members,
            shown.extra_payload_bytes
        ),
        (1, 5, 50, 4, 40)
    );
    view.max_copies = "4".into();
    view.invalidate();
    view.indices(&groups);
    assert_eq!(view.shown_totals, Some(Default::default()));
    assert_eq!(view.scanned_totals, Some(all));
    view.min_copies = "bad".into();
    view.invalidate();
    view.indices(&groups);
    assert!(view.shown_totals.is_none() && view.error.is_some());
    assert_eq!(view.scanned_totals, Some(all));
    groups[0].size = 20;
    view.min_copies.clear();
    view.max_copies.clear();
    view.invalidate();
    view.indices(&groups);
    assert_eq!(view.scanned_totals.unwrap().extra_payload_bytes, 80);
    assert_eq!(view.scanned_totals, view.shown_totals);
}
#[test]
fn archive_group_copy_bounds_validate_inclusive_platform_counts() {
    use super::content_group_copy_bounds as bounds;
    assert_eq!(bounds("", " ").unwrap(), (None, None));
    assert_eq!(bounds(" 0 ", "0002").unwrap(), (Some(0), Some(2)));
    assert_eq!(bounds("3", "3").unwrap(), (Some(3), Some(3)));
    assert_eq!(
        bounds(&usize::MAX.to_string(), "").unwrap(),
        (Some(usize::MAX), None)
    );
    for (min, max) in [
        ("4", "3"),
        ("-1", ""),
        ("+1", ""),
        ("1.0", ""),
        ("1e3", ""),
        ("１", ""),
        ("1 2", ""),
        ("", "x"),
        ("18446744073709551616", ""),
        ("000000000000000000000", ""),
    ] {
        assert!(bounds(min, max).is_err(), "{min} / {max}");
    }
}
#[test]
fn archive_group_copy_bounds_intersect_search_sort_and_paginate_original_groups() {
    use super::ContentGroupView;
    let groups = (0..202)
        .map(|rank| sort_group(rank as u64, if rank % 2 == 0 { 2 } else { 5 }))
        .collect::<Vec<_>>();
    let original = serde_json::to_value(&groups).unwrap();
    let mut view = ContentGroupView {
        min_copies: "3".into(),
        sort: crate::dedup::GroupSort::Copies,
        ..Default::default()
    };
    let wanted = (1..202).step_by(2).collect::<Vec<_>>();
    assert_eq!(view.indices(&groups), wanted);
    let mut page = usize::MAX;
    assert_eq!(
        super::content_group_page_range(view.order.len(), &mut page),
        100..101
    );
    assert_eq!(view.order[100], 201);
    view.max_copies = "4".into();
    view.invalidate();
    assert!(view.indices(&groups).is_empty() && view.error.is_none());
    view.min_copies.clear();
    view.invalidate();
    assert_eq!(
        view.indices(&groups),
        (0..202).step_by(2).collect::<Vec<_>>()
    );
    view.min_copies = "3".into();
    view.max_copies = "5".into();
    view.query = "no-such-member".into();
    view.invalidate();
    assert!(view.indices(&groups).is_empty() && view.error.is_none());
    view.query.clear();
    view.min_copies = "6".into();
    view.invalidate();
    assert!(view.indices(&groups).is_empty() && view.error.as_ref().unwrap().contains("exceeds"));
    view.min_copies.clear();
    view.max_copies.clear();
    view.invalidate();
    assert_eq!(view.indices(&groups).len(), 202);
    assert!(view.error.is_none());
    assert_eq!(serde_json::to_value(&groups).unwrap(), original);
}
#[test]
fn archive_group_search_keeps_complete_groups_with_bounded_path_modes() {
    use super::{ContentGroupView, GroupMatch as M};
    let mut groups = vec![sort_group(10, 3), sort_group(0, 2), sort_group(20, 2)];
    for (group, paths) in groups.iter_mut().zip([
        vec!["root/Alpha.TXT", ".hidden/ёж.txt", "unmatched"],
        vec!["empty-one", "empty-two"],
        vec!["folder/nested.txt", "other"],
    ]) {
        for (member, path) in group.members.iter_mut().zip(paths) {
            member.path = path.into();
        }
    }
    let original = serde_json::to_value(&groups).unwrap();
    for (query, mode, basename, case, wanted) in [
        ("alpha", M::Literal, false, false, vec![0]),
        ("ЁЖ", M::Literal, false, false, vec![0]),
        ("*.txt", M::Glob, false, false, vec![]),
        ("*.txt", M::Glob, true, false, vec![0, 2]),
        ("root/**", M::Glob, false, false, vec![0]),
        ("root/**", M::Glob, true, false, vec![]),
        (r"root/.*\.txt$", M::Regex, false, false, vec![0]),
        ("Alpha.TXT", M::Exact, false, false, vec![]),
        ("Alpha.TXT", M::Exact, true, true, vec![0]),
        ("alpha.txt", M::Exact, true, true, vec![]),
        ("  ", M::Literal, false, false, vec![0, 1, 2]),
    ] {
        let mut view = ContentGroupView {
            query: query.into(),
            mode,
            basename_only: basename,
            case_sensitive: case,
            ..Default::default()
        };
        assert_eq!(view.indices(&groups), wanted, "{query}");
        assert!(view.error.is_none());
        assert_eq!(serde_json::to_value(&groups).unwrap(), original);
    }
    let mut view = ContentGroupView {
        query: "[".into(),
        mode: M::Regex,
        ..Default::default()
    };
    assert!(view.indices(&groups).is_empty() && view.error.is_some());
    view.query = "x".repeat(257);
    view.mode = M::Literal;
    view.invalidate();
    assert!(view.indices(&groups).is_empty() && view.error.is_some());
    view.query.clear();
    view.invalidate();
    assert_eq!(view.indices(&groups), vec![0, 1, 2]);
    assert!(view.error.is_none());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        groups[1].members[1].path = std::ffi::OsString::from_vec(vec![255]).into();
        view.invalidate();
        assert_eq!(view.indices(&groups), vec![0, 1, 2]);
        view.query = "alpha".into();
        view.invalidate();
        assert!(view.indices(&groups).is_empty());
        assert!(view.error.as_ref().unwrap().contains("UTF-8"));
    }
}
#[test]
fn archive_shown_group_actions_preserve_hidden_selection_and_validate_every_identity_first() {
    use super::change_shown_content_groups_selection as change;
    use std::collections::BTreeSet;
    let mut listing = listing(&["same", "same", "second", "third", "unrelated"]);
    for (id, member) in listing.members.iter_mut().enumerate() {
        member.index = [9, 2, 8, 4, 99][id];
        member.size = 3;
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 15,
        strict_headers: false,
        content_groups: Some(
            (0..2)
                .map(|rank| crate::archive_check::ContentGroup {
                    size: 3,
                    sha256: format!("hash-{rank}"),
                    members: listing.members[rank * 2..rank * 2 + 2]
                        .iter()
                        .map(|m| crate::archive_check::ContentMember {
                            index: m.index,
                            path: m.path.clone(),
                            size: m.size,
                        })
                        .collect(),
                })
                .collect(),
        ),
    };
    let mut selected = BTreeSet::from([4]);
    change(&report, &listing, &[1], true, &mut selected).unwrap();
    assert_eq!(selected, BTreeSet::from([2, 3, 4]));
    change(&report, &listing, &[0], true, &mut selected).unwrap();
    assert_eq!(selected, BTreeSet::from([0, 1, 2, 3, 4]));
    change(&report, &listing, &[1], false, &mut selected).unwrap();
    assert_eq!(selected, BTreeSet::from([0, 1, 4]));
    assert!(change(&report, &listing, &[0, 99], false, &mut selected).is_err());
    assert_eq!(selected, BTreeSet::from([0, 1, 4]));
    change(&report, &listing, &[], false, &mut selected).unwrap();
    assert_eq!(selected, BTreeSet::from([0, 1, 4]));
    report.content_groups.as_mut().unwrap()[1].members[1].path = "stale".into();
    assert!(change(&report, &listing, &[0, 1], false, &mut selected).is_err());
    assert_eq!(selected, BTreeSet::from([0, 1, 4]));
}

#[test]
fn archive_group_sort_uses_wide_payload_keys_and_preserves_scan_identities() {
    use crate::dedup::GroupSort as S;
    let groups = vec![
        sort_group(10, 4),
        sort_group(20, 2),
        sort_group(0, 10),
        sort_group(10, 4),
        sort_group(u64::MAX, 3),
        sort_group(u64::MAX, 2),
    ];
    let before = serde_json::to_value(&groups).unwrap();
    for (sort, wanted) in [
        (S::Scan, vec![0, 1, 2, 3, 4, 5]),
        (S::Savings, vec![4, 5, 0, 3, 1, 2]),
        (S::Size, vec![4, 5, 1, 0, 3, 2]),
        (S::Copies, vec![2, 0, 3, 4, 1, 5]),
    ] {
        assert_eq!(super::content_group_order(&groups, sort), wanted);
        assert_eq!(serde_json::to_value(&groups).unwrap(), before);
    }
    assert!(super::content_group_order(&[], S::Savings).is_empty());
    assert_eq!(
        super::content_group_order(&[sort_group(0, 0)], S::Savings),
        vec![0]
    );
}
#[test]
fn archive_group_view_pages_cover_original_ranks_and_rebuild_after_same_size_scan() {
    use super::ContentGroupView;
    use crate::dedup::GroupSort;
    let mut groups = (0..201).map(|i| sort_group(i, 2)).collect::<Vec<_>>();
    let mut view = ContentGroupView {
        sort: GroupSort::Size,
        ..Default::default()
    };
    let mut seen = Vec::new();
    for mut page in 0..3 {
        let range = super::content_group_page_range(groups.len(), &mut page);
        seen.extend_from_slice(&view.indices(&groups)[range]);
    }
    assert_eq!(seen, (0..201).rev().collect::<Vec<_>>());
    groups[0].size = 1000;
    view.invalidate();
    assert_eq!(view.indices(&groups)[0], 0);
    assert_eq!(view.sort, GroupSort::Size);
    view.sort = GroupSort::Scan;
    view.invalidate();
    assert_eq!(view.indices(&groups), (0..201).collect::<Vec<_>>());
}

fn preferred_fixture() -> (crate::archive::ArchiveListing, crate::archive_check::Report) {
    let mut listing = listing(&["first", "preferred/a", "preferred/b", "other", "unrelated"]);
    for (id, m) in listing.members.iter_mut().enumerate() {
        m.index = [9, 8, 2, 4, 99][id];
        m.size = 3;
        m.modified = None;
    }
    let report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 15,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..4]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    (listing, report)
}
#[test]
fn archive_preferred_keepers_rank_without_overriding_counts_protection_or_cutoff() {
    use super::{ContentKeep, ContentKeepOrder as O, select_redundant_content_groups as apply};
    use std::collections::BTreeSet;
    let (listing, report) = preferred_fixture();
    for scope in [None, Some(0)] {
        for order in O::ALL {
            let keep = ContentKeep {
                preferred_globs: "preferred/**".into(),
                order,
                ..Default::default()
            };
            let mut selected = BTreeSet::from([4]);
            apply(&report, &listing, scope, &keep, &mut selected).unwrap();
            let keeper = if order == O::Last { 1 } else { 2 };
            assert_eq!(
                selected,
                (0..5).filter(|&id| id != keeper).collect(),
                "{order:?}"
            );
        }
        for (count, protection, candidates, wanted) in [
            (2, "", "", vec![0, 3, 4]),
            (1, "first", "", vec![1, 2, 3, 4]),
            (3, "first", "", vec![3, 4]),
            (1, "", "preferred/**", vec![1, 2, 4]),
            (9, "", "", vec![4]),
        ] {
            let keep = ContentKeep {
                preferred_globs: "preferred/**".into(),
                count,
                protected_glob: protection.into(),
                candidate_globs: candidates.into(),
                ..Default::default()
            };
            let mut selected = BTreeSet::from([4]);
            apply(&report, &listing, scope, &keep, &mut selected).unwrap();
            assert_eq!(selected.into_iter().collect::<Vec<_>>(), wanted);
        }
        let keep = ContentKeep {
            preferred_globs: "preferred/**".into(),
            modified_before: "2026-01-01".into(),
            ..Default::default()
        };
        let mut selected = BTreeSet::from([2, 4]);
        apply(&report, &listing, scope, &keep, &mut selected).unwrap();
        assert_eq!(selected, BTreeSet::from([2, 4]));
    }
}
#[test]
fn archive_preferred_keepers_share_pattern_modes_limits_and_atomic_identity_guards() {
    use super::{ContentKeep, select_redundant_content_groups as apply};
    use std::collections::BTreeSet;
    let (mut listing, mut report) = preferred_fixture();
    for (id, path) in [(1, "deep/MASTER.TXT"), (2, "master.txt")] {
        listing.members[id].path = path.into();
        report.content_groups.as_mut().unwrap()[0].members[id].path = path.into();
    }
    for (pattern, basename, case, keeper) in [
        ("MASTER.TXT", false, true, 2),
        ("MASTER.TXT", true, true, 1),
        ("MASTER.TXT", true, false, 2),
        ("deep/**", false, true, 1),
        ("deep/**", true, true, 2),
        ("deep/**\nno-match", false, true, 1),
    ] {
        let keep = ContentKeep {
            preferred_globs: pattern.into(),
            protect_basename: basename,
            protect_case_sensitive: case,
            ..Default::default()
        };
        let mut selected = BTreeSet::from([4]);
        apply(&report, &listing, None, &keep, &mut selected).unwrap();
        assert_eq!(selected, (0..5).filter(|&id| id != keeper).collect());
    }
    for pattern in [
        "[".into(),
        "x".repeat(257),
        (0..17)
            .map(|n| format!("p{n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "x".repeat(8193),
    ] {
        let keep = ContentKeep {
            preferred_globs: pattern,
            ..Default::default()
        };
        let mut selected = BTreeSet::from([2, 4]);
        assert!(apply(&report, &listing, None, &keep, &mut selected).is_err());
        assert_eq!(selected, BTreeSet::from([2, 4]));
        assert!(keep.saved().is_null());
    }
    let keep = ContentKeep {
        preferred_globs: "deep/**".into(),
        ..Default::default()
    };
    report.content_groups.as_mut().unwrap()[0].members[3].path = "stale".into();
    let mut selected = BTreeSet::from([2, 4]);
    assert!(apply(&report, &listing, None, &keep, &mut selected).is_err());
    assert_eq!(selected, BTreeSet::from([2, 4]));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = std::ffi::OsString::from_vec(vec![255]);
        listing.members[3].path = path.clone().into();
        report.content_groups.as_mut().unwrap()[0].members[3].path = path.into();
        assert!(
            apply(&report, &listing, None, &keep, &mut selected)
                .unwrap_err()
                .contains("UTF-8")
        );
        assert_eq!(selected, BTreeSet::from([2, 4]));
    }
}
#[test]
fn archive_preferred_keepers_persist_and_legacy_rules_keep_all_other_fields() {
    use super::{ContentKeep, ContentKeepOrder as O};
    let keep = ContentKeep {
        count: 2,
        order: O::Newest,
        tie_breakers: vec![O::Shallowest],
        preferred_globs: "originals/**\n*.master".into(),
        candidate_globs: "backups/**".into(),
        ..Default::default()
    };
    let saved = keep.saved();
    assert_eq!(ContentKeep::from_saved(&saved), keep);
    let mut legacy = saved.clone();
    legacy["policy"]
        .as_object_mut()
        .unwrap()
        .remove("preferred_globs");
    let mut expected = keep.clone();
    expected.preferred_globs.clear();
    assert_eq!(ContentKeep::from_saved(&legacy), expected);
    for invalid in [
        serde_json::json!("["),
        serde_json::json!(false),
        serde_json::json!("x".repeat(8193)),
    ] {
        let mut corrupt = saved.clone();
        corrupt["policy"]["preferred_globs"] = invalid;
        assert_eq!(ContentKeep::from_saved(&corrupt), ContentKeep::default());
    }
}
#[test]
fn archive_priority_chains_resolve_ties_in_order_with_stable_fallback_and_atomic_guards() {
    use super::{ContentKeep, ContentKeepOrder as O, select_redundant_content_groups};
    use std::{
        collections::BTreeSet,
        time::{Duration, UNIX_EPOCH},
    };
    let mut listing = listing(&["a", "b", "folder/c", "other/d", "e", "unrelated"]);
    for (id, member) in listing.members.iter_mut().enumerate() {
        member.index = [9, 2, 8, 4, 1, 99][id];
        member.size = 3;
        member.modified = [Some(100), Some(200), Some(200), Some(200), None, None][id]
            .map(|t| UNIX_EPOCH + Duration::from_secs(t));
    }
    let report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 6,
        decoded: 18,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..5]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for group in [None, Some(0)] {
        for (order, chain, count, protected, candidates, wanted) in [
            (O::Shallowest, vec![], 1, "", "", vec![0, 1, 2, 3, 5]),
            (
                O::Shallowest,
                vec![O::Newest],
                1,
                "",
                "",
                vec![0, 2, 3, 4, 5],
            ),
            (
                O::Shallowest,
                vec![O::Newest, O::Last],
                1,
                "",
                "",
                vec![0, 2, 3, 4, 5],
            ),
            (
                O::Shallowest,
                vec![O::Last, O::Newest],
                1,
                "",
                "",
                vec![1, 2, 3, 4, 5],
            ),
            (
                O::Newest,
                vec![O::Deepest, O::Last],
                1,
                "",
                "",
                vec![0, 1, 3, 4, 5],
            ),
            (O::Newest, vec![O::Deepest], 1, "", "", vec![0, 1, 2, 4, 5]),
            (O::Shallowest, vec![O::Newest], 2, "", "", vec![2, 3, 4, 5]),
            (O::Shallowest, vec![O::Newest], 2, "e", "", vec![0, 2, 3, 5]),
            (
                O::Shallowest,
                vec![O::Newest],
                1,
                "",
                "folder/*",
                vec![2, 5],
            ),
            (O::Shallowest, vec![O::Newest], usize::MAX, "", "", vec![5]),
        ] {
            let keep = ContentKeep {
                order,
                tie_breakers: chain,
                count,
                protected_glob: protected.into(),
                candidate_globs: candidates.into(),
                ..Default::default()
            };
            let mut selected = BTreeSet::from([5]);
            select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                .unwrap();
            assert_eq!(selected.into_iter().collect::<Vec<_>>(), wanted, "{keep:?}");
        }
        for chain in [
            vec![O::First],
            vec![O::Newest, O::Newest],
            vec![O::Newest; 6],
        ] {
            let keep = ContentKeep {
                tie_breakers: chain,
                ..Default::default()
            };
            let mut selected = BTreeSet::from([2, 5]);
            assert!(
                select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                    .is_err()
            );
            assert_eq!(selected, BTreeSet::from([2, 5]));
            assert!(keep.saved().is_null());
        }
    }
}

#[test]
fn archive_copy_preferences_round_trip_all_orders_and_refuse_invalid_saved_rules() {
    use super::{ContentKeep, ContentKeepOrder};
    for order in [
        ContentKeepOrder::First,
        ContentKeepOrder::Last,
        ContentKeepOrder::Newest,
        ContentKeepOrder::Oldest,
        ContentKeepOrder::Shallowest,
        ContentKeepOrder::Deepest,
    ] {
        for flag in [false, true] {
            let keep = ContentKeep {
                count: 3,
                order,
                tie_breakers: ContentKeepOrder::ALL
                    .into_iter()
                    .filter(|&rule| rule != order)
                    .take(2)
                    .collect(),
                protected_glob: "originals/**\n*.bak".into(),
                protect_basename: flag,
                protect_case_sensitive: !flag,
                modified_before: "2026-01-01T12:30:00+01:00".into(),
                candidate_globs: "backups/**\n*.tmp".into(),
                preferred_globs: "originals/**\nmaster.*".into(),
            };
            let saved = keep.saved();
            assert_eq!(ContentKeep::from_saved(&saved), keep);
            assert_eq!(saved.as_object().unwrap().len(), 2);
            assert_eq!(saved["policy"].as_object().unwrap().len(), 9);
            for (key, value) in [
                ("count", serde_json::json!(0)),
                ("count", serde_json::json!(100_001)),
                ("order", serde_json::json!("unknown")),
                ("protected_glob", serde_json::json!("[")),
                ("candidate_globs", serde_json::json!("[")),
                ("modified_before", serde_json::json!("invalid")),
            ] {
                let mut legacy = saved.clone();
                legacy["policy"]
                    .as_object_mut()
                    .unwrap()
                    .remove("tie_breakers");
                let mut without_chain = keep.clone();
                without_chain.tie_breakers.clear();
                assert_eq!(ContentKeep::from_saved(&legacy), without_chain);
                let mut invalid = saved.clone();
                invalid["policy"][key] = value;
                assert_eq!(ContentKeep::from_saved(&invalid), ContentKeep::default());
            }
            let mut invalid = saved.clone();
            invalid["version"] = serde_json::json!(99);
            assert_eq!(ContentKeep::from_saved(&invalid), ContentKeep::default());
            invalid = saved.clone();
            invalid["policy"]["password"] = serde_json::json!("not-a-setting");
            assert_eq!(ContentKeep::from_saved(&invalid), ContentKeep::default());
        }
    }
    assert_eq!(
        ContentKeep::from_saved(&serde_json::Value::Null),
        ContentKeep::default()
    );
    assert!(
        ContentKeep {
            candidate_globs: "[".into(),
            ..Default::default()
        }
        .saved()
        .is_null()
    );
}

#[test]
fn archive_selection_history_round_trips_hidden_ids_noops_and_new_branches() {
    use super::SelectionHistory;
    use std::collections::BTreeSet;
    let mut history = SelectionHistory::default();
    let original = BTreeSet::from([0, 2, 4]);
    let edited = BTreeSet::from([1, 5]);
    history.reset(&original, 6);
    history.observe(&edited, 6);
    let mut selected = edited.clone();
    history.restore(&mut selected, 6, false);
    assert_eq!(selected, original);
    assert!(history.can_redo());
    history.observe(&selected, 6); // filtering, policy edits and refused/no-op actions
    assert!(history.can_redo());
    history.restore(&mut selected, 6, true);
    assert_eq!(selected, edited);
    history.restore(&mut selected, 6, false);
    selected = BTreeSet::from([3]);
    history.observe(&selected, 6);
    assert!(!history.can_redo());
    history.restore(&mut selected, 6, false);
    assert_eq!(selected, original);
    history.reset(&BTreeSet::from([1]), 6); // a new listing with the same row count
    assert!(!history.can_undo() && !history.can_redo());
}

#[test]
fn archive_selection_history_bounds_total_ids_and_invalidates_listing_changes() {
    use super::SelectionHistory;
    use std::collections::BTreeSet;
    let mut history = SelectionHistory::default();
    history.reset(&BTreeSet::new(), 100);
    for id in 0..40 {
        history.observe(&BTreeSet::from([id]), 100);
    }
    assert_eq!(history.undo.len(), 32);
    let mut selected = BTreeSet::from([39]);
    for _ in 0..32 {
        history.restore(&mut selected, 100, false);
    }
    assert!(!history.can_undo() && history.can_redo());
    assert_eq!(history.redo.len(), 32);
    history.observe(&selected, 101);
    assert!(!history.can_undo() && !history.can_redo());
    let large = (0..40_000).collect::<BTreeSet<_>>();
    let other = (40_000..80_000).collect::<BTreeSet<_>>();
    history.reset(&large, 100_000);
    history.observe(&other, 100_000);
    assert!(!history.can_undo());
    let too_big = (0..SelectionHistory::MAX_IDS + 1).collect::<BTreeSet<_>>();
    history.observe(&too_big, 100_000);
    assert!(history.current.is_none() && !history.can_undo() && !history.can_redo());
    history.observe(&BTreeSet::from([1]), 100_000);
    history.observe(&BTreeSet::from([2]), 100_000);
    assert!(history.can_undo());
    history.observe(&BTreeSet::from([100_000]), 100_000);
    assert!(history.current.is_none() && !history.can_undo());
}

#[test]
fn archive_candidate_patterns_limit_selection_and_count_other_members_as_keepers() {
    use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
    use std::collections::BTreeSet;
    let mut listing = listing(&[
        "a.txt",
        "b.bak",
        "nested/c.txt",
        "nested/D.TXT",
        "unrelated",
    ]);
    for (id, member) in listing.members.iter_mut().enumerate() {
        member.index = id;
        member.size = 3;
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 15,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..4]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for group in [None, Some(0)] {
        for (candidates, protect, basename, case_sensitive, count, wanted) in [
            ("", "", false, true, 1, vec![1, 2, 3, 4]),
            ("*.bak", "", false, true, 1, vec![1, 4]),
            ("nested/**", "", false, true, 1, vec![2, 3, 4]),
            ("*.txt\n*.bak", "", false, true, 1, vec![0, 1, 4]),
            ("*.TXT", "", true, false, 1, vec![0, 2, 3, 4]),
            ("*.TXT", "", true, true, 1, vec![3, 4]),
            ("*.txt", "a.txt", true, false, 1, vec![2, 3, 4]),
            ("*.txt", "a.txt", true, false, 3, vec![3, 4]),
            ("nested/**", "nested/**", false, true, 1, vec![4]),
            ("missing", "", false, true, 1, vec![4]),
            ("\r\n", "", false, true, 1, vec![1, 2, 3, 4]),
        ] {
            let keep = ContentKeep {
                candidate_globs: candidates.into(),
                protected_glob: protect.into(),
                protect_basename: basename,
                protect_case_sensitive: case_sensitive,
                count,
                ..Default::default()
            };
            let mut selected = BTreeSet::from([0, 1, 2, 3, 4]);
            select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                .unwrap();
            assert_eq!(
                selected.into_iter().collect::<Vec<_>>(),
                wanted,
                "{candidates:?}, {protect:?}, {count}"
            );
        }
        for pattern in [
            "[".to_owned(),
            format!("*.txt\n{}", "x".repeat(257)),
            std::iter::repeat_n("x", 17).collect::<Vec<_>>().join("\n"),
            "\n".repeat(8193),
        ] {
            let keep = ContentKeep {
                candidate_globs: pattern,
                ..Default::default()
            };
            let mut selected = BTreeSet::from([1, 4]);
            assert!(
                select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                    .is_err()
            );
            assert_eq!(selected, BTreeSet::from([1, 4]));
        }
    }
    let keep = ContentKeep {
        candidate_globs: "*.txt".into(),
        order: ContentKeepOrder::Last,
        tie_breakers: Vec::new(),
        ..Default::default()
    };
    report.content_groups.as_mut().unwrap()[0].members[2].path = "stale".into();
    let mut selected = BTreeSet::from([1, 4]);
    assert!(
        select_redundant_content_groups(&report, &listing, None, &keep, &mut selected).is_err()
    );
    assert_eq!(selected, BTreeSet::from([1, 4]));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        report.content_groups.as_mut().unwrap()[0].members[2].path =
            listing.members[2].path.clone();
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 255]));
        listing.members[0].path = path.clone();
        report.content_groups.as_mut().unwrap()[0].members[0].path = path;
        assert!(
            select_redundant_content_groups(&report, &listing, None, &keep, &mut selected)
                .unwrap_err()
                .contains("UTF-8")
        );
        assert_eq!(selected, BTreeSet::from([1, 4]));
    }
}

#[test]
fn archive_group_cutoff_parses_dates_offsets_fractional_and_pre_epoch_times() {
    use super::ContentKeep;
    use std::time::{Duration, UNIX_EPOCH};
    for (text, expected) in [
        ("", None),
        ("  ", None),
        ("1970-01-01", Some(UNIX_EPOCH)),
        ("1970-01-01T01:00:00+01:00", Some(UNIX_EPOCH)),
        (
            "1969-12-31T23:59:59.5Z",
            UNIX_EPOCH.checked_sub(Duration::from_millis(500)),
        ),
        (
            "1970-01-01T00:00:00.5Z",
            UNIX_EPOCH.checked_add(Duration::from_millis(500)),
        ),
    ] {
        assert_eq!(
            ContentKeep {
                modified_before: text.into(),
                ..Default::default()
            }
            .cutoff()
            .unwrap(),
            expected,
            "{text}"
        );
    }
    for text in [
        "1970-02-30",
        "1970-01-01..",
        "1970-01-01T00:00:00",
        "1970-01-01T00:00:60Z",
        "1970-1-1",
    ] {
        assert!(
            ContentKeep {
                modified_before: text.into(),
                ..Default::default()
            }
            .cutoff()
            .is_err(),
            "{text}"
        );
    }
    assert!(
        ContentKeep {
            modified_before: " ".repeat(65),
            ..Default::default()
        }
        .cutoff()
        .is_err()
    );
}

#[test]
fn archive_group_cutoff_skips_complete_new_or_unknown_groups_without_changing_selection() {
    use super::{ContentKeep, select_redundant_content_groups};
    use std::{
        collections::BTreeSet,
        time::{Duration, UNIX_EPOCH},
    };
    let mut listing = listing(&["a", "b", "c", "d", "e", "f", "unrelated"]);
    for (id, member) in listing.members.iter_mut().enumerate() {
        member.index = id;
        member.size = 3;
        member.modified = [
            Some(100),
            Some(200),
            Some(300),
            Some(400),
            None,
            Some(100),
            None,
        ][id]
            .map(|t| UNIX_EPOCH + Duration::from_secs(t));
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 7,
        decoded: 21,
        strict_headers: false,
        content_groups: Some(
            (0..3)
                .map(|group| crate::archive_check::ContentGroup {
                    sha256: format!("hash-{group}"),
                    size: 3,
                    members: listing.members[group * 2..group * 2 + 2]
                        .iter()
                        .map(|m| crate::archive_check::ContentMember {
                            index: m.index,
                            path: m.path.clone(),
                            size: m.size,
                        })
                        .collect(),
                })
                .collect(),
        ),
    };
    for (cutoff, wanted) in [
        ("1970-01-01T00:03:20Z", vec![2, 4, 6]),
        ("1970-01-01T00:05:00Z", vec![1, 2, 4, 6]),
        ("1970-01-01T00:06:40Z", vec![1, 2, 4, 6]),
        ("1970-01-01T00:06:40.001Z", vec![1, 3, 4, 6]),
        ("1970-01-02", vec![1, 3, 4, 6]),
        ("", vec![1, 3, 5, 6]),
    ] {
        let keep = ContentKeep {
            modified_before: cutoff.into(),
            ..Default::default()
        };
        let mut selected = BTreeSet::from([2, 4, 6]);
        select_redundant_content_groups(&report, &listing, None, &keep, &mut selected).unwrap();
        assert_eq!(selected.into_iter().collect::<Vec<_>>(), wanted, "{cutoff}");
    }
    for (group, wanted) in [
        (0, vec![1, 2, 4, 6]),
        (1, vec![2, 4, 6]),
        (2, vec![2, 4, 6]),
    ] {
        let keep = ContentKeep {
            modified_before: "1970-01-01T00:05:00Z".into(),
            ..Default::default()
        };
        let mut selected = BTreeSet::from([2, 4, 6]);
        select_redundant_content_groups(&report, &listing, Some(group), &keep, &mut selected)
            .unwrap();
        assert_eq!(selected.into_iter().collect::<Vec<_>>(), wanted);
    }
    let keep = ContentKeep {
        modified_before: "1970-01-01T00:05:00Z".into(),
        protected_glob: "a\nb".into(),
        ..Default::default()
    };
    let mut selected = BTreeSet::from([0, 1, 2, 4, 6]);
    select_redundant_content_groups(&report, &listing, None, &keep, &mut selected).unwrap();
    assert_eq!(selected, BTreeSet::from([2, 4, 6]));
    let invalid = ContentKeep {
        modified_before: "invalid".into(),
        ..Default::default()
    };
    assert!(
        select_redundant_content_groups(&report, &listing, None, &invalid, &mut selected).is_err()
    );
    assert_eq!(selected, BTreeSet::from([2, 4, 6]));
    report.content_groups.as_mut().unwrap()[1].members[1].path = "stale".into();
    assert!(
        select_redundant_content_groups(&report, &listing, None, &keep, &mut selected).is_err()
    );
    assert_eq!(selected, BTreeSet::from([2, 4, 6]));
}

#[test]
fn archive_protection_lists_union_matches_and_refuse_invalid_lines_atomically() {
    use super::{ContentKeep, select_redundant_content_groups};
    use std::collections::BTreeSet;
    let mut listing = listing(&["a.txt", "b.bak", "nested/a.txt", "other", "unrelated"]);
    for (index, member) in listing.members.iter_mut().enumerate() {
        member.index = index;
        member.size = 3;
    }
    let report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 15,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..4]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for group in [None, Some(0)] {
        for (glob, basename, case_sensitive, count, wanted) in [
            ("*.txt\n*.bak", false, true, 1, vec![2, 3, 4]),
            ("*.txt\r\n\r\n*.bak\r\n", false, true, 1, vec![2, 3, 4]),
            ("*.TXT\n*.BAK", true, false, 1, vec![3, 4]),
            ("*.TXT\n*.BAK", true, true, 1, vec![1, 2, 3, 4]),
            ("*.txt\n*.txt", false, true, 2, vec![2, 3, 4]),
            ("*.txt\n*.bak", false, true, 3, vec![3, 4]),
            ("\n\r\n", false, true, 1, vec![1, 2, 3, 4]),
            (" a.txt\n b.bak", false, true, 1, vec![1, 2, 3, 4]),
        ] {
            let keep = ContentKeep {
                protected_glob: glob.into(),
                protect_basename: basename,
                protect_case_sensitive: case_sensitive,
                count,
                ..Default::default()
            };
            let mut selected = BTreeSet::from([4]);
            select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                .unwrap();
            assert_eq!(selected.into_iter().collect::<Vec<_>>(), wanted, "{glob:?}");
        }
        for (glob, error) in [
            ("*.txt\n[".to_owned(), "Protection line 2:"),
            (format!("*.txt\n{}", "x".repeat(257)), "Protection line 2:"),
            (
                std::iter::repeat_n("*.txt", 17)
                    .collect::<Vec<_>>()
                    .join("\n"),
                "at most 16",
            ),
            ("\n".repeat(8193), "8192"),
        ] {
            let keep = ContentKeep {
                protected_glob: glob,
                ..Default::default()
            };
            let mut selected = BTreeSet::from([2, 4]);
            let actual =
                select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                    .unwrap_err();
            assert!(actual.contains(error), "{actual}");
            assert_eq!(selected, BTreeSet::from([2, 4]));
        }
    }
    let keep = ContentKeep {
        protected_glob: std::iter::repeat_n("x".repeat(256), 16)
            .collect::<Vec<_>>()
            .join("\n"),
        ..Default::default()
    };
    assert_eq!(keep.protection().unwrap().len(), 16);
}

#[test]
fn archive_protection_can_match_names_at_any_depth_with_explicit_letter_case() {
    use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
    use std::collections::BTreeSet;
    let mut listing = listing(&[
        "original.txt",
        "folder/original.txt",
        "folder/Original.TXT",
        "ёж/КОПИЯ.txt",
        "other",
        "unrelated",
    ]);
    for (row, member) in listing.members.iter_mut().enumerate() {
        member.index = [9, 2, 8, 4, 1, 99][row];
        member.size = 3;
    }
    let report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 6,
        decoded: 18,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..5]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for (glob, basename, case_sensitive, wanted) in [
        ("", true, false, vec![0, 1, 2, 3, 5]),
        ("original.txt", false, true, vec![1, 2, 3, 4, 5]),
        ("original.txt", true, true, vec![2, 3, 4, 5]),
        ("original.txt", true, false, vec![3, 4, 5]),
        ("original.txt", false, false, vec![1, 2, 3, 4, 5]),
        ("копия.TXT", true, false, vec![0, 1, 2, 4, 5]),
        ("копия.TXT", true, true, vec![0, 1, 2, 3, 5]),
        ("*.txt", false, true, vec![1, 2, 3, 4, 5]),
        ("*.txt", true, true, vec![2, 4, 5]),
        ("*.txt", true, false, vec![4, 5]),
        ("folder/*", false, true, vec![0, 3, 4, 5]),
        ("folder/*", true, true, vec![0, 1, 2, 3, 5]),
    ] {
        for group in [None, Some(0)] {
            let keep = ContentKeep {
                preferred_globs: String::new(),
                count: 1,
                order: ContentKeepOrder::First,
                tie_breakers: Vec::new(),
                protected_glob: glob.into(),
                protect_basename: basename,
                protect_case_sensitive: case_sensitive,
                modified_before: String::new(),
                candidate_globs: String::new(),
            };
            let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
            select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                .unwrap();
            assert_eq!(
                selected.into_iter().collect::<Vec<_>>(),
                wanted,
                "{glob}, basename={basename}, case={case_sensitive}"
            );
        }
    }
}

#[test]
fn protected_archive_paths_are_always_kept_and_fill_the_total_minimum() {
    use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
    use std::collections::BTreeSet;
    let mut listing = listing(&[
        "keep/a",
        "keep/b",
        "other/c",
        "KEEP/d",
        "nested/ёж",
        "unrelated",
    ]);
    for (row, member) in listing.members.iter_mut().enumerate() {
        member.index = [9, 2, 8, 4, 1, 99][row];
        member.size = 3;
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 6,
        decoded: 18,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..5]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for (glob, count, wanted) in [
        ("keep/*", 1, vec![2, 3, 4, 5]),
        ("keep/*", 3, vec![2, 3, 5]),
        ("keep/*", usize::MAX, vec![5]),
        ("KEEP/*", 1, vec![0, 1, 2, 4, 5]),
        ("**", 1, vec![5]),
        ("*", 1, vec![0, 1, 2, 3, 5]),
        ("{keep,nested}/**", 1, vec![2, 3, 5]),
        ("keep/[ab]", 1, vec![2, 3, 4, 5]),
        ("nested/ёж", 1, vec![0, 1, 2, 3, 5]),
        ("", 1, vec![0, 1, 2, 3, 5]),
    ] {
        for group in [None, Some(0)] {
            let keep = ContentKeep {
                preferred_globs: String::new(),
                count,
                order: ContentKeepOrder::First,
                tie_breakers: Vec::new(),
                protected_glob: glob.into(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            };
            let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
            select_redundant_content_groups(&report, &listing, group, &keep, &mut selected)
                .unwrap();
            assert_eq!(
                selected.into_iter().collect::<Vec<_>>(),
                wanted,
                "{glob} keep {count}"
            );
        }
    }
    for glob in ["[".to_owned(), "a".repeat(257)] {
        let mut selected = BTreeSet::from([2, 5]);
        assert!(
            select_redundant_content_groups(
                &report,
                &listing,
                None,
                &ContentKeep {
                    preferred_globs: String::new(),
                    count: 1,
                    order: ContentKeepOrder::First,
                    tie_breakers: Vec::new(),
                    protected_glob: glob,
                    protect_basename: false,
                    protect_case_sensitive: true,
                    modified_before: String::new(),
                    candidate_globs: String::new(),
                },
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, BTreeSet::from([2, 5]));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
        listing.members[0].path = invalid.clone();
        report.content_groups.as_mut().unwrap()[0].members[0].path = invalid;
        let mut selected = BTreeSet::from([2, 5]);
        assert!(
            select_redundant_content_groups(
                &report,
                &listing,
                None,
                &ContentKeep {
                    preferred_globs: String::new(),
                    count: 1,
                    order: ContentKeepOrder::First,
                    tie_breakers: Vec::new(),
                    protected_glob: "**".into(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                    modified_before: String::new(),
                    candidate_globs: String::new(),
                },
                &mut selected
            )
            .is_err()
        );
        assert_eq!(selected, BTreeSet::from([2, 5]));
    }
    report.content_groups.as_mut().unwrap()[0].members[1].path = "stale".into();
    let mut selected = BTreeSet::from([2, 5]);
    assert!(
        select_redundant_content_groups(
            &report,
            &listing,
            None,
            &ContentKeep {
                preferred_globs: String::new(),
                count: 1,
                order: ContentKeepOrder::First,
                tie_breakers: Vec::new(),
                protected_glob: "keep/*".into(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            },
            &mut selected
        )
        .is_err()
    );
    assert_eq!(selected, BTreeSet::from([2, 5]));
}

#[test]
fn archive_depth_keepers_count_folders_and_preserve_stable_identity() {
    use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
    use std::collections::BTreeSet;
    assert_eq!(
        super::content_path_depth(std::path::Path::new("a///./b")),
        1
    );
    assert_eq!(super::content_path_depth(std::path::Path::new("./ёж")), 0);
    let long = "ёж".repeat(100);
    let mut listing = listing(&[
        &long,
        ".hidden/same",
        ".hidden/same",
        "deep/ёж/y",
        "x",
        "unrelated",
    ]);
    for (row, member) in listing.members.iter_mut().enumerate() {
        member.index = [9, 2, 8, 4, 1, 99][row];
        member.size = 3;
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 6,
        decoded: 18,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..5]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for (order, count, wanted) in [
        (ContentKeepOrder::Shallowest, 1, vec![0, 1, 2, 3, 5]),
        (ContentKeepOrder::Deepest, 1, vec![0, 1, 2, 4, 5]),
        (ContentKeepOrder::Shallowest, 2, vec![1, 2, 3, 5]),
        (ContentKeepOrder::Deepest, 2, vec![0, 2, 4, 5]),
        (ContentKeepOrder::Shallowest, 4, vec![3, 5]),
        (ContentKeepOrder::Deepest, 4, vec![0, 5]),
        (ContentKeepOrder::Deepest, usize::MAX, vec![5]),
    ] {
        for group in [None, Some(0)] {
            let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
            select_redundant_content_groups(
                &report,
                &listing,
                group,
                &ContentKeep {
                    preferred_globs: String::new(),
                    count,
                    order,
                    tie_breakers: Vec::new(),
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                    modified_before: String::new(),
                    candidate_globs: String::new(),
                },
                &mut selected,
            )
            .unwrap();
            assert_eq!(
                selected.into_iter().collect::<Vec<_>>(),
                wanted,
                "{order:?}, keep {count}"
            );
        }
    }
    report.content_groups.as_mut().unwrap()[0].members[1].path = "stale".into();
    let mut selected = BTreeSet::from([2, 5]);
    assert!(
        select_redundant_content_groups(
            &report,
            &listing,
            None,
            &ContentKeep {
                preferred_globs: String::new(),
                count: 1,
                order: ContentKeepOrder::Deepest,
                tie_breakers: Vec::new(),
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            },
            &mut selected
        )
        .is_err()
    );
    assert_eq!(selected, BTreeSet::from([2, 5]));
}

#[test]
fn archive_time_keepers_use_declared_dates_stable_ties_and_missing_last() {
    use super::{ContentKeep, ContentKeepOrder, select_redundant_content_groups};
    use std::collections::BTreeSet;
    use std::time::{Duration, UNIX_EPOCH};
    let mut listing = listing(&["same", "same", "c", "d", "e", "unrelated"]);
    for (row, member) in listing.members.iter_mut().enumerate() {
        member.index = [9, 2, 8, 4, 1, 99][row];
        member.size = 3;
        member.modified = [Some(10), Some(30), Some(30), None, None, Some(100)][row]
            .map(|s| UNIX_EPOCH + Duration::from_secs(s));
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 6,
        decoded: 18,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members: listing.members[..5]
                .iter()
                .map(|m| crate::archive_check::ContentMember {
                    index: m.index,
                    path: m.path.clone(),
                    size: m.size,
                })
                .collect(),
        }]),
    };
    for (order, count, wanted) in [
        (ContentKeepOrder::Newest, 1, vec![0, 2, 3, 4, 5]),
        (ContentKeepOrder::Oldest, 1, vec![1, 2, 3, 4, 5]),
        (ContentKeepOrder::Newest, 2, vec![0, 3, 4, 5]),
        (ContentKeepOrder::Oldest, 2, vec![2, 3, 4, 5]),
        (ContentKeepOrder::Newest, 4, vec![3, 5]),
        (ContentKeepOrder::Oldest, 4, vec![3, 5]),
        (ContentKeepOrder::Newest, usize::MAX, vec![5]),
    ] {
        for group in [None, Some(0)] {
            let mut selected = BTreeSet::from([0, 1, 2, 3, 4, 5]);
            select_redundant_content_groups(
                &report,
                &listing,
                group,
                &ContentKeep {
                    preferred_globs: String::new(),
                    count,
                    order,
                    tie_breakers: Vec::new(),
                    protected_glob: String::new(),
                    protect_basename: false,
                    protect_case_sensitive: true,
                    modified_before: String::new(),
                    candidate_globs: String::new(),
                },
                &mut selected,
            )
            .unwrap();
            assert_eq!(
                selected.into_iter().collect::<Vec<_>>(),
                wanted,
                "{order:?} count={count}"
            );
        }
    }
    listing.members[2].modified =
        Some(UNIX_EPOCH + Duration::from_secs(30) + Duration::from_nanos(1));
    let mut selected = BTreeSet::from([5]);
    select_redundant_content_groups(
        &report,
        &listing,
        None,
        &ContentKeep {
            preferred_globs: String::new(),
            count: 1,
            order: ContentKeepOrder::Newest,
            tie_breakers: Vec::new(),
            protected_glob: String::new(),
            protect_basename: false,
            protect_case_sensitive: true,
            modified_before: String::new(),
            candidate_globs: String::new(),
        },
        &mut selected,
    )
    .unwrap();
    assert_eq!(selected, BTreeSet::from([0, 1, 3, 4, 5]));
    listing.members[0].modified = Some(UNIX_EPOCH - Duration::from_secs(1));
    selected = BTreeSet::from([5]);
    select_redundant_content_groups(
        &report,
        &listing,
        None,
        &ContentKeep {
            preferred_globs: String::new(),
            count: 1,
            order: ContentKeepOrder::Oldest,
            tie_breakers: Vec::new(),
            protected_glob: String::new(),
            protect_basename: false,
            protect_case_sensitive: true,
            modified_before: String::new(),
            candidate_globs: String::new(),
        },
        &mut selected,
    )
    .unwrap();
    assert_eq!(selected, BTreeSet::from([1, 2, 3, 4, 5]));
    for member in &mut listing.members {
        member.modified = None;
    }
    for order in [ContentKeepOrder::Newest, ContentKeepOrder::Oldest] {
        selected = BTreeSet::from([5]);
        select_redundant_content_groups(
            &report,
            &listing,
            None,
            &ContentKeep {
                preferred_globs: String::new(),
                count: 1,
                order,
                tie_breakers: Vec::new(),
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            },
            &mut selected,
        )
        .unwrap();
        assert_eq!(selected, BTreeSet::from([0, 1, 2, 3, 5]));
    }
    report.content_groups.as_mut().unwrap()[0].members[1].path = "stale".into();
    selected = BTreeSet::from([2, 5]);
    assert!(
        select_redundant_content_groups(
            &report,
            &listing,
            None,
            &ContentKeep {
                preferred_globs: String::new(),
                count: 1,
                order: ContentKeepOrder::Newest,
                tie_breakers: Vec::new(),
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            },
            &mut selected
        )
        .is_err()
    );
    assert_eq!(selected, BTreeSet::from([2, 5]));
}

#[test]
fn archive_replica_count_and_order_keep_exact_original_indices() {
    let mut listing = listing(&["dup", "dup", "a", "z", "unrelated"]);
    for (row, m) in listing.members.iter_mut().enumerate() {
        m.index = [8, 2, 5, 1, 99][row];
        m.size = 3;
    }
    let members = [0, 2, 3, 1]
        .into_iter()
        .map(|row| {
            let m = &listing.members[row];
            crate::archive_check::ContentMember {
                index: m.index,
                path: m.path.clone(),
                size: m.size,
            }
        })
        .collect();
    let report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 15,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: 3,
            members,
        }]),
    };
    for (count, last, expected) in [
        (1, false, vec![0, 1, 2, 4]),
        (2, false, vec![0, 2, 4]),
        (2, true, vec![1, 3, 4]),
        (4, false, vec![4]),
        (usize::MAX, true, vec![4]),
    ] {
        let mut selected = std::collections::BTreeSet::from([0, 1, 2, 3, 4]);
        super::select_redundant_content_groups(
            &report,
            &listing,
            None,
            &super::ContentKeep {
                preferred_globs: String::new(),
                count,
                order: if last {
                    super::ContentKeepOrder::Last
                } else {
                    super::ContentKeepOrder::First
                },
                tie_breakers: Vec::new(),
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            },
            &mut selected,
        )
        .unwrap();
        assert_eq!(selected.into_iter().collect::<Vec<_>>(), expected);
    }
    let mut selected = std::collections::BTreeSet::from([4]);
    assert!(
        super::select_redundant_content_groups(
            &report,
            &listing,
            None,
            &super::ContentKeep {
                preferred_globs: String::new(),
                count: 0,
                order: super::ContentKeepOrder::First,
                tie_breakers: Vec::new(),
                protected_glob: String::new(),
                protect_basename: false,
                protect_case_sensitive: true,
                modified_before: String::new(),
                candidate_globs: String::new(),
            },
            &mut selected
        )
        .is_err()
    );
    assert_eq!(selected, std::collections::BTreeSet::from([4]));
}

#[test]
fn redundant_archive_selection_preserves_lowest_original_index_and_refuses_stale_groups() {
    let mut listing = listing(&["same", "same", "empty-a", "empty-b", "unrelated"]);
    for (row, m) in listing.members.iter_mut().enumerate() {
        m.index = [9, 2, 20, 1, 99][row];
        m.size = if row < 2 { 3 } else { 0 };
    }
    let groups = vec![vec![0, 1], vec![2, 3]]
        .into_iter()
        .map(|rows| crate::archive_check::ContentGroup {
            sha256: "hash".into(),
            size: listing.members[rows[0]].size,
            members: rows
                .into_iter()
                .map(|row| {
                    let m = &listing.members[row];
                    crate::archive_check::ContentMember {
                        index: m.index,
                        path: m.path.clone(),
                        size: m.size,
                    }
                })
                .collect(),
        })
        .collect();
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 6,
        strict_headers: false,
        content_groups: Some(groups),
    };
    let mut selected = std::collections::BTreeSet::from([1, 3, 4]);
    super::select_redundant_content_groups(
        &report,
        &listing,
        Some(0),
        &super::ContentKeep::default(),
        &mut selected,
    )
    .unwrap();
    assert_eq!(selected, std::collections::BTreeSet::from([0, 3, 4]));
    super::select_redundant_content_groups(
        &report,
        &listing,
        None,
        &super::ContentKeep::default(),
        &mut selected,
    )
    .unwrap();
    assert_eq!(selected, std::collections::BTreeSet::from([0, 2, 4]));
    let before = selected.clone();
    assert!(
        super::select_redundant_content_groups(
            &report,
            &listing,
            Some(99),
            &super::ContentKeep::default(),
            &mut selected
        )
        .is_err()
    );
    listing.members[2].size = 42;
    assert!(
        super::select_redundant_content_groups(
            &report,
            &listing,
            None,
            &super::ContentKeep::default(),
            &mut selected
        )
        .is_err()
    );
    assert_eq!(selected, before);
    listing.members[2].size = 0;
    report.content_groups.as_mut().unwrap()[1].members[0].index = 9;
    report.content_groups.as_mut().unwrap()[1].members[0].path = "same".into();
    report.content_groups.as_mut().unwrap()[1].members[0].size = 3;
    assert!(
        super::select_redundant_content_groups(
            &report,
            &listing,
            None,
            &super::ContentKeep::default(),
            &mut selected
        )
        .is_err()
    );
    assert_eq!(selected, before);
}

#[test]
fn content_member_pages_are_independent_and_keep_original_entry_order() {
    let ctx = egui::Context::default();
    let mut pages = std::collections::BTreeMap::<usize, usize>::new();
    let mut frame = |events| {
        let mut ranges = Vec::new();
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                for rank in [0, 100] {
                    let range = ui
                        .push_id(("archive-group-members", rank), |ui| {
                            super::content_pager(
                                ui,
                                21,
                                pages.entry(rank).or_default(),
                                super::CONTENT_MEMBERS_PER_PAGE,
                                ["Previous members", "Next members", "Members"],
                            )
                        })
                        .inner;
                    ranges.push(range);
                }
            },
        ));
        ranges
    };
    assert_eq!(frame(vec![]), [0..10, 0..10]);
    // The recorded rect belongs to group 100, which renders last.
    for expected in [10..20, 20..21, 20..21] {
        let pos = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("Next members")))
            .unwrap()
            .center();
        frame(vec![egui::Event::PointerMoved(pos)]);
        frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        assert_eq!(
            frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default()
            }]),
            [0..10, expected]
        );
    }
    assert_eq!(pages.get(&0), Some(&0));
    assert_eq!(pages.get(&100), Some(&2));
    let entries = ["duplicate-name"; 21];
    let mut page = 2;
    let range = super::bounded_page_range(entries.len(), &mut page, 10);
    assert_eq!(
        entries
            .iter()
            .enumerate()
            .skip(range.start)
            .take(range.len())
            .map(|(i, _)| i)
            .collect::<Vec<_>>(),
        [20]
    );
    let mut stale = usize::MAX;
    assert_eq!(super::bounded_page_range(0, &mut stale, 10), 0..0);
    assert_eq!(stale, 0);
}

#[test]
fn content_group_page_buttons_reach_final_group_and_return() {
    let ctx = egui::Context::default();
    let mut page = 0;
    let mut frame = |events| {
        let mut range = 0..0;
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                range = super::content_group_pager(ui, 201, &mut page);
            },
        ));
        range
    };
    assert_eq!(frame(vec![]), 0..100);
    for (label, expected) in [
        ("Next groups", 100..200),
        ("Next groups", 200..201),
        ("Next groups", 200..201),
        ("Previous groups", 100..200),
        ("Previous groups", 0..100),
        ("Previous groups", 0..100),
    ] {
        let pos = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(label)))
            .unwrap()
            .center();
        frame(vec![egui::Event::PointerMoved(pos)]);
        frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        }]);
        assert_eq!(
            frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default()
            }]),
            expected
        );
    }
}

#[test]
fn content_group_pages_cover_all_absolute_ranks_and_clamp_stale_state() {
    for count in [0usize, 1, 99, 100, 101, 200, 201, 10_001] {
        let mut visited = Vec::new();
        for requested in 0..count.div_ceil(super::CONTENT_GROUPS_PER_PAGE).max(1) {
            let mut page = requested;
            let range = super::content_group_page_range(count, &mut page);
            assert!(range.len() <= super::CONTENT_GROUPS_PER_PAGE);
            assert_eq!(page, requested);
            visited.extend(range);
        }
        assert_eq!(visited, (0..count).collect::<Vec<_>>());
        let mut page = usize::MAX;
        let range = super::content_group_page_range(count, &mut page);
        assert_eq!(range.end, count);
        assert!(range.len() <= super::CONTENT_GROUPS_PER_PAGE);
    }
    let mut page = usize::MAX;
    let range = super::content_group_page_range(usize::MAX, &mut page);
    assert_eq!(range.end, usize::MAX);
    assert!(range.len() <= super::CONTENT_GROUPS_PER_PAGE);
}

use super::{
    MemberFilter, extract_all_label, extract_error_summary, extract_selected_label,
    step_member_cursor,
};

fn listing(paths: &[&str]) -> crate::archive::ArchiveListing {
    crate::archive::ArchiveListing {
        tree: Err("Not indexed".into()),
        kind: crate::archive::ArchiveKind::Zip,
        members: paths
            .iter()
            .enumerate()
            .map(|(index, path)| crate::archive::ArchiveMember {
                index,
                path: std::path::PathBuf::from(path),
                is_dir: false,
                is_regular: true,
                unix_mode: None,
                owner_uid: None,
                owner_gid: None,
                file_type_bits: 0,
                is_symlink: false,
                is_executable: Some(false),
                size: 1,
                compressed_size: 1,
                zip_info: None,
                tar_name: None,
                modified: None,
            })
            .collect(),
        declared_members: paths.len(),
        declared_uncompressed_bytes: paths.len() as u128,
        unsafe_members: 0,
        unreadable_members: 0,
        truncated: false,
    }
}

#[test]
fn password_widget_masks_text_and_discards_undo_snapshots() {
    let ctx = egui::Context::default();
    let mut password = zeroize::Zeroizing::new("private-fixture-password".to_string());
    let mut id = None;
    let output = crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
        id = Some(super::archive_password_input(ui, &mut password).id);
    }));
    assert!(!format!("{:?}", output.shapes).contains("private-fixture-password"));
    let state = egui::text_edit::TextEditState::load(&ctx, id.unwrap()).unwrap();
    let current = (egui::text::CCursorRange::default(), String::new());
    assert!(!state.undoer().has_undo(&current));
    assert!(!state.undoer().has_redo(&current));
}

#[test]
fn archive_copy_visible_omits_navigation_and_selected_keeps_hidden_sort_order() {
    let mut listing = listing(&[
        "hidden/file.txt",
        "src/file10.rs",
        "src/file2.rs",
        "src/deep/x",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let selected = [0, 1, 2, 100].into_iter().collect();
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    assert_eq!(filter.copy_rows(&listing, &selected, false), [2, 1]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 2, 1]);
    filter.order.descending = true;
    filter.visible(&listing, "file2");
    assert_eq!(filter.copy_rows(&listing, &selected, false), [2]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [1, 2, 0]);
    assert_eq!(selected, [0, 1, 2, 100].into_iter().collect());
}

#[test]
fn selecting_folder_contents_includes_only_real_subtree_members() {
    let listing = listing(&["a/deep/file", "a/direct", "a-not/sibling", "root"]);
    let mut selected = [3].into_iter().collect();
    super::select_folder_contents(&listing, std::path::Path::new("a"), &mut selected);
    assert_eq!(selected, [0, 1, 3].into_iter().collect());
    super::select_folder_contents(&listing, std::path::Path::new(""), &mut selected);
    assert_eq!(selected, [0, 1, 2, 3].into_iter().collect());
}

#[test]
fn archive_folder_navigation_preserves_member_ids_and_hidden_selection() {
    let mut listing = listing(&[
        "a/deep/child.txt",
        "a/one.txt",
        "a/one.txt",
        "root.txt",
        "b/two.txt",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let tree = listing.tree.as_ref().unwrap();
    let mut filter = MemberFilter::default();
    let selected: std::collections::BTreeSet<_> = [0, 2, 4].into_iter().collect();
    filter.navigate(Some(Default::default()));
    let root = filter.visible(&listing, "").to_vec();
    assert_eq!(root.len(), 3, "two implicit folders and one root file");
    assert!(
        root[..2]
            .iter()
            .all(|&index| index >= listing.members.len())
    );
    assert_eq!(root[2], 3);
    filter.navigate(Some("a".into()));
    let a = filter.visible(&listing, "").to_vec();
    assert_eq!(a.len(), 3);
    assert_eq!(
        tree.directories[a[0] - listing.members.len()].path,
        std::path::Path::new("a/deep")
    );
    assert_eq!(&a[1..], &[1, 2], "duplicate archive members stay distinct");
    assert_eq!(
        selected
            .iter()
            .filter(|index| !filter.visible_set.contains(index))
            .count(),
        2
    );
    filter.order.choose(crate::archive_order::Field::Size);
    filter.visible(&listing, "one");
    assert_eq!(filter.visible_set, [1, 2].into_iter().collect());
    filter.navigate(Some("a/deep".into()));
    assert_eq!(filter.visible(&listing, ""), [0]);
    filter.navigate(None);
    assert_eq!(filter.visible(&listing, "").len(), 5);
    assert_eq!(selected, [0, 2, 4].into_iter().collect());
    assert!(filter.visible(&listing, "missing").is_empty());
}

#[test]
fn natural_toggle_reorders_cached_members_without_changing_focus_or_selection() {
    let listing = listing(&["file10", "file2", "file02", "file2"]);
    let mut filter = MemberFilter::default();
    let selected: std::collections::BTreeSet<_> = [0, 3].into_iter().collect();
    assert_eq!(filter.visible(&listing, ""), [1, 3, 2, 0]);
    filter.focused = Some(0);
    filter.order.natural = false;
    assert_eq!(filter.visible(&listing, ""), [2, 0, 1, 3]);
    assert_eq!(filter.focused, Some(0));
    assert!(filter.scroll_to_focused);
    assert_eq!(filter.visible_set, [0, 1, 2, 3].into_iter().collect());
    filter.visible(&listing, "2");
    filter.order.natural = true;
    assert_eq!(filter.visible(&listing, "2"), [1, 3, 2]);
    assert_eq!(
        selected
            .iter()
            .filter(|index| !filter.visible_set.contains(index))
            .count(),
        1
    );
}

#[test]
fn navigation_folders_share_natural_name_order_and_reversal() {
    let mut listing = listing(&["folder10/file", "folder2/file"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some(Default::default()));
    assert_eq!(filter.visible(&listing, ""), [3, 2]);
    filter.order.descending = true;
    assert_eq!(filter.visible(&listing, ""), [2, 3]);
    filter.order.descending = false;
    filter.order.natural = false;
    assert_eq!(filter.visible(&listing, ""), [2, 3]);
    assert!(
        filter.visible_set.is_empty(),
        "navigation IDs are never extraction IDs"
    );
}

#[test]
fn archive_member_details_distinguish_zero_false_and_unavailable_metadata() {
    let mut listing = listing(&["file"]);
    let member = &mut listing.members[0];
    member.size = 0;
    member.compressed_size = 0;
    member.unix_mode = Some(0);
    member.is_executable = Some(false);
    member.modified = Some(std::time::UNIX_EPOCH);
    let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
    assert_eq!(
        fields.map(|(_, value)| value),
        [
            "Regular file",
            "0 bytes",
            "0 bytes",
            "1970-01-01T00:00:00Z",
            "0000",
            "No",
            "Unavailable (not recorded per member)",
            "Unavailable (not recorded per member)",
            "Unavailable (not recorded per member)"
        ]
    );
    member.unix_mode = None;
    member.is_executable = None;
    member.modified = None;
    let fields = super::member_details(crate::archive::ArchiveKind::TarXz, member);
    assert_eq!(fields[2].1, "Unavailable");
    assert_eq!(fields[3].1, "Unavailable");
    assert_eq!(fields[4].1, "Unavailable");
    assert_eq!(fields[5].1, "Unavailable");
}

#[test]
fn archive_details_show_zip_method_and_password_requirement() {
    let mut listing = listing(&["file"]);
    let member = &mut listing.members[0];
    member.zip_info = Some(crate::archive::ZipMemberInfo {
        compression: "Deflated".into(),
        encrypted: true,
        encryption: "ZipCrypto".into(),
        crc32: Some(0x1234abcd),
    });
    let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
    assert_eq!(fields[6], ("Compression method", "Deflated".into()));
    assert_eq!(fields[8], ("Declared CRC-32", "1234ABCD".into()));
    assert_eq!(
        fields[7],
        ("Encrypted", "Yes (ZipCrypto, password required)".into())
    );
    member.zip_info.as_mut().unwrap().encrypted = false;
    assert_eq!(
        super::member_details(crate::archive::ArchiveKind::Zip, member)[7].1,
        "No"
    );
}

#[test]
fn archive_member_details_preserve_special_permission_bits_and_record_types() {
    let mut listing = listing(&["entry"]);
    let member = &mut listing.members[0];
    member.unix_mode = Some(0o107755);
    member.is_executable = Some(true);
    let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
    assert_eq!(fields[4].1, "7755");
    assert_eq!(fields[5].1, "Yes");
    member.is_regular = false;
    member.is_symlink = true;
    assert_eq!(
        super::member_details(crate::archive::ArchiveKind::Tar, member)[0].1,
        "Symbolic link"
    );
    member.is_symlink = false;
    assert_eq!(
        super::member_details(crate::archive::ArchiveKind::Tar, member)[0].1,
        "Other archive record"
    );
    member.is_dir = true;
    let fields = super::member_details(crate::archive::ArchiveKind::Zip, member);
    assert_eq!(fields[0].1, "Directory");
    assert_eq!(fields[1].1, "Not applicable (directory)");
    assert_eq!(fields[2].1, "Unavailable");
}

#[test]
fn archive_reset_filters_restores_folder_rows_without_resetting_sort_or_selection_memory() {
    let mut listing = listing(&["src/a.txt", "src/b.rs", "other/c.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    let selected = [0, 2].into_iter().collect();
    filter
        .selection_memory
        .save(&selected, listing.members.len());
    filter.navigate(Some("src".into()));
    filter.narrow(&selected, listing.members.len());
    filter.navigate(Some("src".into()));
    filter.order.descending = true;
    filter.options.regex = true;
    filter.options.case_sensitive = true;
    filter.options.smart_case = true;
    filter.exclude_matches = true;
    filter.kinds.files = false;
    let mut query = "[".to_owned();
    assert!(filter.visible(&listing, &query).is_empty());
    assert!(filter.has_filters(&query));
    let history_len = filter.past.len();
    filter.reset_filters(&mut query);
    assert!(!filter.has_filters(&query));
    assert_eq!(filter.visible(&listing, &query), [1, 0]);
    assert!(filter.pattern.as_ref().unwrap().is_ok());
    assert_eq!(filter.folder, Some("src".into()));
    assert!(filter.order.descending);
    assert_eq!(filter.past.len(), history_len);
    let mut restored = Default::default();
    filter
        .selection_memory
        .restore(&mut restored, listing.members.len());
    assert_eq!(restored, selected);
    filter.reset_filters(&mut query);
    assert_eq!(filter.visible(&listing, &query), [1, 0]);
}

#[test]
fn archive_reset_filters_rebuilds_even_when_query_and_pattern_options_were_default() {
    let listing = listing(&["a", "b"]);
    let mut filter = MemberFilter::default();
    assert!(!filter.has_filters(""));
    assert!(filter.has_filters(" "));
    filter.visible(&listing, "");
    filter.narrow(&[0].into_iter().collect(), listing.members.len());
    assert_eq!(filter.visible(&listing, ""), [0]);
    filter.reset_filters(&mut String::new());
    assert_eq!(filter.visible(&listing, ""), [0, 1]);
    filter.exclude_matches = true;
    assert!(filter.visible(&listing, "").is_empty());
    filter.reset_filters(&mut String::new());
    assert_eq!(filter.visible(&listing, ""), [0, 1]);
}

#[test]
fn archive_exclusion_inverts_valid_patterns_but_preserves_errors_and_selection() {
    let listing = listing(&["src/A.TXT", "src/b.rs", "src/b.rs", "other/c.txt"]);
    let mut filter = MemberFilter::default();
    assert_eq!(filter.visible(&listing, ".rs"), [1, 2]);
    filter.focused = Some(1);
    let selected = [1, 3].into_iter().collect();
    filter.exclude_matches = true;
    assert_eq!(filter.visible(&listing, ".rs"), [3, 0]);
    assert_eq!(filter.focused, None);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 1]);
    assert!(filter.visible(&listing, "").is_empty());
    filter.options.regex = true;
    assert_eq!(filter.visible(&listing, r"\.rs$"), [3, 0]);
    assert!(filter.visible(&listing, "[").is_empty());
    assert!(filter.pattern.as_ref().unwrap().is_err());
    filter.exclude_matches = false;
    assert!(filter.visible(&listing, "[").is_empty());
    filter.exclude_matches = true;
    filter.options.smart_case = true;
    assert_eq!(filter.visible(&listing, "TXT$"), [3, 1, 2]);
    assert_eq!(selected, [1, 3].into_iter().collect());
    assert!(!MemberFilter::default().exclude_matches);
}

#[test]
fn archive_exclusion_applies_to_navigation_and_intersects_type_and_snapshot() {
    let mut listing = listing(&["src/a.txt", "docs/readme.txt", "src/b.rs"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter {
        exclude_matches: true,
        ..Default::default()
    };
    filter.navigate(Some(Default::default()));
    let visible = filter.visible(&listing, "docs").to_vec();
    assert_eq!(visible.len(), 1);
    let tree = listing.tree.as_ref().unwrap();
    assert_eq!(
        tree.directories[visible[0] - listing.members.len()].path,
        std::path::Path::new("src")
    );
    assert!(filter.visible_set.is_empty());
    filter.navigate(Some("src".into()));
    assert_eq!(filter.visible(&listing, ".rs"), [0]);
    filter.narrow(&[0, 2].into_iter().collect(), listing.members.len());
    assert_eq!(filter.visible(&listing, ".rs"), [0]);
    filter.kinds.files = false;
    assert!(filter.visible(&listing, ".rs").is_empty());
}

#[test]
fn archive_selection_stats_count_distinct_records_without_directory_or_link_payloads() {
    let mut listing = listing(&["same", "same", "folder", "link", "fifo"]);
    listing.members[0].size = u64::MAX;
    listing.members[1].size = u64::MAX;
    listing.members[0].compressed_size = u64::MAX;
    listing.members[1].compressed_size = 1;
    listing.members[2].is_dir = true;
    listing.members[2].is_regular = false;
    listing.members[3].is_symlink = true;
    listing.members[3].is_regular = false;
    listing.members[4].is_regular = false;
    let selected = [0, 1, 2, 3, 4, 99].into_iter().collect();
    let stats = super::SelectionStats::compute(&listing, &selected);
    assert_eq!(
        (stats.files, stats.folders, stats.links, stats.other),
        (2, 1, 1, 1)
    );
    assert_eq!(stats.unpacked, u128::from(u64::MAX) * 2);
    assert_eq!(stats.packed, Some(u128::from(u64::MAX) + 1));
    assert!(
        stats
            .label()
            .contains("36893488147419103230 unpacked bytes")
    );
    listing.kind = crate::archive::ArchiveKind::Tar;
    let tar_stats = super::SelectionStats::compute(&listing, &selected);
    assert_eq!(tar_stats.packed, None);
    assert!(tar_stats.label().contains("packed size unavailable"));
}

#[test]
fn archive_selection_stats_cache_tracks_replacement_and_retains_hidden_members() {
    let listing = listing(&["src/a", "other/b"]);
    let mut filter = MemberFilter::default();
    let mut selected = [0, 1].into_iter().collect();
    assert_eq!(filter.selection_stats.get(&listing, &selected).unpacked, 2);
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "a");
    assert_eq!(filter.selection_stats.get(&listing, &selected).files, 2);
    assert_eq!(filter.selection_stats.ids, [0, 1]);
    selected = [1].into_iter().collect();
    assert_eq!(filter.selection_stats.get(&listing, &selected).files, 1);
    assert_eq!(filter.selection_stats.ids, [1]);
    selected.clear();
    assert_eq!(filter.selection_stats.get(&listing, &selected).unpacked, 0);
    assert_eq!(
        filter.selection_stats.get(&listing, &selected).packed,
        Some(0)
    );
    assert!(filter.selection_stats.ids.is_empty());
    assert!(!MemberFilter::default().selection_stats.initialized);
}

#[test]
fn archive_type_filter_rebuilds_cached_rows_preserving_selection_and_exports() {
    let mut listing = listing(&["file.txt", "dir", "link", "fifo"]);
    listing.members[1].is_dir = true;
    listing.members[1].is_regular = false;
    listing.members[2].is_symlink = true;
    listing.members[2].is_regular = false;
    listing.members[3].is_regular = false;
    let mut filter = MemberFilter::default();
    assert_eq!(filter.visible(&listing, "").len(), 4);
    filter.focused = Some(0);
    let selected = [0, 2].into_iter().collect();
    filter.kinds.files = false;
    assert_eq!(filter.visible(&listing, ""), [1, 3, 2]);
    assert_eq!(filter.focused, None);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 2]);
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 3, 2]);
    filter.kinds.folders = false;
    filter.kinds.other = false;
    assert_eq!(filter.visible(&listing, ""), [2]);
    assert!(filter.visible(&listing, "txt").is_empty());
    filter.kinds.files = true;
    assert_eq!(filter.visible(&listing, "txt"), [0]);
    filter.kinds.links = false;
    filter.kinds.files = false;
    assert!(filter.visible(&listing, "").is_empty());
    filter.kinds = Default::default();
    assert_eq!(filter.visible(&listing, "").len(), 4);
    assert_eq!(selected, [0, 2].into_iter().collect());
}

#[test]
fn archive_exclusion_glob_intersects_query_and_excludes_directory_descendants() {
    let mut listing = listing(&[
        "src/main.rs",
        "src/main.rs",
        "src/main.rs.bak",
        "build/main.rs",
        "build/deep/a.txt",
        "src/FILE.TMP",
        "other/main.rs",
        "src/a.txt",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.exclusion.settings.query = "{*.bak,*.tmp,build}".into();
    let mut actual = filter.visible(&listing, "main").to_vec();
    actual.sort_unstable();
    assert_eq!(actual, [0, 1, 6]);
    let selected = [2, 4, 5].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, true).len(), 3);
    filter.navigate(Some(std::path::PathBuf::new()));
    filter.visible(&listing, "");
    let tree = listing.tree.as_ref().unwrap();
    let build = tree
        .directories
        .iter()
        .position(|d| d.path == std::path::Path::new("build"))
        .unwrap()
        + listing.members.len();
    assert!(filter.row_position(build).is_none());
    filter.navigate(Some("build/deep".into()));
    assert!(filter.visible(&listing, "").is_empty());
    filter.navigate(None);
    filter.exclusion.settings = super::ExclusionSettings {
        query: "src/main.rs".into(),
        full_path: true,
    };
    let mut actual = filter.visible(&listing, "main").to_vec();
    actual.sort_unstable();
    assert_eq!(actual, [2, 3, 6]);
    filter.exclusion.settings.query = "src".into();
    let mut actual = filter.visible(&listing, "").to_vec();
    actual.sort_unstable();
    assert_eq!(actual, [3, 4, 6]);
    assert!(filter.save_filter(""));
    let saved = filter.saved_filters[0].clone();
    assert!(saved.label().contains("Exclude glob (Full path): src"));
    let mut query = String::new();
    filter.reset_filters(&mut query);
    assert_eq!(filter.visible(&listing, "").len(), 8);
    filter.restore_filter(saved, &mut query);
    assert_eq!(filter.visible(&listing, "").len(), 3);
    filter.exclusion.settings.query = "[".into();
    assert!(filter.visible(&listing, "").is_empty());
    assert!(!filter.save_filter(""));
    filter.exclusion.settings.query = "x".repeat(257);
    assert!(filter.visible(&listing, "").is_empty());
    filter.reset_filters(&mut query);
    assert!(!filter.has_filters(""));
}

#[test]
fn archive_exclusion_glob_pointer_entry_preserves_active_query_and_selection() {
    let listing = listing(&["src/a.txt", "src/a.bak", "elsewhere/a.txt"]);
    let mut filter = MemberFilter::default();
    let selected = [1].into_iter().collect();
    let ctx = egui::Context::default();
    for frame in 0..9 {
        let mut input = egui::RawInput {
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4].contains(&frame) {
            let rect = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new("archive-exclusion-query-rect"))
                })
                .unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3,
                    modifiers: Default::default(),
                },
            ];
        } else if frame == 5 {
            input.events.push(egui::Event::Text("*.bak".into()));
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            filter.exclusion.ui(ui, true);
        }));
        filter.visible(&listing, "src/");
    }
    assert_eq!(filter.exclusion.settings.query, "*.bak");
    assert_eq!(filter.needle.as_deref(), Some("src/"));
    assert_eq!(filter.visible, [0]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [1]);
    assert!(filter.save_filter("src/"));
    filter.exclusion.settings.full_path = true;
    assert!(filter.save_filter("src/"));
    assert_eq!(filter.saved_filters.len(), 2);
}

#[test]
fn archive_dotfile_filter_hides_components_navigation_and_retains_selected_snapshots() {
    let mut listing = listing(&[
        ".env",
        ".git/config",
        "src/.cache/data",
        "src/a.txt",
        "src/a.txt",
        "src/link",
        "ordinary.name",
        "src/.秘密/file",
    ]);
    listing.members[5].is_symlink = true;
    listing.members[5].is_regular = false;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.kinds.hide_dotfiles = true;
    assert_eq!(
        filter
            .visible(&listing, "")
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        [3, 4, 5, 6].into_iter().collect()
    );
    assert!(filter.save_filter(""));
    let saved = filter.saved_filters[0].clone();
    assert!(saved.label().contains("Hide dotfiles"));
    filter.navigate(Some("src".into()));
    assert_eq!(filter.visible(&listing, ""), [3, 4, 5]);
    let selected = [0, 2, 4].into_iter().collect();
    let copied = filter.copy_rows(&listing, &selected, true);
    assert_eq!(
        copied
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        selected
    );
    filter.kinds.links = false;
    assert_eq!(filter.visible(&listing, ""), [3, 4]);
    filter.navigate(Some(".git".into()));
    assert!(filter.visible(&listing, "").is_empty());
    let mut query = String::new();
    filter.reset_filters(&mut query);
    assert_eq!(filter.visible(&listing, ""), [1]);
    filter.restore_filter(saved, &mut query);
    assert!(filter.visible(&listing, "").is_empty());
    filter.navigate(Some("src".into()));
    assert_eq!(filter.visible(&listing, "a.txt"), [3, 4]);
    filter.kinds.hide_dotfiles = false;
    filter.visible(&listing, "");
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
    assert_eq!(filter.saved_filters.len(), 1);
}

#[test]
fn archive_dotfile_checkbox_pointer_changes_view_without_deselecting() {
    let listing = listing(&[".env", "visible", "dir/.hidden/item"]);
    let selected: std::collections::BTreeSet<_> = [0, 2].into_iter().collect();
    let mut filter = MemberFilter::default();
    let ctx = egui::Context::default();
    for frame in 0..9 {
        let mut input = egui::RawInput {
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7].contains(&frame) {
            let rect = ctx
                .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("archive-hide-dotfiles")))
                .unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 6,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            super::hidden_entries_ui(ui, &mut filter.kinds, true);
        }));
        let visible = filter.visible(&listing, "");
        if (4..7).contains(&frame) {
            assert_eq!(visible, [1]);
        } else if frame >= 7 {
            assert_eq!(visible.len(), 3);
        }
        assert_eq!(filter.copy_rows(&listing, &selected, true).len(), 2);
    }
    assert!(!filter.kinds.hide_dotfiles);
    assert!(!crate::archive_filter::hidden_path(std::path::Path::new(
        "ordinary.name/./file"
    )));
    assert!(crate::archive_filter::hidden_path(std::path::Path::new(
        "dir/.hidden/file"
    )));
}

#[test]
fn archive_type_filter_keeps_navigation_but_intersects_narrowing_and_query() {
    let mut listing = listing(&["src/file.txt", "src/link", "src/fifo"]);
    listing.members[1].is_symlink = true;
    listing.members[1].is_regular = false;
    listing.members[2].is_regular = false;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter {
        kinds: crate::archive_filter::Kinds {
            hide_dotfiles: false,
            executable_only: false,
            files: false,
            folders: false,
            links: true,
            other: false,
        },
        ..Default::default()
    };
    filter.navigate(Some(Default::default()));
    assert_eq!(filter.visible(&listing, "").len(), 1);
    assert!(filter.visible_set.is_empty());
    filter.navigate(Some("src".into()));
    assert_eq!(filter.visible(&listing, ""), [1]);
    filter.narrow(&[0, 2].into_iter().collect(), listing.members.len());
    assert!(filter.visible(&listing, "").is_empty());
    filter.kinds.other = true;
    assert_eq!(filter.visible(&listing, ""), [2]);
    filter.options.regex = true;
    assert!(filter.visible(&listing, "[").is_empty());
    filter.unnarrow();
    assert_eq!(filter.visible(&listing, ""), [2, 1]);
    assert_eq!(MemberFilter::default().kinds, Default::default());
}

#[test]
fn archive_narrow_snapshot_keeps_hidden_duplicates_and_survives_selection_changes() {
    let mut listing = listing(&["src/a.txt", "src/a.txt", "other/b.txt", "other/c.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "a");
    let mut selected = [1, 2, 99].into_iter().collect();
    filter.narrow(&selected, listing.members.len());
    assert_eq!(filter.folder, None);
    assert_eq!(filter.visible(&listing, "a"), [1]);
    assert_eq!(filter.visible(&listing, ""), [2, 1]);
    selected.remove(&1);
    assert_eq!(filter.visible(&listing, ""), [2, 1]);
    filter.order.descending = true;
    assert_eq!(filter.visible(&listing, ""), [1, 2]);
    filter.narrow(&selected, listing.members.len());
    assert_eq!(filter.visible(&listing, ""), [2]);
    filter.unnarrow();
    assert_eq!(filter.visible(&listing, "").len(), 4);
    assert_eq!(selected, [2, 99].into_iter().collect());
}

#[test]
fn archive_narrow_empty_reset_and_folder_filter_never_admit_navigation_rows() {
    let mut listing = listing(&["src/a.txt", "other/b.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.narrow(&[0].into_iter().collect(), listing.members.len());
    filter.navigate(Some(Default::default()));
    assert!(filter.visible(&listing, "").is_empty());
    filter.navigate(Some("src".into()));
    assert_eq!(filter.visible(&listing, ""), [0]);
    filter.options.regex = true;
    assert!(filter.visible(&listing, "[").is_empty());
    filter.narrow(&[99].into_iter().collect(), listing.members.len());
    assert!(filter.narrowed.is_none());
    filter.navigate(None);
    assert_eq!(filter.visible(&listing, "").len(), 2);
    assert!(MemberFilter::default().narrowed.is_none());
}

#[test]
fn scanned_content_groups_match_original_index_path_size_before_selection() {
    let mut listing = listing(&["same", "same", "other"]);
    for (index, member) in listing.members.iter_mut().enumerate() {
        member.index = index;
        member.size = 3;
    }
    let report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Zip,
        members: 3,
        decoded: 9,
        strict_headers: false,
        content_groups: Some(vec![crate::archive_check::ContentGroup {
            sha256: "abc".into(),
            size: 3,
            members: vec![
                crate::archive_check::ContentMember {
                    index: 0,
                    path: "same".into(),
                    size: 3,
                },
                crate::archive_check::ContentMember {
                    index: 1,
                    path: "same".into(),
                    size: 3,
                },
            ],
        }]),
    };
    assert_eq!(
        super::content_group_rows(&report, &listing).unwrap(),
        [0, 1]
    );
    listing.members[1].size = 4;
    assert!(super::content_group_rows(&report, &listing).is_err());
    listing.members[1].size = 3;
    listing.members[1].path = "changed".into();
    assert!(super::content_group_rows(&report, &listing).is_err());
    listing.members[1].path = "same".into();
    listing.members[1].index = 0;
    assert!(super::content_group_rows(&report, &listing).is_err());
}

#[test]
fn content_group_selection_changes_only_one_group_and_refuses_atomically() {
    let mut listing = listing(&["dup", "dup", "empty-one", "empty-two", "unrelated"]);
    for (index, m) in listing.members.iter_mut().enumerate() {
        m.index = index;
        m.size = if matches!(index, 2 | 3) { 0 } else { 3 };
    }
    let mut report = crate::archive_check::Report {
        kind: crate::archive::ArchiveKind::Tar,
        members: 5,
        decoded: 9,
        strict_headers: false,
        content_groups: Some(vec![
            crate::archive_check::ContentGroup {
                sha256: "abc".into(),
                size: 3,
                members: vec![
                    crate::archive_check::ContentMember {
                        index: 0,
                        path: "dup".into(),
                        size: 3,
                    },
                    crate::archive_check::ContentMember {
                        index: 1,
                        path: "dup".into(),
                        size: 3,
                    },
                ],
            },
            crate::archive_check::ContentGroup {
                sha256: "empty".into(),
                size: 0,
                members: vec![
                    crate::archive_check::ContentMember {
                        index: 2,
                        path: "empty-one".into(),
                        size: 0,
                    },
                    crate::archive_check::ContentMember {
                        index: 3,
                        path: "empty-two".into(),
                        size: 0,
                    },
                ],
            },
        ]),
    };
    let mut selected = std::collections::BTreeSet::from([4]);
    for (group, select, expected) in [
        (Some(0), true, vec![0, 1, 4]),
        (Some(1), true, vec![0, 1, 2, 3, 4]),
        (Some(0), false, vec![2, 3, 4]),
        (Some(1), false, vec![4]),
        (None, true, vec![0, 1, 2, 3, 4]),
    ] {
        super::change_content_group_selection(&report, &listing, group, select, &mut selected)
            .unwrap();
        assert_eq!(selected.iter().copied().collect::<Vec<_>>(), expected);
    }
    let before = selected.clone();
    assert!(
        super::change_content_group_selection(&report, &listing, Some(99), false, &mut selected)
            .is_err()
    );
    assert_eq!(selected, before);
    report.content_groups.as_mut().unwrap()[1].members[1].index = 99;
    assert!(
        super::change_content_group_selection(&report, &listing, None, false, &mut selected)
            .is_err()
    );
    assert_eq!(selected, before);
    assert!(
        super::change_content_group_selection(&report, &listing, Some(1), false, &mut selected)
            .is_err()
    );
    assert_eq!(selected, before);
}

#[test]
fn archive_repeated_crc_candidates_preserve_distinct_ids_zero_and_visible_scope() {
    let mut listing = listing(&[
        "dup", "dup", "zero1", "zero2", "size", "unknown", "link", "folder", "other",
    ]);
    listing.kind = crate::archive::ArchiveKind::Zip;
    for member in &mut listing.members {
        member.size = 10;
        member.zip_info = Some(crate::archive::ZipMemberInfo {
            compression: "Stored".into(),
            encrypted: false,
            encryption: "None".into(),
            crc32: Some(7),
        });
    }
    for id in [2, 3] {
        listing.members[id].size = 0;
        listing.members[id].zip_info.as_mut().unwrap().crc32 = Some(0);
    }
    listing.members[4].size = 11;
    listing.members[5].zip_info.as_mut().unwrap().crc32 = None;
    listing.members[6].is_symlink = true;
    listing.members[7].is_dir = true;
    listing.members[8].is_regular = false;
    assert_eq!(
        super::repeated_crc_rows(&listing, &[3, 2, 1, 0, 4, 5, 6, 7, 8, 99, 0]),
        [3, 2, 1, 0]
    );
    assert!(super::repeated_crc_rows(&listing, &[0, 0, 4, 5, 6, 7, 8, 99]).is_empty());
    let mut selected = std::collections::BTreeSet::from([5]);
    selected.extend(super::repeated_crc_rows(&listing, &[0, 1]));
    assert_eq!(selected, std::collections::BTreeSet::from([0, 1, 5]));
    listing.kind = crate::archive::ArchiveKind::Tar;
    assert!(super::repeated_crc_rows(&listing, &[0, 1, 2, 3]).is_empty());
}

#[test]
fn archive_same_extension_matches_visible_regular_members_and_duplicate_paths() {
    let mut listing = listing(&[
        "src/a.TXT",
        "src/a.TXT",
        "src/b.txt",
        "other/c.txt",
        "src/d.rs",
        "src/folder.txt",
        "src/link.txt",
    ]);
    listing.members[5].is_dir = true;
    listing.members[5].is_regular = false;
    listing.members[6].is_regular = false;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    let rows = super::same_extension_rows(
        &listing,
        filter.lower_paths.as_deref().unwrap(),
        &filter.visible,
        Some(0),
    );
    assert_eq!(rows, [0, 1, 2]);
    let mut selected: std::collections::BTreeSet<_> = [3, 4].into_iter().collect();
    selected.extend(rows.iter().copied());
    assert_eq!(selected, [0, 1, 2, 3, 4].into_iter().collect());
    for id in rows {
        selected.remove(&id);
    }
    assert_eq!(selected, [3, 4].into_iter().collect());
    filter.visible(&listing, "b.txt");
    assert!(
        super::same_extension_rows(
            &listing,
            filter.lower_paths.as_deref().unwrap(),
            &filter.visible,
            Some(0)
        )
        .is_empty()
    );
    assert_eq!(
        super::same_extension_rows(
            &listing,
            filter.lower_paths.as_deref().unwrap(),
            &filter.visible,
            Some(2)
        ),
        [2]
    );
    assert!(super::same_extension_rows(&listing, &[], &filter.visible, Some(2)).is_empty());
}

#[test]
fn archive_same_extension_handles_empty_suffixes_and_navigation_endpoints() {
    let listing = listing(&[
        ".profile",
        "noext",
        "trailing.",
        "archive.tar.gz",
        "other.gz",
        "a.ЁЖ",
        "b.ёж",
    ]);
    let mut filter = MemberFilter::default();
    filter.visible(&listing, "");
    let lower = filter.lower_paths.as_deref().unwrap();
    assert_eq!(
        super::same_extension_rows(&listing, lower, &filter.visible, Some(0)),
        [0, 1, 2]
    );
    assert_eq!(
        super::same_extension_rows(&listing, lower, &filter.visible, Some(3)),
        [3, 4]
    );
    assert_eq!(
        super::same_extension_rows(&listing, lower, &filter.visible, Some(5)),
        [5, 6]
    );
    assert!(super::same_extension_rows(&listing, lower, &[7], Some(7)).is_empty());
    assert!(super::same_extension_rows(&listing, lower, &filter.visible, None).is_empty());
}

#[test]
fn archive_saved_selection_survives_filter_navigation_and_keeps_duplicate_identity() {
    let listing = listing(&["src/a", "src/a", "other/b"]);
    let mut filter = MemberFilter::default();
    let mut selected = [1, 2, 99].into_iter().collect();
    filter
        .selection_memory
        .save(&selected, listing.members.len());
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "a");
    selected = [0].into_iter().collect();
    filter.order.descending = true;
    filter
        .selection_memory
        .restore(&mut selected, listing.members.len());
    assert_eq!(selected, [1, 2].into_iter().collect());
    assert_eq!(filter.copy_rows(&listing, &selected, true), [1, 2]);
    assert!(filter.history_step(true));
    selected.clear();
    filter
        .selection_memory
        .restore(&mut selected, listing.members.len());
    assert_eq!(selected, [1, 2].into_iter().collect());
    assert!(MemberFilter::default().selection_memory.0.is_none());
}

#[test]
fn archive_saved_empty_selection_differs_from_no_snapshot_and_save_replaces() {
    let mut memory = super::SelectionMemory::default();
    let mut selected = [0, 1].into_iter().collect();
    memory.restore(&mut selected, 2);
    assert_eq!(selected.len(), 2);
    memory.save(&selected, 2);
    selected.clear();
    memory.restore(&mut selected, 2);
    assert_eq!(selected.len(), 2);
    memory.save(&Default::default(), 2);
    assert_eq!(memory.0, Some(vec![]));
    memory.restore(&mut selected, 2);
    assert!(selected.is_empty());
    memory.save(&[1].into_iter().collect(), 2);
    memory.restore(&mut selected, 2);
    assert_eq!(selected, [1].into_iter().collect());
}

#[test]
fn archive_range_follows_display_order_and_preserves_hidden_selection() {
    let visible = [5, 3, 1, 4, 0]; // 5 is a synthetic navigation folder.
    let mut selected = [2].into_iter().collect();
    super::select_member_range(&visible, Some(3), Some(0), 5, &mut selected);
    assert_eq!(selected, [0, 1, 2, 3, 4].into_iter().collect());
    selected = [2].into_iter().collect();
    super::select_member_range(&visible, Some(4), Some(5), 5, &mut selected);
    assert_eq!(selected, [1, 2, 3, 4].into_iter().collect());
    super::select_member_range(&visible, Some(4), Some(99), 5, &mut selected);
    assert_eq!(selected, [1, 2, 3, 4].into_iter().collect());
    selected.clear();
    super::select_member_range(&visible, Some(99), Some(0), 5, &mut selected);
    assert_eq!(selected, [0].into_iter().collect());
}

#[test]
fn archive_range_anchor_resets_when_hidden_or_navigating() {
    let listing = listing(&["a.txt", "b.txt"]);
    let mut filter = MemberFilter::default();
    filter.visible(&listing, "");
    filter.range_anchor = Some(1);
    filter.visible(&listing, "b");
    assert_eq!(filter.range_anchor, Some(1));
    filter.visible(&listing, "a");
    assert_eq!(filter.range_anchor, None);
    filter.range_anchor = Some(0);
    filter.navigate(Some(Default::default()));
    assert_eq!(filter.range_anchor, None);
    filter.range_anchor = Some(0);
    assert!(filter.history_step(true));
    assert_eq!(filter.range_anchor, None);
}

#[test]
fn archive_visible_selection_preserves_hidden_and_separate_duplicate_entries() {
    let mut listing = listing(&["src/a.txt", "src/a.txt", "src/b.txt", "other/c.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "a.txt");
    let mut selected = [0, 3].into_iter().collect();
    super::change_visible_selection(&filter.visible_set, &mut selected, true);
    assert_eq!(selected, [1, 3].into_iter().collect());
    super::change_visible_selection(&filter.visible_set, &mut selected, true);
    assert_eq!(selected, [0, 3].into_iter().collect());
    super::change_visible_selection(&filter.visible_set, &mut selected, false);
    assert_eq!(selected, [3].into_iter().collect());
    filter.visible(&listing, "b.txt");
    super::change_visible_selection(&filter.visible_set, &mut selected, true);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 2]);
}

#[test]
fn archive_visible_selection_excludes_navigation_and_invalid_filter_rows() {
    let mut listing = listing(&["src/a.txt", "other/b.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some(Default::default()));
    assert!(!filter.visible(&listing, "").is_empty());
    assert!(filter.visible_set.is_empty());
    let mut selected = [0].into_iter().collect();
    super::change_visible_selection(&filter.visible_set, &mut selected, true);
    assert_eq!(selected, [0].into_iter().collect());
    filter.navigate(None);
    filter.options.regex = true;
    filter.visible(&listing, "[");
    for invert in [true, false] {
        super::change_visible_selection(&filter.visible_set, &mut selected, invert);
        assert_eq!(selected, [0].into_iter().collect());
    }
    filter.visible(&listing, "");
    super::change_visible_selection(&filter.visible_set, &mut selected, true);
    assert_eq!(selected, [1].into_iter().collect());
    super::change_visible_selection(&filter.visible_set, &mut selected, false);
    assert!(selected.is_empty());
}

#[test]
fn archive_history_walks_modes_and_folders_and_restores_visible_cursor() {
    let mut listing = listing(&["src/a.txt", "src/deep/b.txt", "other/c.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    let selected = [0, 1].into_iter().collect();
    filter.visible(&listing, "");
    filter.focused = Some(1);
    filter.navigate(Some(Default::default()));
    filter.visible(&listing, "");
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    filter.focused = Some(0);
    filter.navigate(Some("src/deep".into()));
    filter.visible(&listing, "");
    filter.focused = Some(1);
    assert!(filter.history_step(true));
    filter.visible(&listing, "");
    assert_eq!(filter.folder, Some("src".into()));
    assert_eq!(filter.focused, Some(0));
    assert!(filter.scroll_to_focused && filter.reset_scroll);
    assert!(filter.history_step(true));
    assert_eq!(filter.folder, Some(Default::default()));
    assert!(filter.history_step(true));
    assert_eq!(filter.folder, None);
    filter.visible(&listing, "");
    assert_eq!(filter.focused, Some(1));
    assert!(!filter.history_step(true));
    for folder in [
        Some(Default::default()),
        Some("src".into()),
        Some("src/deep".into()),
    ] {
        assert!(filter.history_step(false));
        assert_eq!(filter.folder, folder);
        filter.visible(&listing, "");
    }
    assert_eq!(filter.focused, Some(1));
    assert!(!filter.history_step(false));
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 1]);
    assert_eq!(selected, [0, 1].into_iter().collect());
}

#[test]
fn archive_history_new_branch_clears_forward_and_same_location_does_not_duplicate() {
    let mut filter = MemberFilter::default();
    assert!(!filter.history_step(true));
    assert!(!filter.history_step(false));
    filter.navigate(None);
    assert!(filter.past.is_empty());
    filter.navigate(Some("a".into()));
    filter.navigate(Some("a".into()));
    assert_eq!(filter.past.len(), 1);
    filter.navigate(Some("b".into()));
    assert!(filter.history_step(true));
    assert_eq!(filter.folder, Some("a".into()));
    filter.navigate(Some("c".into()));
    assert!(filter.future.is_empty());
    assert!(!filter.history_step(false));
    assert!(filter.history_step(true));
    assert_eq!(filter.folder, Some("a".into()));
    assert!(filter.history_step(true));
    assert_eq!(filter.folder, None);
}

#[test]
fn archive_history_keeps_current_filter_sort_and_selection_and_rejects_hidden_cursor() {
    let listing = listing(&["a/first.txt", "b/second.txt"]);
    let selected = [0].into_iter().collect();
    let mut filter = MemberFilter::default();
    filter.navigate(Some("a".into()));
    filter.visible(&listing, "");
    filter.focused = Some(0);
    filter.navigate(Some("b".into()));
    filter.options.regex = true;
    filter.options.smart_case = true;
    filter.order.descending = true;
    filter.visible(&listing, "second");
    assert!(filter.history_step(true));
    assert!(filter.visible(&listing, "second").is_empty());
    assert_eq!(filter.focused, None);
    assert!(filter.options.regex && filter.options.smart_case && filter.order.descending);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0]);
}

#[test]
fn archive_history_caps_encoded_paths_and_entries_and_default_resets_owner() {
    let mut filter = MemberFilter::default();
    for i in 0..100 {
        filter.navigate(Some(format!("folder-{i}").into()));
    }
    assert_eq!(filter.past.len(), super::MAX_ARCHIVE_HISTORY);
    let mut count = 0;
    while filter.history_step(true) {
        count += 1;
    }
    assert_eq!(count, super::MAX_ARCHIVE_HISTORY);
    assert_eq!(filter.future.len(), super::MAX_ARCHIVE_HISTORY);
    while filter.history_step(false) {}
    for i in 0..20 {
        filter.navigate(Some(format!("{}-{i}", "x".repeat(32 * 1024)).into()));
    }
    assert!(
        filter
            .past
            .iter()
            .chain(filter.future.iter())
            .map(|point| point.bytes())
            .sum::<usize>()
            <= super::MAX_ARCHIVE_HISTORY_BYTES
    );
    let reset = MemberFilter::default();
    assert!(reset.past.is_empty() && reset.future.is_empty());
    assert_eq!(reset.folder, None);
}

#[test]
fn archive_regex_and_case_changes_rebuild_real_and_navigation_rows() {
    let mut listing = listing(&[
        "src/Alpha.rs",
        "src/alpha.rs",
        "src/deep/file.rs",
        "Other/x.rs",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let selected = [0, 2].into_iter().collect();
    let mut filter = MemberFilter::default();
    filter.options.regex = true;
    assert_eq!(filter.visible(&listing, r"^src/alpha\.rs$"), [0, 1]);
    filter.options.case_sensitive = true;
    assert_eq!(filter.visible(&listing, r"^src/alpha\.rs$"), [1]);
    filter.options.case_sensitive = false;
    filter.options.smart_case = true;
    assert_eq!(filter.visible(&listing, r"^src/Alpha\.rs$"), [0]);
    filter.navigate(Some("src".into()));
    filter.visible(&listing, r"(?i)^src/(?:alpha\.rs|deep)$");
    assert_eq!(filter.copy_rows(&listing, &selected, false), [0, 1]);
    let tree = listing.tree.as_ref().unwrap();
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()
        && tree.directories[id - listing.members.len()].path == std::path::Path::new("src/deep")));
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 2]);
}

#[test]
fn invalid_archive_regex_hides_every_row_and_clears_invisible_cursor_not_selection() {
    let mut listing = listing(&["src/file*.rs", "src/other.rs", "src/deep/x"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let selected = [0, 2].into_iter().collect();
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    assert_eq!(filter.visible(&listing, "*.rs"), [0]);
    filter.focused = Some(0);
    filter.options.regex = true;
    assert!(filter.visible(&listing, "*.rs").is_empty());
    assert!(matches!(filter.pattern, Some(Err(_))));
    assert_eq!(filter.focused, None);
    assert_eq!(
        step_member_cursor(&filter.visible, filter.focused, false, true, 1),
        None
    );
    assert_eq!(filter.copy_rows(&listing, &selected, true), [2, 0]);
    assert!(filter.copy_rows(&listing, &selected, false).is_empty());
    filter.options.regex = false;
    assert_eq!(filter.visible(&listing, "*.rs"), [0]);
    assert!(matches!(filter.pattern, Some(Ok(_))));
    assert_eq!(selected, [0, 2].into_iter().collect());
}

#[test]
fn literal_archive_filter_preserves_visible_cursor_but_clears_a_filtered_one() {
    let listing = listing(&["first.txt", "second.txt"]);
    let mut filter = MemberFilter::default();
    filter.visible(&listing, "");
    filter.focused = Some(0);
    filter.visible(&listing, "first");
    assert_eq!(filter.focused, Some(0));
    filter.visible(&listing, "second");
    assert_eq!(filter.focused, None);
    assert!(filter.reset_scroll);
    assert!(filter.visible(&listing, &" ".repeat(257)).is_empty());
    assert!(matches!(filter.pattern, Some(Err(_))));
}

#[test]
fn member_filter_matches_case_insensitively_and_tracks_the_text() {
    let listing = listing(&["src/Main.rs", "README.md", "src/lib.rs"]);
    let mut filter = MemberFilter::default();
    assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
    assert_eq!(filter.visible(&listing, "  SRC "), [2, 0]);
    assert_eq!(filter.visible(&listing, "main"), [0]);
    assert_eq!(filter.visible(&listing, "nothing"), [] as [usize; 0]);
    assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
}

#[test]
fn unchanged_filter_reuses_its_matches() {
    let listing = listing(&["a.txt", "b.txt"]);
    let mut filter = MemberFilter::default();
    let first = filter.visible(&listing, "a").as_ptr();
    // Same text, even with a different listing: no rescan until reset.
    let other = self::listing(&["b.txt", "a.txt"]);
    assert_eq!(filter.visible(&other, "a").as_ptr(), first);
    assert_eq!(filter.visible(&other, "a"), [0]);
}

#[test]
fn archive_sort_preserves_focus_and_selection_membership_after_filtering() {
    let mut listing = listing(&["first.txt", "middle.txt", "last.txt"]);
    listing.members[0].size = 2;
    listing.members[1].size = 100;
    listing.members[2].size = 50;
    let mut filter = MemberFilter::default();
    filter.visible(&listing, "");
    filter.focused = Some(2);
    let selected = std::collections::BTreeSet::from([1, 2]);
    filter.order.choose(crate::archive_order::Field::Size);
    assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
    assert_eq!(filter.focused, Some(2));
    assert!(filter.scroll_to_focused);
    assert_eq!(
        selected
            .iter()
            .filter(|index| !filter.visible_set.contains(*index))
            .count(),
        0
    );
    assert_eq!(filter.visible(&listing, "last"), [2]);
    assert_eq!(
        selected
            .iter()
            .filter(|index| !filter.visible_set.contains(*index))
            .count(),
        1
    );
    assert_eq!(filter.visible(&listing, ""), [1, 2, 0]);
}

#[test]
fn many_extract_errors_toast_a_count_and_cap_the_inline_list() {
    let one = vec!["Could not create /ro/a: denied".to_string()];
    assert_eq!(
        extract_error_summary(&one),
        (one[0].clone(), one[0].clone())
    );

    let errors: Vec<String> = (0..8)
        .map(|i| format!("Could not create /ro/{i}"))
        .collect();
    let (toast, inline) = extract_error_summary(&errors);
    assert_eq!(toast, "8 members failed: Could not create /ro/0");
    assert_eq!(inline.lines().count(), 6);
    assert!(inline.ends_with("… and 3 more"));
}

#[test]
fn extract_labels_name_hidden_selection_and_capped_listings() {
    assert_eq!(
        extract_selected_label(0, 0),
        "Extract selected to other panel"
    );
    assert_eq!(extract_selected_label(3, 0), "Extract 3 to other panel");
    assert_eq!(
        extract_selected_label(3, 2),
        "Extract 3 (2 hidden) to other panel"
    );

    let mut capped = listing(&["a", "b"]);
    assert_eq!(extract_all_label(&capped), "Extract all");
    capped.truncated = true;
    assert_eq!(extract_all_label(&capped), "Extract 2 listed");
}

#[test]
fn member_cursor_steps_through_visible_rows_only() {
    let visible = [2, 5, 9];
    assert_eq!(step_member_cursor(&visible, None, false, false, 1), None);
    assert_eq!(step_member_cursor(&visible, None, false, true, 1), Some(2));
    assert_eq!(
        step_member_cursor(&visible, Some(2), false, true, 1),
        Some(5)
    );
    assert_eq!(
        step_member_cursor(&visible, Some(9), false, true, 1),
        Some(9)
    );
    assert_eq!(
        step_member_cursor(&visible, Some(5), true, false, 1),
        Some(2)
    );
    assert_eq!(
        step_member_cursor(&visible, Some(2), true, false, 1),
        Some(2)
    );
    // A cursor the filter now hides restarts at the top.
    assert_eq!(
        step_member_cursor(&visible, Some(4), true, false, 1),
        Some(2)
    );
    assert_eq!(step_member_cursor(&[], Some(4), false, true, 1), Some(4));
}

#[test]
fn archive_selection_mask_keeps_view_hidden_choices_and_duplicate_ids() {
    let mut listing = listing(&[
        "src/A.rs",
        "src/b.txt",
        "hidden/keep.rs",
        "src/dup.rs",
        "src/dup.rs",
        "src/deep/c.rs",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    let visible = filter.visible(&listing, "").to_vec();
    let mut selected = [1, 2].into_iter().collect();
    filter
        .selection_memory
        .save(&selected, listing.members.len());
    assert!(visible.iter().any(|&id| id >= listing.members.len()));
    filter.selection_mask.query = "*.rs".into();
    let lower = filter.lower_paths.as_deref().unwrap();
    assert_eq!(
        filter
            .selection_mask
            .apply(&listing, lower, &visible, &mut selected, true),
        Ok(3)
    );
    assert_eq!(selected, [0, 1, 2, 3, 4].into_iter().collect());
    assert_eq!(
        filter
            .selection_mask
            .apply(&listing, lower, &visible, &mut selected, false),
        Ok(3)
    );
    assert_eq!(selected, [1, 2].into_iter().collect());
    filter.selection_mask.full_path = true;
    filter.selection_mask.query = "src/d*.rs".into();
    assert_eq!(
        filter
            .selection_mask
            .apply(&listing, lower, &visible, &mut selected, true),
        Ok(2)
    );
    assert_eq!(selected, [1, 2, 3, 4].into_iter().collect());
    assert_eq!(filter.visible, visible);
    assert_eq!(filter.folder, Some("src".into()));
    assert_eq!(filter.needle.as_deref(), Some(""));
    let mut saved = Default::default();
    filter
        .selection_memory
        .restore(&mut saved, listing.members.len());
    assert_eq!(saved, [1, 2].into_iter().collect());
}

#[test]
fn archive_selection_mask_rejects_invalid_input_and_respects_case_and_real_records() {
    let mut listing = listing(&["Dir.RS", "link.rs", "plain.rs"]);
    listing.members[0].is_dir = true;
    listing.members[1].is_symlink = true;
    let lower: Vec<_> = listing
        .members
        .iter()
        .map(|m| m.path.to_string_lossy().to_lowercase())
        .collect();
    let mut mask = super::MaskSelection {
        query: "*.RS".into(),
        case_sensitive: true,
        ..Default::default()
    };
    let mut selected = [2].into_iter().collect();
    assert_eq!(
        mask.apply(&listing, &lower, &[0, 1, 99], &mut selected, true),
        Ok(1)
    );
    assert_eq!(selected, [0, 2].into_iter().collect());
    mask.case_sensitive = false;
    assert_eq!(
        mask.apply(&listing, &lower, &[0, 1, 99], &mut selected, true),
        Ok(2)
    );
    assert_eq!(selected, [0, 1, 2].into_iter().collect());
    let before = selected.clone();
    for query in [String::new(), "[".into(), "x".repeat(257)] {
        mask.query = query;
        assert!(
            mask.apply(&listing, &lower, &[0, 1], &mut selected, false)
                .is_err()
        );
        assert_eq!(selected, before);
    }
    mask.query = "*".into();
    assert!(
        mask.apply(&listing, &[], &[0, 1], &mut selected, false)
            .is_err()
    );
    assert_eq!(selected, before);
}

#[test]
fn archive_selection_mask_pointer_actions_change_selection_without_changing_mask() {
    let listing = listing(&["a.rs", "b.txt", "c.rs", "hidden.rs"]);
    let lower: Vec<_> = listing
        .members
        .iter()
        .map(|m| m.path.to_string_lossy().to_lowercase())
        .collect();
    let mut mask = super::MaskSelection {
        query: "*.rs".into(),
        ..Default::default()
    };
    let mut selected = [3].into_iter().collect();
    let ctx = egui::Context::default();
    let mut actions = 0;
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let rect = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "archive-selection-mask-action",
                        frame < 10,
                    )))
                })
                .unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            if mask.ui(ui, &listing, &lower, &[0, 1, 2, 99], &mut selected) {
                actions += 1;
            }
        }));
        if frame == 5 {
            assert_eq!(selected, [0, 2, 3].into_iter().collect());
        }
    }
    assert_eq!(actions, 2);
    assert_eq!(selected, [3].into_iter().collect());
    assert_eq!(mask.query, "*.rs");
}

#[test]
fn archive_similar_name_uses_final_stem_casefold_and_real_regular_ids() {
    let mut listing = listing(&[
        "src/Archive.tar.gz",
        "other/archive.tar.xz",
        "src/archive.txt",
        "src/Archive.tar.gz",
        "src/archive.tar.link",
        "src/archive.tar.folder",
        ".env",
        "other/.env.txt",
    ]);
    listing.members[4].is_symlink = true;
    listing.members[5].is_dir = true;
    let lower: Vec<_> = listing
        .members
        .iter()
        .map(|m| m.path.to_string_lossy().to_lowercase())
        .collect();
    let visible = [3, 0, 4, 1, 5, 2, 6, 7, 100];
    assert_eq!(
        super::similar_member_rows(
            &listing,
            &lower,
            &visible,
            Some(0),
            super::SimilarMember::Name
        ),
        [3, 0, 1]
    );
    assert_eq!(
        super::similar_member_rows(
            &listing,
            &lower,
            &visible,
            Some(6),
            super::SimilarMember::Name
        ),
        [6, 7]
    );
    assert!(
        super::similar_member_rows(
            &listing,
            &lower,
            &visible,
            Some(100),
            super::SimilarMember::Name
        )
        .is_empty()
    );
    assert!(
        super::similar_member_rows(&listing, &[], &visible, Some(0), super::SimilarMember::Name)
            .is_empty()
    );
    assert!(
        super::similar_member_rows(&listing, &lower, &[1], Some(0), super::SimilarMember::Name)
            .is_empty()
    );
}

#[test]
fn archive_similar_folder_is_case_sensitive_and_includes_real_record_types() {
    let mut listing = listing(&["src/a", "src/b", "src/deep/c", "Src/d", "root", "folder"]);
    listing.members[1].is_symlink = true;
    listing.members[5].is_dir = true;
    let visible = [1, 0, 3, 100];
    let rows = super::similar_member_rows(
        &listing,
        &[],
        &visible,
        Some(0),
        super::SimilarMember::Folder,
    );
    assert_eq!(rows, [1, 0]);
    assert_eq!(
        super::similar_member_rows(
            &listing,
            &[],
            &[4, 5],
            Some(4),
            super::SimilarMember::Folder
        ),
        [4, 5]
    );
    assert!(
        super::similar_member_rows(
            &listing,
            &[],
            &visible,
            Some(100),
            super::SimilarMember::Folder
        )
        .is_empty()
    );
}

#[test]
fn archive_similar_selection_pointer_adds_and_removes_visible_matches_only() {
    for (mode, label, matches) in [
        (super::SimilarMember::Name, "Same name", vec![0, 2]),
        (super::SimilarMember::Folder, "Same folder", vec![0, 3]),
    ] {
        let listing = listing(&["src/one.txt", "hidden/keep", "other/one.md", "src/two.txt"]);
        let lower: Vec<_> = listing
            .members
            .iter()
            .map(|m| m.path.to_string_lossy().to_lowercase())
            .collect();
        let visible = [0, 2, 3, 99];
        let mut selected = [1].into_iter().collect();
        let ctx = egui::Context::default();
        let mut actions = 0;
        for frame in 0..28 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 10, 11, 17, 18, 24, 25].contains(&frame) {
                let id = if [3, 4, 17, 18].contains(&frame) {
                    egui::Id::new(("archive-similar-menu", label))
                } else {
                    egui::Id::new(("archive-similar-action", label, frame < 14))
                };
                let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: [3, 10, 17, 24].contains(&frame),
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    if super::similar_selection_menu(
                        ui,
                        &listing,
                        &lower,
                        &visible,
                        Some(0),
                        &mut selected,
                        mode,
                    ) {
                        actions += 1;
                    }
                });
            }));
            if frame == 14 {
                let expected: std::collections::BTreeSet<_> =
                    matches.iter().copied().chain([1]).collect();
                assert_eq!(selected, expected, "{label}");
            }
        }
        assert_eq!(actions, 2, "{label}");
        assert_eq!(selected, [1].into_iter().collect(), "{label}");
    }
}

#[test]
fn archive_modified_filter_preserves_navigation_selection_and_saved_ranges() {
    use std::time::{Duration, UNIX_EPOCH};
    let mut listing = listing(&["root.txt", "src/a.txt", "src/deep/b.txt", "src/unknown"]);
    listing.members[0].modified = Some(UNIX_EPOCH - Duration::from_secs(1));
    listing.members[1].modified = Some(UNIX_EPOCH);
    listing.members[2].modified = Some(UNIX_EPOCH + Duration::from_secs(86400));
    listing.members[3].modified = None;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter {
        modified_input: "1970-01-01".into(),
        ..Default::default()
    };
    assert!(filter.save_filter(""));
    let saved = filter.saved_filters[0].clone();
    filter.modified_input = "1970-01-01..1970-01-03".into();
    assert!(filter.save_filter(""));
    assert_eq!(filter.saved_filters.len(), 2);
    filter.restore_filter(saved.clone(), &mut String::new());
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    let selected = [0, 3].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 3]);
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
    let mut query = String::new();
    filter.reset_filters(&mut query);
    filter.visible(&listing, &query);
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 3]);
    filter.restore_filter(saved, &mut query);
    filter.visible(&listing, &query);
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1]);
    filter.modified_input = "bad".into();
    assert!(filter.visible(&listing, &query).is_empty());
    assert!(!filter.save_filter(""));
    filter.modified_input = "\u{a0}".repeat(13);
    assert!(filter.visible(&listing, &query).is_empty());
    assert!(filter.has_filters(&query));
    filter.reset_filters(&mut query);
    assert!(!filter.visible(&listing, &query).is_empty());
}

#[test]
fn archive_extensions_match_literal_suffixes_and_reject_invalid_queries() {
    let listing = listing(&[
        "report.PDF",
        "photos/a.jpg",
        "archive.tar.gz",
        "archive.gz",
        ".env",
        "dir.pdf/no_extension",
        "name.pdf.bak",
        "a.ЁЖ",
        "a.jp[g]",
    ]);
    let mut filter = MemberFilter::default();
    for (input, expected) in [
        ("pdf .JPG", vec![0, 1]),
        ("tar.gz", vec![2]),
        ("gz", vec![2, 3]),
        ("env", vec![4]),
        ("ёж", vec![7]),
        ("pdf PDF .pdf", vec![0]),
        ("", (0..9).collect()),
    ] {
        filter.extensions_input = input.into();
        let mut actual = filter.visible(&listing, "").to_vec();
        actual.sort_unstable();
        assert_eq!(actual, expected, "{input}");
    }
    for input in [
        ".", "..pdf", "pdf.", "tar..gz", "*.pdf", "jp[g]", "a/b", "a\\b", "pdf,jpg", "a\0b",
    ] {
        filter.extensions_input = input.into();
        assert!(filter.visible(&listing, "").is_empty(), "{input:?}");
        assert!(matches!(filter.extensions_pattern, Some(Err(_))));
        assert!(!filter.save_filter(""));
    }
    for input in ["a ".repeat(33), "x".repeat(257), "\u{a0}".repeat(129)] {
        filter.extensions_input = input;
        assert!(filter.visible(&listing, "").is_empty());
        assert!(filter.has_filters(""));
    }
    filter.reset_filters(&mut String::new());
    assert_eq!(filter.visible(&listing, "").len(), 9);
}

#[test]
fn archive_extensions_compose_preserve_hidden_selection_and_restore_saved_settings() {
    let mut listing = listing(&[
        "root.pdf",
        "src/a.PDF",
        "src/a.PDF",
        "src/b.jpg",
        "src/folder.pdf",
        "src/link.pdf",
        "src/deep/note.txt",
    ]);
    listing.members[4].is_dir = true;
    listing.members[4].is_regular = false;
    listing.members[5].is_symlink = true;
    listing.members[5].is_regular = false;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter {
        extensions_input: "pdf".into(),
        ..Default::default()
    };
    assert_eq!(filter.visible(&listing, ""), [0, 1, 2, 4, 5]);
    assert!(filter.save_filter(""));
    let saved = filter.saved_filters[0].clone();
    filter.extensions_input = "jpg".into();
    assert!(filter.save_filter(""));
    assert_eq!(filter.saved_filters.len(), 2);
    filter.restore_filter(saved.clone(), &mut String::new());
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    let selected = [0, 3].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 2, 5]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [0, 3]);
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
    filter.kinds.links = false;
    filter.visible(&listing, "a.");
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 2]);
    filter.exclude_matches = true;
    filter.visible(&listing, "a.");
    assert!(filter.copy_rows(&listing, &selected, false).is_empty());
    filter.extensions_input = "*.pdf".into();
    assert!(filter.visible(&listing, "").is_empty());
    let mut query = String::new();
    filter.reset_filters(&mut query);
    assert_eq!(filter.saved_filters.len(), 2);
    filter.restore_filter(saved, &mut query);
    assert_eq!(
        filter
            .visible(&listing, &query)
            .iter()
            .filter(|&&id| id < listing.members.len())
            .count(),
        3
    );
}

#[test]
fn archive_depth_filter_exact_open_ranges_and_invalid_inputs() {
    let listing = listing(&["a", "dir/b", "dir/deep/c", "dir/deep/more/d"]);
    let mut filter = MemberFilter::default();
    for (text, expected) in [
        ("1", vec![0]),
        ("2..3", vec![1, 2]),
        ("..2", vec![0, 1]),
        ("3..", vec![2, 3]),
        ("0", vec![]),
        ("0..4294967295", vec![0, 1, 2, 3]),
        ("", vec![0, 1, 2, 3]),
    ] {
        filter.depth_input = text.into();
        assert_eq!(filter.visible(&listing, ""), expected, "{text}");
    }
    for text in [
        "..",
        "3..2",
        "-1",
        "+2",
        "1.2",
        "1..2..3",
        "4294967296",
        "one",
        "1 2",
    ] {
        filter.depth_input = text.into();
        assert!(filter.visible(&listing, "").is_empty(), "{text}");
        assert!(matches!(filter.depth_pattern, Some(Err(_))));
        assert!(!filter.save_filter(""));
    }
    filter.depth_input = "\u{a0}".repeat(13);
    assert!(filter.visible(&listing, "").is_empty());
    assert!(filter.has_filters(""));
    filter.reset_filters(&mut String::new());
    assert_eq!(filter.visible(&listing, "").len(), 4);
}

#[test]
fn archive_depth_filter_keeps_archive_root_basis_navigation_selection_and_saved_identity() {
    let mut listing = listing(&[
        "root.txt",
        "src/a.txt",
        "src/deep/b.txt",
        "elsewhere/hidden",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter {
        depth_input: "2".into(),
        ..Default::default()
    };
    assert!(filter.save_filter(""));
    let saved = filter.saved_filters[0].clone();
    filter.depth_input = "1..2".into();
    assert!(filter.save_filter(""));
    assert_eq!(filter.saved_filters.len(), 2);
    filter.restore_filter(saved.clone(), &mut String::new());
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    let selected = [0, 3].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 0]);
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
    filter.navigate(Some("src/deep".into()));
    assert!(filter.visible(&listing, "").is_empty());
    let mut query = String::new();
    filter.reset_filters(&mut query);
    assert_eq!(filter.visible(&listing, &query), [2]);
    filter.restore_filter(saved, &mut query);
    assert!(filter.visible(&listing, &query).is_empty());
    filter.depth_input = "bad".into();
    filter.navigate(Some("src".into()));
    assert!(filter.visible(&listing, &query).is_empty());
}

#[test]
fn executable_archive_filter_requires_known_regular_file_status() {
    let mut listing = listing(&["a", "b", "c", "link", "folder", "other"]);
    listing.members[0].is_executable = Some(true);
    listing.members[1].is_executable = Some(false);
    listing.members[3].is_executable = Some(true);
    listing.members[3].is_symlink = true;
    listing.members[4].is_executable = Some(true);
    listing.members[4].is_dir = true;
    listing.members[5].is_executable = Some(true);
    listing.members[5].is_regular = false;
    let mut filter = MemberFilter::default();
    filter.visible(&listing, "");
    assert_eq!(filter.visible.len(), 6);
    filter.kinds.executable_only = true;
    assert_eq!(filter.visible(&listing, ""), [0]);
    filter.kinds.files = false;
    assert!(filter.visible(&listing, "").is_empty());
    filter.kinds = Default::default();
    assert_eq!(filter.visible(&listing, "").len(), 6);
}

#[test]
fn executable_archive_filter_preserves_navigation_hidden_selection_and_saved_identity() {
    let mut listing = listing(&[
        "src/run.sh",
        "src/plain.txt",
        "src/deep/tool",
        "elsewhere/hidden",
    ]);
    listing.members[0].is_executable = Some(true);
    listing.members[1].is_executable = Some(false);
    listing.members[2].is_executable = Some(true);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    assert!(filter.save_filter("src"));
    filter.kinds.executable_only = true;
    assert!(filter.save_filter("src"));
    assert_eq!(filter.saved_filters.len(), 2);
    let saved = filter.saved_filters[0].clone();
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    let selected = [1, 3].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [3, 1]);
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
    let mut query = String::new();
    filter.reset_filters(&mut query);
    assert!(!filter.kinds.executable_only);
    filter.visible(&listing, &query);
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 0]);
    filter.restore_filter(saved, &mut query);
    filter.visible(&listing, &query);
    assert!(filter.kinds.executable_only);
    assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
}

#[test]
fn archive_fuzzy_mode_rebuilds_duplicates_scopes_intersections_and_saved_navigation() {
    let mut listing = listing(&[
        "src/main.rs",
        "src/map.rs",
        "src/main.rs",
        "docs/Maintenance.md",
        "src/deep/lib.rs",
    ]);
    listing.members[2].size = 2;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let selected: std::collections::BTreeSet<_> = [0, 2, 3].into_iter().collect();
    let mut filter = MemberFilter::default();
    assert!(filter.visible(&listing, "mnrs").is_empty());
    filter.options.fuzzy = true;
    assert_eq!(filter.visible(&listing, "mnrs"), [0, 2]);
    filter.focused = Some(2);
    filter.options.fuzzy = false;
    assert!(filter.visible(&listing, "mnrs").is_empty());
    assert!(filter.focused.is_none());
    filter.options.fuzzy = true;
    filter.options.basename_only = true;
    assert!(filter.visible(&listing, "smrs").is_empty());
    assert_eq!(filter.visible(&listing, "mnrs"), [0, 2]);
    filter.size_input = "1b".into();
    assert_eq!(filter.visible(&listing, "mnrs"), [0]);
    filter.size_input.clear();
    filter.kinds.files = false;
    assert!(filter.visible(&listing, "mnrs").is_empty());
    filter.kinds.files = true;
    filter.exclusion.settings.query = "*main*".into();
    assert!(filter.visible(&listing, "mnrs").is_empty());
    filter.exclusion.settings.query.clear();
    let mut query = "mnrs".to_owned();
    assert_eq!(filter.visible(&listing, &query), [0, 2]);
    assert!(filter.save_filter(&query));
    let saved = filter.saved_filters[0].clone();
    assert!(saved.label().contains("Fuzzy"));
    filter.reset_filters(&mut query);
    assert!(!filter.options.fuzzy);
    filter.restore_filter(saved, &mut query);
    assert!(filter.options.fuzzy);
    assert_eq!(filter.visible(&listing, &query), [0, 2]);
    assert_eq!(
        filter
            .copy_rows(&listing, &selected, true)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        selected
    );
    filter.navigate(Some(Default::default()));
    filter.visible(&listing, "sc");
    let tree = listing.tree.as_ref().unwrap();
    assert_eq!(filter.visible.len(), 1);
    assert_eq!(
        tree.directories[filter.visible[0] - listing.members.len()].path,
        std::path::Path::new("src")
    );
    assert!(filter.copy_rows(&listing, &selected, false).is_empty());
    filter.exclude_matches = true;
    filter.visible(&listing, "sc");
    assert_eq!(filter.visible.len(), 1);
    assert_eq!(
        tree.directories[filter.visible[0] - listing.members.len()].path,
        std::path::Path::new("docs")
    );
    assert_eq!(
        filter
            .copy_rows(&listing, &selected, true)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        selected
    );
    filter.options.regex = true;
    assert!(filter.visible(&listing, "").is_empty());
    assert!(!filter.save_filter(""));
}

#[test]
fn archive_matching_mode_checkboxes_are_exclusive_and_preserve_scope_and_case() {
    let ctx = egui::Context::default();
    let mut options = crate::archive_filter::Options {
        regex: true,
        smart_case: true,
        basename_only: true,
        ..Default::default()
    };
    for _ in 0..2 {
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal_wrapped(|ui| super::matching_modes_ui(ui, &mut options, true));
        }));
    }
    for label in ["Fuzzy", "Glob", "Exact", "Regex", "Regex"] {
        for pressed in [true, false] {
            let rect = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("archive-match-mode", label))))
                .unwrap();
            let input = egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            };
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                ui.horizontal_wrapped(|ui| super::matching_modes_ui(ui, &mut options, true));
            }));
        }
        assert!(
            u8::from(options.fuzzy)
                + u8::from(options.glob)
                + u8::from(options.exact)
                + u8::from(options.regex)
                <= 1
        );
        assert!(options.smart_case && options.basename_only);
        if label == "Fuzzy" {
            assert!(options.fuzzy);
        }
        if label == "Glob" {
            assert!(options.glob);
        }
        if label == "Exact" {
            assert!(options.exact);
        }
    }
    assert!(!options.regex && !options.exact && !options.glob && !options.fuzzy);
}

#[test]
fn archive_glob_filter_composes_with_scope_navigation_exclusion_and_saved_modes() {
    let mut listing = listing(&["src/a.rs", "src/deep/b.rs", "src/c.txt", "other/a.rs"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.options.glob = true;
    assert!(filter.visible(&listing, "*.rs").is_empty());
    filter.options.basename_only = true;
    assert_eq!(filter.visible(&listing, "*.rs"), [3, 0, 1]);
    filter.options.basename_only = false;
    let mut query = "src/**/*.rs".to_string();
    assert_eq!(filter.visible(&listing, &query), [0, 1]);
    assert!(filter.save_filter(&query));
    let saved = filter.saved_filters[0].clone();
    filter.reset_filters(&mut query);
    assert!(!filter.options.glob);
    filter.restore_filter(saved, &mut query);
    assert_eq!(filter.visible(&listing, &query), [0, 1]);
    filter.navigate(Some(Default::default()));
    filter.visible(&listing, "src*");
    let tree = listing.tree.as_ref().unwrap();
    assert_eq!(filter.visible.len(), 1);
    assert_eq!(
        tree.directories[filter.visible[0] - listing.members.len()].path,
        std::path::Path::new("src")
    );
    filter.exclude_matches = true;
    filter.visible(&listing, "src*");
    assert_eq!(filter.visible.len(), 1);
    assert_eq!(
        tree.directories[filter.visible[0] - listing.members.len()].path,
        std::path::Path::new("other")
    );
    assert_eq!(
        filter.copy_rows(&listing, &[2].into_iter().collect(), true),
        [2]
    );
    assert!(filter.visible(&listing, "[").is_empty());
    assert!(!filter.save_filter("["));
}

#[test]
fn archive_exact_filter_rebuilds_and_restores_saved_mode_with_hidden_selection() {
    let mut listing = listing(&["src/a.txt", "src/prefix-a.txt", "a.txt/child", "src/a.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.options.basename_only = true;
    let mut query = "a.txt".to_string();
    assert_eq!(filter.visible(&listing, &query), [0, 3, 1]);
    assert!(filter.save_filter(&query));
    filter.options.exact = true;
    assert_eq!(filter.visible(&listing, &query), [0, 3]);
    assert!(filter.save_filter(&query));
    assert_eq!(filter.saved_filters.len(), 2);
    let saved = filter.saved_filters[0].clone();
    filter.reset_filters(&mut query);
    assert!(!filter.options.exact);
    filter.restore_filter(saved, &mut query);
    assert_eq!(filter.visible(&listing, &query), [0, 3]);
    filter.navigate(Some(Default::default()));
    filter.visible(&listing, &query);
    assert_eq!(filter.visible.len(), 1);
    assert!(filter.visible[0] >= listing.members.len());
    filter.exclude_matches = true;
    filter.visible(&listing, &query);
    let tree = listing.tree.as_ref().unwrap();
    assert_eq!(filter.visible.len(), 1);
    assert_eq!(
        tree.directories[filter.visible[0] - listing.members.len()].path,
        std::path::Path::new("src")
    );
    assert_eq!(
        filter.copy_rows(&listing, &[1].into_iter().collect(), true),
        [1]
    );
    filter.options.regex = true;
    assert!(filter.visible(&listing, &query).is_empty());
    assert!(!filter.save_filter(&query));
}

#[test]
fn archive_filename_scope_rebuilds_duplicates_navigation_and_saved_filters() {
    let mut listing = listing(&[
        "src/main.rs",
        "src/other.txt",
        "main.rs/nested.txt",
        "src/main.rs",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.options.regex = true;
    let mut query = r"^main\.rs$".to_string();
    assert!(filter.visible(&listing, &query).is_empty());
    assert!(filter.save_filter(&query));
    filter.options.basename_only = true;
    assert_eq!(filter.visible(&listing, &query), [0, 3]);
    assert!(filter.save_filter(&query));
    assert_eq!(filter.saved_filters.len(), 2);
    let saved = filter.saved_filters[0].clone();
    filter.reset_filters(&mut query);
    assert!(!filter.options.basename_only);
    filter.restore_filter(saved, &mut query);
    assert_eq!(filter.visible(&listing, &query), [0, 3]);
    filter.navigate(Some(Default::default()));
    filter.visible(&listing, &query);
    assert_eq!(filter.visible.len(), 1);
    assert!(filter.visible[0] >= listing.members.len());
    let selected = [1].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, true), [1]);
    filter.exclude_matches = true;
    filter.visible(&listing, &query);
    assert_eq!(filter.visible.len(), 1);
    let tree = listing.tree.as_ref().unwrap();
    assert_eq!(
        tree.directories[filter.visible[0] - listing.members.len()].path,
        std::path::Path::new("src")
    );
    assert!(filter.visible(&listing, "[").is_empty());
}

#[test]
fn archive_size_filter_units_boundaries_and_overflow_are_exact() {
    let mut listing = listing(&["a", "b", "c", "d", "e", "f"]);
    for (member, size) in
        listing
            .members
            .iter_mut()
            .zip([0, 1000, 1024, 1048576, 9007199254740993, u64::MAX])
    {
        member.size = size;
    }
    let mut filter = MemberFilter::default();
    for (input, expected) in [
        ("0b", vec![0]),
        ("+1k -1ki", vec![1, 2]),
        ("1MI", vec![3]),
        ("9007199254740993", vec![4]),
        ("18446744073709551615b", vec![5]),
        ("+18446744073709551615", vec![5]),
        ("-0", vec![0]),
        ("+2ki -1ki", vec![]),
        ("", vec![0, 1, 2, 3, 4, 5]),
    ] {
        filter.size_input = input.into();
        assert_eq!(filter.visible(&listing, ""), expected, "{input}");
    }
    for input in [
        "+",
        "--1",
        "1.5mi",
        "1kb",
        "18446744073709551616",
        "18446744073709551615ki",
        "1b 2b 3b",
        "é",
    ] {
        filter.size_input = input.into();
        assert!(filter.visible(&listing, "").is_empty(), "{input}");
        assert!(matches!(filter.size_pattern, Some(Err(_))));
        assert!(!filter.save_filter(""));
    }
    filter.size_input = "0".repeat(65);
    assert!(filter.visible(&listing, "").is_empty());
    filter.size_input = "\u{a0}".repeat(40);
    assert!(filter.visible(&listing, "").is_empty());
    assert!(filter.has_filters(""));
    filter.reset_filters(&mut String::new());
    assert_eq!(filter.visible(&listing, "").len(), 6);
}

#[test]
fn archive_size_filter_retains_navigation_hidden_selection_and_saved_settings() {
    let mut listing = listing(&[
        "src/empty.txt",
        "src/data.bin",
        "src/link",
        "src/deep/child",
        "elsewhere/hidden",
    ]);
    for (member, size) in listing.members.iter_mut().zip([0, 10, 0, 20, 100]) {
        member.size = size;
    }
    listing.members[2].is_symlink = true;
    listing.members[2].is_regular = false;
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    filter.size_input = "0b".into();
    filter.visible(&listing, "");
    let selected = [0, 4].into_iter().collect();
    assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
    assert_eq!(filter.copy_rows(&listing, &selected, true), [4, 0]);
    assert!(filter.visible.iter().any(|&id| id >= listing.members.len()));
    assert!(filter.save_filter(""));
    let saved = filter.saved_filters[0].clone();
    filter.size_input = "+0".into();
    assert!(filter.save_filter(""));
    assert_eq!(filter.saved_filters.len(), 2);
    let mut query = String::new();
    filter.reset_filters(&mut query);
    filter.visible(&listing, &query);
    assert_eq!(filter.copy_rows(&listing, &selected, false), [1, 0, 2]);
    filter.restore_filter(saved, &mut query);
    filter.visible(&listing, &query);
    assert_eq!(filter.size_input, "0b");
    assert_eq!(filter.copy_rows(&listing, &selected, false), [0]);
    assert!(filter.has_filters(&query));
    filter.size_input = "bad".into();
    assert!(filter.visible(&listing, &query).is_empty());
    filter.reset_filters(&mut query);
    assert!(!filter.has_filters(&query));
    assert!(!filter.visible(&listing, &query).is_empty());
}

#[test]
fn saved_archive_filters_bound_text_count_and_deduplicate_complete_settings() {
    let mut filter = MemberFilter::default();
    assert!(!filter.save_filter(""));
    assert!(filter.save_filter("a"));
    filter.options.regex = true;
    assert!(!filter.save_filter("["));
    assert!(filter.save_filter("a"));
    filter.exclude_matches = true;
    assert!(filter.save_filter("a"));
    filter.kinds.links = false;
    assert!(filter.save_filter("a"));
    assert_eq!(filter.saved_filters.len(), 4);
    assert!(filter.save_filter("a"));
    assert_eq!(filter.saved_filters.len(), 4);
    for index in 0..30 {
        assert!(filter.save_filter(&index.to_string()));
    }
    assert_eq!(filter.saved_filters.len(), 20);
    assert_eq!(filter.saved_filters.back().unwrap().query, "10");
    filter.options.regex = false;
    for index in 10..30 {
        assert!(filter.save_filter(&format!("{}{index}", "😀".repeat(254))));
    }
    assert!(
        filter
            .saved_filters
            .iter()
            .map(|e| e.query.len())
            .sum::<usize>()
            <= 16 * 1024
    );
    let before = filter.saved_filters.clone();
    assert!(!filter.save_filter(&"x".repeat(257)));
    assert_eq!(filter.saved_filters, before);
}

#[test]
fn removing_one_saved_archive_filter_uses_all_settings_and_preserves_active_state() {
    let listing = listing(&["a.txt", "b.rs"]);
    let mut filter = MemberFilter::default();
    assert!(filter.save_filter("a"));
    let first = filter.saved_filters[0].clone();
    filter.options.regex = true;
    assert!(filter.save_filter("a"));
    let second = filter.saved_filters[0].clone();
    filter.visible(&listing, "a");
    filter.focused = Some(0);
    filter.range_anchor = Some(0);
    filter.selection_memory.save(&[1].into_iter().collect(), 2);
    assert!(filter.remove_saved_filter(&first));
    assert_eq!(filter.saved_filters.len(), 1);
    assert_eq!(filter.saved_filters.front(), Some(&second));
    assert!(!filter.remove_saved_filter(&first));
    assert_eq!(filter.visible, [0]);
    assert_eq!(filter.needle.as_deref(), Some("a"));
    assert!(filter.options.regex);
    assert_eq!(filter.focused, Some(0));
    assert_eq!(filter.range_anchor, Some(0));
    let mut selected = Default::default();
    filter.selection_memory.restore(&mut selected, 2);
    assert_eq!(selected, [1].into_iter().collect());
    assert!(filter.remove_saved_filter(&second));
    assert!(filter.saved_filters.is_empty());
    assert_eq!(filter.visible(&listing, "a"), [0]);
}

#[test]
fn saved_filter_search_pointer_input_targets_original_identity_for_recall_and_removal() {
    for remove in [false, true] {
        let listing = listing(&["current.txt", "ёж.txt"]);
        let mut filter = MemberFilter::default();
        assert!(filter.save_filter("ЁЖ"));
        assert!(filter.save_filter("other"));
        let mut query = "current".to_string();
        filter.visible(&listing, &query);
        let ctx = egui::Context::default();
        let mut recalls = 0;
        for frame in 0..17 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 6, 7, 12, 13].contains(&frame) {
                let id = if frame < 6 {
                    egui::Id::new("archive-saved-filters-menu")
                } else if frame < 12 {
                    egui::Id::new("archive-saved-filter-search-rect")
                } else {
                    egui::Id::new((
                        if remove {
                            "archive-remove-saved-filter"
                        } else {
                            "archive-saved-filter"
                        },
                        1usize,
                    ))
                };
                let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: [3, 6, 12].contains(&frame),
                        modifiers: Default::default(),
                    },
                ];
            } else if frame == 8 {
                input.events.push(egui::Event::Text("ёж".into()));
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if filter.saved_filters_ui(ui, &mut query, true) {
                        recalls += 1;
                    }
                });
            }));
            if (9..12).contains(&frame) {
                assert_eq!(filter.saved_filter_search, "ёж");
                assert_eq!(
                    ctx.data(|data| data
                        .get_temp::<usize>(egui::Id::new("archive-saved-filter-matches"))),
                    Some(1)
                );
                assert_eq!(query, "current");
                assert_eq!(filter.needle.as_deref(), Some("current"));
                assert_eq!(filter.saved_filters.len(), 2);
            }
        }
        if remove {
            assert_eq!(recalls, 0);
            assert_eq!(query, "current");
            assert_eq!(filter.saved_filters.len(), 1);
            assert_eq!(filter.saved_filters[0].query, "other");
        } else {
            assert_eq!(recalls, 1);
            assert_eq!(query, "ЁЖ");
            assert_eq!(filter.saved_filters.len(), 2);
        }
    }
}

#[test]
fn saved_archive_filter_pointer_removal_does_not_recall_the_entry() {
    let listing = listing(&["a.txt", "b.rs"]);
    let mut filter = MemberFilter::default();
    assert!(filter.save_filter("a"));
    assert!(filter.save_filter("b"));
    let mut query = "current".to_string();
    filter.visible(&listing, &query);
    let ctx = egui::Context::default();
    let mut recalls = 0;
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let id = if frame < 10 {
                egui::Id::new("archive-saved-filters-menu")
            } else {
                egui::Id::new(("archive-remove-saved-filter", 0usize))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            ui.horizontal_wrapped(|ui| {
                if filter.saved_filters_ui(ui, &mut query, true) {
                    recalls += 1;
                }
            });
        }));
    }
    assert_eq!(recalls, 0);
    assert_eq!(query, "current");
    assert_eq!(filter.needle.as_deref(), Some("current"));
    assert_eq!(filter.saved_filters.len(), 1);
    assert_eq!(filter.saved_filters[0].query, "a");
}

#[test]
fn saved_archive_filter_restore_rebuilds_rows_and_preserves_location_and_selection_owners() {
    let mut listing = listing(&["src/a.txt", "src/B.rs", "hidden/c.txt"]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.options.case_sensitive = true;
    filter.exclude_matches = true;
    filter.kinds.links = false;
    assert!(filter.save_filter(".rs"));
    let saved = filter.saved_filters[0].clone();
    let mut query = "other".to_string();
    filter.reset_filters(&mut query);
    assert_eq!(filter.saved_filters.len(), 1);
    filter.navigate(Some("src".into()));
    filter.order.descending = true;
    filter.narrowed = Some([0, 1].into_iter().collect());
    let selected = [0, 2].into_iter().collect();
    filter.selection_memory.save(&selected, 3);
    let history_len = filter.past.len();
    filter.restore_filter(saved.clone(), &mut query);
    assert_eq!(filter.visible(&listing, &query), [0]);
    assert_eq!(filter.options, saved.options);
    assert_eq!(filter.kinds, saved.kinds);
    assert!(filter.exclude_matches && filter.order.descending);
    assert_eq!(filter.folder, Some("src".into()));
    assert_eq!(filter.narrowed, Some([0, 1].into_iter().collect()));
    assert_eq!(filter.past.len(), history_len);
    let mut restored = Default::default();
    filter.selection_memory.restore(&mut restored, 3);
    assert_eq!(restored, selected);
    assert_eq!(filter.copy_rows(&listing, &selected, true).len(), 2);
    assert!(filter.reset_scroll);
}

#[test]
fn saved_archive_filter_pointer_recall_restores_query_and_flags() {
    let listing = listing(&["a.txt", "b.rs"]);
    let mut filter = MemberFilter::default();
    filter.options.regex = true;
    filter.options.case_sensitive = true;
    filter.exclude_matches = true;
    filter.kinds.links = false;
    assert!(filter.save_filter(r"\.rs$"));
    let mut query = String::new();
    filter.reset_filters(&mut query);
    let ctx = egui::Context::default();
    let mut recalls = 0;
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let id = if frame < 10 {
                egui::Id::new("archive-saved-filters-menu")
            } else {
                egui::Id::new(("archive-saved-filter", 0usize))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            ui.horizontal_wrapped(|ui| {
                if filter.saved_filters_ui(ui, &mut query, true) {
                    recalls += 1;
                }
            });
        }));
        filter.visible(&listing, &query);
    }
    assert_eq!(recalls, 1);
    assert_eq!(query, r"\.rs$");
    assert!(filter.options.regex && filter.options.case_sensitive && filter.exclude_matches);
    assert!(!filter.kinds.links);
    assert_eq!(filter.visible(&listing, &query), [0]);
}

#[test]
fn extra_copy_formats_use_sorted_real_rows_and_include_hidden_selection() {
    use crate::archive_export::{Format, format};
    let mut listing = listing(&[
        "src/one.txt",
        "src/two.md",
        "hidden/one.rs",
        "src/deep/file.txt",
    ]);
    listing.tree = crate::archive_tree::Tree::build(&listing.members, || false);
    let mut filter = MemberFilter::default();
    filter.navigate(Some("src".into()));
    filter.visible(&listing, "");
    let selected = [0, 1, 2].into_iter().collect();
    let visible = filter.copy_rows(&listing, &selected, false);
    let selected = filter.copy_rows(&listing, &selected, true);
    assert!(visible.iter().all(|&id| id < listing.members.len()));
    let archive = std::path::Path::new("/host/fixture.zip");
    assert_eq!(
        format(archive, &listing, &visible, Format::NamesWithoutExtension).unwrap(),
        "one\ntwo"
    );
    assert_eq!(
        format(archive, &listing, &selected, Format::NamesWithoutExtension).unwrap(),
        "one\none\ntwo"
    );
    assert_eq!(
        format(archive, &listing, &visible, Format::ParentFolders).unwrap(),
        "src"
    );
    assert_eq!(
        format(archive, &listing, &selected, Format::ParentFolders).unwrap(),
        "hidden\nsrc"
    );
}

#[test]
fn member_page_cursor_clamps_in_display_order_and_preserves_hidden_selection() {
    let visible = [2, 9, 6, 7, 1]; // 9 is a synthetic navigation folder.
    assert_eq!(
        step_member_cursor(&visible, Some(2), false, true, 3),
        Some(7)
    );
    assert_eq!(
        step_member_cursor(&visible, Some(7), false, true, usize::MAX),
        Some(1)
    );
    assert_eq!(
        step_member_cursor(&visible, Some(7), true, false, usize::MAX),
        Some(2)
    );
    assert_eq!(step_member_cursor(&visible, None, false, true, 3), Some(2));
    assert_eq!(
        step_member_cursor(&visible, Some(0), true, false, 3),
        Some(2)
    );
    assert_eq!(step_member_cursor(&[], Some(0), false, true, 3), Some(0));
    let mut selected = [0].into_iter().collect();
    let target = step_member_cursor(&visible, Some(2), false, true, 3);
    super::select_member_range(&visible, Some(2), target, 8, &mut selected);
    assert_eq!(selected, [0, 2, 6, 7].into_iter().collect());
    let target = step_member_cursor(&visible, target, true, false, 3);
    super::select_member_range(&visible, Some(2), target, 8, &mut selected);
    assert_eq!(selected, [0, 2, 6, 7].into_iter().collect());
}

#[test]
fn member_page_input_is_focus_scoped_shift_aware_and_preserves_modified_keys() {
    for (focused, modifiers, key, expected) in [
        (
            true,
            egui::Modifiers::NONE,
            egui::Key::PageDown,
            Some((true, 7, false)),
        ),
        (
            true,
            egui::Modifiers::SHIFT,
            egui::Key::PageUp,
            Some((false, 7, true)),
        ),
        (
            true,
            egui::Modifiers::SHIFT,
            egui::Key::ArrowDown,
            Some((true, 1, true)),
        ),
        (true, egui::Modifiers::CTRL, egui::Key::PageDown, None),
        (true, egui::Modifiers::ALT, egui::Key::PageUp, None),
        (false, egui::Modifiers::NONE, egui::Key::PageDown, None),
    ] {
        let ctx = egui::Context::default();
        let id = egui::Id::new("archive-filter");
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            if focused {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
            let motion = super::member_cursor_input(&ctx, id, 7);
            assert_eq!(motion.map(|m| (m.down, m.rows, m.extend)), expected);
            assert_eq!(
                ctx.input(|input| input.key_pressed(key)),
                expected.is_none()
            );
        }));
    }
}
