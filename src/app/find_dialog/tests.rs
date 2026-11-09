use super::{FindCopyFormat, FindState};
use super::{
    add_smart_folder, explain_key, explained_hit, fail_find_search_start, history_row_label,
    index_build_refusal, index_skipped_summary, move_result_highlight, pivot_order,
    result_row_height, search_root_error,
};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

fn json_state() -> FindState {
    let root = if cfg!(windows) {
        PathBuf::from(r"C:\x")
    } else {
        PathBuf::from("/x")
    };
    let mut state = FindState {
        root: root.clone(),
        expression: "name:report".into(),
        result_copy_format: FindCopyFormat::DetailsJson,
        result_complete: true,
        ran: true,
        scanned: 8,
        matched: 3,
        search_source: "Indexed snapshot".into(),
        results: vec![
            hit("report-a", None),
            hit("report-hidden", None),
            hit("bundle.zip!/report-a", None),
        ],
        ..Default::default()
    };
    for hit in &mut state.results {
        hit.entry.path = root.join(&hit.entry.name);
        hit.location = crate::search::SearchLocation::Native(hit.entry.path.clone());
    }
    state.results[2].location = crate::search::SearchLocation::ArchiveMember {
        archive: root.join("bundle.zip"),
        member: "report-a".into(),
        index: 7,
    };
    state.results[0].explanation = crate::search::MatchExplanation {
        total_score: 42,
        components: vec![crate::search::MatchComponent {
            field: "name",
            detail: "пример\n".into(),
            score: 42,
            matched_ranges: vec![(0, 3)],
        }],
    };
    state.last_query = Some(("name:report".into(), state.mode, state.root.clone()));
    state
}
#[test]
fn created_report_preserves_exact_native_times_without_archive_fallback() {
    let mut state = json_state();
    for (value, expected) in [
        (None, None),
        (Some(SystemTime::UNIX_EPOCH), Some("1970-01-01T00:00:00Z")),
        (
            Some(SystemTime::UNIX_EPOCH - std::time::Duration::from_millis(500)),
            Some("1969-12-31T23:59:59.500Z"),
        ),
    ] {
        state.results[0].entry.created = value;
        state.results[2].entry.created = value;
        let (text, _) = state
            .result_json(SystemTime::now(), false)
            .unwrap()
            .unwrap();
        let r: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(r["rows"][0]["created_utc"].as_str(), expected);
        assert!(r["rows"][2]["created_utc"].is_null());
    }
}

#[test]
fn changed_report_preserves_exact_native_times_without_archive_fallback() {
    let mut state = json_state();
    for (value, expected) in [
        (None, None),
        (Some(SystemTime::UNIX_EPOCH), Some("1970-01-01T00:00:00Z")),
        (
            Some(SystemTime::UNIX_EPOCH - std::time::Duration::from_millis(500)),
            Some("1969-12-31T23:59:59.500Z"),
        ),
    ] {
        state.results[0].entry.changed = value;
        state.results[2].entry.changed = value;
        let (text, _) = state
            .result_json(SystemTime::now(), false)
            .unwrap()
            .unwrap();
        let r: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(r["rows"][0]["changed_utc"].as_str(), expected);
        assert!(r["rows"][2]["changed_utc"].is_null());
    }
}

#[test]
fn metric_query_report_preserves_zero_full_width_and_unknown_values() {
    let mut state = json_state();
    for value in [Some(0), Some(u64::MAX), None] {
        state.results[0].entry.allocated_size = value;
        state.results[0].entry.hard_links = value;
        let (text, _) = state
            .result_json(SystemTime::now(), false)
            .unwrap()
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(report["rows"][0]["allocated_bytes"].as_u64(), value);
        assert_eq!(report["rows"][0]["hard_links"].as_u64(), value);
    }
}

