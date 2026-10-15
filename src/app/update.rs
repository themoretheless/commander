//! Per-frame orchestration: each concern lives in its own method/module.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeEffectFeedback {
    message: String,
    status: NativeEffectStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeEffectStatus {
    Success,
    Submitted,
    Failure,
}

/// Type-ahead capsule text. A miss is spelled out as well as tinted, so it
/// does not rely on color alone.
fn type_ahead_label(buffer: &str, miss: bool) -> String {
    if miss {
        format!("\u{00bb} {buffer}  \u{2014} no match")
    } else {
        format!("\u{00bb} {buffer}")
    }
}

fn perform_clipboard_write(
    port: &dyn crate::ports::ClipboardPort,
    text: &str,
    label: &str,
) -> NativeEffectFeedback {
    match port.write_text(text) {
        crate::ports::ClipboardOutcome::Committed => NativeEffectFeedback {
            message: format!("Copied {label}"),
            status: NativeEffectStatus::Success,
        },
        crate::ports::ClipboardOutcome::Submitted => NativeEffectFeedback {
            message: format!("Submitted {label} to the clipboard"),
            status: NativeEffectStatus::Submitted,
        },
        crate::ports::ClipboardOutcome::Unsupported(failure) => NativeEffectFeedback {
            message: format!("Clipboard unavailable: {}", failure.message),
            status: NativeEffectStatus::Failure,
        },
        crate::ports::ClipboardOutcome::Failed(failure) => NativeEffectFeedback {
            message: format!("Could not copy {label}: {}", failure.message),
            status: NativeEffectStatus::Failure,
        },
    }
}

fn perform_open(
    port: &dyn crate::ports::OpenerPort,
    request: &crate::ports::OpenRequest,
) -> Option<NativeEffectFeedback> {
    let failure = match port.open(request) {
        crate::ports::OpenOutcome::Accepted => return None,
        crate::ports::OpenOutcome::Unsupported(failure)
        | crate::ports::OpenOutcome::Failed(failure) => failure,
    };
    Some(NativeEffectFeedback {
        message: format!(
            "Could not open {}: {}",
            request.path().display(),
            failure.message
        ),
        status: NativeEffectStatus::Failure,
    })
}

const fn hidden_files_rejection_message(status: crate::panel::DirStatus) -> &'static str {
    match status {
        crate::panel::DirStatus::Denied => "folder access was denied",
        crate::panel::DirStatus::Unavailable => "the folder could not be read",
        crate::panel::DirStatus::Gone => "the folder is no longer available",
        crate::panel::DirStatus::Partial => "the folder could not be read completely",
        crate::panel::DirStatus::Loading => "the folder is still loading",
        crate::panel::DirStatus::Listed
        | crate::panel::DirStatus::Empty
        | crate::panel::DirStatus::HiddenOnly(_) => "the folder could not be refreshed",
    }
}

struct AppUiRequestSink<'app, 'ctx> {
    app: &'app mut App,
    ctx: &'ctx egui::Context,
}

impl crate::ui_request::UiRequestSink for AppUiRequestSink<'_, '_> {
    fn any_modal_open(&self) -> bool {
        self.app.has_modal_surface()
    }

    fn is_modal_open(&self, modal: UiModal) -> bool {
        self.app.ui.modals.is_open(modal)
    }

