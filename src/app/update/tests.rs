use super::pane_tool_covers;
use super::{
    CloseGuardAction, FrameInputPolicy, ModalOwnershipSnapshot, NativeEffectStatus,
    any_modal_surface_open, close_guard_action, hidden_files_rejection_message,
    perform_clipboard_write, perform_open, type_ahead_label,
};
use super::{
    DragAnnouncementMemo, invalidate_tree_children, newest_undoable_toast, trash_undo_matches,
};
use super::{
    MenuFocusStep, fallback_menu_focus_step, fallback_menu_pos, fallback_menu_restores_focus,
};
use super::{
    average_file_size, opens_in_archive_viewer, resized_tree_width, unique_smart_folder_name,
};
use super::{
    clipped_label, command_shortcut, copied_paths_label, delete_outcome_toast, selection_headline,
    selection_hud_head, undo_receipt, undo_recorded_toast,
};
use super::{drag_path_rejection, finish_drop, sources_already_in};
use super::{dragged_split_ratio, key_bar_modifier_held, selection_hud_bottom, split_left_width};

#[test]
fn pane_viewport_keeps_user_width_when_child_requests_more_space() {
    for width in [340.0, 637.0, 900.0] {
        let ctx = egui::Context::default();
        for frame in 0..6 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 760.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let response = egui::Panel::left("bounded-test")
                    .resizable(true)
                    .default_size(width)
                    .min_size(237.0)
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        super::pane_viewport(ui, |child| {
                            child.set_min_width(4096.0);
                            child.label("Oversized intrinsic content");
                        });
                    });
                assert!(
                    (response.response.rect.width() - width).abs() <= 1.0,
                    "frame {frame}: {:?}",
                    response.response.rect
                );
                assert!(ui.available_width() >= 1280.0 - width - 1.0);
            });
            let _ = crate::testutil::take_egui_output(output);
        }
    }
}

#[test]
fn close_guard_prompts_then_waits_for_cancelled_operations_to_settle() {
    assert_eq!(close_guard_action(true, 2, false), CloseGuardAction::Prompt);
    assert_eq!(close_guard_action(false, 2, true), CloseGuardAction::Wait);
    assert_eq!(close_guard_action(true, 0, true), CloseGuardAction::Close);
    assert_eq!(close_guard_action(true, 0, false), CloseGuardAction::None);
}
use super::{drop_busy_reason, smart_folder_saved_toast};
use super::{pointer_over_toast, toast_live_region, toast_stack_offsets, toast_text};
use std::cell::RefCell;
use std::collections::VecDeque;

struct ScriptedClipboard {
    outcomes: RefCell<VecDeque<crate::ports::ClipboardOutcome>>,
    writes: RefCell<Vec<String>>,
}

impl crate::ports::ClipboardPort for ScriptedClipboard {
    fn write_text(&self, text: &str) -> crate::ports::ClipboardOutcome {
        self.writes.borrow_mut().push(text.to_string());
        self.outcomes
            .borrow_mut()
            .pop_front()
            .expect("scripted clipboard outcome")
    }
}

struct ScriptedOpener {
    outcomes: RefCell<VecDeque<crate::ports::OpenOutcome>>,
    requests: RefCell<Vec<crate::ports::OpenRequest>>,
}

impl crate::ports::OpenerPort for ScriptedOpener {
    fn open(&self, request: &crate::ports::OpenRequest) -> crate::ports::OpenOutcome {
        self.requests.borrow_mut().push(request.clone());
        self.outcomes
            .borrow_mut()
            .pop_front()
            .expect("scripted opener outcome")
    }
}

fn failure(message: &str) -> crate::ports::NativeFailure {
    crate::ports::NativeFailure {
        kind: crate::ports::NativeFailureKind::Denied,
        message: message.to_string(),
    }
}

fn frame_policy(
    containing_ui_enabled: bool,
    prior_modal_open: bool,
    current_modal_open: bool,
) -> FrameInputPolicy {
    FrameInputPolicy::resolve(
        containing_ui_enabled,
        ModalOwnershipSnapshot::capture(prior_modal_open),
        current_modal_open,
    )
}

#[test]
fn type_ahead_capsule_spells_out_a_miss() {
    assert_eq!(type_ahead_label("rea", false), "\u{00bb} rea");
    assert!(type_ahead_label("zzz", true).ends_with("no match"));
}