#[test]
fn search_json_uses_applied_context_and_distinct_locations_in_visible_selected_order() {
    let now = SystemTime::now();
    let mut state = json_state();
    state.hide_result(state.results[1].location.clone());
    state.select_result(state.results[2].location.clone(), false, now);
    state.results[0].entry.modified =
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(1 << 43));
    state.expression = "type:invalid".into();
    state.root = "/another".into();
    state.mode = crate::query::MatchMode::Regex;
    state.pending_rerun = true;
    let (text, count) = state.result_json(now, false).unwrap().unwrap();
    let full: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(count, 2);
    assert_eq!(full["context"]["expression"], "name:report");
    assert_eq!(full["context"]["mode"], "Exact");
    assert_eq!(
        full["root"]["text"],
        state.last_query.as_ref().unwrap().2.to_str().unwrap()
    );
    assert_eq!(full["context"]["source"], "Indexed snapshot");
    assert_eq!(full["context"]["received_rows"], 3);
    assert_eq!(full["context"]["visible_rows"], 2);
    assert_eq!(full["context"]["visible_selected_rows"], 1);
    assert_eq!(full["context"]["draft_differs"], true);
    assert_eq!(full["context"]["pending_rerun"], true);
    assert_eq!(full["partial"], false);
    assert_eq!(full["rows"][0]["location"]["kind"], "native");
    assert!(full["rows"][0]["modified_utc"].is_null());
    assert_eq!(full["rows"][0]["score"], 42);
    assert_eq!(full["rows"][0]["explanation"][0]["detail"], "пример\n");
    assert_eq!(
        full["rows"][0]["explanation"][0]["matched_character_ranges"],
        serde_json::json!([[0, 3]])
    );
    assert_eq!(full["rows"][1]["location"]["kind"], "archive_member");
    assert_eq!(full["rows"][1]["location"]["member"]["text"], "report-a");
    assert_eq!(full["rows"][1]["location"]["index"], 7);
    assert!(full["rows"][1]["native_id"].is_null());
    let (text, count) = state.result_json(now, true).unwrap().unwrap();
    let selected: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(count, 1);
    assert_eq!(selected["scope"], "visible_selected_results");
    assert_eq!(selected["rows"][0]["location"], full["rows"][1]["location"]);
}
#[test]
fn search_json_marks_partial_and_can_export_an_empty_completed_query() {
    let now = SystemTime::now();
    let mut state = json_state();
    state.results.clear();
    state.matched = 0;
    let (text, count) = state.prepare_result_copy(now, false).unwrap();
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(count, 0);
    assert_eq!(report["rows"], serde_json::json!([]));
    assert_eq!(report["partial"], false);
    assert!(state.result_json(now, true).unwrap().is_none());
    for reason in 0..4 {
        state.result_complete = reason != 0;
        state.truncated = reason == 1;
        state.content_skipped = usize::from(reason == 2);
        state.archive_error = (reason == 3).then(|| "bad archive".into());
        let (text, _) = state.result_json(now, false).unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["partial"],
            true
        );
    }
    state.last_query = None;
    assert!(state.prepare_result_copy(now, false).is_none());
    assert!(
        state
            .result_action_error
            .as_ref()
            .unwrap()
            .contains("applied query")
    );
}
#[test]
fn search_json_is_bounded_and_reports_refusal_before_a_clipboard_value_exists() {
    let now = SystemTime::now();
    let mut state = json_state();
    state.results[0].entry.name = "x".repeat(1024 * 1024 + 1);
    assert!(state.prepare_result_copy(now, false).is_none());
    assert!(
        state
            .result_action_error
            .as_ref()
            .unwrap()
            .contains("1 MiB")
    );
    state.results[0].entry.name = "=formula,\"\n\t".into();
    let (text, count) = state.prepare_result_copy(now, false).unwrap();
    assert_eq!(count, 3);
    assert!(state.result_action_error.is_none());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["rows"][0]["display_name"],
        state.results[0].entry.name
    );
    state.results[2].location = crate::search::SearchLocation::ArchiveMember {
        archive: "/x/bundle.zip".into(),
        member: "../escape".into(),
        index: 7,
    };
    assert!(state.prepare_result_copy(now, false).is_none());
    assert!(
        state
            .result_action_error
            .as_ref()
            .unwrap()
            .contains("traversal")
    );
    state.results[2].location = crate::search::SearchLocation::Native("/different".into());
    assert!(state.prepare_result_copy(now, false).is_none());
}
#[cfg(unix)]
#[test]
fn search_json_preserves_non_utf8_path_bytes_without_replacement_and_ties_by_location() {
    use std::os::unix::ffi::OsStringExt;
    let mut state = json_state();
    let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/x/raw\xff\n.txt".to_vec()));
    state.results[0].entry.path = raw.clone();
    state.results[0].location = crate::search::SearchLocation::Native(raw);
    state.results[0].identity = crate::search::FileIdentity::Native {
        volume: u64::MAX,
        file: u64::MAX - 1,
    };
    state.results[1].identity = state.results[0].identity.clone();
    let (text, _) = state
        .result_json(SystemTime::now(), false)
        .unwrap()
        .unwrap();
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        report["rows"][0]["native_id"]["device_id"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        report["rows"][0]["native_id"]["file_id"].as_u64(),
        Some(u64::MAX - 1)
    );
    assert_eq!(
        report["rows"][0]["location"]["path"],
        serde_json::json!({"encoding":"unix_bytes_hex","hex":"2f782f726177ff0a2e747874"})
    );
    state.select_result(state.results[0].location.clone(), false, SystemTime::now());
    state.select_result(state.results[1].location.clone(), false, SystemTime::now());
    let (text, count) = state.result_json(SystemTime::now(), true).unwrap().unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["exported_rows"],
        2
    );
}

#[test]
fn exclusion_editor_lines_become_trimmed_paths() {
    let state = FindState {
        index_exclusions: "  /a/node_modules \n\n/b\n   ".to_string(),
        ..FindState::default()
    };
    assert_eq!(
        state.exclusion_paths(),
        [PathBuf::from("/a/node_modules"), PathBuf::from("/b")]
    );
}

#[test]
fn replay_and_root_change_reset_the_index_rerun() {
    let mut state = FindState {
        history_open: true,
        index_rerun_after_build: true,
        ..FindState::default()
    };
    state.replay(
        crate::search::QueryHistoryEntry {
            expression: "report".to_string(),
            mode: crate::query::MatchMode::default(),
            root: PathBuf::from("/docs"),
            result_count: 3,
            scanned: 10,
            duration_ms: 5,
            last_run: 1,
        },
        "/docs/tmp".to_string(),
    );
    assert_eq!(state.expression, "report");
    assert_eq!(state.root, PathBuf::from("/docs"));
    assert_eq!(state.index_exclusions, "/docs/tmp");
    assert!(!state.history_open && !state.index_rerun_after_build);

    state.index_rerun_after_build = true;
    state.change_root(PathBuf::from("/other"), String::new());
    assert_eq!(state.root, PathBuf::from("/other"));
    assert!(!state.index_rerun_after_build);
}

#[test]
fn failed_restart_retires_the_previous_search_run() {
    let root = crate::testutil::TempDir::new();
    let engine = crate::search::SearchEngine::default();
    let run = engine
        .start(
            root.path().to_path_buf(),
            crate::query::Query::default(),
            10,
            std::sync::Arc::new(|| {}),
        )
        .unwrap();
    let mut state = FindState {
        run: Some(run),
        pending_rerun: true,
        ..FindState::default()
    };

    fail_find_search_start(&mut state, "provider refused".to_string());

    assert!(!state.searching());
    assert!(!state.pending_rerun);
    assert_eq!(state.error.as_deref(), Some("provider refused"));
    engine.cancel();
}

#[test]
fn removing_a_predicate_rewrites_the_expression() {
    let mut state = FindState::default();
    assert!(!state.remove_predicate(0), "an empty query has no chips");
    state.expression = "name:report path:docs".to_string();
    assert!(!state.remove_predicate(2));
    assert!(state.remove_predicate(0));
    let query = state.build_query().unwrap();
    assert_eq!(
        query.predicates,
        [crate::query::Predicate::PathContains("docs".to_string())]
    );
}

fn hit(name: &str, modified: Option<SystemTime>) -> crate::search::SearchHit {
    let path = PathBuf::from(format!("/x/{name}"));
    crate::search::SearchHit {
        entry: crate::panel::FileEntry {
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
            sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
            path: path.clone(),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size: 1,
            extension: String::new(),
            modified,
            modified_str: String::new(),
            size_str: String::new(),
        },
        identity: crate::search::FileIdentity::Native { volume: 1, file: 1 },
        accessed: None,
        explanation: crate::search::MatchExplanation::default(),
        location: crate::search::SearchLocation::Native(path),
    }
}