    fn apply(&mut self, request: UiRequest) {
        self.app.dispatch_ui_request(request, self.ctx);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModalOwnershipSnapshot {
    prior_open: bool,
}

impl ModalOwnershipSnapshot {
    const fn capture(prior_open: bool) -> Self {
        Self { prior_open }
    }

    const fn trap_active_after(self, current_open: bool) -> bool {
        crate::accessibility::modal_trap_active(self.prior_open, current_open)
    }

    /// Whether the last modal surface closed during this frame.
    const fn closed_after(self, current_open: bool) -> bool {
        self.prior_open && !current_open
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameInputPolicy {
    trapped: bool,
    background_enabled: bool,
}

impl FrameInputPolicy {
    const fn resolve(
        containing_ui_enabled: bool,
        modal_ownership: ModalOwnershipSnapshot,
        modal_is_open: bool,
    ) -> Self {
        let trapped = modal_ownership.trap_active_after(modal_is_open);
        Self {
            trapped,
            background_enabled: containing_ui_enabled && !trapped,
        }
    }

    const fn trapped(self) -> bool {
        self.trapped
    }

    const fn background_enabled(self) -> bool {
        self.background_enabled
    }

    const fn allows_raw_input(self, containing_ui_enabled: bool, requested: bool) -> bool {
        self.background_enabled && containing_ui_enabled && requested
    }

    const fn allows_drop_target(self, containing_ui_enabled: bool, requested: bool) -> bool {
        self.allows_raw_input(containing_ui_enabled, requested)
    }

    const fn allows_drop_execution(self, pointer_released: bool) -> bool {
        self.background_enabled && pointer_released
    }

    const fn allows_divider_reset(self, containing_ui_enabled: bool, double_clicked: bool) -> bool {
        self.allows_raw_input(containing_ui_enabled, double_clicked)
    }

    const fn clear_stale_drag(self) -> bool {
        self.trapped
    }
}

const fn any_modal_surface_open(confirmation_open: bool, app_modal_open: bool) -> bool {
    confirmation_open || app_modal_open
}

/// Per-frame drag state shared by both panes' drop-target updates.
struct PaneDropContext {
    input_policy: FrameInputPolicy,
    containing_ui_enabled: bool,
    dragging: bool,
    drag_source: Option<ActivePanel>,
    hover_pos: Option<egui::Pos2>,
}

/// Seconds a modifier must be held before the key bar floats in.
const KEY_BAR_REVEAL_DELAY: f64 = 0.45;

/// Whether held modifiers should float the key bar in. Shift is left out:
/// it is held for range selection, and a revealed bar over the bottom rows
/// would turn a Shift-click there into a command.
const fn key_bar_modifier_held(modifiers: egui::Modifiers) -> bool {
    modifiers.command || modifiers.alt || modifiers.ctrl
}

/// Temp-data key for the floating key bar's rect while it shows this frame.
const KEY_BAR_OVERLAY_RECT: &str = "shortcuts_overlay_rect";

/// The selection HUD's bottom edge: just above the central area's bottom,
/// and above the floating key bar while that shows, since both sit at the
/// bottom centre.
fn selection_hud_bottom(area_bottom: f32, key_bar_top: Option<f32>) -> f32 {
    let bottom = area_bottom - 14.0;
    key_bar_top.map_or(bottom, |top| bottom.min(top - 6.0))
}

/// Accessible name of the pane-tool close button, whose visible text is
/// only a multiplication sign.
const CLOSE_PANE_TOOL_LABEL: &str = "Close pane tool";

/// Temp-data key for the left pane's share of the two panes' width.
const PANE_SPLIT_RATIO: &str = "pane_split_ratio";

/// The left pane width that keeps the user's split `ratio` of `remaining`,
/// leaving both panes at least `pane_min` wide when there is room.
fn split_left_width(ratio: f32, remaining: f32, pane_min: f32) -> f32 {
    let half = remaining / 2.0;
    (remaining * ratio.clamp(0.0, 1.0)).clamp(pane_min.min(half), (remaining - pane_min).max(half))
}

/// The split ratio to keep after this frame. Only a divider drag (pointer
/// held, the left pane's width changed since last frame, no window or tree
/// re-layout this frame) stores a new ratio, so a width clamped by a narrow
/// window never overwrites the user's split.
fn dragged_split_ratio(
    stored: f32,
    prev_width: Option<f32>,
    width: f32,
    remaining: f32,
    relayout: bool,
    pointer_down: bool,
) -> f32 {
    match prev_width {
        Some(prev)
            if !relayout && pointer_down && remaining > 0.0 && (width - prev).abs() > 0.5 =>
        {
            width / remaining
        }
        _ => stored,
    }
}

/// The keyboard focus ring (a double outline, as on file rows) for a
/// widget whose own frame is hidden, so tabbing to it stays visible.
fn paint_focus_ring(ui: &egui::Ui, response: &egui::Response, t: &crate::theme::ThemeColors) {
    if !response.has_focus() {
        return;
    }
    ui.painter().rect_stroke(
        response.rect,
        crate::theme::ROUNDING_SM,
        Stroke::new(2.0, t.text_primary),
        egui::StrokeKind::Inside,
    );
    ui.painter().rect_stroke(
        response.rect.shrink(2.0),
        crate::theme::ROUNDING_SM,
        Stroke::new(1.0, t.bg_panel),
        egui::StrokeKind::Inside,
    );
}

fn clipped_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut out: String = text.chars().take(keep).collect();
    out.push_str("...");
    out
}

/// Drag-overlay verdict for the hovered target and Alt state. Computing it
/// stats the target and probes its volume, so the overlay reuses it while the
/// pointer stays on the same folder instead of re-probing every frame. It is
/// cleared whenever no drag is in progress, so each drag starts fresh.
#[derive(Default)]
pub struct DragAnnouncementMemo {
    cached: Option<(
        Option<PathBuf>,
        bool,
        crate::operation_view::DragAnnouncement,
    )>,
}

impl DragAnnouncementMemo {
    fn get_or_compute(
        &mut self,
        target: Option<&PathBuf>,
        alt: bool,
        compute: impl FnOnce() -> crate::operation_view::DragAnnouncement,
    ) -> crate::operation_view::DragAnnouncement {
        if let Some((cached_target, cached_alt, announcement)) = &self.cached
            && cached_target.as_ref() == target
            && *cached_alt == alt
        {
            return announcement.clone();
        }
        let announcement = compute();
        self.cached = Some((target.cloned(), alt, announcement.clone()));
        announcement
    }

    fn clear(&mut self) {
        self.cached = None;
    }
}

/// Drop the cached tree children of `dir`, of its descendants and of its
/// parent, for both hidden-file modes, so the tree re-reads only the folders
/// a pane change can have altered. Descendants are included because an
/// operation dropped onto a subfolder row (or a refresh after one) changes
/// that subfolder without either pane showing it. Scanning the cache keys is
/// far cheaper than the `read_dir` calls a full clear would trigger.
fn invalidate_tree_children(cache: &mut super::tree::ChildCache, dir: &std::path::Path) {
    cache.retain(|(path, _), _| !path.starts_with(dir));
    if let Some(parent) = dir.parent() {
        for show_hidden in [false, true] {
            cache.remove(&(parent.to_path_buf(), show_hidden));
        }
    }
}

/// Whether an open pane tool replaces `side`'s file list.
fn pane_tool_covers(tools: &super::ui_state::PaneTools, side: ActivePanel) -> bool {
    tools.side == Some(side) && tools.any_open()
}

/// Whether `command` would make a tool-covered pane active, where file
/// commands (F5/F6/F8/Enter) would act on its hidden listing.
pub(super) fn switch_into_covered_pane(
    tools: &super::ui_state::PaneTools,
    active: ActivePanel,
    command: crate::command::Command,
) -> bool {
    command == crate::command::Command::SwitchPanel && pane_tool_covers(tools, active.other())
}

/// Feedback when a panel switch is refused because a tool covers the pane.
pub(super) const SWITCH_INTO_COVERED_PANE: &str = "Close the pane tool to switch to that pane";

/// Whether `action` is the Trash undo recorded for exactly these delete
/// results: every trashed item, with the version preserved for it.
fn trash_undo_matches(
    items: &[crate::workspace::DeleteItemResult],
    action: &crate::undo::Action,
) -> bool {
    let crate::undo::Action::Trash { items: recorded } = action else {
        return false;
    };
    let trashed = items
        .iter()
        .filter(|item| item.outcome == crate::ports::TrashItemOutcome::Trashed);
    trashed.clone().count() == recorded.len()
        && trashed.zip(recorded).all(|(item, recorded)| {
            item.path == recorded.path && item.version.as_ref() == Some(&recorded.version)
        })
}

/// The id of the newest toast that offers Undo.
fn newest_undoable_toast(toasts: &[crate::toasts::Toast]) -> Option<u64> {
    toasts
        .iter()
        .rev()
        .find(|toast| toast.undoable)
        .map(crate::toasts::Toast::id)
}

/// The auto-generated smart folder name, suffixed ` (2)`, ` (3)`, ... when
/// another search (a same-named folder elsewhere, or a clipped label)
/// already holds it, since adding under a taken name replaces that search.
/// Re-saving the identical search keeps its name and updates it in place.
fn unique_smart_folder_name(
    existing: &[crate::smart_folder::Definition],
    base: String,
    root: &std::path::Path,
    query: &crate::query::Query,
) -> String {
    let free = |name: &str| {
        existing
            .iter()
            .find(|def| def.name == name)
            .is_none_or(|def| def.root == root && &def.query == query)
    };
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|name| free(name))
        .expect("an unused suffix exists")
}

/// The toast after saving a smart folder, worded as the "Save filter as
/// search" menu item and the Saved searches sheet name it. A failed save
/// gets none here: the persistence layer already records the failure with
/// its cause, and `begin_frame` toasts that, so a second, vaguer error toast
/// would repeat it.
fn smart_folder_saved_toast(name: &str, saved: bool) -> Option<String> {
    saved.then(|| format!("Saved search \"{name}\""))
}

/// `n` with the singular or plural noun: "1 item", "3 items".
fn count_noun(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

fn item_count(n: usize) -> String {
    count_noun(n, "item", "items")
}

/// A command shortcut as this platform's key bar and menus spell it:
/// the Command glyph on macOS, `Ctrl+Z` elsewhere (egui's `command` is Ctrl there).
fn command_shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("\u{2318}{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The toast after a clean, undoable transfer, naming where the items went:
/// `Moved 3 items to "Photos"`, or the folder a gather created.
fn undo_recorded_toast(action: &crate::undo::Action) -> String {
    let head = format!("{} {}", action.verb(), item_count(action.item_count()));
    let name = |path: &std::path::Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    match action {
        crate::undo::Action::Gather { folder, .. } => match name(folder) {
            Some(name) => format!("{head} into \"{name}\""),
            None => head,
        },
        crate::undo::Action::Move { .. } => match action.jump_to().as_deref().and_then(name) {
            Some(name) => format!("{head} to \"{name}\""),
            None => head,
        },
        _ => head,
    }
}

/// The receipt for `action`, the undo entry an operation just recorded: it
/// replays that same action, so its Undo stays in sync with Cmd+Z. An action
/// with no folder to jump back to gets none.
fn undo_receipt(action: crate::undo::Action, now: f64) -> Option<crate::receipts::Receipt> {
    let jump_to = action.jump_to()?;
    Some(crate::receipts::Receipt {
        verb: action.verb(),
        item_count: action.item_count(),
        timestamp: now,
        jump_to,
        undo_action: Some(action),
    })
}

/// The toast text and kind for a finished Move to Trash.
fn delete_outcome_toast(
    submitted: usize,
    trashed: usize,
    failed: usize,
    cancelled: bool,
    indeterminate: bool,
) -> (String, crate::toasts::ToastKind) {
    use crate::toasts::ToastKind;
    if indeterminate {
        (
            format!(
                "Trash of {} may be incomplete; review the errors in the Operations panel ({})",
                item_count(submitted),
                command_shortcut("J")
            ),
            ToastKind::Error,
        )
    } else if cancelled {
        (
            if trashed == 0 {
                "Moving to Trash was cancelled".to_string()
            } else {
                format!("Moved {} to Trash before cancellation", item_count(trashed))
            },
            ToastKind::Info,
        )
    } else if failed == 0 {
        (
            format!("Moved {} to Trash", item_count(trashed)),
            ToastKind::Success,
        )
    } else if trashed == 0 {
        (
            format!("Could not move {} to Trash", item_count(failed)),
            ToastKind::Error,
        )
    } else {
        (
            format!("Moved {} to Trash, {failed} failed", item_count(trashed)),
            ToastKind::Error,
        )
    }
}

/// The clipboard label after copying `n` paths in `style`: "path" for one,
/// "3 paths" for several.
fn copied_paths_label(style: crate::clipboard::PathStyle, n: usize) -> String {
    let label = crate::clipboard::style_label(style);
    if n == 1 {
        label.to_string()
    } else {
        format!("{n} {label}s")
    }
}

/// The selection HUD's lead text. With nothing picked in this folder it
/// names only the selection kept in other folders.
fn selection_hud_head(
    count: usize,
    dir_count: usize,
    elsewhere: usize,
    other_folders: usize,
) -> String {
    let other = count_noun(other_folders, "other folder", "other folders");
    let mut head = if count == 0 {
        format!("{elsewhere} selected in {other}")
    } else if elsewhere > 0 {
        format!("{} + {elsewhere} in {other}", item_count(count))
    } else {
        item_count(count)
    };
    if dir_count > 0 {
        head.push_str(&format!(
            " \u{00b7} {}",
            count_noun(dir_count, "folder", "folders")
        ));
    }
    head
}

/// The selection HUD's first line: counts, total and average size, and for
/// two or more items the largest and oldest names, cut to keep their
/// extension.
/// The selection HUD's text, keyed by what it was built from.
pub struct SelectionHudCache {
    side: ActivePanel,
    key: crate::panel::SelectionViewKey,
    /// `(headline, kind breakdown)`, or `None` when nothing is selected.
    text: Option<(String, String)>,
}

/// The selection HUD's headline and kind breakdown for `panel`, or `None`
/// when nothing is selected here or in other folders.
fn selection_hud_text(panel: &crate::panel::PanelState) -> Option<(String, String)> {
    let selected = panel.selected_entries();
    let elsewhere = panel.selected_elsewhere_count();
    let other_folders = panel
        .selection_folder_count()
        .saturating_sub(usize::from(!selected.is_empty()));
    if selected.is_empty() && elsewhere == 0 {
        return None;
    }
    let s = crate::selection_summary::summarize(&selected);
    let head = selection_headline(&s, elsewhere, other_folders);
    let breakdown: String = s
        .kinds
        .iter()
        .take(4)
        .map(|(k, n)| format!("{n} {}", k.label()))
        .collect::<Vec<_>>()
        .join("  \u{00b7}  ");
    Some((head, breakdown))
}

fn selection_headline(
    s: &crate::selection_summary::SelectionSummary,
    elsewhere: usize,
    other_folders: usize,
) -> String {
    let mut head = selection_hud_head(s.count, s.dir_count, elsewhere, other_folders);
    if s.total_bytes > 0 {
        head.push_str(&format!(
            " \u{00b7} {}",
            crate::panel::format_size(s.total_bytes)
        ));
    }
    if let Some(avg) = average_file_size(s) {
        head.push_str(&format!(" \u{00b7} avg {}", crate::panel::format_size(avg)));
    }
    // Largest/oldest are only meaningful once more than one item is picked.
    if s.count >= 2 {
        let short = |name| crate::display_name::truncate_preserving_extension(name, 18);
        if let Some((name, sz)) = &s.largest
            && *sz > 0
        {
            head.push_str(&format!(
                " \u{00b7} largest {} ({})",
                short(name),
                crate::panel::format_size(*sz)
            ));
        }
        if let Some((name, _)) = &s.oldest {
            head.push_str(&format!(" \u{00b7} oldest {}", short(name)));
        }
    }
    head
}

/// Where the egui fallback context menu opens: below the row for a
/// keyboard invocation (its anchor is the row rect relative to `origin`, the
/// viewport's top-left), otherwise at the pointer.
fn fallback_menu_pos(
    invocation: &crate::ports::ContextMenuInvocation,
    origin: egui::Pos2,
    pointer: Option<egui::Pos2>,
    fallback: egui::Pos2,
) -> egui::Pos2 {
    match (invocation.trigger, invocation.anchor) {
        (
            crate::ports::ContextMenuTrigger::Keyboard,
            crate::ports::ContextMenuAnchor::ViewRect(rect),
        ) => origin + egui::vec2(rect.min_x as f32, rect.max_y as f32),
        _ => pointer.unwrap_or(fallback),
    }
}

/// Keyboard focus bookkeeping for the fallback context menu, kept in egui
/// temp data under [`FALLBACK_MENU_FOCUS`] while the menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FallbackMenuFocus {
    /// The row that opened the menu; it gets focus back when the menu closes.
    restore: egui::Id,
    /// Whether the first item already took focus.
    armed: bool,
    /// Whether focus in the menu is keyboard-driven (opened from the
    /// keyboard, or navigated by key since), so the focus ring shows.
    keyboard: bool,
}

const FALLBACK_MENU_FOCUS: &str = "fallback_context_menu_focus";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuFocusStep {
    FocusFirst,
    Keep,
    Close,
}

/// The fallback menu takes focus on its first item when it opens, like a
/// native menu, and closes once focus leaves it (Tab out, a click on another
/// widget), so it never floats unfocused while keys act on the list.
const fn fallback_menu_focus_step(armed: bool, focus_in_menu: bool) -> MenuFocusStep {
    if !armed {
        MenuFocusStep::FocusFirst
    } else if focus_in_menu {
        MenuFocusStep::Keep
    } else {
        MenuFocusStep::Close
    }
}

/// Whether closing the fallback menu hands focus back to the row: after
/// Escape or a chosen item, and when a key (an arrow, Tab) moved focus out
/// of the menu. A pointer press elsewhere leaves focus where it landed.
const fn fallback_menu_restores_focus(
    escaped: bool,
    chose: bool,
    focus_lost: bool,
    pointer_active: bool,
) -> bool {
    escaped || chose || (focus_lost && !pointer_active)
}

/// Height a toast row takes before it has been measured once.
const TOAST_ROW_HEIGHT: f32 = 44.0;
/// Gap between stacked toasts.
const TOAST_GAP: f32 = 6.0;

/// Top offset of each stacked toast (newest first) and the stack's total
/// height, from each toast's last measured height. A wrapped message makes
/// its toast taller, so a fixed row pitch would overlap the next one.
fn toast_stack_offsets(heights: &[Option<f32>]) -> (Vec<f32>, f32) {
    let mut offsets = Vec::with_capacity(heights.len());
    let mut y = 0.0;
    for height in heights {
        offsets.push(y);
        y += height.unwrap_or(TOAST_ROW_HEIGHT - TOAST_GAP) + TOAST_GAP;
    }
    (offsets, y)
}

/// The visible toast text. Errors carry a warning icon so their kind is not
/// told by the accent colour alone.
fn toast_text(kind: crate::toasts::ToastKind, message: &str) -> String {
    match kind {
        crate::toasts::ToastKind::Error => format!("\u{26a0} {message}"),
        crate::toasts::ToastKind::Success | crate::toasts::ToastKind::Info => message.to_string(),
    }
}

/// How assistive technology announces a toast: errors interrupt, every
/// other result is read when the user is idle.
const fn toast_live_region(
    kind: crate::toasts::ToastKind,
) -> (egui::accesskit::Role, egui::accesskit::Live) {
    match kind {
        crate::toasts::ToastKind::Error => (
            egui::accesskit::Role::Alert,
            egui::accesskit::Live::Assertive,
        ),
        crate::toasts::ToastKind::Success | crate::toasts::ToastKind::Info => {
            (egui::accesskit::Role::Status, egui::accesskit::Live::Polite)
        }
    }
}

/// Why a drop would be cancelled right now, worded to follow "Cannot drop: ".
/// Mirrors the guard in `drop_dragged_as`, which cancels silently.
fn drop_busy_reason(
    mutation_block: Option<String>,
    transfer_busy: bool,
    pending_op: bool,
) -> Option<String> {
    if let Some(reason) = mutation_block {
        let mut chars = reason.chars();
        return Some(
            chars
                .next()
                .map(|first| first.to_lowercase().chain(chars).collect())
                .unwrap_or_default(),
        );
    }
    if transfer_busy {
        Some("wait for the running transfer to finish".to_string())
    } else if pending_op {
        Some("answer the open confirmation first".to_string())
    } else {
        None
    }
}

/// Whether every dragged source already sits directly in `target`, so a
/// move there would only conflict with itself.
fn sources_already_in(target: &std::path::Path, sources: &[PathBuf]) -> bool {
    !sources.is_empty() && sources.iter().all(|source| source.parent() == Some(target))
}

/// Why the dragged `sources` cannot land in `target`, judged from their
/// paths and worded to follow "Cannot drop: ". `copy` is the Alt-drag
/// effect: copying items next to themselves is allowed (it duplicates them).
fn drag_path_rejection(
    target: &std::path::Path,
    sources: &[PathBuf],
    copy: bool,
) -> Option<&'static str> {
    if sources
        .iter()
        .any(|source| crate::fs_util::is_within_or_equal(target, source))
    {
        Some(if copy {
            "a folder cannot be copied into itself"
        } else {
            "a folder cannot be moved into itself"
        })
    } else if !copy && sources_already_in(target, sources) {
        Some(if sources.len() == 1 {
            "the item is already in this folder"
        } else {
            "the items are already in this folder"
        })
    } else {
        None
    }
}

/// Average file size once more than one file is picked. Folders report no
/// size, so they are left out of the divisor.
fn average_file_size(s: &crate::selection_summary::SelectionSummary) -> Option<u64> {
    let files = s.count.saturating_sub(s.dir_count);
    (files >= 2 && s.total_bytes > 0).then(|| s.total_bytes / files as u64)
}

/// The folder-tree width to persist after this frame: the rendered width
/// once the user resized the sidebar, or the stored preference while the
/// rendered width only reflects the window's cap.
fn resized_tree_width(stored: f32, max: f32, actual: f32) -> f32 {
    if (actual - stored.min(max)).abs() > 0.5 {
        actual
    } else {
        stored
    }
}

/// Open routes an archive file to the built-in viewer; a folder that merely
/// has an archive-like name (`x.zip/`) opens as a folder.
fn opens_in_archive_viewer(path: &std::path::Path) -> bool {
    crate::archive::is_supported(path) && !path.is_dir()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let _frame_latency =
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::FrameTime);
        let ctx = ui.ctx().clone();
        #[cfg(feature = "visual-qa")]
        crate::visual_qa::begin_probe_frame(&ctx);
        let prior_modal_ownership = ModalOwnershipSnapshot::capture(self.has_modal_surface());
        self.begin_frame(&ctx);
        self.process_pending_context_menu(&ctx);
        self.capture_operation_failures(&ctx);
        self.poll_recovery(&ctx);
        self.show_history_dialog(&ctx);
        self.show_confirm_dialog(&ctx);
        self.show_rename_dialog(&ctx);
        self.show_batch_rename_dialog(&ctx);
        self.show_find_dialog(&ctx);
        self.show_saved_search_dialog(&ctx);
        self.show_collections_dialog(&ctx);
        self.show_mask_dialog(&ctx);
        self.show_run_command_dialog(&ctx);
        self.show_palette_dialog(&ctx);
        // Keep the background disabled on the close frame too, so the pointer
        // release that dismissed a modal cannot click through into a file row.
        let input_policy = FrameInputPolicy::resolve(
            ui.is_enabled(),
            prior_modal_ownership,
            self.has_modal_surface(),
        );
        let modal_open = input_policy.trapped();
        // The closed modal took the focused widget with it; give focus back
        // to the active pane's cursor row so keyboard and assistive-tech
        // users land where they were.
        if prior_modal_ownership.closed_after(self.has_modal_surface()) {
            super::file_list::request_cursor_row_refocus(&ctx);
        }
        #[cfg(feature = "visual-qa")]
        {
            let owner =
                crate::accessibility::top_open_modal(|surface| self.is_modal_surface_open(surface))
                    .map(|surface| format!("{surface:?}"));
            crate::visual_qa::record_input_policy(&ctx, input_policy.background_enabled(), owner);
        }
        let trapped = crate::accessibility::focus_order(crate::accessibility::FocusLayout {
            toolbar_visible: !self.ui.focus_mode,
            operations_open: self.show_operations_center,
            dialog_open: modal_open,
        }) == [crate::accessibility::FocusRegion::Dialog];
        debug_assert_eq!(trapped, input_policy.trapped());
        if input_policy.clear_stale_drag() {
            self.ws.cancel_drag();
        }
        let background_order =
            crate::accessibility::focus_order(crate::accessibility::FocusLayout {
                toolbar_visible: !self.ui.focus_mode,
                operations_open: self.show_operations_center,
                dialog_open: false,
            });
        ui.add_enabled_ui(input_policy.background_enabled(), |ui| {
            for region in background_order {
                match region {
                    crate::accessibility::FocusRegion::Toolbar => self.show_toolbar_panel(ui),
                    crate::accessibility::FocusRegion::Operations => {
                        self.show_operations_center(ui);
                    }
                    crate::accessibility::FocusRegion::LeftPanel => {
                        if !self.ui.focus_mode {
                            self.show_shortcut_bar(ui);
                        }
                        self.show_activity_strip(ui);
                        if !self.ui.focus_mode {
                            self.show_selection_hud(ui);
                        }
                        self.show_main_area(ui, input_policy);
                    }
                    crate::accessibility::FocusRegion::RightPanel
                    | crate::accessibility::FocusRegion::Dialog => {}
                }
            }
        });
        self.show_drag_overlay(&ctx);
        self.show_type_ahead_overlay(&ctx, input_policy.background_enabled());
        self.show_toasts(&ctx, input_policy.background_enabled());
        self.show_fallback_context_menu(&ctx);
        self.show_developer_panel(&ctx, input_policy.background_enabled());
        self.handle_drop(&ctx, input_policy);
    }