#[test]
fn only_the_pane_holding_an_open_tool_is_covered() {
    use crate::workspace::ActivePanel;
    let mut tools = crate::app::ui_state::PaneTools::default();
    tools.replace_on(ActivePanel::Right);
    assert!(
        !pane_tool_covers(&tools, ActivePanel::Right),
        "nothing open yet"
    );
    tools.diff = Some(crate::app::DiffState {
        intraline: Default::default(),
        source: None,
        options: Default::default(),
        paths: Default::default(),
        jump: Default::default(),
        find: Default::default(),
        context_view: Default::default(),
        navigation: Default::default(),
        name_a: "a".into(),
        name_b: "b".into(),
        lines: Vec::new(),
        message: None,
        keyboard_focused: false,
        scroll_offset: egui::Vec2::ZERO,
    });
    assert!(pane_tool_covers(&tools, ActivePanel::Right));
    assert!(!pane_tool_covers(&tools, ActivePanel::Left));
}

#[test]
fn switching_into_a_tool_covered_pane_is_refused() {
    use super::switch_into_covered_pane;
    use crate::command::Command;
    use crate::workspace::ActivePanel;
    let mut tools = crate::app::ui_state::PaneTools::default();
    tools.replace_on(ActivePanel::Right);
    tools.diff = Some(crate::app::DiffState {
        intraline: Default::default(),
        source: None,
        options: Default::default(),
        paths: Default::default(),
        jump: Default::default(),
        find: Default::default(),
        context_view: Default::default(),
        navigation: Default::default(),
        name_a: "a".into(),
        name_b: "b".into(),
        lines: Vec::new(),
        message: None,
        keyboard_focused: false,
        scroll_offset: egui::Vec2::ZERO,
    });
    assert!(switch_into_covered_pane(
        &tools,
        ActivePanel::Left,
        Command::SwitchPanel
    ));
    // Leaving the covered pane, or any other command, is fine.
    assert!(!switch_into_covered_pane(
        &tools,
        ActivePanel::Right,
        Command::SwitchPanel
    ));
    assert!(!switch_into_covered_pane(
        &tools,
        ActivePanel::Left,
        Command::CursorDown
    ));
}

#[test]
fn disabled_background_cannot_activate_a_panel() {
    let policy = frame_policy(true, false, false);
    assert!(!policy.allows_raw_input(false, true));
    assert!(!policy.allows_raw_input(true, false));
    assert!(policy.allows_raw_input(true, true));
}

#[test]
fn trapped_frame_never_targets_or_executes_a_drop_and_clears_stale_drag() {
    for policy in [
        frame_policy(true, false, true),
        frame_policy(true, true, false),
    ] {
        assert!(policy.trapped());
        assert!(!policy.allows_drop_target(true, true));
        assert!(!policy.allows_drop_execution(true));
        assert!(policy.clear_stale_drag());
    }
}

#[test]
fn untrapped_frame_preserves_drop_and_divider_input() {
    let policy = frame_policy(true, false, false);
    assert!(!policy.trapped());
    assert!(policy.allows_drop_target(true, true));
    assert!(policy.allows_drop_execution(true));
    assert!(policy.allows_divider_reset(true, true));
    assert!(!policy.clear_stale_drag());
}

#[test]
fn trapped_frame_blocks_divider_and_auxiliary_actions() {
    let policy = frame_policy(true, true, false);
    assert!(!policy.allows_divider_reset(true, true));
    assert!(!policy.background_enabled());
}

#[test]
fn only_the_frame_a_modal_closes_on_is_a_close_edge() {
    assert!(ModalOwnershipSnapshot::capture(true).closed_after(false));
    assert!(!ModalOwnershipSnapshot::capture(true).closed_after(true));
    assert!(!ModalOwnershipSnapshot::capture(false).closed_after(false));
    assert!(!ModalOwnershipSnapshot::capture(false).closed_after(true));
}

#[test]
fn prior_modal_ownership_survives_poll_and_render_transitions() {
    let prior_confirmation = any_modal_surface_open(true, false);
    let closed_confirmation = frame_policy(true, prior_confirmation, false);
    assert!(
        closed_confirmation.trapped(),
        "a confirmation closed by begin_frame still owns its close frame"
    );

    let idle = frame_policy(true, false, false);
    assert!(!idle.trapped());

    let newly_opened = frame_policy(true, false, true);
    assert!(newly_opened.trapped());
}

#[test]
fn confirmation_and_app_modals_both_activate_the_frame_trap() {
    for modal_is_open in [
        any_modal_surface_open(true, false),
        any_modal_surface_open(false, true),
    ] {
        let policy = frame_policy(true, false, modal_is_open);
        assert!(policy.trapped());
        assert!(!policy.allows_drop_execution(true));
    }
}

#[test]
fn a_running_transfer_alone_leaves_the_panes_interactive() {
    // The Activity strip reports transfers and safe state; neither is a
    // modal surface.
    assert!(!any_modal_surface_open(false, false));
    assert!(frame_policy(true, false, false).background_enabled());
}