#[test]
fn highlighted_background_tab_target_uses_visible_order_and_archive_container() {
    let now = SystemTime::now();
    let mut archived = hit("member.txt", Some(now));
    let archive = PathBuf::from("/bundle.zip");
    archived.location = crate::search::SearchLocation::ArchiveMember {
        archive: archive.clone(),
        member: PathBuf::from("member.txt"),
        index: 0,
    };
    let mut state = FindState {
        results: vec![hit("hidden", None), archived],
        ..FindState::default()
    };
    assert!(state.highlighted_result_target(now).is_none());
    state.hide_result(state.results[0].location.clone());
    for pivot in crate::search::TimePivot::ALL {
        state.time_pivot = pivot;
        state.highlighted_result = Some(0);
        assert_eq!(state.highlighted_result_target(now), Some(archive.clone()));
    }
    state.highlighted_result = Some(1);
    assert!(state.highlighted_result_target(now).is_none());
    state.hide_result(state.results[1].location.clone());
    state.highlighted_result = Some(0);
    assert!(state.highlighted_result_target(now).is_none());
}

#[test]
fn context_selection_tracks_visible_locations_and_ignores_hidden_targets() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![
            hit("one.txt", None),
            hit("hard.txt", None),
            hit("hidden.txt", None),
        ],
        ..FindState::default()
    };
    let one = state.results[0].location.clone();
    let other = state.results[1].location.clone();
    let hidden = state.results[2].location.clone();
    state.select_all_visible_results();
    state.hide_result(hidden.clone());
    assert!(state.select_only_result(other.clone(), now));
    assert_eq!(
        state.selected_results,
        std::collections::HashSet::from([other.clone()])
    );
    assert_eq!(state.highlighted_result, Some(1));
    assert_eq!(state.result_selection_anchor, Some(other.clone()));
    assert!(!state.select_only_result(hidden, now));
    assert!(!state.select_only_result(
        crate::search::SearchLocation::Native(PathBuf::from("/missing")),
        now
    ));
    assert_eq!(
        state.selected_results,
        std::collections::HashSet::from([other])
    );
    state.results.swap(0, 1);
    assert!(state.select_only_result(one.clone(), now));
    assert_eq!(
        state.selected_results,
        std::collections::HashSet::from([one])
    );
    assert_eq!(state.highlighted_result, Some(1));
}

#[test]
fn find_relative_paths_bind_applied_root_and_preserve_members_view_and_selection() {
    let now = SystemTime::now();
    let mut state = FindState {
        root: "/draft/elsewhere".into(),
        last_query: Some(("".into(), crate::query::MatchMode::Exact, "/x".into())),
        result_copy_format: FindCopyFormat::RelativePaths,
        results: vec![
            hit("nested/файл.txt", None),
            hit("bundle.zip!/ignored", None),
            hit("hidden.txt", None),
        ],
        ..Default::default()
    };
    state.results[1].location = crate::search::SearchLocation::ArchiveMember {
        archive: "/x/packs/bundle.zip".into(),
        member: "deep/member.txt".into(),
        index: 7,
    };
    state
        .hidden_results
        .insert(state.results[2].location.clone());
    state
        .selected_results
        .insert(state.results[1].location.clone());
    state.result_sort = crate::search_order::Sort {
        field: crate::search_order::Field::Rank,
        descending: true,
    };
    assert_eq!(
        state.prepare_result_copy(now, false),
        Some((
            "packs/bundle.zip!/deep/member.txt\nnested/файл.txt".into(),
            2
        ))
    );
    assert_eq!(
        state.prepare_result_copy(now, true),
        Some(("packs/bundle.zip!/deep/member.txt".into(), 1))
    );
    assert_eq!(state.root, PathBuf::from("/draft/elsewhere"));
    assert_eq!(state.selected_results.len(), 1);
    state.results[0].location = crate::search::SearchLocation::Native("/x".into());
    assert_eq!(
        state.prepare_result_copy(now, false),
        Some(("packs/bundle.zip!/deep/member.txt\n.".into(), 2))
    );
    state.selected_results.clear();
    assert!(state.prepare_result_copy(now, true).is_none());
    state.results[0].location = crate::search::SearchLocation::Native("/x/line\nbreak.txt".into());
    assert_eq!(
        state.prepare_result_copy(now, false),
        Some((
            "packs/bundle.zip!/deep/member.txt\nline\\nbreak.txt".into(),
            2
        ))
    );
}
#[test]
fn find_relative_paths_refuse_unbound_outside_and_traversing_rows_atomically() {
    let now = SystemTime::now();
    let mut state = FindState {
        result_copy_format: FindCopyFormat::RelativePaths,
        results: vec![hit("good.txt", None), hit("second.txt", None)],
        ..Default::default()
    };
    assert!(state.prepare_result_copy(now, false).is_none());
    assert!(
        state
            .result_action_error
            .as_ref()
            .unwrap()
            .contains("applied search root")
    );
    state.last_query = Some(("".into(), crate::query::MatchMode::Exact, "/x".into()));
    for path in ["/xy/other", "/x/../outside"] {
        state.results[1].location = crate::search::SearchLocation::Native(path.into());
        assert!(state.prepare_result_copy(now, false).is_none());
        assert!(state.result_action_error.is_some());
    }
    state.results[1].location = crate::search::SearchLocation::ArchiveMember {
        archive: "/x/a.zip".into(),
        member: "../bad".into(),
        index: 0,
    };
    assert!(state.prepare_result_copy(now, false).is_none());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        state.results[1].location = crate::search::SearchLocation::Native(PathBuf::from(
            std::ffi::OsString::from_vec(b"/x/\xff".to_vec()),
        ));
        assert!(state.prepare_result_copy(now, false).is_none());
        assert!(
            state
                .result_action_error
                .as_ref()
                .unwrap()
                .contains("UTF-8")
        );
    }
    state.results[1].location = crate::search::SearchLocation::Native("/x/second.txt".into());
    assert_eq!(
        state.prepare_result_copy(now, false),
        Some(("good.txt\nsecond.txt".into(), 2))
    );
    assert!(state.result_action_error.is_none());
}