    /// eframe calls this on exit and on its auto-save interval; persist our
    /// own session snapshot (panel paths, layout, view toggles).
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        let session = self.to_session();
        crate::persistence::report_save(
            "Session",
            crate::session::save_with(self.persistence.as_ref(), &session, &mut self.session_gate),
        );
    }
}

impl App {
    /// Log a receipt for the action on top of the undo stack, which the
    /// operation that just finished pushed (see [`undo_receipt`]), and
    /// return that action.
    pub(crate) fn push_receipt_for_top_undo(&mut self, now: f64) -> Option<crate::undo::Action> {
        let action = self.ws.top_undo_action().cloned()?;
        if let Some(receipt) = undo_receipt(action.clone(), now) {
            self.receipts.push(receipt);
        }
        Some(action)
    }

    fn process_pending_context_menu(&mut self, ctx: &egui::Context) {
        if self.has_modal_surface() || !ctx.input(|input| input.focused) {
            self.ui.cancel_context_menu();
            return;
        }
        let request = match self.ui.poll_context_menu() {
            ui_state::ContextMenuPoll::Idle => return,
            ui_state::ContextMenuPoll::AwaitingPaint => {
                // The rest of frame B paints and publishes the new row cursor
                // and AccessKit tree; request frame C for native tracking.
                ctx.request_repaint();
                return;
            }
            ui_state::ContextMenuPoll::Ready(request) => request,
        };
        if self.ws.active != request.panel {
            return;
        }
        if !self.context_menu.presents_native_menu() {
            let pos = fallback_menu_pos(
                &request.invocation,
                ctx.input(|input| input.viewport_rect().min),
                ctx.pointer_latest_pos(),
                ctx.content_rect().center(),
            );
            self.fallback_menu = Some((request.panel, request.invocation.target.path.clone(), pos));
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(FALLBACK_MENU_FOCUS),
                    FallbackMenuFocus {
                        restore: request.focus_id,
                        armed: false,
                        keyboard: matches!(
                            request.invocation.trigger,
                            crate::ports::ContextMenuTrigger::Keyboard
                        ),
                    },
                );
            });
            return;
        }
        let context_menu = std::rc::Rc::clone(&self.context_menu);
        let effect = crate::provider_runtime::request_context_menu(
            context_menu.as_ref(),
            &request.invocation,
        );
        self.apply_context_menu_effect(request.panel, effect, ctx);
        if !self.has_modal_surface() {
            ctx.memory_mut(|memory| memory.request_focus(request.focus_id));
        }
    }

    /// Context menu drawn by egui where no native menu exists (Linux, Windows).
    fn show_fallback_context_menu(&mut self, ctx: &egui::Context) {
        use crate::provider_runtime::ContextMenuUiEffect as Effect;
        let focus_key = egui::Id::new(FALLBACK_MENU_FOCUS);
        // A modal opened from the keyboard (F8, F2...) dismisses the menu,
        // which would otherwise float above it with live items.
        if self.has_modal_surface() {
            self.fallback_menu = None;
        }
        let Some((panel, path, pos)) = self.fallback_menu.clone() else {
            ctx.data_mut(|data| data.remove::<FallbackMenuFocus>(focus_key));
            return;
        };
        let t = self.colors;
        let mut effect: Option<Effect> = None;
        let mut items: Vec<egui::Id> = Vec::new();
        let mut focus = ctx.data(|data| data.get_temp::<FallbackMenuFocus>(focus_key));
        // A right-click focuses the first item too, but shows the keyboard
        // focus ring only once the user starts navigating by key.
        let key_nav = ctx.input(|input| {
            [
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
                egui::Key::Tab,
            ]
            .into_iter()
            .any(|key| input.key_pressed(key))
        });
        if key_nav && let Some(focus) = focus.as_mut() {
            focus.keyboard = true;
            ctx.data_mut(|data| data.insert_temp(focus_key, *focus));
        }
        let show_ring = focus.is_some_and(|focus| focus.keyboard);
        let area = egui::Area::new(egui::Id::new("fallback_context_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(t.bg_panel)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(crate::theme::ROUNDING_MD)
                    .inner_margin(Margin::same(4))
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        let mut item = |ui: &mut egui::Ui, label: &str, value: Effect| {
                            let response = ui.add(
                                egui::Button::new(label)
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE)
                                    .min_size(Vec2::new(ui.available_width(), 24.0)),
                            );
                            if show_ring {
                                paint_focus_ring(ui, &response, &t);
                            }
                            items.push(response.id);
                            // The first item is focused on open, so Space
                            // (select, Quick Look) must not run it; Enter does.
                            if super::confirm_dialog::confirm_clicked(ui, &response) {
                                effect = Some(value);
                            }
                        };
                        item(
                            ui,
                            "Open",
                            Effect::Open(crate::ports::OpenRequest::OpenPath(path.clone())),
                        );
                        item(
                            ui,
                            "Show in file manager",
                            Effect::Open(crate::ports::OpenRequest::Reveal(path.clone())),
                        );
                        item(ui, "Copy path", Effect::CopyPath(path.clone()));
                        ui.separator();
                        item(ui, "Move to Trash", Effect::MoveToTrash(path.clone()));
                        ui.separator();
                        item(ui, "Refresh", Effect::RefreshPanels);
                    });
            });
        let focus_in_menu = ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| items.contains(&id));
        let escaped = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let mut focus_lost = false;
        match focus.map(|focus| fallback_menu_focus_step(focus.armed, focus_in_menu)) {
            Some(MenuFocusStep::FocusFirst) => {
                if let Some(first) = items.first() {
                    ctx.memory_mut(|memory| memory.request_focus(*first));
                }
                if let Some(focus) = focus.as_mut() {
                    focus.armed = true;
                    ctx.data_mut(|data| data.insert_temp(focus_key, *focus));
                }
            }
            Some(MenuFocusStep::Close) => focus_lost = true,
            Some(MenuFocusStep::Keep) | None => {}
        }
        let dismissed =
            escaped || focus_lost || (area.response.clicked_elsewhere() && effect.is_none());
        if effect.is_some() || dismissed {
            self.fallback_menu = None;
            ctx.data_mut(|data| data.remove::<FallbackMenuFocus>(focus_key));
            // Like the native menu, hand focus back to the row, unless the
            // user clicked elsewhere. Arrow keys and Tab move egui focus
            // spatially out of the menu; that closes it but returns to the row.
            let pointer_active = ctx.input(|input| {
                input.pointer.any_pressed()
                    || input.pointer.any_down()
                    || input.pointer.any_released()
            });
            if fallback_menu_restores_focus(escaped, effect.is_some(), focus_lost, pointer_active)
                && let Some(focus) = focus
            {
                ctx.memory_mut(|memory| memory.request_focus(focus.restore));
            }
        }
        if effect.is_some() {
            self.apply_context_menu_effect(panel, effect, ctx);
        }
    }

    fn queue_context_menu_candidate(
        &mut self,
        panel: ActivePanel,
        candidate: ui_state::ContextMenuCandidate,
        ctx: &egui::Context,
    ) {
        self.ws.active = ui_state::context_menu_owner(self.ws.active, panel);
        let invocation = self.context_menu.prepare_context_menu(
            candidate.target,
            candidate.trigger,
            candidate.anchor,
        );
        self.ui
            .queue_context_menu(panel, invocation, candidate.focus_id);
        ctx.request_repaint();
    }

    fn has_modal_surface(&self) -> bool {
        any_modal_surface_open(self.ws.pending_op.is_some(), self.ui.modals.any_open())
            || self.ws.delete_active()
    }

    pub(crate) fn is_modal_surface_open(
        &self,
        surface: crate::accessibility::ModalSurface,
    ) -> bool {
        match surface {
            crate::accessibility::ModalSurface::Confirmation => self.ws.pending_op.is_some(),
            _ => self.ui.modals.is_surface_open(surface),
        }
    }

    /// Frame bookkeeping: repaint heuristics, notify wiring, fs polling,
    /// input handling and background-task polling.
    fn begin_frame(&mut self, ctx: &egui::Context) {
        self.sync_ui_scale_with_zoom(ctx);
        ctx.data_mut(|data| {
            data.remove::<egui::Rect>(egui::Id::new("current_focus_indicator"));
            data.remove::<egui::Rect>(egui::Id::new("active_error_surface"));
        });
        let persistence_generation = crate::persistence::issue_generation();
        if persistence_generation > self.persistence_issue_seen {
            let persistence = crate::persistence::health_snapshot();
            self.persistence_issue_seen = persistence.issue_generation;
            if let Some(message) = persistence.last_issue {
                self.toast(ctx, crate::toasts::ToastKind::Error, message, false);
            }
        }
        // Static previews do not need a frame loop. Preview workers repaint on
        // completion, while loading state schedules its own bounded cadence.
        let has_animation =
            ctx.egui_is_using_pointer() || ctx.input(|i| i.smooth_scroll_delta.length() > 0.0);
        if has_animation {
            ctx.request_repaint();
        }

        let index_idle = ctx.input(|input| {
            input.events.is_empty()
                && !input.pointer.any_down()
                && input.smooth_scroll_delta == Vec2::ZERO
        }) && self.ws.active_transfer_view().is_none()
            && self.ws.pending_op.is_none()
            && !self.ws.delete_active()
            && self
                .ui
                .modals
                .find
                .as_ref()
                .is_none_or(|state| !state.searching());
        if !index_idle {
            crate::io_budget::note_foreground_activity();
        }
        self.content_index.set_idle(index_idle);
        self.content_index.poll();
        {
            let repaint = ctx.clone();
            self.ws.poll_space_probe(move || repaint.request_repaint());
        }

        // First frame: wire the repaint callback into both panels and do
        // the initial directory read.
        let first_listing = !self.ws.left.has_notify() || !self.ws.right.has_notify();
        let listing_latency = first_listing.then(|| {
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::FirstListing)
        });
        if !self.ws.left.has_notify() {
            let c = ctx.clone();
            self.ws
                .left
                .set_notify(std::sync::Arc::new(move || c.request_repaint()));
            self.ws.left.set_workload(self.workload.clone());
            self.ws.left.refresh();
        }
        if !self.ws.right.has_notify() {
            let c = ctx.clone();
            self.ws
                .right
                .set_notify(std::sync::Arc::new(move || c.request_repaint()));
            self.ws.right.set_workload(self.workload.clone());
            self.ws.right.refresh();
        }
        drop(listing_latency);
        if first_listing && let Some(mut trace) = self.startup_trace.take() {
            trace.checkpoint(crate::measurement::StartupPhase::FirstListing);
            trace.finish();
        }

        let left_listing = self.ws.left.poll_listing();
        let right_listing = self.ws.right.poll_listing();
        let left_fs = self.ws.left.poll_fs_changes();
        let right_fs = self.ws.right.poll_fs_changes();
        // A pane change can only alter the tree rows for that pane's folder,
        // its subfolders and its parent; clearing everything made the tree re-read every
        // visible folder synchronously on each watcher refresh.
        if left_listing || left_fs {
            invalidate_tree_children(&mut self.tree_children_cache, &self.ws.left.current_path);
        }
        if right_listing || right_fs {
            invalidate_tree_children(&mut self.tree_children_cache, &self.ws.right.current_path);
        }
        if !self.ws.left.watcher_active() || !self.ws.right.watcher_active() {
            ctx.request_repaint_after(crate::panel::WATCHER_RETRY_BACKOFF);
        }

        // Drop targets are only valid for the frame that set them
        // (rows re-assert them while hovered during render).
        self.ws.left.drag.clear_drop_target();
        self.ws.right.drag.clear_drop_target();

        // The fallback context menu owns the keyboard while it is open:
        // arrows, Enter and Delete act on its items, not the list behind it.
        if self.fallback_menu.is_some() {
            self.ui.type_ahead = None;
            self.ui.chord = None;
        } else {
            self.handle_keys(ctx);
        }
        self.preload_images(ctx);
        {
            let c = ctx.clone();
            // poll_transfer drains the next queued transfer (two-way sync second
            // pass, or any op queued behind the active one) via this notify.
            let outcome = self.ws.poll_transfer(move || c.request_repaint());
            let now = ctx.input(|input| input.time);
            if let Some(report) = outcome.terminal {
                self.capture_terminal_report(report, super::secs_to_millis(now));
            }
            if outcome.undo_recorded {
                // A clean move just finished: raise an undoable toast and
                // log a receipt (jump-back + the same live undo affordance).
                if let Some(a) = self.push_receipt_for_top_undo(now) {
                    self.toast(
                        ctx,
                        crate::toasts::ToastKind::Success,
                        undo_recorded_toast(&a),
                        true,
                    );
                }
            }
        }
        if let Some(outcome) = self.ws.poll_delete() {
            self.apply_delete_outcome(outcome, ctx);
        }
        self.toasts.prune(ctx.input(|i| i.time));
        self.dispatch_ui_requests(ctx);
        self.capture_escape_request(ctx);
        self.update_focus_mode(ctx);
    }

    /// Dispatch a fixed frame snapshot. Open modal surfaces serialize behind
    /// one another; duplicate payload-free opens are idempotent, while
    /// payload-bearing modal requests retain their FIFO identity.
    fn dispatch_ui_requests(&mut self, ctx: &egui::Context) {
        let requests = self.ws.drain_ui_requests();
        let deferred = crate::ui_request::dispatch_snapshot(
            requests,
            &mut AppUiRequestSink { app: self, ctx },
        );
        self.ws.defer_ui_requests(deferred);
    }

    fn apply_delete_outcome(
        &mut self,
        outcome: crate::workspace::DeleteOutcome,
        ctx: &egui::Context,
    ) {
        let now = ctx.input(|input| input.time);
        let (message, kind) = delete_outcome_toast(
            outcome.submitted.item_count,
            outcome.trashed,
            outcome.failed,
            outcome.cancelled,
            outcome.indeterminate,
        );
        // Only this delete's own history record may back the Undo: when it
        // recorded none, the top entry is an older Trash.
        let undo_action = self
            .ws
            .top_undo_action()
            .filter(|action| trash_undo_matches(&outcome.items, action))
            .filter(|_| outcome.trashed > 0 && !outcome.indeterminate)
            .cloned();
        self.toast(ctx, kind, message, undo_action.is_some());

        if outcome.trashed > 0 {
            let jump_to = outcome
                .submitted
                .source_roots
                .first()
                .cloned()
                .unwrap_or_else(|| self.ws.active_panel_ref().current_path.clone());
            self.receipts.push(crate::receipts::Receipt {
                verb: "Deleted",
                item_count: outcome.trashed,
                timestamp: now,
                jump_to,
                undo_action,
            });
        }

        if !outcome.failures.is_empty() {
            self.ingest_failure_notice(crate::operation_view::FailureNotice {
                attempt_id: outcome.attempt_id,
                operation_id: outcome.operation_id,
                summary: outcome.submitted,
                errors: outcome
                    .failures
                    .iter()
                    .map(|failure| failure.message.clone())
                    .collect(),
                failures: outcome.failures,
                created_at_millis: super::secs_to_millis(now),
            });
        }
    }

    fn dispatch_ui_request(&mut self, request: UiRequest, ctx: &egui::Context) {
        match request {
            UiRequest::Rename(path) => self.open_rename(path, ctx),
            UiRequest::SelectMask => self.open_mask(ctx),
            UiRequest::RunCommand => self.open_run_command(ctx),
            UiRequest::GatherIntoFolder => {
                let c = ctx.clone();
                self.ws.gather_into_folder(move || c.request_repaint());
            }
            UiRequest::TransferIntoCursorFolder(kind) => {
                let c = ctx.clone();
                self.ws
                    .transfer_selection_into_cursor_folder(kind, move || c.request_repaint());
            }
            UiRequest::GoToPath => self.open_path(ctx),
            UiRequest::Recent => self.open_recent(ctx),
            UiRequest::Undo => self.open_history_preview(HistoryReplayMode::Undo, ctx),
            UiRequest::Palette => self.open_palette(ctx),
            UiRequest::BatchRename => self.open_batch_rename(ctx),
            UiRequest::Sync => self.open_sync(),
            UiRequest::FindDuplicates => self.open_duplicates(ctx),
            UiRequest::DiffFiles => self.open_diff(ctx),
            UiRequest::DiskTreemap => self.open_treemap(ctx),
            UiRequest::Find => self.open_find(ctx),
            UiRequest::FocusFilter => {
                let side = match self.ws.active {
                    ActivePanel::Left => "left",
                    ActivePanel::Right => "right",
                };
                ctx.memory_mut(|memory| memory.request_focus(super::render::pane_filter_id(side)));
            }
            UiRequest::Archive(path) => self.open_archive(path, ctx),
            UiRequest::SavedSearch => self.open_saved_search(),
            UiRequest::ProjectCollections => self.open_collections(ctx),
            UiRequest::ToggleQueuePanel => self.toggle_queue_panel(),
            UiRequest::ToggleCompare => self.show_compare = !self.show_compare,
            UiRequest::ToggleTheme => self.switch_theme(ctx),
            UiRequest::ToggleSizeBars => self.show_size_bars = !self.show_size_bars,
            UiRequest::OperationHistory => self.open_operation_history(),
            UiRequest::OpenRecoveryCenter => self.open_recovery_center(),
            UiRequest::ReviewRecovery(operation_id) => {
                self.open_recovery_operation(operation_id, RecoveryDetail::Inspect)
            }
            UiRequest::OpenExternal(request) => self.open_external(request, ctx),
            UiRequest::CopyPaths(style) => self.copy_paths(style, ctx),
            UiRequest::CopyText { text, label } => {
                self.write_clipboard(&text, &label, ctx);
            }
            UiRequest::Notice { message, error } => {
                self.toast(
                    ctx,
                    if error {
                        crate::toasts::ToastKind::Error
                    } else {
                        crate::toasts::ToastKind::Info
                    },
                    message,
                    false,
                );
            }
            UiRequest::ChecksumReport(report) => {
                let summary = report.summary_line();
                let detail = report.detailed_text();
                let failed = report.rows.iter().any(|row| row.error.is_some());
                self.toast(
                    ctx,
                    if failed {
                        crate::toasts::ToastKind::Error
                    } else {
                        crate::toasts::ToastKind::Info
                    },
                    summary,
                    false,
                );
                if !detail.is_empty() {
                    self.write_clipboard(&detail, "checksum report", ctx);
                }
            }
            UiRequest::HiddenFilesOutcome(outcome) => {
                self.apply_hidden_files_outcome(outcome, ctx);
            }
            UiRequest::Redo => self.open_history_preview(HistoryReplayMode::Redo, ctx),
        }
    }

    fn apply_hidden_files_outcome(
        &mut self,
        outcome: crate::panel::ViewApplyOutcome,
        ctx: &egui::Context,
    ) {
        match outcome {
            crate::panel::ViewApplyOutcome::Applied => {
                self.tree_children_cache.clear();
            }
            crate::panel::ViewApplyOutcome::ReadRejected(status) => {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    format!(
                        "Hidden files unchanged: {}",
                        hidden_files_rejection_message(status)
                    ),
                    false,
                );
            }
        }
    }

    fn open_history_preview(&mut self, mode: HistoryReplayMode, ctx: &egui::Context) {
        let preview = match mode {
            HistoryReplayMode::Undo => self.ws.preview_undo(),
            HistoryReplayMode::Redo => self.ws.preview_redo(),
        };
        self.ui.modals.history_preview = preview.map(|preview| HistoryPreviewState {
            mode,
            preview,
            error: None,
        });
        if self.ui.modals.history_preview.is_some() {
            return;
        }
        match mode {
            HistoryReplayMode::Undo => self.push_history_notice(ctx, "Nothing to undo", false),
            HistoryReplayMode::Redo => {
                let reason = self.ws.redo_unavailable_reason();
                let is_error = reason.is_some();
                let message = reason.unwrap_or_else(|| "Nothing to redo".to_string());
                self.push_history_notice(ctx, &message, is_error);
            }
        }
    }

    fn copy_paths(&mut self, style: crate::clipboard::PathStyle, ctx: &egui::Context) {
        let paths: Vec<std::path::PathBuf> = self
            .ws
            .active_panel_ref()
            .selected_or_cursor()
            .unwrap_or_default()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        if !paths.is_empty() {
            let other_root = self.ws.inactive_panel().current_path.clone();
            let text = crate::clipboard::format(&paths, style, Some(&other_root));
            self.write_clipboard(&text, &copied_paths_label(style, paths.len()), ctx);
        }
    }

    pub(super) fn write_clipboard(&mut self, text: &str, label: &str, ctx: &egui::Context) {
        let feedback = perform_clipboard_write(self.clipboard.as_ref(), text, label);
        let kind = match feedback.status {
            NativeEffectStatus::Success => crate::toasts::ToastKind::Success,
            NativeEffectStatus::Submitted => crate::toasts::ToastKind::Info,
            NativeEffectStatus::Failure => crate::toasts::ToastKind::Error,
        };
        self.toast(ctx, kind, feedback.message, false);
    }

    pub(crate) fn open_external(
        &mut self,
        request: crate::ports::OpenRequest,
        ctx: &egui::Context,
    ) {
        if let crate::ports::OpenRequest::OpenPath(path) = &request
            && opens_in_archive_viewer(path)
        {
            self.ws.emit_ui_request(UiRequest::Archive(path.clone()));
            ctx.request_repaint();
            return;
        }
        let Some(feedback) = perform_open(self.opener.as_ref(), &request) else {
            return;
        };
        self.toast(
            ctx,
            crate::toasts::ToastKind::Error,
            feedback.message,
            false,
        );
    }

    fn show_toolbar_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let ctx = ui.ctx().clone();
        egui::Panel::top(crate::accessibility::FocusRegion::Toolbar.id())
            .frame(Frame::NONE.fill(t.bg_toolbar))
            .show(ui, |ui| {
                self.toolbar(ui, &ctx);
            });
    }

    /// The F-key bar. Pinned at the bottom when the user asks for it;
    /// otherwise it floats over the panels while a modifier key is held, so
    /// the default screen is only the two panels.
    fn show_shortcut_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let ctx = ui.ctx().clone();
        let now = ctx.input(|input| input.time);
        ctx.data_mut(|data| data.remove::<egui::Rect>(egui::Id::new(KEY_BAR_OVERLAY_RECT)));
        let held = ctx.input(|input| key_bar_modifier_held(input.modifiers));
        self.ui.modifier_held_since = match (held, self.ui.modifier_held_since) {
            (true, None) => Some(now),
            (true, since) => since,
            (false, _) => None,
        };
        let reveal = self
            .ui
            .modifier_held_since
            .is_some_and(|since| now - since >= KEY_BAR_REVEAL_DELAY);
        if held && !reveal {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(KEY_BAR_REVEAL_DELAY));
        }
        if !self.show_key_bar && !reveal {
            return;
        }
        let shortcut_context = self.ws.action_bar_command_context();
        let hints = crate::command::contextual_shortcuts(&shortcut_context);
        let mut run: Option<crate::command::Command> = None;
        let draw = |ui: &mut egui::Ui, run: &mut Option<crate::command::Command>| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for hint in &hints {
                    let response = ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("{}  {}", hint.key, hint.label))
                                    .size(11.0)
                                    .color(t.text_secondary),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .corner_radius(crate::theme::ROUNDING_SM),
                        )
                        .on_hover_text(hint.label);
                    paint_focus_ring(ui, &response, &t);
                    if response.clicked() {
                        *run = Some(hint.command);
                    }
                }
            });
        };
        if self.show_key_bar {
            egui::Panel::bottom("shortcuts")
                .frame(
                    Frame::NONE
                        .fill(t.bg_toolbar)
                        .inner_margin(Margin::symmetric(8, 3)),
                )
                .show(ui, |ui| draw(ui, &mut run));
        } else if ui.is_enabled() {
            // The overlay is a root Area that would not inherit this disabled
            // Ui; skip it while a dialog owns input, so holding a modifier
            // in a dialog cannot reveal clickable commands above it.
            let overlay = egui::Area::new(egui::Id::new("shortcuts_overlay"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -10.0])
                .order(egui::Order::Foreground)
                .interactable(true)
                .show(&ctx, |ui| {
                    Frame::NONE
                        .fill(t.bg_panel)
                        .stroke(Stroke::new(1.0_f32, t.border))
                        .corner_radius(crate::theme::ROUNDING_MD)
                        .inner_margin(Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.set_max_width(ctx.content_rect().width() - 32.0);
                            draw(ui, &mut run);
                        });
                });
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::new(KEY_BAR_OVERLAY_RECT), overlay.response.rect);
            });
        }
        if let Some(command) = run
            && ui.is_enabled()
            && !switch_into_covered_pane(&self.ui.tools, self.ws.active, command)
        {
            self.ws.execute(command);
        }
    }

    fn update_focus_mode(&mut self, ctx: &egui::Context) {
        if !self.ui.focus_mode {
            return;
        }
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::FocusMode);
        let exit_focus = ctx.input(|i| {
            // Accumulate movement so a slow continuous drag still exits, but
            // only once the mode is armed so pre-arm motion does not count.
            if i.time - self.ui.focus_started_at >= crate::focus_mode::ARM_DELAY_SECS {
                self.ui.focus_moved += i.pointer.delta().length();
            }
            crate::focus_mode::should_exit(
                self.ui.focus_started_at,
                i.time,
                self.ui.focus_moved * self.ui.focus_moved,
                escape_requested,
            )
        });
        if exit_focus {
            self.ui.focus_mode = false;
        }
    }

    pub(crate) fn save_active_filter_as_smart_folder(&mut self, ctx: &egui::Context) {
        let (name, root, query) = {
            let active = self.ws.active_panel_ref();
            let query = crate::query::from_panel_filter(active.search_query(), &active.facets());
            if query.predicates.is_empty() {
                return;
            }
            let folder = active
                .current_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| active.current_path.display().to_string());
            let descriptor = if active.search_query().trim().is_empty() {
                count_noun(active.facets().active_count(), "facet", "facets")
            } else {
                active.search_query().trim().to_string()
            };
            (
                format!(
                    "Filter: {} - {}",
                    clipped_label(&folder, 24),
                    clipped_label(&descriptor, 32)
                ),
                active.current_path.clone(),
                query,
            )
        };
        let name = unique_smart_folder_name(&self.smart_folders_mut().items, name, &root, &query);
        self.smart_folders_mut()
            .add(crate::smart_folder::Definition {
                name: name.clone(),
                root,
                query,
            });
        let saved = crate::smart_folder::save(self.smart_folders_mut());
        if let Some(message) = smart_folder_saved_toast(&name, saved) {
            self.toast(ctx, crate::toasts::ToastKind::Success, message, false);
        }
    }

    /// A quiet pill near the bottom of the central area summarizing the active
    /// panel's selection (count, folders, size, kind breakdown). Shown only
    /// when something is selected; complements the per-panel status bar.
    fn show_selection_hud(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let side = self.ws.active;
        let key = self.ws.active_panel_ref().selection_view_key();
        // Summarizing clones every selected entry, so it is redone only when
        // the selection, the filter or the listing changed.
        let cached = self
            .selection_hud_cache
            .as_ref()
            .is_some_and(|cache| cache.side == side && cache.key == key);
        if !cached {
            let text = selection_hud_text(self.ws.active_panel_ref());
            self.selection_hud_cache = Some(SelectionHudCache { side, key, text });
        }
        let Some((head, breakdown)) = self
            .selection_hud_cache
            .as_ref()
            .and_then(|cache| cache.text.clone())
        else {
            return;
        };
        let t = self.colors;

        let area = ui.available_rect_before_wrap();
        let screen = ctx.input(|i| i.viewport_rect());
        let key_bar =
            ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new(KEY_BAR_OVERLAY_RECT)));
        let bottom = selection_hud_bottom(area.bottom(), key_bar.map(|rect| rect.top()));
        egui::Area::new(egui::Id::new("selection_hud"))
            .anchor(
                egui::Align2::CENTER_BOTTOM,
                [
                    area.center().x - screen.center().x,
                    bottom - screen.bottom(),
                ],
            )
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.bg_card)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(head)
                                    .size(11.0)
                                    .strong()
                                    .color(t.accent),
                            );
                            if !breakdown.is_empty() {
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(breakdown)
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                            }
                        });
                    });
            });
    }

    fn apply_context_menu_effect(
        &mut self,
        panel: ActivePanel,
        effect: Option<crate::provider_runtime::ContextMenuUiEffect>,
        ctx: &egui::Context,
    ) {
        let Some(effect) = effect else {
            return;
        };
        let effect = match effect {
            crate::provider_runtime::ContextMenuUiEffect::Perform(action) => {
                let path = action.path().to_path_buf();
                let result = self.context_menu.perform_deferred_action(&action);
                crate::provider_runtime::reduce_deferred_context_menu_result(result, &path)
            }
            terminal => Some(terminal),
        };
        let Some(effect) = effect else {
            return;
        };
        match effect {
            crate::provider_runtime::ContextMenuUiEffect::RefreshPanels => {
                self.ws.left.refresh();
                self.ws.right.refresh();
            }
            crate::provider_runtime::ContextMenuUiEffect::Open(request) => {
                self.open_external(request, ctx);
            }
            crate::provider_runtime::ContextMenuUiEffect::CopyPath(path) => {
                let text = crate::clipboard::format(
                    std::slice::from_ref(&path),
                    crate::clipboard::PathStyle::FullPath,
                    None,
                );
                self.write_clipboard(&text, "path", ctx);
            }
            crate::provider_runtime::ContextMenuUiEffect::MoveToTrash(path) => {
                self.ws.request_context_delete(panel, &path);
            }
            crate::provider_runtime::ContextMenuUiEffect::Perform(action) => {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    format!(
                        "Could not {:?}: nested context-menu actions are not allowed",
                        action.command()
                    ),
                    false,
                );
            }
            crate::provider_runtime::ContextMenuUiEffect::Notice { level, message } => {
                let kind = match level {
                    crate::provider_runtime::ContextMenuNoticeLevel::Info => {
                        crate::toasts::ToastKind::Info
                    }
                    crate::provider_runtime::ContextMenuNoticeLevel::Error => {
                        crate::toasts::ToastKind::Error
                    }
                };
                self.toast(ctx, kind, message, false);
            }
        }
    }

    /// Tree sidebar plus the two file panels with the resizable divider.
    fn show_main_area(&mut self, ui: &mut egui::Ui, input_policy: FrameInputPolicy) {
        let ctx = ui.ctx().clone();
        let t = self.colors;
        let containing_ui_enabled = ui.is_enabled();
        let active_left = self.ws.active == ActivePanel::Left;
        let close_active_preview =
            self.take_escape_request(crate::accessibility::EscapeRoute::ActivePreview);
        let (close_left_preview, close_right_preview) = if active_left {
            (close_active_preview, false)
        } else {
            (false, close_active_preview)
        };
        let left_metrics = crate::density::metrics(self.ws.left.density());
        let right_metrics = crate::density::metrics(self.ws.right.density());
        let drag_source = self.ws.drag_session().map(|session| session.source);
        let dragging = input_policy.allows_raw_input(containing_ui_enabled, drag_source.is_some());
        // The dragged paths live in the source pane's drag state; both panes
        // need them to keep a dragged folder from lighting up as a target.
        let dragged: Vec<std::path::PathBuf> = if dragging {
            self.ws
                .drag_session()
                .map(|session| session.entries.to_vec())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // Side surfaces have already carved their space from this Ui. Base
        // pane geometry on the remaining width so panels never overlap them.
        let window_width = ui.available_width();
        let panel_id = egui::Id::new(crate::accessibility::FocusRegion::LeftPanel.id());
        // Build cross-panel comparison maps before borrowing panels mutably:
        // each panel is tinted against the OTHER panel's entries. Reuse the
        // cached maps while neither panel's entries changed, so compare mode does
        // not rebuild two HashMaps (cloning every name) on every painted frame.
        let cmp_right_gen = self.ws.right.entries_gen();
        let cmp_left_gen = self.ws.left.entries_gen();
        let (left_compare, right_compare) = if self.show_compare {
            match self.compare_cache.take() {
                Some((rg, lg, lmap, rmap)) if rg == cmp_right_gen && lg == cmp_left_gen => {
                    (Some(lmap), Some(rmap))
                }
                _ => (
                    Some(crate::compare::build_compare_map(self.ws.right.entries())),
                    Some(crate::compare::build_compare_map(self.ws.left.entries())),
                ),
            }
        } else {
            self.compare_cache = None;
            (None, None)
        };

        // Global tree sidebar
        let mut tree_actual_width: f32 = 0.0;
        if self.show_tree {
            let tree_max = (window_width * 0.3).clamp(100.0, 400.0);
            let tree_resp = egui::Panel::left("global_tree")
                .resizable(true)
                .default_size(self.tree_width.min(tree_max))
                .min_size(100.0)
                .max_size(tree_max)
                .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .id_salt("global_tree_scroll")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.style_mut().interaction.selectable_labels = false;
                            let tree_nav = self.render_global_tree(ui, &t);
                            if let Some(path) = tree_nav {
                                // A pane tool over the active side would hide
                                // the navigation; return that pane to its list.
                                if pane_tool_covers(&self.ui.tools, self.ws.active) {
                                    self.ui.tools = super::ui_state::PaneTools::default();
                                }
                                self.tree_expand_to_path(&path);
                                self.ws.active_panel().navigate_to(path);
                            }
                        });
                });
            tree_actual_width = tree_resp.response.rect.width();
            // Persist a user resize; a width only clamped by a narrow window
            // keeps the stored preference.
            self.tree_width = resized_tree_width(self.tree_width, tree_max, tree_actual_width);
        }

        // Compute half from remaining space (after tree)
        let remaining = window_width - tree_actual_width - 6.0;
        let half = remaining / 2.0;
        let pane_min = crate::accessibility::pane_min_width(remaining);
        let split_ratio_id = egui::Id::new(PANE_SPLIT_RATIO);
        let split_ratio: f32 = ctx.data_mut(|d| d.get_temp(split_ratio_id).unwrap_or(0.5));
        // Re-lay the panes when the window resizes or the tree toggles,
        // keeping the user's split ratio rather than snapping to 50/50.
        let relayout = {
            let prev_half: f32 =
                ctx.data_mut(|d| d.get_temp(egui::Id::new("prev_half")).unwrap_or(0.0));
            let relayout = self.prev_window_width > 0.0
                && ((window_width - self.prev_window_width).abs() > 1.0
                    || (half - prev_half).abs() > 1.0);
            if relayout {
                ctx.data_mut(|d| {
                    d.remove::<egui::containers::panel::PanelState>(panel_id);
                });
            }
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("prev_half"), half));
            relayout
        };
        self.prev_window_width = window_width;

        let mut tree_toggle = false;
        let external_open = std::cell::RefCell::new(Vec::new());
        let opener = |request: crate::ports::OpenRequest| {
            external_open.borrow_mut().push(request);
        };
        // Left panel
        let left_resp = egui::Panel::left(panel_id)
            .resizable(true)
            .default_size(split_left_width(split_ratio, remaining, pane_min))
            .min_size(pane_min)
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ui, |ui| {
                self.render_pane_contents(
                    ActivePanel::Left,
                    ui,
                    input_policy,
                    close_left_preview,
                    left_compare.as_ref(),
                    &opener,
                    dragging,
                    &dragged,
                    left_metrics,
                )
            });
        let left_outcome = left_resp.inner;
        #[cfg(feature = "visual-qa")]
        crate::visual_qa::record_response(
            &ctx,
            crate::visual_qa::ProbeId::LeftPane,
            &left_resp.response,
        );
        tree_toggle |= left_outcome.tree_toggle;

        let pane_drop = PaneDropContext {
            input_policy,
            containing_ui_enabled,
            dragging,
            drag_source,
            hover_pos: ctx.input(|i| i.pointer.hover_pos()),
        };
        self.update_pane_drop_target(ActivePanel::Left, &left_resp.response, &pane_drop);

        // Double-click on panel divider → reset to 50/50
        {
            let panel_rect = left_resp.response.rect;
            let divider_rect = egui::Rect::from_min_max(
                egui::pos2(panel_rect.right() - 4.0, panel_rect.top()),
                egui::pos2(panel_rect.right() + 4.0, panel_rect.bottom()),
            );
            let double_clicked = ctx.input(|i| {
                if let Some(pos) = i.pointer.latest_pos() {
                    divider_rect.contains(pos)
                        && i.pointer
                            .button_double_clicked(egui::PointerButton::Primary)
                } else {
                    false
                }
            });
            if input_policy.allows_divider_reset(containing_ui_enabled, double_clicked) {
                ctx.data_mut(|d| {
                    d.remove::<egui::containers::panel::PanelState>(panel_id);
                    d.insert_temp(split_ratio_id, 0.5_f32);
                });
            } else {
                let width_id = egui::Id::new("pane_split_prev_width");
                let prev_width: Option<f32> = ctx.data_mut(|d| d.get_temp(width_id));
                let pointer_down = ctx.input(|i| i.pointer.primary_down());
                let ratio = dragged_split_ratio(
                    split_ratio,
                    prev_width,
                    panel_rect.width(),
                    remaining,
                    relayout,
                    pointer_down,
                );
                ctx.data_mut(|d| {
                    d.insert_temp(split_ratio_id, ratio);
                    d.insert_temp(width_id, panel_rect.width());
                });
            }
        }

        // Right panel (takes remaining space)
        let right_resp = egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ui, |ui| {
                ui.push_id(crate::accessibility::FocusRegion::RightPanel.id(), |ui| {
                    self.render_pane_contents(
                        ActivePanel::Right,
                        ui,
                        input_policy,
                        close_right_preview,
                        right_compare.as_ref(),
                        &opener,
                        dragging,
                        &dragged,
                        right_metrics,
                    )
                })
                .inner
            });
        let right_outcome = right_resp.inner;
        #[cfg(feature = "visual-qa")]
        crate::visual_qa::record_response(
            &ctx,
            crate::visual_qa::ProbeId::RightPane,
            &right_resp.response,
        );
        tree_toggle |= right_outcome.tree_toggle;

        let pending_external_opens = external_open.into_inner();
        for (panel, request) in [
            (ActivePanel::Left, left_outcome.context_menu_request),
            (ActivePanel::Right, right_outcome.context_menu_request),
        ] {
            match request {
                Some(Ok(candidate)) => self.queue_context_menu_candidate(panel, candidate, &ctx),
                Some(Err(reason)) => {
                    self.toast(&ctx, crate::toasts::ToastKind::Info, reason, false);
                }
                None => {}
            }
        }
        for request in pending_external_opens {
            self.open_external(request, &ctx);
        }

        self.update_pane_drop_target(ActivePanel::Right, &right_resp.response, &pane_drop);

        if tree_toggle {
            self.show_tree = !self.show_tree;
            if self.show_tree {
                let path = self.ws.active_panel().current_path.clone();
                self.tree_reveal_path(&path);
            }
        }

        // Stash the freshly-used compare maps (keyed by the generations they were
        // built from) so the next frame reuses them while the entries are
        // unchanged.
        if let (Some(lmap), Some(rmap)) = (left_compare, right_compare) {
            self.compare_cache = Some((cmp_right_gen, cmp_left_gen, lmap, rmap));
        }
    }

    /// One pane's body: pointer activation, then the panel renderer.
    #[allow(clippy::too_many_arguments)]
    fn render_pane_contents(
        &mut self,
        side: ActivePanel,
        ui: &mut egui::Ui,
        input_policy: FrameInputPolicy,
        close_preview: bool,
        compare: Option<&crate::compare::CompareMap>,
        opener: &dyn Fn(crate::ports::OpenRequest),
        dragging: bool,
        dragged: &[std::path::PathBuf],
        metrics: crate::density::DensityMetrics,
    ) -> super::render::PanelRenderOutcome {
        // A pane showing a tool never becomes active: keys would otherwise
        // act on its hidden listing.
        if pane_tool_covers(&self.ui.tools, side) {
            self.show_pane_tool(ui);
            return super::render::PanelRenderOutcome::default();
        }
        if input_policy.allows_raw_input(
            ui.is_enabled(),
            ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()),
        ) {
            self.ws.active = side;
        }
        let t = self.colors;
        let is_active = self.ws.active == side;
        let (panel, label) = match side {
            ActivePanel::Left => (&mut self.ws.left, "left"),
            ActivePanel::Right => (&mut self.ws.right, "right"),
        };
        Self::render_panel(
            panel,
            ui,
            is_active,
            close_preview,
            &t,
            &mut self.image_cache,
            label,
            self.show_tree,
            self.show_size_bars,
            compare,
            opener,
            dragging,
            dragged,
            metrics,
            self.accessibility_preferences.reduced_motion,
            self.ws.nav_lock,
        )
    }

    /// Draw the open pane tool (disk usage, diff, sync, duplicates or archive)
    /// in place of a file list, with a title bar whose close button and
    /// Escape both return the pane to its listing.
    fn show_pane_tool(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let mut close = false;
        Frame::NONE
            .fill(t.bg_panel)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                // Each tool draws its own heading; the host only adds Close.
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close_button = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("\u{00d7}").color(t.text_muted),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Close (Esc)");
                        // egui names a button by its text; the glyph alone
                        // would be read as "multiplication sign".
                        let enabled = close_button.enabled();
                        close_button.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                enabled,
                                CLOSE_PANE_TOOL_LABEL,
                            )
                        });
                        if close_button.clicked() {
                            close = true;
                        }
                    });
                });
                if self.ui.tools.treemap.is_some() {
                    self.show_treemap_tool(ui);
                } else if self.ui.tools.diff.is_some() {
                    self.show_diff_tool(ui);
                } else if self.ui.tools.sync.is_some() {
                    self.show_sync_tool(ui);
                } else if self.ui.tools.duplicates.is_some() {
                    self.show_duplicates_tool(ui);
                } else {
                    self.show_archive_tool(ui);
                }
            });
        if close {
            self.ui.tools = super::ui_state::PaneTools::default();
        }
    }

    /// Make a pane's own folder the drop target while a drag from the other
    /// pane hovers it, and outline the pane that holds the target.
    fn update_pane_drop_target(
        &mut self,
        side: ActivePanel,
        response: &egui::Response,
        drop: &PaneDropContext,
    ) {
        // A pane showing a tool has no visible folder to drop into.
        if pane_tool_covers(&self.ui.tools, side) {
            return;
        }
        let accent = self.colors.accent;
        let panel = match side {
            ActivePanel::Left => &mut self.ws.left,
            ActivePanel::Right => &mut self.ws.right,
        };
        if drop.input_policy.allows_drop_target(
            drop.containing_ui_enabled,
            drop.dragging
                && drop.drag_source != Some(side)
                && drop
                    .hover_pos
                    .is_some_and(|pos| response.rect.contains(pos))
                && !panel.drag.has_drop_target(),
        ) {
            panel.drag.set_drop_target(panel.current_path.clone());
        }
        if panel.drag.has_drop_target() {
            response.ctx.layer_painter(response.layer_id).rect_stroke(
                response.rect.shrink(1.0),
                CornerRadius::ZERO,
                Stroke::new(1.0, accent),
                egui::StrokeKind::Inside,
            );
        }
    }

    /// Floating label with the dragged file count next to the pointer.
    fn show_drag_overlay(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let Some(session) = self.ws.drag_session() else {
            self.drag_announcement.clear();
            return;
        };
        let drag_entries = session.entries;
        let count = drag_entries.len();
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            let label = if count == 1 {
                drag_entries[0]
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "1 item".into())
            } else {
                format!("{} items", count)
            };
            let alt = ctx.input(|input| input.modifiers.alt);
            let target = session.target;
            // A drop while other work runs is cancelled (`drop_dragged_as`), so
            // say why up front instead of announcing a move that never happens.
            // Checked every frame: the busy state can change mid-drag.
            let busy = drop_busy_reason(
                self.ws.mutation_block_reason("dropping"),
                self.ws.has_unfinished_transfer_work(),
                self.ws.pending_op.is_some(),
            );
            let announcement = if let Some(reason) = busy {
                crate::operation_view::DragAnnouncement::rejected(reason)
            } else {
                self.drag_announcement.get_or_compute(target, alt, || {
                    let Some(target) = target else {
                        return crate::operation_view::DragAnnouncement::rejected(
                            "point at a folder or the other pane",
                        );
                    };
                    if !target.is_dir() {
                        crate::operation_view::DragAnnouncement::rejected(
                            "destination is not a folder",
                        )
                    } else if crate::volume_profile::profile(target).read_only {
                        crate::operation_view::DragAnnouncement::rejected(
                            "destination volume is read-only",
                        )
                    } else if let Some(reason) = drag_path_rejection(target, drag_entries, alt) {
                        crate::operation_view::DragAnnouncement::rejected(reason)
                    } else {
                        let effect = if alt {
                            crate::operation_view::DragEffect::Copy
                        } else {
                            crate::operation_view::DragEffect::Move
                        };
                        crate::operation_view::DragAnnouncement::valid(target.clone(), effect)
                    }
                })
            };
            let target_label = announcement.text();
            let target_color = if matches!(
                announcement,
                crate::operation_view::DragAnnouncement::Valid { .. }
            ) {
                t.accent
            } else {
                t.accent_red
            };
            egui::Area::new(egui::Id::new("drag_overlay"))
                .fixed_pos(pos + egui::vec2(12.0, 12.0))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(Margin::symmetric(9, 6))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(label)
                                    .size(12.0)
                                    .strong()
                                    .color(t.text_primary),
                            );
                            ui.label(
                                egui::RichText::new(target_label)
                                    .size(10.0)
                                    .color(target_color),
                            );
                        });
                });
        }
        ctx.request_repaint();
    }

    /// Floating capsule showing the current type-ahead buffer.
    fn show_type_ahead_overlay(&mut self, ctx: &egui::Context, allow_state_updates: bool) {
        let Some((buffer, last)) = &self.ui.type_ahead else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now - last > super::keys::TYPE_AHEAD_IDLE {
            if allow_state_updates {
                self.ui.type_ahead = None;
            }
            return;
        }
        let t = self.colors;
        let miss = self.ui.type_ahead_miss;
        let label = type_ahead_label(buffer, miss);
        egui::Area::new(egui::Id::new("type_ahead_overlay"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -50.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.bg_card)
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(label)
                                .size(13.0)
                                .strong()
                                .color(if miss { t.accent_red } else { t.accent }),
                        );
                    });
            });
        // Keep repainting so the capsule fades out on idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Bottom-right stack of operation toasts, each with a hairline countdown
    /// and an inline Undo on undoable ops.
    fn show_toasts(&mut self, ctx: &egui::Context, input_enabled: bool) {
        if self.toasts.is_empty() {
            return;
        }
        let t = self.colors;
        let now = ctx.input(|i| i.time);
        let screen = ctx.input(|i| i.viewport_rect());
        let active = self.toasts.active();
        let mut undo = false;
        let mut avoid = Vec::with_capacity(2);
        for id in ["current_focus_indicator", "active_error_surface"] {
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new(id))) {
                avoid.push(crate::accessibility::Rect {
                    x: rect.left(),
                    y: rect.top(),
                    width: rect.width(),
                    height: rect.height(),
                });
            }
        }
        // Newest on top; each row is as tall as it measured last frame.
        let heights: Vec<Option<f32>> = active
            .iter()
            .rev()
            .map(|toast| {
                ctx.memory(|memory| memory.area_rect(egui::Id::new(("toast", toast.id()))))
                    .map(|rect| rect.height())
            })
            .collect();
        let (offsets, stack_height) = toast_stack_offsets(&heights);
        let Some(stack) = crate::accessibility::place_overlay(
            crate::accessibility::Rect {
                x: screen.left(),
                y: screen.top(),
                width: screen.width(),
                height: screen.height(),
            },
            268.0,
            stack_height,
            &avoid,
        ) else {
            // Hidden toasts must not expire unseen (an error would be lost);
            // they keep their full lifetime until a corner is clear again.
            self.toasts.hold(now);
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        };

        // Undo reverses the newest action, so only the newest undoable toast
        // offers it; an older toast's button would undo something else.
        let undo_toast = newest_undoable_toast(active);
        // Newest on top, in the nearest corner that preserves focus and errors.
        let undo_label = format!("Undo {}", command_shortcut("Z"));
        for (toast, offset) in active.iter().rev().zip(offsets) {
            let y = stack.y + offset;
            let accent = match toast.kind {
                crate::toasts::ToastKind::Success => t.accent,
                crate::toasts::ToastKind::Error => t.accent_red,
                crate::toasts::ToastKind::Info => t.text_muted,
            };
            let frac = if self.accessibility_preferences.reduced_motion {
                1.0
            } else {
                crate::toasts::remaining_fraction(toast, now)
            };
            egui::Area::new(egui::Id::new(("toast", toast.id())))
                .fixed_pos(egui::pos2(stack.x, y))
                .order(egui::Order::Tooltip)
                .enabled(input_enabled)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(t.bg_card)
                        .stroke(Stroke::new(1.0_f32, t.border))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(236.0);
                            ui.horizontal(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if undo_toast == Some(toast.id())
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&undo_label)
                                                        .size(11.0)
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(accent)
                                                .corner_radius(CornerRadius::ZERO),
                                            )
                                            .clicked()
                                    {
                                        undo = true;
                                    }
                                    // The message wraps in the space the Undo
                                    // button leaves instead of widening the toast.
                                    ui.allocate_ui_with_layout(
                                        Vec2::new(ui.available_width(), 0.0),
                                        Layout::top_down(Align::Min),
                                        |ui| {
                                            let message = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(toast_text(
                                                        toast.kind,
                                                        &toast.message,
                                                    ))
                                                    .size(12.0)
                                                    .color(t.text_primary),
                                                )
                                                .wrap(),
                                            );
                                            let (role, live) = toast_live_region(toast.kind);
                                            ui.ctx().accesskit_node_builder(message.id, |node| {
                                                node.set_role(role);
                                                node.set_live(live);
                                            });
                                        },
                                    );
                                });
                            });
                            // Hairline countdown.
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 2.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min,
                                    Vec2::new(rect.width() * frac, 2.0),
                                ),
                                CornerRadius::ZERO,
                                accent,
                            );
                        });
                });
        }
        if input_enabled && undo {
            self.ws.execute(crate::command::Command::Undo);
        }
        let repaint_ms = if self.accessibility_preferences.reduced_motion {
            1_000
        } else {
            100
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));
    }

    /// On mouse release, move dragged files into the hovered directory.
    fn handle_drop(&mut self, ctx: &egui::Context, input_policy: FrameInputPolicy) {
        if !input_policy.allows_drop_execution(ctx.input(|i| i.pointer.any_released())) {
            return;
        }
        let ctx2 = ctx.clone();
        let kind = if ctx.input(|input| input.modifiers.alt) {
            TransferKind::Copy
        } else {
            TransferKind::Move
        };
        finish_drop(&mut self.ws, kind, move || ctx2.request_repaint());
    }
}