fn version(path: &str, n: u64) -> crate::version_store::VersionRecord {
    crate::version_store::VersionRecord {
        operation_id: crate::operation::OperationId(format!("op-{n}")),
        key: crate::operation::IdempotencyKey(format!("key-{n}")),
        original: std::path::PathBuf::from(path),
        stored: std::path::PathBuf::from(format!("stored-{n}")),
        created_at_secs: n,
    }
}

fn trashed(
    path: &str,
    version: Option<crate::version_store::VersionRecord>,
) -> crate::workspace::DeleteItemResult {
    crate::workspace::DeleteItemResult {
        path: std::path::PathBuf::from(path),
        outcome: crate::ports::TrashItemOutcome::Trashed,
        version,
    }
}

#[test]
fn delete_undo_attaches_only_its_own_trash_record() {
    let older = crate::undo::Action::Trash {
        items: vec![crate::undo::TrashedItem {
            path: "/old".into(),
            version: version("/old", 1),
        }],
    };
    // This delete preserved no version, so it recorded no undo.
    assert!(!trash_undo_matches(&[trashed("/new", None)], &older));
    // A versioned delete matches the record it produced.
    let own = crate::undo::Action::Trash {
        items: vec![crate::undo::TrashedItem {
            path: "/new".into(),
            version: version("/new", 2),
        }],
    };
    assert!(trash_undo_matches(
        &[trashed("/new", Some(version("/new", 2)))],
        &own
    ));
    assert!(!trash_undo_matches(
        &[trashed("/new", Some(version("/new", 2)))],
        &older
    ));
}

#[test]
fn only_the_newest_undoable_toast_offers_undo() {
    use crate::toasts::{Toast, ToastKind, ToastQueue};
    let mut queue = ToastQueue::default();
    queue.push(Toast::new(
        "Moved 1 to Trash",
        ToastKind::Success,
        true,
        0.0,
    ));
    queue.push(Toast::new("Renamed a", ToastKind::Success, true, 1.0));
    queue.push(Toast::new("Copied path", ToastKind::Info, false, 2.0));
    let newest = queue.active()[1].id();
    assert_eq!(newest_undoable_toast(queue.active()), Some(newest));
    assert_eq!(newest_undoable_toast(&[]), None);
}

#[test]
fn clipboard_feedback_reports_success_only_after_committed_outcome() {
    let port = ScriptedClipboard {
        outcomes: RefCell::new(
            [
                crate::ports::ClipboardOutcome::Failed(failure("denied")),
                crate::ports::ClipboardOutcome::Submitted,
                crate::ports::ClipboardOutcome::Committed,
            ]
            .into_iter()
            .collect(),
        ),
        writes: RefCell::new(Vec::new()),
    };

    let failed = perform_clipboard_write(&port, "first", "path");
    let submitted = perform_clipboard_write(&port, "second", "path");
    let committed = perform_clipboard_write(&port, "third", "path");

    assert_eq!(failed.status, NativeEffectStatus::Failure);
    assert!(failed.message.contains("denied"));
    assert_eq!(submitted.status, NativeEffectStatus::Submitted);
    assert_eq!(committed.status, NativeEffectStatus::Success);
    assert_eq!(*port.writes.borrow(), ["first", "second", "third"]);
}

#[test]
fn opener_acceptance_is_quiet_and_failure_is_reduced() {
    let port = ScriptedOpener {
        outcomes: RefCell::new(
            [
                crate::ports::OpenOutcome::Accepted,
                crate::ports::OpenOutcome::Failed(failure("launch denied")),
            ]
            .into_iter()
            .collect(),
        ),
        requests: RefCell::new(Vec::new()),
    };
    let request = crate::ports::OpenRequest::OpenPath(std::path::PathBuf::from("/tmp/value.txt"));

    assert_eq!(perform_open(&port, &request), None);
    let failed = perform_open(&port, &request).expect("failure feedback");
    assert_eq!(failed.status, NativeEffectStatus::Failure);
    assert!(failed.message.contains("launch denied"));
    assert_eq!(port.requests.borrow().len(), 2);
}

#[test]
fn hidden_view_rejections_have_specific_non_modal_feedback() {
    assert_eq!(
        hidden_files_rejection_message(crate::panel::DirStatus::Denied),
        "folder access was denied"
    );
    assert_eq!(
        hidden_files_rejection_message(crate::panel::DirStatus::Gone),
        "the folder is no longer available"
    );
    assert_eq!(
        hidden_files_rejection_message(crate::panel::DirStatus::Partial),
        "the folder could not be read completely"
    );
    assert_eq!(
        hidden_files_rejection_message(crate::panel::DirStatus::Unavailable),
        "the folder could not be read"
    );
}