#[test]
fn find_parent_folders_deduplicate_real_targets_in_visible_order() {
    let now = SystemTime::now();
    let mut state = FindState {
        result_copy_format: FindCopyFormat::ParentFolders,
        results: vec![
            hit("first/a", None),
            hit("first/b", None),
            hit("second/c", None),
            hit("hidden/d", None),
            hit("bundle.zip!/one", None),
            hit("bundle.zip!/two", None),
            hit("folder", None),
        ],
        ..Default::default()
    };
    for index in [4, 5] {
        state.results[index].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/packs/bundle.zip"),
            member: PathBuf::from(index.to_string()),
            index,
        };
    }
    state.results[6].entry.is_dir = true;
    state.hide_result(state.results[3].location.clone());
    assert_eq!(
        state.visible_result_paths_clipboard(now),
        Some(("/x/first\n/x/second\n/packs\n/x".into(), 4))
    );
    state.selected_results.extend([
        state.results[1].location.clone(),
        state.results[4].location.clone(),
    ]);
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("/x/first\n/packs".into(), 2))
    );
    state.selected_results.clear();
    assert!(state.selected_result_paths_clipboard(now).is_none());
    state.results = vec![hit("root", None)];
    state.results[0].location = crate::search::SearchLocation::Native(PathBuf::from("/"));
    assert!(state.visible_result_paths_clipboard(now).is_none());
}

#[test]
fn find_names_without_extensions_preserve_members_dotfiles_order_and_selection() {
    let now = SystemTime::now();
    let mut state = FindState {
        result_copy_format: FindCopyFormat::NamesWithoutExtension,
        results: vec![
            hit(".env", None),
            hit("archive.tar.gz", None),
            hit("hidden.txt", None),
            hit("folder.with.dots", None),
            hit("bundle.zip!/name.txt", None),
            hit("name.md", None),
            hit("line\nname.txt", None),
            hit("ёж.文.txt", None),
        ],
        ..Default::default()
    };
    state.results[3].entry.is_dir = true;
    state.results[4].location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/bundles/container.zip"),
        member: PathBuf::from("name.txt"),
        index: 0,
    };
    state.hide_result(state.results[2].location.clone());
    assert_eq!(
        state.visible_result_paths_clipboard(now),
        Some((
            ".env\narchive.tar\nfolder.with\nname\nname\nline\\nname\nёж.文".into(),
            7
        ))
    );
    state.selected_results.extend([
        state.results[4].location.clone(),
        state.results[5].location.clone(),
    ]);
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("name\nname".into(), 2))
    );
    state.selected_results.clear();
    assert!(state.selected_result_paths_clipboard(now).is_none());
    state.results.clear();
    assert!(state.visible_result_paths_clipboard(now).is_none());
}

#[test]
fn find_file_urls_preserve_visible_selection_order_and_real_archive_containers() {
    let now = SystemTime::now();
    let mut state = FindState {
        result_copy_format: FindCopyFormat::FileUrls,
        results: vec![
            hit("a b#?%.txt", None),
            hit("hidden.txt", None),
            hit("bundle.zip!/one.txt", None),
            hit("bundle.zip!/two.txt", None),
        ],
        ..Default::default()
    };
    for (index, member) in [(2, "one.txt"), (3, "two.txt")] {
        state.results[index].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/a b.zip"),
            member: PathBuf::from(member),
            index,
        };
    }
    let hidden = state.results[1].location.clone();
    state.hide_result(hidden);
    assert_eq!(
        state.visible_result_paths_clipboard(now),
        Some((
            "file:///x/a%20b%23%3F%25.txt\nfile:///x/a%20b.zip\nfile:///x/a%20b.zip".into(),
            3
        ))
    );
    state
        .selected_results
        .insert(state.results[3].location.clone());
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("file:///x/a%20b.zip".into(), 1))
    );
    state.selected_results.clear();
    assert!(state.selected_result_paths_clipboard(now).is_none());
    state.results.clear();
    assert!(state.visible_result_paths_clipboard(now).is_none());
}

#[cfg(unix)]
#[test]
fn find_file_urls_preserve_raw_name_bytes_controls_and_relative_targets() {
    use std::os::unix::ffi::OsStringExt;
    let now = SystemTime::now();
    let mut raw = hit("raw", None);
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"/x/raw\xff\n\t.txt".to_vec()));
    raw.entry.path = path.clone();
    raw.location = crate::search::SearchLocation::Native(path);
    let mut state = FindState {
        result_copy_format: FindCopyFormat::FileUrls,
        results: vec![raw],
        ..Default::default()
    };
    assert_eq!(
        state.visible_result_paths_clipboard(now),
        Some(("file:///x/raw%FF%0A%09.txt".into(), 1))
    );
    let relative = PathBuf::from("relative file.txt");
    state.results[0].entry.path = relative.clone();
    state.results[0].location = crate::search::SearchLocation::Native(relative);
    let (text, count) = state.visible_result_paths_clipboard(now).unwrap();
    assert!(text.starts_with("file:///"));
    assert!(text.ends_with("/relative%20file.txt"));
    assert_eq!(count, 1);
}

#[test]
fn find_details_csv_copies_visible_or_selected_metadata_in_display_order() {
    let now = SystemTime::now();
    let mut state = FindState {
        result_copy_format: FindCopyFormat::DetailsCsv,
        results: vec![
            hit("same.txt", None),
            hit("hidden.txt", None),
            hit("bundle.zip!/same.txt", None),
        ],
        ..FindState::default()
    };
    state.results[2].entry.name = "same.txt".into();
    state.results[2].entry.name_lower = "same.txt".into();
    state.results[2].entry.sort_key = "same.txt".into();
    state.results[2].location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("same.txt"),
        index: 0,
    };
    state.hide_result(state.results[1].location.clone());
    let (text, count) = state.visible_result_paths_clipboard(now).unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        text,
        "Path,Name,Kind,Size (bytes),Modified\n/x/same.txt,same.txt,file,1,\n/x/bundle.zip!/same.txt,same.txt,file,1,\n"
    );
    state.select_result(state.results[2].location.clone(), false, now);
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some((
            "Path,Name,Kind,Size (bytes),Modified\n/x/bundle.zip!/same.txt,same.txt,file,1,\n"
                .into(),
            1
        ))
    );
    state.results.swap(0, 2);
    assert!(
        state
            .visible_result_paths_clipboard(now)
            .unwrap()
            .0
            .starts_with("Path,Name,Kind,Size (bytes),Modified\n/x/bundle.zip!/same.txt,")
    );
    state.clear_result_selection();
    assert!(state.selected_result_paths_clipboard(now).is_none());
}