/// Complete a released drag with `kind`. A Move onto the sources' own
/// folder is cancelled: the overlay already said the items are there, and
/// moving them onto themselves would only raise a conflict dialog.
fn finish_drop(
    ws: &mut crate::workspace::Workspace,
    kind: TransferKind,
    notify: impl Fn() + Send + 'static,
) {
    let onto_own_folder = ws.drag_session().is_some_and(|session| {
        session
            .target
            .is_some_and(|target| sources_already_in(target, session.entries))
    });
    if kind == TransferKind::Move && onto_own_folder {
        ws.cancel_drag();
    } else {
        ws.drop_dragged_as(kind, notify);
    }
}

#[cfg(test)]
mod tests {
    use super::pane_tool_covers;
    use super::{
        DragAnnouncementMemo, invalidate_tree_children, newest_undoable_toast, trash_undo_matches,
    };
    use super::{
        FrameInputPolicy, ModalOwnershipSnapshot, NativeEffectStatus, any_modal_surface_open,
        hidden_files_rejection_message, perform_clipboard_write, perform_open, type_ahead_label,
    };
    use super::{
        MenuFocusStep, fallback_menu_focus_step, fallback_menu_pos, fallback_menu_restores_focus,
    };
    use super::{
        average_file_size, opens_in_archive_viewer, resized_tree_width, unique_smart_folder_name,
    };
    use super::{
        clipped_label, command_shortcut, copied_paths_label, delete_outcome_toast,
        selection_headline, selection_hud_head, undo_receipt, undo_recorded_toast,
    };
    use super::{drag_path_rejection, finish_drop, sources_already_in};
    use super::{
        dragged_split_ratio, key_bar_modifier_held, selection_hud_bottom, split_left_width,
    };
    use super::{drop_busy_reason, smart_folder_saved_toast};
    use super::{toast_live_region, toast_stack_offsets, toast_text};
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
            name_a: "a".into(),
            name_b: "b".into(),
            lines: Vec::new(),
            message: None,
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
            name_a: "a".into(),
            name_b: "b".into(),
            lines: Vec::new(),
            message: None,
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
        let request =
            crate::ports::OpenRequest::OpenPath(std::path::PathBuf::from("/tmp/value.txt"));

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
        };
        let receipt = undo_receipt(rename.clone(), 12.5).expect("receipt");
        assert_eq!(receipt.verb, rename.verb());
        assert_eq!(receipt.item_count, 2);
        assert_eq!(receipt.timestamp, 12.5);
        assert_eq!(receipt.jump_to, PathBuf::from("/photos"));
        assert_eq!(receipt.undo_action, Some(rename));
        // A move with no pairs has nowhere to jump back to.
        assert!(undo_receipt(Action::Move { pairs: Vec::new() }, 1.0).is_none());
    }

    #[test]
    fn copied_path_labels_agree_with_their_count() {
        use crate::clipboard::PathStyle;
        assert_eq!(copied_paths_label(PathStyle::FullPath, 1), "path");
        assert_eq!(copied_paths_label(PathStyle::FullPath, 3), "3 paths");
        assert_eq!(copied_paths_label(PathStyle::NameOnly, 2), "2 names");
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
        let mut ws =
            crate::workspace::Workspace::new(l.path().to_path_buf(), r.path().to_path_buf());
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
        let mut ws =
            crate::workspace::Workspace::new(l.path().to_path_buf(), r.path().to_path_buf());
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
}