#[test]
fn a_folder_named_like_an_archive_opens_as_a_folder() {
    let dir = crate::testutil::TempDir::new();
    let folder = dir.dir("photos.zip");
    let file = dir.file("bundle.zip", "");
    assert!(!opens_in_archive_viewer(&folder));
    assert!(opens_in_archive_viewer(&file));
}

#[test]
fn a_clashing_smart_folder_name_gets_a_suffix_instead_of_replacing() {
    let query = |text: &str| crate::query::Query {
        predicates: vec![crate::query::Predicate::NameContains(text.into())],
        ..Default::default()
    };
    let def = |name: &str, root: &str, text: &str| crate::smart_folder::Definition {
        name: name.into(),
        root: root.into(),
        query: query(text),
    };
    let existing = vec![
        def("Filter: src - rs", "/a/src", "rs"),
        def("Filter: src - rs (2)", "/b/src", "rs"),
    ];
    let base = || "Filter: src - rs".to_string();
    // Same search re-saved: keeps its name.
    assert_eq!(
        unique_smart_folder_name(&existing, base(), "/a/src".as_ref(), &query("rs")),
        "Filter: src - rs"
    );
    // Same name, another root: the first free suffix.
    assert_eq!(
        unique_smart_folder_name(&existing, base(), "/c/src".as_ref(), &query("rs")),
        "Filter: src - rs (3)"
    );
    assert_eq!(
        unique_smart_folder_name(&existing, base(), "/b/src".as_ref(), &query("rs")),
        "Filter: src - rs (2)"
    );
}

#[test]
fn selection_average_leaves_folders_out() {
    let one_file_one_folder = crate::selection_summary::SelectionSummary {
        count: 2,
        dir_count: 1,
        total_bytes: 1_000,
        ..Default::default()
    };
    assert_eq!(average_file_size(&one_file_one_folder), None);
    let two_files_one_folder = crate::selection_summary::SelectionSummary {
        count: 3,
        dir_count: 1,
        total_bytes: 1_000,
        ..Default::default()
    };
    assert_eq!(average_file_size(&two_files_one_folder), Some(500));
}

#[test]
fn tree_width_persists_resizes_but_not_window_caps() {
    // Window cap narrower than the preference: keep the preference.
    assert_eq!(resized_tree_width(300.0, 200.0, 200.0), 300.0);
    // Unchanged width: nothing to persist.
    assert_eq!(resized_tree_width(220.0, 400.0, 220.0), 220.0);
    // User dragged the sidebar.
    assert_eq!(resized_tree_width(220.0, 400.0, 260.0), 260.0);
    assert_eq!(resized_tree_width(300.0, 200.0, 150.0), 150.0);
}

#[test]
fn tree_invalidation_drops_the_changed_folder_its_descendants_and_parent() {
    use std::path::PathBuf;
    let mut cache = super::super::tree::ChildCache::new();
    for path in ["/a", "/a/b", "/a/b/c", "/a/b/c/d", "/a/bc", "/a/z", "/x"] {
        for show_hidden in [false, true] {
            cache.insert((PathBuf::from(path), show_hidden), Vec::new().into());
        }
    }
    invalidate_tree_children(&mut cache, std::path::Path::new("/a/b"));
    let mut kept: Vec<_> = cache
        .keys()
        .map(|(path, hidden)| (path.display().to_string(), *hidden))
        .collect();
    kept.sort();
    assert_eq!(
        kept,
        vec![
            ("/a/bc".to_string(), false),
            ("/a/bc".to_string(), true),
            ("/a/z".to_string(), false),
            ("/a/z".to_string(), true),
            ("/x".to_string(), false),
            ("/x".to_string(), true),
        ]
    );
}

#[test]
fn drag_announcement_is_recomputed_only_when_target_or_alt_changes() {
    use crate::operation_view::{DragAnnouncement, DragEffect};
    use std::path::PathBuf;
    let mut memo = DragAnnouncementMemo::default();
    let calls = std::cell::Cell::new(0);
    let a = PathBuf::from("/a");
    let b = PathBuf::from("/b");
    let ask = |memo: &mut DragAnnouncementMemo, target: Option<&PathBuf>, alt| {
        memo.get_or_compute(target, alt, || {
            calls.set(calls.get() + 1);
            DragAnnouncement::valid(
                target.cloned().unwrap_or_default(),
                if alt {
                    DragEffect::Copy
                } else {
                    DragEffect::Move
                },
            )
        })
    };
    let first = ask(&mut memo, Some(&a), false);
    assert_eq!(ask(&mut memo, Some(&a), false), first);
    assert_eq!(calls.get(), 1);
    assert_eq!(
        ask(&mut memo, Some(&a), true),
        DragAnnouncement::valid(a.clone(), DragEffect::Copy)
    );
    assert_eq!(
        ask(&mut memo, Some(&b), true),
        DragAnnouncement::valid(b.clone(), DragEffect::Copy)
    );
    ask(&mut memo, None, true);
    assert_eq!(calls.get(), 4);
    memo.clear();
    ask(&mut memo, None, true);
    assert_eq!(calls.get(), 5);
}