#[test]
fn find_copy_formats_preserve_names_and_parse_as_literal_shell_arguments() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![
            hit("it's $(printf HACK)\nfile.txt", None),
            hit("hidden.txt", None),
            hit("bundle.zip!/a.txt", None),
        ],
        ..FindState::default()
    };
    state.results[2].location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/archive space.zip"),
        member: PathBuf::from("a.txt"),
        index: 0,
    };
    state.select_all_visible_results();
    state.hide_result(state.results[1].location.clone());
    state.result_copy_format = FindCopyFormat::Names;
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("it's $(printf HACK)\\nfile.txt\na.txt".into(), 2))
    );
    state.result_copy_format = FindCopyFormat::ShellArguments;
    let (text, count) = state.selected_result_paths_clipboard(now).unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        text,
        "'/x/it'\\''s $(printf HACK)\nfile.txt' '/x/archive space.zip'"
    );
    #[cfg(unix)]
    {
        let script = format!("set -- {text}; printf '%s\\0' \"$@\"");
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            output.stdout,
            b"/x/it's $(printf HACK)\nfile.txt\0/x/archive space.zip\0"
        );
    }
    state.result_copy_format = FindCopyFormat::Paths;
    assert!(
        state
            .selected_result_paths_clipboard(now)
            .unwrap()
            .0
            .ends_with("/x/bundle.zip!/a.txt")
    );
    state.clear_result_selection();
    assert!(state.selected_result_paths_clipboard(now).is_none());
    assert_eq!(state.visible_result_paths_clipboard(now).unwrap().1, 2);
}

#[test]
fn remembered_find_selection_is_root_bound_and_restores_only_current_visible_locations() {
    let now = SystemTime::now();
    let mut state = FindState {
        root: PathBuf::from("/x"),
        results: vec![
            hit("a.txt", None),
            hit("hard.txt", None),
            hit("hidden.txt", None),
            hit("bundle.zip!/a.txt", None),
        ],
        ..FindState::default()
    };
    state.last_query = Some((String::new(), state.mode, state.root.clone()));
    let a = state.results[0].location.clone();
    let hard = state.results[1].location.clone();
    let archive = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 0,
    };
    state.results[3].location = archive.clone();
    let mut duplicate = state.results[3].clone();
    duplicate.location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 1,
    };
    state.results.push(duplicate);
    assert!(!state.remember_result_selection());
    assert!(!state.restore_result_selection());
    state.select_result(a.clone(), false, now);
    state.select_result(archive.clone(), false, now);
    assert!(state.remember_result_selection());
    assert_eq!(state.restorable_result_selection_count(), 2);
    state.hide_result(archive.clone());
    state.clear_result_selection();
    state.select_result(hard.clone(), false, now);
    assert!(state.restore_result_selection());
    assert_eq!(
        state.selected_results,
        std::collections::HashSet::from([a.clone()])
    );
    assert!(state.result_selection_anchor.is_none());
    state.restore_hidden_results();
    assert_eq!(state.restorable_result_selection_count(), 2);
    assert!(state.restore_result_selection());
    assert_eq!(state.selected_results.len(), 2);
    // A refined query no longer returns the native remembered path.
    state.results.retain(|hit| hit.location != a);
    state.clear_result_selection();
    assert!(state.restore_result_selection());
    assert_eq!(
        state.selected_results,
        std::collections::HashSet::from([archive.clone()])
    );
    state.results.push(hit("a.txt", None));
    assert_eq!(
        state.selected_results.len(),
        1,
        "later hits are not auto-selected"
    );
    assert!(state.restore_result_selection());
    assert_eq!(state.selected_results.len(), 2);
    state.root = PathBuf::from("/other");
    assert_eq!(state.restorable_result_selection_count(), 0);
    let before = state.selected_results.clone();
    assert!(!state.restore_result_selection());
    assert_eq!(state.selected_results, before);
    // Remembering again replaces the one snapshot, including its root.
    state.clear_result_selection();
    state.select_result(hard, false, now);
    assert!(
        !state.remember_result_selection(),
        "old results cannot be rebound before the new-root search starts"
    );
    state.last_query = Some((String::new(), state.mode, state.root.clone()));
    assert!(state.remember_result_selection());
    state.root = PathBuf::from("/x");
    assert_eq!(state.restorable_result_selection_count(), 0);
    state.clear_result_selection();
    assert!(!state.remember_result_selection());
    assert_eq!(
        state.remembered_result_selection.as_ref().unwrap().root,
        PathBuf::from("/other")
    );
}

#[test]
fn bulk_visible_result_selection_excludes_hidden_and_tracks_streamed_locations() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![
            hit("a.txt", None),
            hit("hard.txt", None),
            hit("hidden.txt", None),
            hit("bundle.zip!/a.txt", None),
        ],
        ..FindState::default()
    };
    let hidden = state.results[2].location.clone();
    state.hide_result(hidden.clone());
    state.results[3].location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 0,
    };
    let mut duplicate = state.results[3].clone();
    duplicate.location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 1,
    };
    state.results.push(duplicate);
    state.select_result(state.results[0].location.clone(), false, now);
    state.highlighted_result = Some(1);
    state.invert_visible_result_selection();
    assert_eq!(state.visible_selected_result_count(), 3);
    assert!(!state.selected_results.contains(&state.results[0].location));
    assert!(!state.selected_results.contains(&hidden));
    assert!(state.result_selection_anchor.is_none());
    assert_eq!(state.highlighted_result, Some(1));
    state.invert_visible_result_selection();
    assert_eq!(state.visible_selected_result_count(), 1);
    state.select_all_visible_results();
    assert_eq!(state.visible_selected_result_count(), 4);
    assert_eq!(state.selected_results.len(), 4);
    assert_eq!(state.selected_result_paths_clipboard(now).unwrap().1, 4);
    state.results.swap(0, 4);
    state.results.push(hit("streamed.txt", None));
    assert_eq!(
        state.visible_selected_result_count(),
        4,
        "selection is a snapshot, not a sticky mode"
    );
    state.invert_visible_result_selection();
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("/x/streamed.txt".into(), 1))
    );
    state.restore_hidden_results();
    assert!(!state.selected_results.contains(&hidden));
    state.select_all_visible_results();
    assert_eq!(state.visible_selected_result_count(), 6);
    for hit in &state.results {
        state.hidden_results.insert(hit.location.clone());
    }
    state.select_all_visible_results();
    assert!(state.selected_results.is_empty());
    state.invert_visible_result_selection();
    assert!(state.selected_results.is_empty());
    state.results.clear();
    state.invert_visible_result_selection();
    assert!(state.selected_results.is_empty());
}