#[test]
fn a_failed_smart_folder_save_leaves_the_error_to_persistence_health() {
    assert_eq!(
        smart_folder_saved_toast("Filter: a - b", true).as_deref(),
        Some("Saved search \"Filter: a - b\"")
    );
    assert_eq!(smart_folder_saved_toast("Filter: a - b", false), None);
}

#[test]
fn drag_overlay_names_the_work_that_would_cancel_a_drop() {
    assert_eq!(drop_busy_reason(None, false, false), None);
    assert_eq!(
        drop_busy_reason(None, true, true).as_deref(),
        Some("wait for the running transfer to finish")
    );
    assert_eq!(
        drop_busy_reason(None, false, true).as_deref(),
        Some("answer the open confirmation first")
    );
    assert_eq!(
        drop_busy_reason(
            Some("Wait for the current Trash operation before dropping".to_string()),
            true,
            false
        )
        .as_deref(),
        Some("wait for the current Trash operation before dropping")
    );
}

#[test]
fn delete_toasts_always_name_the_items() {
    use crate::toasts::ToastKind;
    assert_eq!(
        delete_outcome_toast(5, 3, 0, true, false),
        (
            "Moved 3 items to Trash before cancellation".to_string(),
            ToastKind::Info
        )
    );
    assert_eq!(
        delete_outcome_toast(5, 3, 2, false, false),
        (
            "Moved 3 items to Trash, 2 failed".to_string(),
            ToastKind::Error
        )
    );
    assert_eq!(
        delete_outcome_toast(1, 1, 0, false, false).0,
        "Moved 1 item to Trash"
    );
    assert_eq!(
        delete_outcome_toast(2, 0, 2, false, false).0,
        "Could not move 2 items to Trash"
    );
}

#[test]
fn an_uncertain_trash_names_its_size_and_where_to_review() {
    let (message, kind) = delete_outcome_toast(12, 0, 0, false, true);
    assert_eq!(kind, crate::toasts::ToastKind::Error);
    assert_eq!(
        message,
        format!(
            "Trash of 12 items may be incomplete; review the errors in the Operations panel ({})",
            command_shortcut("J")
        )
    );
}

#[test]
fn command_shortcuts_follow_the_platform_modifier() {
    let expected = if cfg!(target_os = "macos") {
        "\u{2318}Z"
    } else {
        "Ctrl+Z"
    };
    assert_eq!(command_shortcut("Z"), expected);
}

#[test]
fn undo_toasts_pluralize_and_name_the_destination() {
    use crate::undo::Action;
    use std::path::PathBuf;
    let one_move = Action::Move {
        pairs: vec![(PathBuf::from("/a/x"), PathBuf::from("/b/Photos/x"))],
    };
    assert_eq!(undo_recorded_toast(&one_move), "Moved 1 item to \"Photos\"");
    let gather = Action::Gather {
        folder: PathBuf::from("/a/Photos 2"),
        pairs: vec![
            (PathBuf::from("/a/x"), PathBuf::from("/a/Photos 2/x")),
            (PathBuf::from("/a/y"), PathBuf::from("/a/Photos 2/y")),
        ],
    };
    assert_eq!(
        undo_recorded_toast(&gather),
        "Gathered 2 items into \"Photos 2\""
    );
    let rename = Action::Rename {
        from: PathBuf::from("/a/x"),
        to: PathBuf::from("/a/y"),
    };
    assert_eq!(undo_recorded_toast(&rename), "Renamed 1 item");
}

#[test]
fn selection_headline_sums_and_names_the_extremes() {
    use crate::selection_summary::SelectionSummary;
    use std::time::SystemTime;
    let single = SelectionSummary {
        count: 1,
        total_bytes: 2_048,
        largest: Some(("only.txt".into(), 2_048)),
        oldest: Some(("only.txt".into(), SystemTime::UNIX_EPOCH)),
        ..SelectionSummary::default()
    };
    // One item: no average and no largest/oldest.
    let line = selection_headline(&single, 0, 0);
    assert!(line.starts_with("1 item \u{00b7} "), "{line}");
    assert!(!line.contains("avg") && !line.contains("largest"), "{line}");

    let many = SelectionSummary {
        count: 3,
        dir_count: 1,
        total_bytes: 3_000,
        largest: Some(("a-very-long-holiday-video.mov".into(), 2_000)),
        oldest: Some(("notes.txt".into(), SystemTime::UNIX_EPOCH)),
        ..SelectionSummary::default()
    };
    let line = selection_headline(&many, 2, 1);
    assert!(line.starts_with("3 items + 2 in 1 other folder"), "{line}");
    assert!(line.contains("1 folder"), "{line}");
    // Two files share the bytes; the folder does not count.
    assert!(line.contains(" \u{00b7} avg 1.5 KB"), "{line}");
    assert!(
        line.contains("largest a-very-long-h\u{2026}.mov ("),
        "{line}"
    );
    assert!(line.ends_with(" \u{00b7} oldest notes.txt"), "{line}");

    // A zero-byte largest entry is not named.
    let empty_files = SelectionSummary {
        count: 2,
        largest: Some(("empty".into(), 0)),
        ..SelectionSummary::default()
    };
    assert_eq!(selection_headline(&empty_files, 0, 0), "2 items");
}

#[test]
fn clipped_labels_count_chars_and_mark_the_cut() {
    assert_eq!(clipped_label("Downloads", 9), "Downloads");
    assert_eq!(clipped_label("Downloads", 8), "Downl...");
    assert_eq!(
        clipped_label("\u{65e5}\u{672c}\u{8a9e}\u{30d5}\u{30a9}", 4),
        "\u{65e5}..."
    );
    assert_eq!(clipped_label("", 3), "");
    // Below the ellipsis' own width only the marker is left.
    assert_eq!(clipped_label("abcdef", 2), "...");
    assert_eq!(clipped_label("abcdef", 0), "...");
}

#[test]
fn undo_receipts_replay_the_recorded_action_and_need_a_jump_target() {
    use crate::undo::Action;
    use std::path::PathBuf;
    let rename = Action::BatchRename {
        dir: PathBuf::from("/photos"),
        pairs: vec![
            ("a.jpg".into(), "x-a.jpg".into()),
            ("b.jpg".into(), "x-b.jpg".into()),
        ],
        folding: crate::rename_order::NameFolding::CaseFolded,
    };
    let mut center = crate::undo::UndoCenter::default();
    center.record(rename.clone()).unwrap();
    let entry = center.top_undo_entry().expect("recorded entry");
    let receipt = undo_receipt(&rename, entry, 12.5).expect("receipt");
    assert_eq!(receipt.verb, rename.verb());
    assert_eq!(receipt.item_count, 2);
    assert_eq!(receipt.timestamp, 12.5);
    assert_eq!(receipt.jump_to, PathBuf::from("/photos"));
    assert_eq!(receipt.undo_entry, Some(entry));
    // A move with no pairs has nowhere to jump back to.
    assert!(undo_receipt(&Action::Move { pairs: Vec::new() }, entry, 1.0).is_none());
}

#[test]
fn copied_path_labels_agree_with_their_count() {
    use crate::clipboard::PathStyle;
    assert_eq!(copied_paths_label(PathStyle::FullPath, 1), "path");
    assert_eq!(copied_paths_label(PathStyle::FullPath, 3), "3 paths");
    assert_eq!(copied_paths_label(PathStyle::NameOnly, 2), "2 names");
    assert_eq!(
        copied_paths_label(PathStyle::NameWithoutExtension, 1),
        "name without extension"
    );
    assert_eq!(
        copied_paths_label(PathStyle::NameWithoutExtension, 2),
        "2 names without extensions"
    );
    assert_eq!(copied_paths_label(PathStyle::FileUrl, 2), "2 file URLs");
}

#[test]
fn selection_hud_pluralizes_and_skips_an_empty_local_count() {
    assert_eq!(selection_hud_head(1, 0, 0, 0), "1 item");
    assert_eq!(selection_hud_head(3, 1, 0, 0), "3 items \u{00b7} 1 folder");
    assert_eq!(
        selection_hud_head(2, 2, 5, 1),
        "2 items + 5 in 1 other folder \u{00b7} 2 folders"
    );
    assert_eq!(
        selection_hud_head(0, 0, 5, 2),
        "5 selected in 2 other folders"
    );
}

#[test]
fn move_drop_onto_the_sources_own_folder_is_cancelled_without_a_dialog() {
    let (l, r) = (
        crate::testutil::TempDir::new(),
        crate::testutil::TempDir::new(),
    );
    let file = l.file("a.txt", "alpha");
    let mut ws = crate::workspace::Workspace::new(l.path().to_path_buf(), r.path().to_path_buf());
    ws.left.refresh();
    ws.right.refresh();
    // Dragging from the left pane onto the right pane, which shows the
    // same folder the item already sits in.
    ws.left.drag.set(vec![file.clone()]);
    ws.right.drag.set_drop_target(l.path().to_path_buf());

    finish_drop(&mut ws, crate::transfer::TransferKind::Move, || {});

    assert!(ws.pending_op.is_none(), "no conflict or transfer is staged");
    assert!(!ws.has_unfinished_transfer_work());
    assert!(ws.drag_session().is_none(), "the drag is cancelled");
    assert!(ws.left.drag.is_empty());
    assert!(!ws.right.drag.has_drop_target());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha");
}