#[test]
fn selected_search_tab_targets_preserve_visible_order_and_archive_containers() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![
            hit("a.txt", None),
            hit("bundle.zip!/a.txt", None),
            hit("bundle.zip!/b.txt", None),
            hit("hidden.txt", None),
        ],
        ..FindState::default()
    };
    let archive = PathBuf::from("/x/bundle.zip");
    for (index, name) in [(1, "a.txt"), (2, "b.txt")] {
        state.results[index].location = crate::search::SearchLocation::ArchiveMember {
            archive: archive.clone(),
            member: PathBuf::from(name),
            index,
        };
    }
    state
        .selected_results
        .extend(state.results.iter().map(|hit| hit.location.clone()));
    state.hide_result(state.results[3].location.clone());
    assert_eq!(
        state.selected_result_tab_targets(now),
        [PathBuf::from("/x/a.txt"), archive.clone(), archive.clone()]
    );
    state.results.swap(0, 2);
    assert_eq!(
        state.selected_result_tab_targets(now),
        [archive.clone(), archive, PathBuf::from("/x/a.txt")]
    );
    state.clear_result_selection();
    assert!(state.selected_result_tab_targets(now).is_empty());
}

#[test]
fn hide_selected_search_results_is_atomic_by_location_and_restorable() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![
            hit("a.txt", None),
            hit("hard.txt", None),
            hit("bundle.zip!/a.txt", None),
        ],
        matched: 3,
        scanned: 5,
        ..FindState::default()
    };
    let a = state.results[0].location.clone();
    let unselected_hard_link = state.results[1].location.clone();
    let archive = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 0,
    };
    state.results[2].location = archive.clone();
    let mut duplicate = state.results[2].clone();
    let duplicate_location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 1,
    };
    duplicate.location = duplicate_location.clone();
    state.results.push(duplicate);
    state.select_result(a.clone(), false, now);
    state.select_result(archive.clone(), false, now);
    state.explanation_open = Some(explain_key(&state.results[2]));
    state.highlighted_result = Some(2);
    state.highlight_scroll_pending = true;
    assert_eq!(state.hide_selected_results(), 2);
    assert_eq!(state.visible_result_order(now), [1, 3]);
    assert!(!state.hidden_results.contains(&unselected_hard_link));
    assert!(!state.hidden_results.contains(&duplicate_location));
    assert!(state.selected_results.is_empty());
    assert!(state.result_selection_anchor.is_none());
    assert!(state.explanation_open.is_none());
    assert!(state.highlighted_result.is_none());
    assert!(!state.highlight_scroll_pending);
    assert_eq!(
        (state.results.len(), state.matched, state.scanned),
        (4, 3, 5)
    );
    assert_eq!(state.hide_selected_results(), 0);
    state.results.swap(0, 3);
    state.results.push(hit("streamed.txt", None));
    assert_eq!(state.visible_result_count(), 3);
    state.restore_hidden_results();
    assert_eq!(state.visible_result_count(), 5);
    assert_eq!(state.visible_selected_result_count(), 0);
    assert_eq!(state.hide_selected_results(), 0);
}

#[test]
fn search_result_selection_survives_streaming_sorting_ranges_and_hiding() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![
            hit("a.txt", None),
            hit("hidden.txt", None),
            hit("b.txt", Some(now)),
            hit("bundle.zip!/a.txt", None),
        ],
        ..FindState::default()
    };
    let a = state.results[0].location.clone();
    let b = state.results[2].location.clone();
    let hidden = state.results[1].location.clone();
    let archived = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 0,
    };
    state.results[3].location = archived.clone();
    let mut duplicate = state.results[3].clone();
    let duplicate_location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 1,
    };
    duplicate.location = duplicate_location.clone();
    state.results.push(duplicate);
    state.hidden_results.insert(hidden.clone());
    state.select_result(a.clone(), false, now);
    state.select_result(b.clone(), true, now);
    assert_eq!(state.visible_selected_result_count(), 2);
    assert!(!state.selected_results.contains(&hidden));
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("/x/a.txt\n/x/b.txt".into(), 2))
    );
    state.results.swap(0, 2);
    state.results.push(hit("streamed.txt", None));
    assert_eq!(
        state.selected_result_paths_clipboard(now),
        Some(("/x/b.txt\n/x/a.txt".into(), 2))
    );
    state.time_pivot = crate::search::TimePivot::Modified;
    state.select_result(archived.clone(), true, now);
    assert_eq!(state.visible_selected_result_count(), 3);
    assert!(!state.selected_results.contains(&duplicate_location));
    state.hide_result(a.clone());
    assert!(!state.selected_results.contains(&a));
    assert!(state.result_selection_anchor.is_none());
    state.restore_hidden_results();
    assert_eq!(state.visible_selected_result_count(), 2);
    state.select_result(duplicate_location.clone(), false, now);
    assert_eq!(state.visible_selected_result_count(), 3);
    let (text, count) = state.selected_result_paths_clipboard(now).unwrap();
    assert_eq!(count, 3);
    assert_eq!(text.matches("/x/bundle.zip!/a.txt").count(), 2);
    state.select_result(b.clone(), false, now);
    assert!(!state.selected_results.contains(&b));
    state.clear_result_selection();
    assert!(state.selected_result_paths_clipboard(now).is_none());
    assert!(state.result_selection_anchor.is_none());
    state.select_result(
        crate::search::SearchLocation::Native(PathBuf::from("/missing")),
        true,
        now,
    );
    assert!(state.selected_results.is_empty());
}

#[test]
fn copied_search_paths_follow_visible_order_preserve_members_and_escape_controls() {
    let now = SystemTime::now();
    let mut state = FindState::default();
    assert!(state.visible_result_paths_clipboard(now).is_none());
    state.results = vec![
        hit("one\nline\t.txt", None),
        hit("hidden.txt", None),
        hit("bundle.zip!/a.txt", None),
        hit("hard.txt", None),
    ];
    state.results[2].location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/x/bundle.zip"),
        member: PathBuf::from("a.txt"),
        index: 0,
    };
    state
        .hidden_results
        .insert(state.results[1].location.clone());
    let (text, count) = state.visible_result_paths_clipboard(now).unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        text,
        "/x/one\\nline\\t.txt\n/x/bundle.zip!/a.txt\n/x/hard.txt"
    );
    assert_eq!(text.lines().count(), count);
    assert_eq!(state.results.len(), 4);
    state.results.swap(0, 3);
    assert_eq!(
        state.visible_result_paths_clipboard(now).unwrap().0,
        "/x/hard.txt\n/x/bundle.zip!/a.txt\n/x/one\\nline\\t.txt"
    );
    state.time_pivot = crate::search::TimePivot::Modified;
    state.results[3].entry.modified = Some(now);
    state.results[0].entry.modified = now.checked_sub(std::time::Duration::from_secs(400 * 86400));
    assert_eq!(
        state.visible_result_paths_clipboard(now).unwrap().0,
        "/x/one\\nline\\t.txt\n/x/hard.txt\n/x/bundle.zip!/a.txt"
    );
    for hit in &state.results {
        state.hidden_results.insert(hit.location.clone());
    }
    assert!(state.visible_result_paths_clipboard(now).is_none());
    state.restore_hidden_results();
    assert_eq!(state.visible_result_paths_clipboard(now).unwrap().1, 4);
}