#[test]
fn move_drop_onto_another_folder_starts_the_transfer() {
    let (l, r) = (
        crate::testutil::TempDir::new(),
        crate::testutil::TempDir::new(),
    );
    let file = l.file("a.txt", "alpha");
    let mut ws = crate::workspace::Workspace::new(l.path().to_path_buf(), r.path().to_path_buf());
    ws.left.refresh();
    ws.right.refresh();
    ws.left.drag.set(vec![file]);
    ws.right.drag.set_drop_target(r.path().to_path_buf());

    finish_drop(&mut ws, crate::transfer::TransferKind::Move, || {});

    assert!(ws.drag_session().is_none(), "the drag is consumed");
    assert!(
        ws.pending_op.is_some() || ws.has_unfinished_transfer_work(),
        "the move is staged, not cancelled"
    );
}

#[test]
fn drag_rejections_follow_the_drag_effect_and_the_source_folder() {
    let dir = crate::testutil::TempDir::new();
    let folder = dir.dir("photos");
    let file = dir.file("a.txt", "");
    let root = dir.path().to_path_buf();
    assert_eq!(
        drag_path_rejection(&folder, std::slice::from_ref(&folder), true),
        Some("a folder cannot be copied into itself")
    );
    assert_eq!(
        drag_path_rejection(&folder, std::slice::from_ref(&folder), false),
        Some("a folder cannot be moved into itself")
    );
    // The other pane showing the sources' own folder.
    assert_eq!(
        drag_path_rejection(&root, std::slice::from_ref(&file), false),
        Some("the item is already in this folder")
    );
    assert_eq!(
        drag_path_rejection(&root, &[file.clone(), folder.clone()], false),
        Some("the items are already in this folder")
    );
    assert_eq!(
        drag_path_rejection(&root, std::slice::from_ref(&file), true),
        None
    );
    assert_eq!(
        drag_path_rejection(&folder, std::slice::from_ref(&file), false),
        None
    );
    assert!(!sources_already_in(&root, &[]));
}

#[test]
fn toasts_stack_by_their_measured_height() {
    let (offsets, total) = toast_stack_offsets(&[None, Some(70.0), Some(30.0)]);
    assert_eq!(offsets, vec![0.0, 44.0, 120.0]);
    assert_eq!(total, 156.0);
    assert_eq!(toast_stack_offsets(&[]), (Vec::new(), 0.0));
}

#[test]
fn hovering_a_toast_holds_the_stack() {
    let rects = [
        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 40.0)),
        egui::Rect::from_min_size(egui::pos2(10.0, 60.0), egui::vec2(100.0, 40.0)),
    ];
    assert!(pointer_over_toast(&rects, Some(egui::pos2(50.0, 70.0))));
    assert!(!pointer_over_toast(&rects, Some(egui::pos2(50.0, 55.0))));
    assert!(!pointer_over_toast(&rects, None));
    assert!(!pointer_over_toast(&[], Some(egui::pos2(50.0, 70.0))));
}

#[test]
fn error_toasts_have_an_icon_and_interrupt_assistive_technology() {
    use crate::toasts::ToastKind;
    use egui::accesskit::{Live, Role};
    assert_eq!(
        toast_text(ToastKind::Error, "Could not copy"),
        "\u{26a0} Could not copy"
    );
    assert_eq!(toast_text(ToastKind::Success, "Copied path"), "Copied path");
    assert_eq!(
        toast_live_region(ToastKind::Error),
        (Role::Alert, Live::Assertive)
    );
    assert_eq!(
        toast_live_region(ToastKind::Info),
        (Role::Status, Live::Polite)
    );
    assert_eq!(
        toast_live_region(ToastKind::Success),
        (Role::Status, Live::Polite)
    );
}

#[test]
fn fallback_menu_takes_focus_once_and_closes_when_focus_leaves() {
    assert_eq!(
        fallback_menu_focus_step(false, false),
        MenuFocusStep::FocusFirst
    );
    assert_eq!(fallback_menu_focus_step(true, true), MenuFocusStep::Keep);
    assert_eq!(fallback_menu_focus_step(true, false), MenuFocusStep::Close);
}

#[test]
fn fallback_menu_returns_focus_to_the_row_unless_the_pointer_moved_it() {
    // Escape and a chosen item restore the row.
    assert!(fallback_menu_restores_focus(true, false, false, false));
    assert!(fallback_menu_restores_focus(false, true, false, true));
    // An arrow key or Tab that moved focus out of the menu restores it.
    assert!(fallback_menu_restores_focus(false, false, true, false));
    // A click on another widget keeps focus where the user clicked.
    assert!(!fallback_menu_restores_focus(false, false, true, true));
    assert!(!fallback_menu_restores_focus(false, false, false, true));
}