#[test]
fn hidden_search_results_follow_locations_across_sorting_streaming_and_restore() {
    let now = SystemTime::now();
    let mut state = FindState {
        results: vec![hit("a", None), hit("b", Some(now)), hit("c", None)],
        matched: 3,
        highlighted_result: Some(1),
        highlight_scroll_pending: true,
        ..FindState::default()
    };
    let hidden = state.results[1].location.clone();
    state.explanation_open = Some(explain_key(&state.results[1]));
    state.hide_result(hidden.clone());
    assert_eq!(state.visible_result_order(now), [0, 2]);
    assert_eq!(state.visible_result_count(), 2);
    assert_eq!(state.results.len(), 3);
    assert_eq!(state.matched, 3);
    assert!(state.explanation_open.is_none());
    assert!(state.highlighted_result.is_none() && !state.highlight_scroll_pending);
    // Hard links share the helper's native identity; only the chosen path is hidden.
    assert_eq!(state.results[0].identity, state.results[1].identity);
    state.results.reverse();
    state.results.push(hit("new", Some(now)));
    for pivot in crate::search::TimePivot::ALL {
        state.time_pivot = pivot;
        let visible = state.visible_result_order(now);
        assert_eq!(visible.len(), 3);
        assert!(
            visible
                .iter()
                .all(|&index| state.results[index].location != hidden)
        );
        let selected = move_result_highlight(None, state.visible_result_count(), true).unwrap();
        assert_ne!(state.results[visible[selected]].location, hidden);
    }
    state.restore_hidden_results();
    assert_eq!(
        state.visible_result_order(now),
        pivot_order(&state.results, state.time_pivot, now)
    );
    assert_eq!(state.visible_result_count(), 4);
    assert!(state.hidden_results.is_empty());
    for location in state
        .results
        .iter()
        .map(|hit| hit.location.clone())
        .collect::<Vec<_>>()
    {
        state.hide_result(location);
    }
    assert_eq!(state.visible_result_count(), 0);
    assert!(state.visible_result_order(now).is_empty());
    state.restore_hidden_results();
    assert_eq!(state.visible_result_count(), 4);
}

#[test]
fn hidden_search_results_distinguish_duplicate_archive_member_positions() {
    let mut first = hit("same", None);
    first.location = crate::search::SearchLocation::ArchiveMember {
        archive: PathBuf::from("/bundle.zip"),
        member: PathBuf::from("same"),
        index: 0,
    };
    let mut second = first.clone();
    if let crate::search::SearchLocation::ArchiveMember { index, .. } = &mut second.location {
        *index = 1;
    }
    let mut state = FindState {
        results: vec![first, second],
        ..FindState::default()
    };
    state.hide_result(state.results[0].location.clone());
    assert_eq!(state.visible_result_order(SystemTime::now()), [1]);
    state.restore_hidden_results();
    assert_eq!(state.visible_result_count(), 2);
}

#[test]
fn hard_links_keep_separate_explanations() {
    // Both helper hits share one native identity, like two hard links.
    let results = [hit("a", None), hit("b", None)];
    assert_eq!(results[0].identity, results[1].identity);
    let key = explain_key(&results[1]);
    let shown = explained_hit(&results, &key).expect("hit is listed");
    assert_eq!(shown.entry.name, "b");
    assert_ne!(explain_key(&results[0]), key);
}

#[test]
fn result_sort_preserves_locations_hidden_selection_and_copied_view_context() {
    let now = SystemTime::now();
    let mut state = json_state();
    state.results[0].entry.size = 5;
    state.results[1].entry.size = 1;
    state.results[2].entry.size = 9;
    let original = state
        .results
        .iter()
        .map(|h| h.location.clone())
        .collect::<Vec<_>>();
    state.highlighted_result = Some(0);
    state
        .selected_results
        .insert(state.results[1].location.clone());
    state.set_result_sort(
        crate::search_order::Sort {
            field: crate::search_order::Field::Size,
            descending: false,
        },
        now,
    );
    assert_eq!(state.visible_result_order(now), [1, 0, 2]);
    assert_eq!(state.highlighted_result, Some(1));
    state
        .hidden_results
        .insert(state.results[1].location.clone());
    // The currently highlighted source row is now visible at index zero.
    state.highlighted_result = Some(0);
    let selected = state.selected_results.clone();
    state.set_result_sort(
        crate::search_order::Sort {
            field: crate::search_order::Field::Size,
            descending: true,
        },
        now,
    );
    assert_eq!(state.visible_result_order(now), [2, 0]);
    assert_eq!(state.highlighted_result, Some(1));
    assert_eq!(state.selected_results, selected);
    let (text, count) = state.result_json(now, false).unwrap().unwrap();
    let r: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        r["context"]["result_sort"],
        serde_json::json!({"field":"size","descending":true})
    );
    assert_eq!(r["rows"][0]["display_name"], state.results[2].entry.name);
    assert!(state.result_json(now, true).unwrap().is_none());
    state.results[0].entry.created = Some(now - std::time::Duration::from_secs(400 * 86400));
    state.set_time_pivot(crate::search::TimePivot::Created, now);
    assert_eq!(state.visible_result_order(now), [0, 2]);
    assert_eq!(state.highlighted_result, Some(0));
    assert_eq!(
        state
            .results
            .iter()
            .map(|h| h.location.clone())
            .collect::<Vec<_>>(),
        original
    );
}

#[test]
fn new_time_pivots_preserve_location_highlight_hidden_selection_and_report_order() {
    let now = SystemTime::now();
    let old = now - std::time::Duration::from_secs(400 * 86400);
    let mut state = json_state();
    state.results[0].entry.created = Some(old);
    state.results[0].entry.changed = Some(now);
    state.results[1].entry.created = Some(now);
    state.results[1].entry.changed = Some(old);
    state.results[2].entry.created = Some(now);
    state.results[2].entry.changed = Some(now);
    state.highlighted_result = Some(0);
    state
        .selected_results
        .insert(state.results[1].location.clone());
    state.set_time_pivot(crate::search::TimePivot::Created, now);
    assert_eq!(state.visible_result_order(now), [1, 0, 2]);
    assert_eq!(state.highlighted_result, Some(1));
    assert!(state.highlight_scroll_pending);
    state.set_time_pivot(crate::search::TimePivot::Changed, now);
    assert_eq!(state.visible_result_order(now), [0, 1, 2]);
    assert_eq!(state.highlighted_result, Some(0));
    let selected = state.selected_results.clone();
    state
        .hidden_results
        .insert(state.results[1].location.clone());
    state.set_time_pivot(crate::search::TimePivot::Created, now);
    assert_eq!(state.visible_result_order(now), [0, 2]);
    assert_eq!(state.selected_results, selected);
    assert_eq!(state.highlighted_result, Some(0));
    let (text, count) = state.result_json(now, false).unwrap().unwrap();
    let r: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(count, 2);
    assert_eq!(r["context"]["pivot"], "Created");
    assert_eq!(r["rows"][0]["display_name"], state.results[0].entry.name);
    state.set_time_pivot(crate::search::TimePivot::None, now);
    assert_eq!(state.highlighted_result, Some(0));
}

#[test]
fn pivot_groups_results_by_bucket_and_keeps_rank_within_one() {
    let now = SystemTime::now();
    let old = now - Duration::from_secs(400 * 86_400);
    let results = [
        hit("old-a", Some(old)),
        hit("new-a", Some(now)),
        hit("unknown", None),
        hit("old-b", Some(old)),
        hit("new-b", Some(now)),
    ];
    assert_eq!(
        pivot_order(&results, crate::search::TimePivot::None, now),
        [0, 1, 2, 3, 4]
    );
    let order = pivot_order(&results, crate::search::TimePivot::Modified, now);
    let names: Vec<_> = order
        .iter()
        .map(|&index| results[index].entry.name.as_str())
        .collect();
    let bucket = |name: &str| {
        let index = results.iter().position(|h| h.entry.name == name).unwrap();
        crate::search::time_bucket(&results[index], crate::search::TimePivot::Modified, now)
    };
    // Sorted by bucket, original rank breaking ties.
    assert!(
        names
            .windows(2)
            .all(|pair| bucket(pair[0]) <= bucket(pair[1]))
    );
    let new_a = names.iter().position(|n| *n == "new-a").unwrap();
    let new_b = names.iter().position(|n| *n == "new-b").unwrap();
    let old_a = names.iter().position(|n| *n == "old-a").unwrap();
    let old_b = names.iter().position(|n| *n == "old-b").unwrap();
    assert_eq!(new_b, new_a + 1);
    assert_eq!(old_b, old_a + 1);
}

#[test]
fn result_rows_are_allocated_at_the_show_rows_stride() {
    // Header line (or 12px gap) + 22px row fits the pivot height; a
    // 4px gap + 22px row is exactly the plain height.
    assert!(result_row_height(true) >= 12.0 + 22.0);
    assert_eq!(result_row_height(false), 4.0 + 22.0);
}

#[test]
fn result_highlight_starts_at_an_edge_and_stays_in_bounds() {
    assert_eq!(move_result_highlight(None, 0, true), None);
    assert_eq!(move_result_highlight(None, 3, true), Some(0));
    assert_eq!(move_result_highlight(None, 3, false), Some(2));
    assert_eq!(move_result_highlight(Some(2), 3, true), Some(2));
    assert_eq!(move_result_highlight(Some(0), 3, false), Some(0));
    assert_eq!(move_result_highlight(Some(1), 3, true), Some(2));
}

#[test]
fn vanished_search_root_is_an_error_not_an_empty_result() {
    let temp = crate::testutil::TempDir::new();
    assert_eq!(search_root_error(temp.path()), None);
    let gone = temp.path().join("gone");
    let error = search_root_error(&gone).unwrap();
    assert!(error.starts_with("Search folder not found: "), "{error}");
    assert!(error.contains("gone"), "{error}");
    let file = temp.path().join("file.txt");
    std::fs::write(&file, b"x").unwrap();
    assert!(
        search_root_error(&file)
            .unwrap()
            .contains("is not a folder")
    );
}

#[test]
fn index_build_refusal_reports_the_index_reason() {
    assert_eq!(
        index_build_refusal(true, Some("deferred under pressure".into())),
        "deferred under pressure"
    );
    assert!(index_build_refusal(true, None).contains("did not start"));
    assert_eq!(
        index_build_refusal(false, Some("ignored".into())),
        "Enable the content index before rebuilding"
    );
}

#[test]
fn index_skipped_summary_lists_only_nonzero_counts() {
    assert_eq!(index_skipped_summary(0, 0, 0, 0), None);
    assert_eq!(
        index_skipped_summary(2, 0, 0, 1).as_deref(),
        Some("Skipped: 2 binary, 1 over the index size limit")
    );
}

#[test]
fn history_rows_name_mode_and_folder() {
    let entry = crate::search::QueryHistoryEntry {
        expression: String::new(),
        mode: crate::query::MatchMode::Fuzzy,
        root: PathBuf::from("/home/me/projects"),
        result_count: 3,
        scanned: 10,
        duration_ms: 7,
        last_run: 0,
    };
    assert_eq!(
        history_row_label(&entry),
        "All files  |  Fuzzy  |  in projects  |  3 results  |  7 ms"
    );
}

#[test]
fn failed_smart_folder_save_rolls_back_the_store() {
    let definition = |name: &str| crate::smart_folder::Definition {
        name: name.to_string(),
        root: PathBuf::from("/x"),
        query: crate::query::Query {
            predicates: vec![crate::query::Predicate::NameContains("a".into())],
            mode: crate::query::MatchMode::Exact,
        },
    };
    let mut store = crate::smart_folder::SmartFolders::default();
    assert!(add_smart_folder(&mut store, definition("kept"), |_| true));
    let before = store.clone();
    assert!(!add_smart_folder(&mut store, definition("lost"), |_| false));
    assert_eq!(store, before);
}