#[test]
fn keyboard_fallback_menu_opens_below_the_row_not_at_the_pointer() {
    use crate::ports::{
        ContextMenuAnchor, ContextMenuInvocation, ContextMenuTarget, ContextMenuTrigger,
        ContextMenuViewPoint, ContextMenuViewRect,
    };
    let path = std::path::PathBuf::from("/tmp/context-target");
    let target = ContextMenuTarget {
        expected: crate::path_identity::PathIdentity::missing(&path),
        path,
    };
    let keyboard = ContextMenuInvocation {
        target: target.clone(),
        trigger: ContextMenuTrigger::Keyboard,
        anchor: ContextMenuAnchor::ViewRect(ContextMenuViewRect {
            min_x: 10.0,
            min_y: 100.0,
            max_x: 400.0,
            max_y: 120.0,
            native_points_per_ui_point: 1.0,
        }),
    };
    let origin = egui::pos2(5.0, 7.0);
    let pointer = Some(egui::pos2(600.0, 500.0));
    let center = egui::pos2(300.0, 300.0);
    assert_eq!(
        fallback_menu_pos(&keyboard, origin, pointer, center),
        egui::pos2(15.0, 127.0)
    );
    let pointer_invocation = ContextMenuInvocation {
        target,
        trigger: ContextMenuTrigger::Pointer,
        anchor: ContextMenuAnchor::ViewPoint(ContextMenuViewPoint {
            x: 1.0,
            y: 2.0,
            native_points_per_ui_point: 1.0,
        }),
    };
    assert_eq!(
        fallback_menu_pos(&pointer_invocation, origin, pointer, center),
        egui::pos2(600.0, 500.0)
    );
    assert_eq!(
        fallback_menu_pos(&pointer_invocation, origin, None, center),
        center
    );
}

#[test]
fn shift_alone_does_not_float_the_key_bar() {
    use egui::Modifiers;
    assert!(!key_bar_modifier_held(Modifiers::SHIFT));
    assert!(!key_bar_modifier_held(Modifiers::NONE));
    assert!(key_bar_modifier_held(Modifiers::COMMAND));
    assert!(key_bar_modifier_held(Modifiers::ALT));
    assert!(key_bar_modifier_held(Modifiers::CTRL));
    assert!(key_bar_modifier_held(Modifiers::SHIFT | Modifiers::ALT));
}

#[test]
fn selection_hud_rises_above_the_floating_key_bar() {
    assert_eq!(selection_hud_bottom(800.0, None), 786.0);
    // The bar's top sits above the HUD's normal bottom: clear it.
    assert_eq!(selection_hud_bottom(800.0, Some(770.0)), 764.0);
    // A bar lower than the HUD changes nothing.
    assert_eq!(selection_hud_bottom(700.0, Some(770.0)), 686.0);
}

#[test]
fn pane_split_keeps_its_ratio_across_resizes() {
    assert_eq!(split_left_width(0.3, 1000.0, 100.0), 300.0);
    // Only a divider drag stores a new ratio.
    assert_eq!(
        dragged_split_ratio(0.3, Some(300.0), 400.0, 1000.0, false, true),
        0.4
    );
    // A narrow window clamps the pane: the preference survives, so
    // widening again restores the 30% split.
    assert_eq!(
        dragged_split_ratio(0.3, Some(300.0), 100.0, 200.0, true, false),
        0.3
    );
    assert_eq!(
        dragged_split_ratio(0.3, Some(300.0), 100.0, 200.0, true, true),
        0.3
    );
    // Width settling without a held pointer, or no change, keeps it too.
    assert_eq!(
        dragged_split_ratio(0.3, Some(300.0), 320.0, 1000.0, false, false),
        0.3
    );
    assert_eq!(
        dragged_split_ratio(0.3, Some(300.0), 300.0, 1000.0, false, true),
        0.3
    );
    assert_eq!(
        dragged_split_ratio(0.3, None, 400.0, 1000.0, false, true),
        0.3
    );
    assert_eq!(split_left_width(0.3, 2000.0, 100.0), 600.0);
    assert_eq!(split_left_width(0.5, 1000.0, 100.0), 500.0);
    // Both panes keep their minimum width.
    assert_eq!(split_left_width(0.02, 1000.0, 100.0), 100.0);
    assert_eq!(split_left_width(0.99, 1000.0, 100.0), 900.0);
    // Too narrow for two minimums: split evenly instead of inverting.
    assert_eq!(split_left_width(0.1, 150.0, 100.0), 75.0);
}
