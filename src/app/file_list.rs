fn icon_columns(width: f32) -> usize {
    ((width / 128.0).floor() as usize).clamp(1, 64)
}

use super::*;

/// Why a row has no context menu: the menu binds to the file identity
/// captured with the listing, so a row without one cannot get a menu.
fn context_menu_unavailable_reason(identity: &crate::panel::ListingIdentity) -> Option<String> {
    match identity {
        crate::panel::ListingIdentity::Captured(_) => None,
        crate::panel::ListingIdentity::CaptureFailed(failure) => Some(format!(
            "Menu unavailable: file identity could not be read ({})",
            failure.message
        )),
        crate::panel::ListingIdentity::Unavailable => {
            Some("Menu unavailable: file identity could not be read".to_string())
        }
    }
}

/// The context-menu request for a row, or the reason it has none.
fn context_menu_candidate(
    entry: &crate::panel::FileEntry,
    trigger: crate::ports::ContextMenuTrigger,
    row_rect: egui::Rect,
    response: &egui::Response,
) -> Result<ui_state::ContextMenuCandidate, String> {
    let crate::panel::ListingIdentity::Captured(expected) = &entry.identity else {
        return Err(context_menu_unavailable_reason(&entry.identity).unwrap_or_default());
    };
    // AppKit converts this anchor through the full content view. Using the
    // viewport origin keeps egui and AppKit coordinates aligned when a safe
    // area changes the narrower content rect.
    let origin = response.ctx.input(|input| input.viewport_rect().min);
    let relative = row_rect.translate(-origin.to_vec2());
    let scale = f64::from(response.ctx.zoom_factor());
    let anchor = match trigger {
        crate::ports::ContextMenuTrigger::Pointer => {
            let point = response
                .interact_pointer_pos()
                .unwrap_or_else(|| row_rect.center())
                - origin.to_vec2();
            crate::ports::ContextMenuAnchor::ViewPoint(crate::ports::ContextMenuViewPoint {
                x: f64::from(point.x),
                y: f64::from(point.y),
                native_points_per_ui_point: scale,
            })
        }
        crate::ports::ContextMenuTrigger::Keyboard => {
            crate::ports::ContextMenuAnchor::ViewRect(crate::ports::ContextMenuViewRect {
                min_x: f64::from(relative.min.x),
                min_y: f64::from(relative.min.y),
                max_x: f64::from(relative.max.x),
                max_y: f64::from(relative.max.y),
                native_points_per_ui_point: scale,
            })
        }
    };
    Ok(ui_state::ContextMenuCandidate {
        target: crate::ports::ContextMenuTarget {
            path: entry.path.clone(),
            expected: expected.clone(),
        },
        trigger,
        anchor,
        focus_id: response.id,
    })
}

/// Filtered rows `[first, last)` to lay out this frame. `scroll_top` is the
/// viewport top measured from the first file row (below the ".." row), so it
/// is negative while the ".." row is in view. `ensure_row` forces that row
/// into the window (keyboard scroll, keyboard context menu).
fn visible_rows(
    scroll_top: f32,
    viewport_h: f32,
    row_h: f32,
    total_rows: usize,
    ensure_row: Option<usize>,
) -> (usize, usize) {
    // Float-to-usize casts saturate, so additions must saturate too.
    let page = ((viewport_h / row_h).ceil() as usize).saturating_add(2);
    let mut first = ((scroll_top.max(0.0) / row_h).floor() as usize).min(total_rows);
    let mut last = ((((scroll_top + viewport_h).max(0.0) / row_h).ceil() as usize)
        .saturating_add(1))
    .min(total_rows);
    if let Some(row) = ensure_row.filter(|_| total_rows > 0) {
        if row < first {
            first = row;
            last = first.saturating_add(page).min(total_rows);
        } else if row >= last {
            last = row.saturating_add(1).min(total_rows);
            first = last.saturating_sub(page);
        }
    }
    // A degenerate clip rect (negative or NaN height) must yield an empty
    // window, never `first > last`, which would panic when slicing.
    (first, last.max(first))
}

/// Whether a drag over this pane may autoscroll it: its folder exists and
/// sits on a writable volume. Probed once per drag and folder and kept in
/// egui temp state, since each probe stats the path (a stall on slow network
/// mounts) and a drag repaints every frame. Cleared when the drag ends.
fn drop_target_writable(
    ctx: &egui::Context,
    panel_side: &str,
    path: &std::path::Path,
    dragging: bool,
) -> bool {
    let key = egui::Id::new(("drop_target_writable", panel_side));
    if !dragging {
        ctx.data_mut(|d| d.remove::<(std::path::PathBuf, bool)>(key));
        return false;
    }
    if let Some((cached, writable)) = ctx.data(|d| d.get_temp::<(std::path::PathBuf, bool)>(key))
        && cached == path
    {
        return writable;
    }
    let writable = path.is_dir() && !crate::volume_profile::profile(path).read_only;
    ctx.data_mut(|d| d.insert_temp(key, (path.to_path_buf(), writable)));
    writable
}

/// Selected rows in the filtered view and their total display size, in one
/// pass, plus whether any selected folder is still being measured (it
/// counts as 0 B until then). Skipped entirely (no hash lookups) when
/// nothing is selected.
fn visible_selection_summary(panel: &PanelState) -> (usize, u64, bool) {
    if panel.selected_count() == 0 {
        return (0, 0, false);
    }
    selection_summary_with(panel, &panel.size_snapshot())
}

fn selection_summary_with(
    panel: &PanelState,
    sizes: &crate::panel::SizeSnapshot,
) -> (usize, u64, bool) {
    let entries = panel.entries();
    panel
        .filtered_indices()
        .iter()
        .filter_map(|&index| entries.get(index))
        .filter(|entry| panel.is_selected(&entry.path))
        .fold((0, 0, false), |(count, size, pending), entry| {
            (
                count + 1,
                size.saturating_add(sizes.display_size(entry)),
                // Folder symlinks are never walked (as in folder_overview),
                // so they count as 0 B rather than as still measuring.
                pending
                    || (entry.is_dir
                        && !entry.is_symlink()
                        && sizes.size_of(&entry.path).is_none()),
            )
        })
}

/// The selection's size for the footer. While a selected folder is still
/// being measured the total is a lower bound, shown as "≥ X…" (or "…"
/// before anything is known), like the folder total's "…". A folder the
/// walk guard skipped as too expensive stays unmeasured until a forced scan,
/// so it also shows as this lower bound.
fn selection_size_label(bytes: u64, pending: bool) -> String {
    match (pending, bytes) {
        (false, _) => format_size(bytes),
        (true, 0) => "\u{2026}".to_string(),
        (true, _) => format!("\u{2265} {}\u{2026}", format_size(bytes)),
    }
}

/// Filtered row the list must bring into view, if any. Cursor 0 is the ".."
/// row, which is scrolled to directly, so it never forces the row window.
/// Remember the folder a pane's list last showed and report whether it
/// changed, so the list can start a newly entered folder at the top while
/// keeping one scroll state per pane (a per-folder id would keep a stored
/// offset for every folder ever visited).
fn folder_changed(ctx: &egui::Context, panel_side: &str, path: &std::path::Path) -> bool {
    let key = egui::Id::new(("file_list_folder", panel_side));
    ctx.data_mut(|d| {
        let changed = d.get_temp::<PathBuf>(key).as_deref() != Some(path);
        if changed {
            d.insert_temp(key, path.to_path_buf());
        }
        changed
    })
}

/// The button an empty list offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmptyAction {
    Reveal,
    GoUp,
    Retry,
    ClearFilters,
}

/// Label of the reveal action, named after the platform's file manager.
const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in file manager"
};

/// Glyph, message and optional action of an empty list. A folder that
/// cannot be listed right now (unreadable, deleted, still loading) says so
/// even under an active filter: "No matches" would blame the filter.
fn empty_state(
    status: crate::panel::DirStatus,
    filter_active: bool,
) -> (
    &'static str,
    std::borrow::Cow<'static, str>,
    Option<(&'static str, EmptyAction)>,
) {
    use crate::panel::DirStatus;
    let (glyph, message, action) = match status {
        DirStatus::Denied => (
            "\u{1f512}",
            "No permission to read this folder",
            Some((REVEAL_LABEL, EmptyAction::Reveal)),
        ),
        // U+26A0 alone: no bundled font maps the U+FE0F variation selector,
        // which rendered as a replacement box.
        DirStatus::Gone => (
            "\u{26a0}",
            "This folder no longer exists",
            Some(("Go up", EmptyAction::GoUp)),
        ),
        DirStatus::Unavailable => (
            "\u{26a0}",
            "This folder can't be read right now",
            Some(("Retry", EmptyAction::Retry)),
        ),
        DirStatus::Loading => ("\u{2026}", "Loading folder\u{2026}", None),
        _ if filter_active => (
            "\u{1f50d}",
            "No matches",
            Some(("Clear filters", EmptyAction::ClearFilters)),
        ),
        DirStatus::Partial => ("\u{21bb}", "Folder changed while reading; retrying", None),
        DirStatus::HiddenOnly(count) => {
            return ("\u{1f4c2}", hidden_only_message(count).into(), None);
        }
        DirStatus::Listed | DirStatus::Empty => ("\u{1f4c2}", "Empty", None),
    };
    (glyph, message.into(), action)
}

/// Empty-state text for a folder whose only entries are hidden, so it does
/// not read as empty and says how to reveal them.
fn hidden_only_message(count: usize) -> String {
    let noun = if count == 1 { "item" } else { "items" };
    match super::toolbar::command_shortcut(crate::command::Command::ToggleHidden) {
        Some(shortcut) => format!("{count} hidden {noun}; press {shortcut} to show"),
        None => format!("{count} hidden {noun}"),
    }
}

/// Spoken stand-in for the "…" a folder shows while its size is measured;
/// screen readers would read the glyph as "horizontal ellipsis".
const PENDING_SIZE_SPOKEN: &str = "calculating";

/// Compare-mode marker for a row: the symbol shown beside it and what it
/// means, for hover text and assistive tech.
fn compare_marker(status: crate::compare::CompareStatus) -> (&'static str, &'static str) {
    use crate::compare::CompareStatus;
    match status {
        CompareStatus::Unique => ("+", "Only in this pane"),
        CompareStatus::Differs => ("\u{2260}", "Differs"),
        CompareStatus::Identical => ("=", "Identical"),
        CompareStatus::DirectoryPair => ("?", "Folder pair; contents not compared"),
        CompareStatus::TypeConflict => ("!", "File/folder type conflict"),
    }
}

/// A row's accessible label: the shared file-row semantics plus what the
/// row only paints, a folder's item count and its compare status.
fn row_spoken_label(base: &str, item_count: Option<usize>, compare: Option<&str>) -> String {
    let mut label = base.to_string();
    if let Some(count) = item_count {
        label.push_str(&format!("; Items: {count}"));
    }
    if let Some(compare) = compare {
        label.push_str(&format!("; Compare: {compare}"));
    }
    label
}

/// Row index of a file row in the list's grid node: the ".." row, when
/// shown, is row 0.
fn grid_row_index(filtered_index: usize, has_parent_row: bool) -> usize {
    filtered_index + usize::from(has_parent_row)
}

/// Allocated height of a file row: the density's text and padding, never
/// below the minimum control size (Compact would otherwise be 23 pt).
fn row_content_height(metrics: crate::density::DensityMetrics) -> f32 {
    (metrics.name_pt + metrics.row_pad_y * 2.0 + 7.0).max(crate::accessibility::MIN_CONTROL_POINTS)
}

/// Whether the cursor chevron fits at the row's leading edge. A selected
/// row's check mark sits there, so the cursor shows by its outline alone.
const fn cursor_chevron_fits(is_selected: bool) -> bool {
    !is_selected
}

/// Room kept after the name for the compare symbol in narrow rows.
const INLINE_COMPARE_WIDTH: f32 = 14.0;

/// Name column width. Narrow rows have no metadata column, so the compare
/// symbol (a non-color cue next to the stripe) follows the name instead.
fn name_column_width(layout: crate::accessibility::FileRowLayout, inline_compare: bool) -> f32 {
    if inline_compare && !layout.show_metadata {
        (layout.name_width - INLINE_COMPARE_WIDTH).max(1.0)
    } else {
        layout.name_width
    }
}

/// What a primary pointer click on a file row does to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowClick {
    /// Move the cursor only.
    Plain,
    /// Cmd-click (Ctrl elsewhere): flip the row in or out of the selection.
    Toggle,
    /// Shift-click: add every row from the cursor to the clicked row.
    Range,
}

fn row_click_kind(modifiers: egui::Modifiers) -> RowClick {
    if modifiers.shift {
        RowClick::Range
    } else if modifiers.command {
        RowClick::Toggle
    } else {
        RowClick::Plain
    }
}

/// Whether a row was clicked by the user: a primary pointer click or an
/// AccessKit Click action. egui also reports Space/Enter on a focused
/// widget as a click, but the list already handles those keys (toggle and
/// advance, activate); taking them as row clicks too would move the cursor
/// back onto the focused row and undo them.
fn row_clicked(ctx: &egui::Context, response: &egui::Response) -> bool {
    response.clicked_by(egui::PointerButton::Primary)
        || ctx.input(|input| {
            input.has_accesskit_action_request(response.id, egui::accesskit::Action::Click)
        })
}

/// Button-style activation without a primary pointer click. This covers
/// AccessKit Click plus egui's Enter/Space fake-click on a focused row.
fn row_activated_without_pointer(ctx: &egui::Context, response: &egui::Response) -> bool {
    (!response.clicked_by(egui::PointerButton::Primary) && response.clicked())
        || ctx.input(|input| {
            input.has_accesskit_action_request(response.id, egui::accesskit::Action::Click)
        })
}

/// Filtered indices a Shift-click selects: from the cursor row to the
/// clicked row, both inclusive. Cursors count the ".." row as 0, so a
/// cursor on ".." starts the range at the first file row.
fn click_range(cursor: usize, clicked: usize) -> std::ops::RangeInclusive<usize> {
    let anchor = cursor.max(1);
    let clicked = clicked.max(1);
    anchor.min(clicked) - 1..=anchor.max(clicked) - 1
}

/// Whether a dragged set may be dropped into the folder `target`. A folder
/// cannot take itself or one of its ancestors, so it must not light up as
/// a target while the overlay says the move is impossible.
fn accepts_row_drop(target: &std::path::Path, dragged: &[PathBuf]) -> bool {
    !dragged
        .iter()
        .any(|source| crate::fs_util::is_within_or_equal(target, source))
}

/// Whether keyboard focus should move to the cursor row, so assistive tech
/// follows keyboard cursor moves. Only after the cursor moved
/// (`cursor_moved`, the pending scroll) in the active, enabled pane, while
/// no text is being edited, and only when focus is not held elsewhere:
/// nothing has it, or a row of this list does (`focus_in_list`).
const fn follow_cursor_focus(
    cursor_moved: bool,
    is_active: bool,
    enabled: bool,
    text_editing: bool,
    focus_in_list: bool,
) -> bool {
    cursor_moved && is_active && enabled && !text_editing && focus_in_list
}

/// Temp-memory key of a pending "give the cursor row focus back" request,
/// holding the last pass on which it may still be honored.
const REFOCUS_CURSOR_ROW: &str = "file_list_refocus_cursor_row";
/// Passes a refocus request stays pending: the closing modal's focused
/// widget is only dropped by egui at the end of the pass after it vanished.
const REFOCUS_PASSES: u64 = 3;

/// Ask the active pane to put keyboard focus back on its cursor row, e.g.
/// after a modal closed and took the focused widget with it.
pub(super) fn request_cursor_row_refocus(ctx: &egui::Context) {
    let deadline = ctx.cumulative_pass_nr() + REFOCUS_PASSES;
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(REFOCUS_CURSOR_ROW), deadline));
    ctx.request_repaint();
}

#[derive(Debug, PartialEq, Eq)]
enum Refocus {
    /// Focus the cursor row now.
    Now,
    /// Not yet possible (focus still held elsewhere, pane disabled); retry.
    Wait,
    /// Drop the request: none pending, or its window passed.
    Drop,
}

/// What to do with a pending refocus request on pass `pass`, given whether
/// the pane may take focus now (`eligible`: active, enabled, no text
/// editing, and focus free or already on this list).
const fn refocus_decision(deadline: Option<u64>, pass: u64, eligible: bool) -> Refocus {
    match deadline {
        Some(deadline) if pass <= deadline => {
            if eligible {
                Refocus::Now
            } else {
                Refocus::Wait
            }
        }
        _ => Refocus::Drop,
    }
}

/// Per-pane id of the list row that last held keyboard focus.
fn list_focus_key(panel_side: &str) -> egui::Id {
    egui::Id::new(("file_list_focus", panel_side))
}

/// Book-keeping for a row that holds or takes keyboard focus: remember it
/// as this list's, and keep egui's arrow-key focus navigation off it, since
/// the arrows move the list cursor (and focus follows the cursor).
fn claim_row_focus(ui: &egui::Ui, panel_side: &str, response: &egui::Response) {
    ui.ctx()
        .data_mut(|data| data.insert_temp(list_focus_key(panel_side), response.id));
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        );
    });
}

/// Move keyboard focus to the cursor row and cancel egui's own arrow-key
/// focus move this frame, which would step one row past it.
fn focus_cursor_row(ui: &egui::Ui, panel_side: &str, response: &egui::Response) {
    response.request_focus();
    ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    claim_row_focus(ui, panel_side, response);
}

/// Footer item count with the folder size. Under a filter the size is
/// still the whole folder's, so it is labelled as such rather than read as
/// the size of the matching rows.
fn count_summary(
    shown: usize,
    total: usize,
    filters_active: bool,
    size: Option<u64>,
    partial: bool,
) -> String {
    // A total that still misses unmeasured folders is a lower bound.
    let size = match size {
        None => "\u{2026}".to_string(),
        Some(bytes) if partial => format!("\u{2265} {}\u{2026}", format_size(bytes)),
        Some(bytes) => format_size(bytes),
    };
    if filters_active {
        format!(
            "{shown} of {} (folder {size})",
            super::confirm_dialog::items_label(total)
        )
    } else {
        format!("{} ({size})", super::confirm_dialog::items_label(shown))
    }
}

/// The footer's filter note: the facet count, or "Filter text" when only
/// the search query filters the list.
fn filter_footer(active_facets: usize) -> String {
    if active_facets > 0 {
        format!("  |  Filters {active_facets}")
    } else {
        "  |  Filter text".to_string()
    }
}

/// The footer's selection note; names how many selected rows the filter
/// still shows when it hides some of them.
fn selection_footer(selected: usize, visible: usize, size: &str) -> String {
    if visible == selected {
        format!("  |  {selected} selected ({size})")
    } else {
        format!("  |  {selected} selected ({visible} visible, {size})")
    }
}

/// A name cut to the footer's fixed budget, keeping its extension.
fn footer_name(name: &str) -> std::borrow::Cow<'_, str> {
    crate::display_name::truncate_preserving_extension(name, 16)
}

/// The footer's folder extremes: the short text, and the full names for
/// its hover text. `None` when there is nothing to show.
fn folder_extremes(largest: Option<(&str, u64)>, oldest: Option<&str>) -> Option<(String, String)> {
    let largest = largest.filter(|&(_, size)| size > 0);
    let mut short = String::new();
    let mut full = Vec::new();
    if let Some((name, size)) = largest {
        let size = format_size(size);
        short.push_str(&format!("  |  largest {} ({size})", footer_name(name)));
        full.push(format!("Largest: {name} ({size})"));
    }
    if let Some(name) = oldest {
        short.push_str(&format!("  |  oldest {}", footer_name(name)));
        full.push(format!("Oldest: {name}"));
    }
    (!short.is_empty()).then(|| (short, full.join("\n")))
}

/// Spoken label of the ".." row. Like file rows it says "State: cursor"
/// while the keyboard cursor sits on it.
fn parent_row_label(is_cursor: bool) -> &'static str {
    if is_cursor {
        "Parent folder; State: cursor"
    } else {
        "Parent folder"
    }
}

/// The keyboard cursor's non-color cue: a 1px accent outline plus a
/// leading chevron. `chevron` is off where the row's own leading glyph
/// would collide with it.
fn paint_cursor_marker(painter: &egui::Painter, rect: egui::Rect, t: &ThemeColors, chevron: bool) {
    painter.rect_stroke(
        rect.shrink(1.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, t.accent),
        egui::StrokeKind::Inside,
    );
    if chevron {
        painter.text(
            egui::pos2(rect.left() + 7.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            "\u{203a}",
            egui::FontId::proportional(13.0),
            t.accent,
        );
    }
}

/// Keyboard focus ring: a 2px outline with an inner halo, so it reads on
/// both the cursor fill and the plain row background.
fn paint_focus_ring(painter: &egui::Painter, rect: egui::Rect, t: &ThemeColors) {
    painter.rect_stroke(
        rect.shrink(2.0),
        CornerRadius::ZERO,
        Stroke::new(2.0, t.text_primary),
        egui::StrokeKind::Inside,
    );
    painter.rect_stroke(
        rect.shrink(4.0),
        CornerRadius::ZERO,
        Stroke::new(1.0, t.bg_panel),
        egui::StrokeKind::Inside,
    );
}

fn cursor_row_to_reveal(cursor: usize, reveal: bool) -> Option<usize> {
    (reveal && cursor > 0).then(|| cursor - 1)
}

/// Name column budget in characters. File names are proportional text, so
/// the budget uses a measured average glyph width, not one em per glyph.
fn name_char_budget(name_width: f32, avg_glyph_width: f32) -> usize {
    if avg_glyph_width <= 0.0 {
        return 1;
    }
    ((name_width / avg_glyph_width).floor() as usize).max(1)
}

/// Map character ranges matched in `name` onto `display`, the output of
/// `truncate_preserving_extension` (`head + "…" + tail`). Matches inside the
/// elided middle land on the ellipsis.
fn map_ranges_to_display(
    name: &str,
    display: &str,
    ranges: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let name_len = name.chars().count();
    let display_len = display.chars().count();
    if display_len >= name_len {
        return ranges.to_vec();
    }
    if display_len == 0 {
        return Vec::new();
    }
    let head = name
        .chars()
        .map(crate::display_name::visible_char)
        .zip(display.chars())
        .take_while(|(a, b)| a == b)
        .count()
        .min(display_len - 1);
    let tail = display_len - 1 - head;
    let tail_start = name_len - tail;
    let map = |idx: usize| {
        if idx < head {
            idx
        } else if idx >= tail_start {
            idx - tail_start + head + 1
        } else {
            head
        }
    };
    let mut mapped: Vec<(usize, usize)> = Vec::new();
    for &(start, end) in ranges {
        if start >= end {
            continue;
        }
        let (start, end) = (map(start), map(end - 1) + 1);
        match mapped.last_mut() {
            Some(last) if last.1 >= start => last.1 = last.1.max(end),
            _ => mapped.push((start, end)),
        }
    }
    mapped
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_file_list(
        ui: &mut egui::Ui,
        panel: &mut PanelState,
        image_cache: &mut crate::image_cache::ImageCache,
        is_active: bool,
        t: &ThemeColors,
        panel_side: &str,
        size_bars: bool,
        compare: Option<&crate::compare::CompareMap>,
        opener: &dyn Fn(crate::ports::OpenRequest),
        dragging: bool,
        dragged: &[PathBuf],
        metrics: crate::density::DensityMetrics,
        reduced_motion: bool,
    ) -> Option<Result<ui_state::ContextMenuCandidate, String>> {
        // Only claim Shift+F10 when the cursor is on a file row; on ".." or an
        // empty list there is nothing for the menu to target.
        let keyboard_context_menu_requested = is_active
            && panel.cursor() > 0
            && ui.is_enabled()
            && crate::accessibility::text_input_state(ui.ctx())
                .mode()
                .is_none()
            && ui
                .ctx()
                .input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::F10));
        let mut context_menu_request = None;
        // Keyboard focus follows the cursor only while it is unclaimed or
        // already on one of this list's rows.
        let focus_in_list = {
            let focused = ui.memory(|memory| memory.focused());
            let ours = ui
                .ctx()
                .data(|data| data.get_temp::<egui::Id>(list_focus_key(panel_side)));
            focused.is_none() || focused == ours
        };
        let refocus_cursor = is_active && {
            let key = egui::Id::new(REFOCUS_CURSOR_ROW);
            let deadline = ui.ctx().data(|data| data.get_temp::<u64>(key));
            let eligible = follow_cursor_focus(
                true,
                is_active,
                ui.is_enabled(),
                crate::accessibility::text_input_state(ui.ctx())
                    .mode()
                    .is_some(),
                focus_in_list,
            );
            match refocus_decision(deadline, ui.ctx().cumulative_pass_nr(), eligible) {
                Refocus::Now => {
                    ui.ctx().data_mut(|data| data.remove::<u64>(key));
                    true
                }
                Refocus::Wait => {
                    ui.ctx().request_repaint();
                    false
                }
                Refocus::Drop => {
                    if deadline.is_some() {
                        ui.ctx().data_mut(|data| data.remove::<u64>(key));
                    }
                    false
                }
            }
        };
        let follow_focus = |ui: &egui::Ui, cursor_moved: bool| {
            follow_cursor_focus(
                cursor_moved,
                is_active,
                ui.is_enabled(),
                crate::accessibility::text_input_state(ui.ctx())
                    .mode()
                    .is_some(),
                focus_in_list,
            )
        };
        // Reserve the footer first so the scroll area fills only what is left;
        // drawn after it, the footer fell below the panel and was clipped.
        egui::Panel::bottom(egui::Id::new(("file_list_status", panel_side)))
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::NONE)
            .show(ui, |ui| Self::render_status_bar(ui, panel, t, compare));
        let mut scroll_area = egui::ScrollArea::vertical()
            .id_salt(("file_list", panel_side))
            .auto_shrink([false; 2]);
        // A newly entered folder must not inherit the previous folder's
        // pixel offset.
        if folder_changed(ui.ctx(), panel_side, &panel.current_path) {
            scroll_area = scroll_area.vertical_scroll_offset(0.0);
        }
        scroll_area
            .animated(!reduced_motion)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;

                // ".." row — go up one directory (cursor == 0)
                let can_go_up = panel.current_path.parent().is_some();
                if !can_go_up && panel.cursor() == 0 && panel.filtered_count() > 0 {
                    // No ".." row at the root: keep the cursor on a visible row.
                    panel.set_cursor(1);
                }
                if can_go_up {
                    let is_cursor_on_up = panel.cursor() == 0;
                    let up_bg = if is_cursor_on_up && is_active {
                        t.bg_selected.linear_multiply(0.25)
                    } else {
                        t.bg_card.linear_multiply(0.3)
                    };
                    let up_frame = Frame::NONE
                        .fill(up_bg)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::ZERO)
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                crate::app::glyphs::parent_up(ui, t.accent);
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new("..")
                                        .size(13.0)
                                        .strong()
                                        .color(t.text_primary),
                                );
                            });
                        })
                        .response;
                    // A widget of its own: re-sensing the frame's response
                    // reuses the frame's id, and the frame (hover only)
                    // surrenders keyboard focus every frame, so ".." could
                    // never keep focus.
                    let up_row = ui.interact(
                        up_frame.rect,
                        egui::Id::new(("file_list_parent_row", panel_side)),
                        Sense::click(),
                    );
                    // Same semantics as file rows, so AT hears when the
                    // cursor sits on the parent row.
                    up_row.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            ui.is_enabled(),
                            false,
                            parent_row_label(is_cursor_on_up),
                        )
                    });
                    ui.ctx().accesskit_node_builder(up_row.id, |node| {
                        node.set_role(egui::accesskit::Role::Row);
                        node.set_row_index(0);
                    });
                    if is_cursor_on_up {
                        if is_active && ui.is_enabled() {
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new("current_focus_indicator"),
                                    up_row.rect,
                                );
                            });
                        }
                        paint_cursor_marker(ui.painter(), up_row.rect, t, true);
                    }
                    if up_row.has_focus() {
                        claim_row_focus(ui, panel_side, &up_row);
                        paint_focus_ring(ui.painter(), up_row.rect, t);
                    }

                    if is_cursor_on_up && panel.scroll_to_cursor() {
                        // Keyboard moved onto "..": reveal it and settle the
                        // request here, since no file row holds the cursor.
                        ui.scroll_to_rect(up_row.rect, None);
                        panel.set_scroll_to_cursor(false);
                        if follow_focus(ui, true) {
                            focus_cursor_row(ui, panel_side, &up_row);
                        }
                    } else if is_cursor_on_up && refocus_cursor {
                        focus_cursor_row(ui, panel_side, &up_row);
                    }
                    if up_row.double_clicked() || row_activated_without_pointer(ui.ctx(), &up_row) {
                        panel.go_up();
                    } else if row_clicked(ui.ctx(), &up_row) {
                        // Take focus like file rows do, so the old row's
                        // focus ring does not linger.
                        up_row.request_focus();
                        panel.set_cursor(0);
                    }
                    if up_row.hovered() && !is_cursor_on_up {
                        ui.painter().rect_filled(
                            up_row.rect,
                            CornerRadius::same(3),
                            t.bg_hover.linear_multiply(0.3),
                        );
                    }
                    // Dropping onto ".." moves into the parent folder. During
                    // a drag only the dragged row counts as hovered, so test
                    // the pointer position instead.
                    if dragging
                        && up_row.contains_pointer()
                        && let Some(parent) = panel.current_path.parent().map(PathBuf::from)
                        && accepts_row_drop(&parent, dragged)
                    {
                        ui.painter().rect_stroke(
                            up_row.rect,
                            CornerRadius::ZERO,
                            Stroke::new(2.0_f32, t.accent),
                            egui::StrokeKind::Inside,
                        );
                        panel.drag.set_drop_target(parent);
                    }
                }

                // Cached filtered view: indices into panel.entries.
                // No FileEntry is cloned per frame; row interactions
                // are recorded and applied after the loop, so the
                // loop body only borrows the panel immutably.
                let filtered = panel.filtered_indices();
                // Active filter query, for highlighting matched characters in
                // each visible row. Trimmed to match panel filtering semantics.
                let query = panel.search_query().trim().to_string();

                if filtered.is_empty() {
                    // Distinguish a filtered-to-nothing list, a truly empty
                    // folder, and an unreadable/vanished one.
                    let (glyph, message, action) = empty_state(
                        panel.dir_status(),
                        crate::panel::filter_is_active(panel.search_query(), &panel.facets()),
                    );
                    ui.add_space(40.0);
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.label(egui::RichText::new(glyph).size(28.0));
                        ui.add_space(6.0);
                        let status =
                            ui.label(egui::RichText::new(message).size(13.0).color(t.text_muted));
                        // Announced when it appears or changes, like other
                        // status lines.
                        ui.ctx().accesskit_node_builder(status.id, |node| {
                            node.set_role(egui::accesskit::Role::Status);
                            node.set_live(egui::accesskit::Live::Polite);
                        });
                        if let Some((label, kind)) = action {
                            ui.add_space(8.0);
                            if ui.button(label).clicked() {
                                match kind {
                                    EmptyAction::Reveal => {
                                        opener(crate::ports::OpenRequest::Reveal(
                                            panel.current_path.clone(),
                                        ))
                                    }
                                    EmptyAction::GoUp => panel.go_up(),
                                    EmptyAction::Retry => panel.refresh(),
                                    EmptyAction::ClearFilters => panel.clear_filters(),
                                }
                            }
                        }
                    });
                    return;
                }

                // The rows are virtualized, so AT learns the full row count
                // and each row's position from a grid node, not from the
                // handful of rows laid out this frame.
                let grid_columns = if panel.browser_mode() == crate::panel::BrowserMode::Icons {
                    icon_columns(ui.available_width())
                } else {
                    1
                };
                let grid_rows = filtered.len().div_ceil(grid_columns) + usize::from(can_go_up);
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(egui::accesskit::Role::Grid);
                    node.set_row_count(grid_rows);
                    node.set_column_count(grid_columns);
                });

                // Deferred row interactions (applied after the loop).
                let mut pending_cursor: Option<usize> = None;
                let mut navigate_to: Option<std::path::PathBuf> = None;
                let mut open_path: Option<std::path::PathBuf> = None;
                let mut drag_anchor: Option<std::path::PathBuf> = None;
                let mut pending_drop_target: Option<std::path::PathBuf> = None;
                let mut pending_toggle: Option<std::path::PathBuf> = None;
                let mut pending_range: Option<std::ops::RangeInclusive<usize>> = None;
                let mut scrolled = false;

                // One immutable metrics snapshot for the frame. Warm frames
                // reuse the same Arc; workers never expose mutable maps here.
                let size_snapshot = panel.size_snapshot();

                let cursor = panel.cursor();
                let scroll_pending = panel.scroll_to_cursor();
                // Row sizing follows the density tier. `row_content` is the
                // allocated row height; `row_h` adds the 1px item spacing so the
                // virtualization stride matches (Comfortable == 28 + 1 == 29,
                // the pre-density default).
                let icons = panel.browser_mode() == crate::panel::BrowserMode::Icons;
                let columns = if icons {
                    icon_columns(ui.available_width())
                } else {
                    1
                };
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new(("icon_columns", panel_side)), columns)
                });
                let row_content = if icons {
                    110.0 + metrics.name_pt
                } else {
                    row_content_height(metrics)
                };
                let row_h = row_content + 1.0;
                // The semantic kind stub picks the matching toned palette.
                let dark = t.is_dark();
                let total_rows = filtered.len();
                let viewport = ui.clip_rect();
                // Measured from where file rows start, i.e. below "..".
                let scroll_top = viewport.top() - ui.cursor().top();

                // Both panes share one drag flag and the same y range, so the
                // pointer must also be over this pane horizontally.
                if drop_target_writable(ui.ctx(), panel_side, &panel.current_path, dragging)
                    && let Some(pointer) = ui.ctx().input(|input| input.pointer.hover_pos())
                    && viewport.x_range().contains(pointer.x)
                {
                    let velocity = crate::operation_view::edge_autoscroll_velocity(
                        pointer.y,
                        viewport.top(),
                        viewport.bottom(),
                        44.0,
                        640.0,
                    );
                    if velocity != 0.0 {
                        let dt = ui
                            .ctx()
                            .input(|input| input.stable_dt)
                            .clamp(1.0 / 120.0, 1.0 / 20.0);
                        ui.scroll_with_delta(egui::vec2(0.0, velocity * dt));
                        ui.ctx().request_repaint();
                    }
                }

                // One "now" for the whole frame, so every visible row's
                // relative Modified date is measured from the same instant.
                let now = std::time::SystemTime::now();

                // Feed the visible-row count back to the core for PageUp/Down.
                panel
                    .set_page_rows(((viewport.height() / row_h).floor() as usize).max(1) * columns);

                // Largest entry size in the listing, used to scale occupancy
                // bars.
                let size_max: u64 = if size_bars {
                    panel.max_display_size(&filtered)
                } else {
                    0
                };

                // Which rows are visible. The cursor row is forced into the
                // window for keyboard scrolling and for a keyboard context
                // menu, which can only open on a laid-out row.
                let reveal_cursor = scroll_pending || keyboard_context_menu_requested;
                let (first_visible, last_visible) = visible_rows(
                    scroll_top,
                    viewport.height(),
                    row_h,
                    total_rows.div_ceil(columns),
                    cursor_row_to_reveal(cursor, reveal_cursor).map(|row| row / columns),
                );
                let first_visible = first_visible * columns;
                let last_visible = (last_visible * columns).min(total_rows);
                panel.set_scroll_anchor(first_visible);
                let avg_glyph_width = ui.ctx().fonts_mut(|fonts| {
                    fonts.glyph_width(&egui::FontId::proportional(metrics.name_pt), 'n')
                });

                // Space before visible rows
                if first_visible > 0 {
                    ui.allocate_space(Vec2::new(
                        ui.available_width(),
                        (first_visible / columns) as f32 * row_h,
                    ));
                }

                let grid_origin = ui.cursor().min;
                let cell_width = ui.available_width() / columns as f32;
                // Render only visible rows
                for (offset, &entry_idx) in filtered[first_visible..last_visible].iter().enumerate()
                {
                    let idx = first_visible + offset;
                    let entry = &panel.entries()[entry_idx];
                    let row_cursor = idx + 1;
                    let is_cursor = row_cursor == cursor;
                    let is_selected = panel.is_selected(&entry.path);

                    let zebra = if idx % 2 == 1 {
                        t.bg_card.linear_multiply(0.3)
                    } else {
                        Color32::TRANSPARENT
                    };

                    let bg = if is_cursor {
                        t.bg_selected
                            .linear_multiply(if is_active { 0.22 } else { 0.08 })
                    } else if is_selected {
                        t.accent_purple.linear_multiply(0.15)
                    } else {
                        zebra
                    };

                    let (row_rect, row_resp) = if icons {
                        let rect = egui::Rect::from_min_size(
                            grid_origin
                                + egui::vec2(
                                    (idx % columns) as f32 * cell_width,
                                    (offset / columns) as f32 * row_h,
                                ),
                            egui::vec2((cell_width - 4.0).max(1.0), row_content),
                        );
                        (
                            rect,
                            ui.interact(
                                rect,
                                egui::Id::new(("icon_entry", panel_side, &entry.path)),
                                Sense::click_and_drag(),
                            ),
                        )
                    } else {
                        ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), row_content),
                            Sense::click_and_drag(),
                        )
                    };
                    let row_resp = if let Some(map) = compare {
                        if let Some(hint) = crate::compare::compare_hint(entry, map) {
                            row_resp.on_hover_text(hint)
                        } else {
                            row_resp
                        }
                    } else {
                        row_resp
                    };
                    let modified_label = match entry.modified {
                        Some(modified) => crate::reldate::relative_date(modified, now),
                        None => entry.modified_display().to_string(),
                    };
                    let measured = if entry.is_dir {
                        size_snapshot
                            .size_of(&entry.path)
                            .map(crate::panel::format_size)
                    } else {
                        Some(entry.size_display().to_string())
                    };
                    let size_text = measured.as_deref().unwrap_or("\u{2026}");
                    let compare_status =
                        compare.map(|map| crate::compare::classify_entry(entry, map));
                    let item_count = if entry.is_dir {
                        size_snapshot.count_of(&entry.path)
                    } else {
                        None
                    };
                    let semantics = crate::accessibility::file_row_semantics(
                        &entry.name,
                        crate::selection_summary::kind_of(entry).label(),
                        measured.as_deref().unwrap_or(PENDING_SIZE_SPOKEN),
                        &modified_label,
                        is_selected,
                        is_cursor,
                        entry.is_dir,
                    );
                    crate::accessibility::publish_row_semantics(
                        ui,
                        &row_resp,
                        &semantics,
                        row_spoken_label(
                            &semantics.label,
                            item_count,
                            compare_status.map(|status| compare_marker(status).1),
                        ),
                        grid_row_index(idx / columns, can_go_up),
                    );
                    if icons {
                        ui.ctx().accesskit_node_builder(row_resp.id, |node| {
                            node.set_column_index(idx % columns);
                        });
                    }
                    let accesskit_context_menu_requested = ui.is_enabled()
                        && crate::accessibility::consume_show_context_menu(ui.ctx(), row_resp.id);
                    #[cfg(feature = "visual-qa")]
                    if panel_side == "left" && (is_cursor || idx == first_visible) {
                        crate::visual_qa::record_response(
                            ui.ctx(),
                            crate::visual_qa::ProbeId::LeftRow,
                            &row_resp,
                        );
                    }

                    // Scroll to cursor row when navigating with keyboard
                    if is_cursor && reveal_cursor {
                        ui.scroll_to_rect(row_rect, Some(Align::Center));
                        scrolled = true;
                    }
                    if is_cursor && (refocus_cursor || follow_focus(ui, scroll_pending)) {
                        focus_cursor_row(ui, panel_side, &row_resp);
                    }

                    // Paint background
                    let full_rect = if icons {
                        row_rect
                    } else {
                        egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), row_rect.y_range())
                    };
                    if is_cursor && is_active && ui.is_enabled() {
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("current_focus_indicator"), full_rect);
                        });
                    }
                    if bg != Color32::TRANSPARENT {
                        ui.painter().rect_filled(full_rect, CornerRadius::ZERO, bg);
                    }
                    if row_resp.hovered() && !is_cursor {
                        ui.painter().rect_filled(
                            full_rect,
                            CornerRadius::ZERO,
                            t.bg_hover.linear_multiply(0.3),
                        );
                    }
                    // Occupancy bar: width proportional to this entry's share
                    // of the largest entry, ramping to a warning tint when it
                    // dominates the directory.
                    if size_max > 0 {
                        let size = size_snapshot.display_size(entry);
                        if size > 0 {
                            let frac = (size as f32 / size_max as f32).clamp(0.0, 1.0);
                            let bar = egui::Rect::from_min_size(
                                full_rect.min,
                                Vec2::new(full_rect.width() * frac, full_rect.height()),
                            );
                            let tint = if frac > 0.66 {
                                t.accent_warning
                            } else {
                                t.accent
                            };
                            ui.painter().rect_filled(
                                bar,
                                CornerRadius::ZERO,
                                tint.linear_multiply(0.12),
                            );
                        }
                    }

                    // Compare mode: a left-edge stripe showing how this entry
                    // relates to the other panel.
                    if let Some(status) = compare_status {
                        use crate::compare::CompareStatus;
                        let stripe = match status {
                            CompareStatus::Unique => Some(t.accent),
                            CompareStatus::Differs => Some(t.accent_warning),
                            CompareStatus::TypeConflict => Some(t.accent_red),
                            CompareStatus::Identical | CompareStatus::DirectoryPair => None,
                        };
                        if let Some(color) = stripe {
                            let edge = egui::Rect::from_min_size(
                                full_rect.min,
                                Vec2::new(3.0, full_rect.height()),
                            );
                            ui.painter().rect_filled(edge, CornerRadius::ZERO, color);
                        }
                    } else {
                        // Semantic kind stub at the leading edge: a glanceable
                        // 3px accent by file kind (only when not comparing, so
                        // it never overlaps the compare stripe).
                        let (r, g, b) = crate::file_color::kind_color(
                            crate::selection_summary::kind_of(entry),
                            dark,
                        );
                        let stub = egui::Rect::from_min_size(
                            full_rect.min,
                            Vec2::new(3.0, full_rect.height()),
                        );
                        ui.painter().rect_filled(
                            stub,
                            CornerRadius {
                                nw: 0,
                                ne: 1,
                                sw: 0,
                                se: 1,
                            },
                            Color32::from_rgb(r, g, b),
                        );
                    }

                    // Cursor and focus go over the bar and the stub/stripe,
                    // which would otherwise cover their edges.
                    if is_cursor {
                        paint_cursor_marker(
                            ui.painter(),
                            full_rect,
                            t,
                            cursor_chevron_fits(is_selected),
                        );
                    }
                    if row_resp.has_focus() {
                        claim_row_focus(ui, panel_side, &row_resp);
                        paint_focus_ring(ui.painter(), full_rect, t);
                    }

                    // Content
                    let mut child_ui = ui.new_child(
                        egui::UiBuilder::new().max_rect(row_rect.shrink2(egui::vec2(6.0, 3.0))),
                    );
                    child_ui.style_mut().interaction.selectable_labels = false;
                    // The row node already carries the composed label; the
                    // painted glyphs and texts would be read a second time.
                    child_ui
                        .ctx()
                        .accesskit_node_builder(child_ui.unique_id(), |node| {
                            node.set_hidden();
                        });
                    if icons {
                        child_ui.with_layout(Layout::top_down(Align::Center), |ui| {
                            ui.add_space(8.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(64.0, 48.0),
                                Layout::top_down(Align::Center),
                                |ui| {
                                    if let Some(texture) =
                                        image_cache.get(&entry.path).filter(|_| entry.is_image())
                                    {
                                        ui.add(
                                            egui::Image::new(texture)
                                                .fit_to_exact_size(egui::vec2(64.0, 48.0)),
                                        );
                                    } else if entry.is_dir {
                                        Self::paint_folder_icon(ui, item_count, t.folder);
                                    } else {
                                        crate::app::glyphs::file_document(
                                            ui,
                                            &entry.extension,
                                            t.accent,
                                            t.bg_panel,
                                        );
                                    }
                                },
                            );
                            ui.add_space(8.0);
                            let name = crate::display_name::truncate_preserving_extension(
                                &entry.name,
                                name_char_budget(cell_width - 16.0, avg_glyph_width),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(name.as_ref())
                                        .size(metrics.name_pt)
                                        .color(t.text_primary),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&entry.name);
                            ui.label(
                                egui::RichText::new(size_text)
                                    .size(metrics.meta_pt)
                                    .color(t.text_muted),
                            );
                            if is_selected {
                                ui.label(
                                    egui::RichText::new("✓ Selected")
                                        .size(metrics.meta_pt)
                                        .color(t.accent_purple),
                                );
                            }
                            if let Some(status) = compare_status {
                                ui.label(
                                    egui::RichText::new(compare_marker(status).0)
                                        .size(metrics.meta_pt),
                                );
                            }
                        });
                    } else {
                        child_ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;

                            if is_selected {
                                ui.label(
                                    egui::RichText::new("\u{2713}")
                                        .size(metrics.meta_pt)
                                        .strong()
                                        .color(t.accent_purple),
                                );
                            } else {
                                ui.add_space(9.0);
                            }

                            if entry.is_dir {
                                Self::paint_folder_icon(ui, item_count, t.folder);
                            } else {
                                ui.add_space(3.0);
                                let (red, green, blue) = crate::file_color::kind_color(
                                    crate::selection_summary::kind_of(entry),
                                    dark,
                                );
                                crate::app::glyphs::file_document(
                                    ui,
                                    &entry.extension,
                                    Color32::from_rgb(red, green, blue),
                                    t.bg_panel,
                                );
                            }
                            ui.add_space(3.0);

                            let name_color = if is_selected {
                                t.accent_purple
                            } else if entry.is_dir {
                                t.text_primary
                            } else {
                                t.text_secondary
                            };
                            let responsive =
                                crate::accessibility::file_row_layout(ui.available_width());
                            let inline_compare =
                                compare_status.filter(|_| !responsive.show_metadata);
                            let name_width =
                                name_column_width(responsive, inline_compare.is_some());
                            let max_name_chars = name_char_budget(name_width, avg_glyph_width);
                            let display_name = crate::display_name::truncate_preserving_extension(
                                &entry.name,
                                max_name_chars,
                            );
                            let shortened = display_name.as_ref() != entry.name;
                            let name_label = if query.is_empty() {
                                egui::Label::new(
                                    egui::RichText::new(display_name.as_ref())
                                        .size(metrics.name_pt)
                                        .color(name_color),
                                )
                            } else {
                                // Score the full name (what the filter matched) and
                                // carry the hits through the truncation.
                                let ranges = crate::fuzzy::score(&query, &entry.name)
                                    .map(|m| m.matched_ranges)
                                    .unwrap_or_default();
                                let ranges = if shortened {
                                    map_ranges_to_display(&entry.name, &display_name, &ranges)
                                } else {
                                    ranges
                                };
                                egui::Label::new(highlight_name_job(
                                    display_name.as_ref(),
                                    &ranges,
                                    name_color,
                                    t.accent,
                                    metrics.name_pt,
                                ))
                            };
                            // `add_sized` would center the label in its slot; names
                            // read from the left edge of the column.
                            let name_response = ui
                                .allocate_ui_with_layout(
                                    egui::vec2(name_width, row_content),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.set_min_width(name_width);
                                        ui.add(name_label.truncate())
                                    },
                                )
                                .inner;
                            if shortened {
                                name_response.on_hover_text(&entry.name);
                            }
                            if let Some(status) = inline_compare {
                                let (symbol, hint) = compare_marker(status);
                                ui.label(
                                    egui::RichText::new(symbol)
                                        .size(metrics.meta_pt)
                                        .strong()
                                        .color(t.text_secondary),
                                )
                                .on_hover_text(hint);
                            }

                            if responsive.show_metadata {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(responsive.metadata_width, row_content),
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        ui.add_space(4.0);
                                        let modified_resp = ui.label(
                                            egui::RichText::new(&modified_label)
                                                .size(metrics.meta_pt)
                                                .color(t.text_muted),
                                        );
                                        if entry.modified.is_some() {
                                            modified_resp.on_hover_text(&entry.modified_str);
                                        }
                                        ui.add_space(10.0);
                                        ui.label(
                                            egui::RichText::new(size_text)
                                                .size(metrics.meta_pt)
                                                .color(t.text_muted),
                                        );
                                        if let Some(status) = compare_status {
                                            let (symbol, hint) = compare_marker(status);
                                            ui.label(
                                                egui::RichText::new(symbol)
                                                    .size(metrics.meta_pt)
                                                    .strong()
                                                    .color(t.text_secondary),
                                            )
                                            .on_hover_text(hint);
                                        }
                                    },
                                );
                            }
                        });
                    }

                    if (keyboard_context_menu_requested && is_cursor)
                        || accesskit_context_menu_requested
                    {
                        row_resp.request_focus();
                        pending_cursor = Some(row_cursor);
                        context_menu_request = Some(context_menu_candidate(
                            entry,
                            crate::ports::ContextMenuTrigger::Keyboard,
                            full_rect,
                            &row_resp,
                        ));
                    }

                    if row_resp.secondary_clicked() {
                        row_resp.request_focus();
                        pending_cursor = Some(row_cursor);
                        context_menu_request = Some(context_menu_candidate(
                            entry,
                            crate::ports::ContextMenuTrigger::Pointer,
                            full_rect,
                            &row_resp,
                        ));
                    }

                    if row_resp.double_clicked() {
                        pending_cursor = Some(row_cursor);
                        if entry.is_dir {
                            navigate_to = Some(entry.path.clone());
                        } else {
                            open_path = Some(entry.path.clone());
                        }
                    } else if row_clicked(ui.ctx(), &row_resp) {
                        row_resp.request_focus();
                        pending_cursor = Some(row_cursor);
                        // Modifiers only count for real pointer clicks; a
                        // Space/Enter on the focused row is not a Shift-click.
                        if row_resp.clicked_by(egui::PointerButton::Primary) {
                            match row_click_kind(ui.input(|input| input.modifiers)) {
                                RowClick::Plain => {}
                                RowClick::Toggle => pending_toggle = Some(entry.path.clone()),
                                RowClick::Range => {
                                    pending_range = Some(click_range(cursor, row_cursor));
                                }
                            }
                        }
                    }

                    // Drag start — selection resolved after the loop
                    if row_resp.drag_started_by(egui::PointerButton::Primary) {
                        pending_cursor = Some(row_cursor);
                        drag_anchor = Some(entry.path.clone());
                    }

                    // Drop target highlight — show when dragging over a
                    // directory that can take the dragged set. During a drag
                    // only the dragged row counts as hovered, so test the
                    // pointer position instead.
                    if row_resp.contains_pointer()
                        && dragging
                        && entry.is_dir
                        && accepts_row_drop(&entry.path, dragged)
                    {
                        pending_drop_target = Some(entry.path.clone());
                        ui.painter().rect_stroke(
                            full_rect,
                            CornerRadius::ZERO,
                            Stroke::new(2.0_f32, t.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                }

                if icons {
                    ui.allocate_space(Vec2::new(
                        ui.available_width(),
                        (last_visible - first_visible).div_ceil(columns) as f32 * row_h,
                    ));
                }
                // Space after visible rows
                let after = total_rows.saturating_sub(last_visible);
                if after > 0 {
                    ui.allocate_space(Vec2::new(
                        ui.available_width(),
                        after.div_ceil(columns) as f32 * row_h,
                    ));
                }

                // Apply the interactions recorded during the loop.
                if let Some(range) = pending_range {
                    let entries = panel.entries();
                    let paths: Vec<PathBuf> = filtered
                        .get(range)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|&index| entries.get(index))
                        .map(|entry| entry.path.clone())
                        .collect();
                    panel.extend_selection(paths);
                }
                if let Some(path) = pending_toggle {
                    panel.toggle_select(path);
                }
                if let Some(c) = pending_cursor {
                    panel.set_cursor(c);
                }
                if scrolled {
                    panel.set_scroll_to_cursor(false);
                }
                if let Some(anchor) = drag_anchor {
                    panel.begin_drag(anchor);
                }
                if let Some(target) = pending_drop_target {
                    panel.drag.set_drop_target(target);
                }
                if let Some(path) = open_path {
                    opener(crate::ports::OpenRequest::OpenPath(path));
                }
                if let Some(path) = navigate_to {
                    panel.navigate_to(path);
                }
            });

        context_menu_request
    }

    /// Per-panel footer: item count and size, filters, selection, folder
    /// extremes, compare selection chips, hidden-files state.
    fn render_status_bar(
        ui: &mut egui::Ui,
        panel: &mut PanelState,
        t: &ThemeColors,
        compare: Option<&crate::compare::CompareMap>,
    ) {
        Frame::NONE
            .fill(Color32::TRANSPARENT)
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // "Hidden" is laid out first so it keeps its width; the
                    // summary on the left gets what remains.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hidden_label = if panel.show_hidden() {
                            "Hidden: ON"
                        } else {
                            "Hidden: OFF"
                        };
                        ui.label(
                            egui::RichText::new(hidden_label)
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        ui.add_space(8.0);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            let shown = panel.filtered_count();
                            let total = panel.entries().len();
                            let selected_total = panel.selected_count();
                            let (selected_visible, selected_bytes, selected_pending) =
                                visible_selection_summary(panel);
                            // One pass for the folder total plus its largest/oldest entry.
                            let overview = panel.folder_overview();
                            let filters_active = crate::panel::filter_is_active(
                                panel.search_query(),
                                &panel.facets(),
                            );
                            // Footer texts truncate (full text on hover) rather
                            // than run past a narrow pane's edge.
                            let footer = |text: String, color: Color32| {
                                egui::Label::new(egui::RichText::new(text).size(11.0).color(color))
                                    .truncate()
                            };
                            ui.add(footer(
                                count_summary(
                                    shown,
                                    total,
                                    filters_active,
                                    overview.total,
                                    overview.partial,
                                ),
                                t.text_muted,
                            ));
                            if filters_active {
                                ui.add(footer(
                                    filter_footer(panel.facets().active_count()),
                                    t.accent,
                                ));
                            }
                            if selected_total > 0 {
                                let selected_size =
                                    selection_size_label(selected_bytes, selected_pending);
                                let selection = ui.add(footer(
                                    selection_footer(
                                        selected_total,
                                        selected_visible,
                                        &selected_size,
                                    ),
                                    t.accent_purple,
                                ));
                                // Spoken when Space/Insert changes the count.
                                ui.ctx().accesskit_node_builder(selection.id, |node| {
                                    node.set_role(egui::accesskit::Role::Status);
                                    node.set_live(egui::accesskit::Live::Polite);
                                });
                            }

                            // Folder largest/oldest: the always-on complement to the
                            // selection HUD (which shows the same for the selection).
                            // Hidden once a selection is active, so the two don't clash.
                            if selected_total == 0
                                && total >= 2
                                && let Some((extra, full)) = folder_extremes(
                                    overview
                                        .largest
                                        .as_ref()
                                        .map(|(name, size)| (name.as_str(), *size)),
                                    overview.oldest.as_ref().map(|(name, _)| name.as_str()),
                                )
                            {
                                // The names are cut short; hover shows them whole.
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(extra).size(11.0).color(t.text_muted),
                                    )
                                    .truncate()
                                    .show_tooltip_when_elided(false),
                                )
                                .on_hover_text(full);
                            }

                            // Compare mode: chips to turn the diff into a selection.
                            if let Some(map) = compare {
                                use crate::compare::CompareCriterion;
                                ui.label(
                                    egui::RichText::new("  |  Select:")
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                                for (label, crit) in [
                                    ("Newer", CompareCriterion::Newer),
                                    ("Differing", CompareCriterion::Differing),
                                    ("Unique", CompareCriterion::Unique),
                                ] {
                                    let chip = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(label).size(11.0).color(t.accent),
                                        )
                                        .sense(Sense::click()),
                                    );
                                    // A clickable label: tell AT it is a button.
                                    chip.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            ui.is_enabled(),
                                            format!("Select {label}"),
                                        )
                                    });
                                    let clicked = chip.clicked();
                                    if clicked {
                                        panel.replace_selection(crate::compare::select_by_compare(
                                            panel
                                                .filtered_indices()
                                                .iter()
                                                .filter_map(|&i| panel.entries().get(i)),
                                            map,
                                            crit,
                                        ));
                                    }
                                }
                            }
                        });
                    });
                });
            });
    }

    pub(crate) fn paint_folder_icon(ui: &mut egui::Ui, count: Option<usize>, color: Color32) {
        let size = Vec2::new(24.0, 18.0);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        let x = rect.left();
        let y = rect.top();
        let w = size.x;
        let h = size.y;

        let s = Stroke::new(1.2_f32, color);

        let tab_w = w * 0.35;
        let tab_h = 3.5;
        let body_top = tab_h;

        // Tab (outline only)
        let tab = [
            egui::pos2(x + 0.5, y + body_top),
            egui::pos2(x + 0.5, y + 0.5),
            egui::pos2(x + tab_w, y + 0.5),
            egui::pos2(x + tab_w + 2.5, y + body_top),
        ];
        for i in 0..tab.len() - 1 {
            p.line_segment([tab[i], tab[i + 1]], s);
        }

        // Body (outline only)
        let body_rect = egui::Rect::from_min_size(
            egui::pos2(x + 0.5, y + body_top),
            egui::vec2(w - 1.0, h - body_top - 0.5),
        );
        p.rect_stroke(body_rect, CornerRadius::ZERO, s, egui::StrokeKind::Outside);

        if let Some(c) = count {
            if c >= 999 {
                // Many items — fill the body
                p.rect_filled(body_rect, CornerRadius::ZERO, color.linear_multiply(0.3));
            } else if c > 0 {
                p.text(
                    body_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", c),
                    egui::FontId::proportional(8.5),
                    color,
                );
            }
        }
    }
}

/// Build a file name as a [`LayoutJob`], tinting the characters the fuzzy
/// filter matched (`ranges`, char indices into `name`) in `accent`, and
/// underlining them, so the user sees why the row survived the filter.
fn highlight_name_job(
    name: &str,
    ranges: &[(usize, usize)],
    base: Color32,
    accent: Color32,
    size: f32,
) -> egui::text::LayoutJob {
    use egui::text::TextFormat;
    let font = egui::FontId::proportional(size);
    let mut job = egui::text::LayoutJob::default();
    // Matches are underlined too, so they do not rely on hue alone.
    append_highlighted(
        &mut job,
        name,
        ranges,
        &TextFormat::simple(font.clone(), base),
        &TextFormat {
            font_id: font,
            color: accent,
            underline: egui::Stroke::new(1.0, accent),
            ..Default::default()
        },
    );
    job
}

/// Append `text` to `job` in `hit` where a char index falls in one of the
/// half-open fuzzy-match `ranges` and in `plain` elsewhere. Neighbouring
/// chars of the same kind share one section, so a long name does not become
/// one section per char.
pub(super) fn append_highlighted(
    job: &mut egui::text::LayoutJob,
    text: &str,
    ranges: &[(usize, usize)],
    plain: &egui::text::TextFormat,
    hit: &egui::text::TextFormat,
) {
    let is_hit = |idx: usize| ranges.iter().any(|&(s, e)| idx >= s && idx < e);
    let mut run: Option<(usize, bool)> = None;
    for (idx, (byte, _)) in text.char_indices().enumerate() {
        let here = is_hit(idx);
        match run {
            Some((_, kind)) if kind == here => {}
            Some((start, kind)) => {
                job.append(
                    &text[start..byte],
                    0.0,
                    if kind { hit } else { plain }.clone(),
                );
                run = Some((byte, here));
            }
            None => run = Some((byte, here)),
        }
    }
    if let Some((start, kind)) = run {
        job.append(&text[start..], 0.0, if kind { hit } else { plain }.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EmptyAction, INLINE_COMPARE_WIDTH, PENDING_SIZE_SPOKEN, Refocus, RowClick,
        accepts_row_drop, append_highlighted, click_range, compare_marker,
        context_menu_unavailable_reason, count_summary, cursor_chevron_fits, cursor_row_to_reveal,
        drop_target_writable, empty_state, filter_footer, folder_changed, folder_extremes,
        follow_cursor_focus, grid_row_index, highlight_name_job, map_ranges_to_display,
        name_char_budget, name_column_width, parent_row_label, refocus_decision, row_click_kind,
        row_content_height, row_spoken_label, selection_footer, selection_size_label,
        selection_summary_with, visible_rows, visible_selection_summary,
    };

    #[test]
    fn modal_close_refocus_waits_for_free_focus_then_expires() {
        assert_eq!(refocus_decision(None, 5, true), Refocus::Drop);
        assert_eq!(refocus_decision(Some(8), 5, false), Refocus::Wait);
        assert_eq!(refocus_decision(Some(8), 8, true), Refocus::Now);
        assert_eq!(refocus_decision(Some(8), 9, true), Refocus::Drop);
        assert_eq!(refocus_decision(Some(8), 9, false), Refocus::Drop);
    }

    #[test]
    fn rows_without_a_captured_identity_explain_the_missing_menu() {
        let failed = crate::panel::ListingIdentity::CaptureFailed(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Denied,
            message: "permission denied".to_string(),
        });
        let reason = context_menu_unavailable_reason(&failed).unwrap();
        assert!(reason.starts_with("Menu unavailable"), "{reason}");
        assert!(reason.contains("permission denied"), "{reason}");
        assert!(
            context_menu_unavailable_reason(&crate::panel::ListingIdentity::Unavailable).is_some()
        );
    }

    #[test]
    fn degenerate_viewport_never_inverts_the_row_window() {
        // Negative height while scrolled: previously (34, 1), a slice panic.
        let (first, last) = visible_rows(1000.0, -2000.0, 29.0, 100, None);
        assert!(first <= last);
        let (first, last) = visible_rows(1000.0, f32::NAN, 29.0, 100, None);
        assert!(first <= last);
        let (first, last) = visible_rows(f32::INFINITY, 290.0, 29.0, 100, None);
        assert!(first <= last && last <= 100);
        let (first, last) = visible_rows(0.0, f32::INFINITY, 29.0, 100, Some(usize::MAX));
        assert!(first <= last && last <= 100);
    }

    #[test]
    fn drop_target_is_probed_once_per_drag() {
        let ctx = egui::Context::default();
        let temp = crate::testutil::TempDir::new();
        let dir = temp.path().join("target");
        std::fs::create_dir(&dir).expect("create dir");
        assert!(drop_target_writable(&ctx, "left", &dir, true));
        std::fs::remove_dir(&dir).expect("remove dir");
        // Same drag: the cached answer is reused without touching the disk.
        assert!(drop_target_writable(&ctx, "left", &dir, true));
        // The drag ended; the next one probes again.
        assert!(!drop_target_writable(&ctx, "left", &dir, false));
        assert!(!drop_target_writable(&ctx, "left", &dir, true));
    }

    #[test]
    fn selection_summary_counts_only_visible_selected_rows() {
        let temp = crate::testutil::TempDir::new();
        std::fs::write(temp.path().join("a.txt"), "aaa").expect("write a");
        std::fs::write(temp.path().join("b.log"), "bbbbb").expect("write b");
        std::fs::write(temp.path().join("c.txt"), "c").expect("write c");
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        assert_eq!(visible_selection_summary(&panel), (0, 0, false));
        panel.toggle_select(temp.path().join("a.txt"));
        panel.toggle_select(temp.path().join("b.log"));
        assert_eq!(visible_selection_summary(&panel), (2, 8, false));
        assert_eq!(panel.visible_selected_count(), 2);
        assert_eq!(panel.visible_selected_size(), 8);
        panel.set_search_query("txt");
        assert_eq!(visible_selection_summary(&panel), (1, 3, false));
        assert_eq!(panel.visible_selected_count(), 1);
    }

    #[test]
    fn unmeasured_selected_folder_marks_the_size_as_a_lower_bound() {
        let temp = crate::testutil::TempDir::new();
        std::fs::write(temp.path().join("a.txt"), "aaa").expect("write a");
        std::fs::create_dir(temp.path().join("sub")).expect("mkdir");
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.toggle_select(temp.path().join("a.txt"));
        panel.toggle_select(temp.path().join("sub"));
        // No folder measured yet.
        let unmeasured = crate::panel::SizeSnapshot::default();
        assert_eq!(selection_summary_with(&panel, &unmeasured), (2, 3, true));
        assert_eq!(selection_size_label(3, true), "\u{2265} 3 B\u{2026}");
        assert_eq!(selection_size_label(0, true), "\u{2026}");
        assert_eq!(selection_size_label(3, false), "3 B");
    }

    #[cfg(unix)]
    #[test]
    fn selected_folder_symlink_is_not_left_measuring() {
        let temp = crate::testutil::TempDir::new();
        std::fs::write(temp.path().join("a.txt"), "aaa").expect("write a");
        std::fs::create_dir(temp.path().join("real")).expect("mkdir");
        std::os::unix::fs::symlink("real", temp.path().join("link")).expect("symlink");
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.toggle_select(temp.path().join("a.txt"));
        panel.toggle_select(temp.path().join("link"));
        let unmeasured = crate::panel::SizeSnapshot::default();
        // The link is never walked, so the total is final, not pending.
        assert_eq!(selection_summary_with(&panel, &unmeasured), (2, 3, false));
    }

    #[test]
    fn entering_another_folder_is_detected_once_per_pane() {
        let ctx = egui::Context::default();
        let a = std::path::Path::new("/a");
        let b = std::path::Path::new("/b");
        assert!(folder_changed(&ctx, "left", a));
        assert!(!folder_changed(&ctx, "left", a));
        assert!(folder_changed(&ctx, "right", a));
        assert!(folder_changed(&ctx, "left", b));
        assert!(!folder_changed(&ctx, "left", b));
        assert!(!folder_changed(&ctx, "right", a));
    }

    #[test]
    fn row_window_is_measured_below_the_parent_row() {
        // ".." still in view: start at the first file row.
        assert_eq!(visible_rows(-26.0, 290.0, 29.0, 100, None), (0, 11));
        // Scrolled exactly ten rows past the header.
        assert_eq!(visible_rows(290.0, 290.0, 29.0, 100, None).0, 10);
        // Partially visible row above stays in the window.
        assert_eq!(visible_rows(300.0, 290.0, 29.0, 100, None).0, 10);
    }

    #[test]
    fn revealed_row_is_pulled_into_the_window() {
        let (first, last) = visible_rows(2900.0, 290.0, 29.0, 200, Some(3));
        assert_eq!(first, 3);
        assert!(last > 3);
        let (first, last) = visible_rows(0.0, 290.0, 29.0, 200, Some(150));
        assert_eq!(last, 151);
        assert!(first <= 150);
        assert_eq!(visible_rows(0.0, 290.0, 29.0, 0, Some(5)), (0, 0));
    }

    #[test]
    fn parent_row_cursor_never_forces_the_file_window() {
        assert_eq!(cursor_row_to_reveal(0, true), None);
        assert_eq!(cursor_row_to_reveal(1, true), Some(0));
        assert_eq!(cursor_row_to_reveal(4, false), None);
    }

    #[test]
    fn name_budget_uses_average_glyph_width() {
        // A 300px column at ~6.5px per glyph fits ~46 chars, not 300/13 = 23.
        assert_eq!(name_char_budget(300.0, 6.5), 46);
        assert_eq!(name_char_budget(5.0, 6.5), 1);
        assert_eq!(name_char_budget(300.0, 0.0), 1);
    }

    #[test]
    fn matches_are_mapped_through_the_ellipsis() {
        let name = "abcdefghij.txt";
        let display = crate::display_name::truncate_preserving_extension(name, 8);
        assert_eq!(display, "abc\u{2026}.txt");
        // "a" stays, "f" (elided) lands on the ellipsis, "t" maps into the suffix.
        assert_eq!(
            map_ranges_to_display(name, &display, &[(0, 1), (5, 6), (11, 12)]),
            vec![(0, 1), (3, 4), (5, 6)]
        );
        // Untruncated names keep their ranges.
        assert_eq!(
            map_ranges_to_display("a.txt", "a.txt", &[(0, 1)]),
            vec![(0, 1)]
        );
        assert!(map_ranges_to_display(name, "", &[(0, 1)]).is_empty());
    }

    #[test]
    fn parent_row_announces_the_cursor() {
        assert_eq!(parent_row_label(false), "Parent folder");
        assert_eq!(parent_row_label(true), "Parent folder; State: cursor");
    }

    #[test]
    fn unlistable_folder_wins_over_the_filter_empty_state() {
        use crate::panel::DirStatus;
        // A remembered filter must not turn these into "No matches".
        for status in [
            DirStatus::Denied,
            DirStatus::Unavailable,
            DirStatus::Gone,
            DirStatus::Loading,
        ] {
            assert_ne!(empty_state(status, true).1, "No matches", "{status:?}");
            assert_eq!(empty_state(status, true), empty_state(status, false));
        }
        assert_eq!(empty_state(DirStatus::Listed, true).1, "No matches");
        assert_eq!(
            empty_state(DirStatus::Empty, true).2,
            Some(("Clear filters", EmptyAction::ClearFilters))
        );
        assert_eq!(empty_state(DirStatus::Empty, false).1, "Empty");
    }

    #[test]
    fn empty_state_text_is_renderable_and_platform_neutral() {
        use crate::panel::DirStatus;
        // No bundled font maps U+FE0F; it rendered as a replacement box.
        assert!(!empty_state(DirStatus::Gone, false).0.contains('\u{fe0f}'));
        let (_, _, action) = empty_state(DirStatus::Denied, false);
        let (label, kind) = action.expect("denied offers a reveal");
        assert_eq!(kind, EmptyAction::Reveal);
        if !cfg!(target_os = "macos") {
            assert!(!label.contains("Finder"), "{label}");
        }
    }

    #[test]
    fn folder_of_only_hidden_items_does_not_read_as_empty() {
        use crate::panel::DirStatus;
        let (_, message, _) = empty_state(DirStatus::HiddenOnly(3), false);
        assert!(message.starts_with("3 hidden items"), "{message}");
        assert_ne!(message, "Empty");
        let (_, one, _) = empty_state(DirStatus::HiddenOnly(1), false);
        assert!(one.starts_with("1 hidden item;"), "{one}");
        // A filter still explains its own empty result.
        assert_eq!(empty_state(DirStatus::HiddenOnly(3), true).1, "No matches");
    }

    #[test]
    fn unavailable_folder_offers_a_retry_not_a_permission_message() {
        use crate::panel::DirStatus;
        let (glyph, message, action) = empty_state(DirStatus::Unavailable, false);
        assert!(!glyph.contains('\u{fe0f}'));
        assert!(!message.contains("permission"), "{message}");
        assert_eq!(action, Some(("Retry", EmptyAction::Retry)));
    }

    #[test]
    fn row_label_speaks_what_the_row_only_paints() {
        use crate::compare::CompareStatus;
        let base = crate::accessibility::file_row_semantics(
            "docs",
            "Folder",
            PENDING_SIZE_SPOKEN,
            "today",
            false,
            false,
            true,
        )
        .label;
        assert!(base.contains("Size: calculating"), "{base}");
        assert!(!PENDING_SIZE_SPOKEN.contains('\u{2026}'));
        let label = row_spoken_label(
            &base,
            Some(12),
            Some(compare_marker(CompareStatus::Differs).1),
        );
        assert!(label.ends_with("; Items: 12; Compare: Differs"), "{label}");
        assert_eq!(row_spoken_label(&base, None, None), base);
        assert_eq!(
            compare_marker(CompareStatus::Unique),
            ("+", "Only in this pane")
        );
    }

    #[test]
    fn grid_rows_count_the_parent_row_first() {
        assert_eq!(grid_row_index(0, true), 1);
        assert_eq!(grid_row_index(0, false), 0);
        assert_eq!(grid_row_index(41, true), 42);
    }

    #[test]
    fn compact_rows_keep_the_minimum_control_height() {
        use crate::density::{Density, metrics};
        assert_eq!(
            row_content_height(metrics(Density::Compact)),
            crate::accessibility::MIN_CONTROL_POINTS
        );
        // Roomier tiers keep their own height (Comfortable is the old 28).
        assert_eq!(row_content_height(metrics(Density::Comfortable)), 28.0);
    }

    #[test]
    fn selected_cursor_row_drops_the_chevron_for_the_check_mark() {
        assert!(cursor_chevron_fits(false));
        assert!(!cursor_chevron_fits(true));
    }

    #[test]
    fn narrow_rows_keep_room_for_the_compare_symbol() {
        let narrow = crate::accessibility::file_row_layout(120.0);
        assert!(!narrow.show_metadata);
        assert_eq!(
            name_column_width(narrow, true),
            narrow.name_width - INLINE_COMPARE_WIDTH
        );
        assert_eq!(name_column_width(narrow, false), narrow.name_width);
        let wide = crate::accessibility::file_row_layout(400.0);
        assert_eq!(name_column_width(wide, true), wide.name_width);
    }

    #[test]
    fn filter_matches_are_underlined_not_only_tinted() {
        let base = egui::Color32::GRAY;
        let accent = egui::Color32::RED;
        let job = highlight_name_job("abc", &[(1, 2)], base, accent, 13.0);
        let underlines: Vec<bool> = job
            .sections
            .iter()
            .map(|section| section.format.underline != egui::Stroke::NONE)
            .collect();
        assert_eq!(underlines, vec![false, true, false]);
    }

    fn runs(job: &egui::text::LayoutJob) -> Vec<(&str, bool)> {
        job.sections
            .iter()
            .map(|section| {
                (
                    &job.text[section.byte_range.start.0..section.byte_range.end.0],
                    section.format.color == egui::Color32::RED,
                )
            })
            .collect()
    }

    #[test]
    fn highlight_ranges_index_chars_not_bytes() {
        let (base, accent) = (egui::Color32::GRAY, egui::Color32::RED);
        let job = highlight_name_job("é日本x.txt", &[(1, 3)], base, accent, 13.0);
        assert_eq!(
            runs(&job),
            vec![("é", false), ("日本", true), ("x.txt", false)]
        );
        let job = highlight_name_job("a😀b", &[(1, 2)], base, accent, 13.0);
        assert_eq!(runs(&job), vec![("a", false), ("😀", true), ("b", false)]);
        // A combining accent is its own char: the range covers it alone.
        let job = highlight_name_job("e\u{301}x", &[(1, 2)], base, accent, 13.0);
        assert_eq!(
            runs(&job),
            vec![("e", false), ("\u{301}", true), ("x", false)]
        );
    }

    #[test]
    fn highlight_merges_runs_and_falls_back_to_plain() {
        let (base, accent) = (egui::Color32::GRAY, egui::Color32::RED);
        let job = highlight_name_job("report.pdf", &[], base, accent, 13.0);
        assert_eq!(runs(&job), vec![("report.pdf", false)]);
        // Adjacent ranges read as one match; ranges past the end are ignored.
        let job = highlight_name_job("abcd", &[(0, 1), (1, 2), (9, 12)], base, accent, 13.0);
        assert_eq!(runs(&job), vec![("ab", true), ("cd", false)]);
        let mut empty = egui::text::LayoutJob::default();
        let format = egui::text::TextFormat::default();
        append_highlighted(&mut empty, "", &[(0, 1)], &format, &format);
        assert!(empty.sections.is_empty() && empty.text.is_empty());
    }

    #[test]
    fn click_modifiers_pick_the_selection_gesture() {
        let mut modifiers = egui::Modifiers::NONE;
        assert_eq!(row_click_kind(modifiers), RowClick::Plain);
        modifiers.command = true;
        assert_eq!(row_click_kind(modifiers), RowClick::Toggle);
        modifiers.shift = true;
        assert_eq!(row_click_kind(modifiers), RowClick::Range);
    }

    #[test]
    fn shift_click_range_spans_cursor_to_clicked_row() {
        // Cursor rows are 1-based (0 is ".."); ranges are filtered indices.
        assert_eq!(click_range(2, 5), 1..=4);
        assert_eq!(click_range(5, 2), 1..=4);
        assert_eq!(click_range(3, 3), 2..=2);
        // From "..", the range starts at the first file row.
        assert_eq!(click_range(0, 3), 0..=2);
    }

    #[test]
    fn dragged_folder_is_not_its_own_drop_target() {
        let temp = crate::testutil::TempDir::new();
        let folder = temp.path().join("folder");
        let inner = folder.join("inner");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&inner).expect("mkdir inner");
        std::fs::create_dir(&other).expect("mkdir other");
        let dragged = vec![folder.clone()];
        assert!(!accepts_row_drop(&folder, &dragged));
        assert!(!accepts_row_drop(&inner, &dragged));
        assert!(accepts_row_drop(&other, &dragged));
        assert!(accepts_row_drop(&folder, &[]));
    }

    #[test]
    fn focus_follows_keyboard_cursor_moves_only() {
        // Cursor moved in the active pane, focus free or on this list.
        assert!(follow_cursor_focus(true, true, true, false, true));
        // No cursor move: focus stays where it is.
        assert!(!follow_cursor_focus(false, true, true, false, true));
        // Inactive or disabled pane, a text field being edited, or focus
        // held by another widget (a dialog button): never steal it.
        assert!(!follow_cursor_focus(true, false, true, false, true));
        assert!(!follow_cursor_focus(true, true, false, false, true));
        assert!(!follow_cursor_focus(true, true, true, true, true));
        assert!(!follow_cursor_focus(true, true, true, false, false));
    }

    #[test]
    fn footer_count_is_singular_for_one_item() {
        assert_eq!(count_summary(1, 1, false, Some(3), false), "1 item (3 B)");
        assert_eq!(
            count_summary(2, 2, false, None, false),
            "2 items (\u{2026})"
        );
        assert_eq!(
            count_summary(0, 1, true, Some(3), false),
            "0 of 1 item (folder 3 B)"
        );
    }

    #[test]
    fn partial_folder_total_reads_as_a_lower_bound() {
        assert_eq!(
            count_summary(2, 2, false, Some(3), true),
            "2 items (\u{2265} 3 B\u{2026})"
        );
    }

    #[test]
    fn filtered_footer_marks_the_size_as_the_folder_total() {
        assert_eq!(
            count_summary(12, 400, true, Some(2048), false),
            format!(
                "12 of 400 items (folder {})",
                crate::panel::format_size(2048)
            )
        );
    }

    #[test]
    fn footer_notes_name_filters_and_hidden_selected_rows() {
        assert_eq!(filter_footer(0), "  |  Filter text");
        assert_eq!(filter_footer(2), "  |  Filters 2");
        assert_eq!(selection_footer(3, 3, "1 KB"), "  |  3 selected (1 KB)");
        assert_eq!(
            selection_footer(3, 1, "1 KB"),
            "  |  3 selected (1 visible, 1 KB)"
        );
        // Every selected row filtered away still says so.
        assert_eq!(
            selection_footer(2, 0, "\u{2026}"),
            "  |  2 selected (0 visible, \u{2026})"
        );
    }

    #[test]
    fn folder_extremes_keep_the_full_names_for_hover() {
        let long = "a-very-long-file-name-indeed.bin";
        let (short, full) =
            folder_extremes(Some((long, 10)), Some("old.log")).expect("something to show");
        // The cut keeps the extension, which says what kind of file it is.
        assert!(
            short.contains("largest a-very-long\u{2026}.bin (10 B)"),
            "{short}"
        );
        assert!(short.ends_with("  |  oldest old.log"), "{short}");
        assert_eq!(full, format!("Largest: {long} (10 B)\nOldest: old.log"));
        // A zero-byte "largest" is not worth showing.
        assert_eq!(folder_extremes(Some(("x", 0)), None), None);
    }

    /// One frame of the list in a headless context, with `events` as input.
    fn render_list_frame(
        ctx: &egui::Context,
        panel: &mut crate::panel::PanelState,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            crate::app::App::render_file_list(
                ui,
                panel,
                &mut crate::image_cache::ImageCache::new(),
                true,
                &crate::theme::ThemeColors::dark(),
                "left",
                false,
                None,
                &|_| {},
                false,
                &[],
                crate::density::metrics(crate::density::Density::default()),
                true,
            );
        });
        crate::testutil::discard_egui_output(output);
    }

    fn key_press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn space_on_a_focused_row_does_not_pull_the_cursor_back() {
        let temp = crate::testutil::TempDir::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(temp.path().join(name), name).expect("write file");
        }
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        let ctx = egui::Context::default();

        for start in [1] {
            // A keyboard move onto `start`: focus follows the cursor there.
            panel.set_cursor(start);
            panel.set_scroll_to_cursor(true);
            render_list_frame(&ctx, &mut panel, Vec::new());
            render_list_frame(&ctx, &mut panel, Vec::new());
            assert!(ctx.memory(|memory| memory.focused().is_some()));

            // Space/Enter, as the key handler sees them: it already moved the
            // cursor on before the list renders the same frame's input.
            for key in [egui::Key::Space, egui::Key::Enter] {
                panel.set_cursor(start + 1);
                render_list_frame(&ctx, &mut panel, vec![key_press(key)]);
                assert_eq!(panel.cursor(), start + 1, "{key:?} from row {start}");
                panel.set_cursor(start);
                render_list_frame(&ctx, &mut panel, Vec::new());
            }
        }
    }

    #[test]
    fn keyboard_activation_of_the_parent_row_goes_up() {
        for key in [egui::Key::Enter, egui::Key::Space] {
            let temp = crate::testutil::TempDir::new();
            let child = temp.path().join("child");
            std::fs::create_dir(&child).expect("create child");
            let mut panel = crate::panel::PanelState::new(child.clone());
            panel.refresh();
            panel.set_cursor(0);
            panel.set_scroll_to_cursor(true);
            let ctx = egui::Context::default();
            render_list_frame(&ctx, &mut panel, Vec::new());
            render_list_frame(&ctx, &mut panel, Vec::new());

            render_list_frame(&ctx, &mut panel, vec![key_press(key)]);

            assert_eq!(panel.current_path, temp.path(), "{key:?}");
        }
    }

    #[test]
    fn a_refocus_request_puts_focus_back_on_the_cursor_row() {
        let temp = crate::testutil::TempDir::new();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(temp.path().join(name), name).expect("write file");
        }
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.set_cursor(2);
        let ctx = egui::Context::default();
        render_list_frame(&ctx, &mut panel, Vec::new());
        render_list_frame(&ctx, &mut panel, Vec::new());
        assert_eq!(ctx.memory(|memory| memory.focused()), None);

        super::request_cursor_row_refocus(&ctx);
        render_list_frame(&ctx, &mut panel, Vec::new());
        render_list_frame(&ctx, &mut panel, Vec::new());
        let focused = ctx.memory(|memory| memory.focused());
        assert!(focused.is_some());
        let ours = ctx.data(|data| data.get_temp::<egui::Id>(super::list_focus_key("left")));
        assert_eq!(focused, ours, "focus is on this list's row");
        assert_eq!(panel.cursor(), 2, "refocusing does not move the cursor");
    }

    #[test]
    fn matches_stay_aligned_when_controls_are_shown_as_symbols() {
        let name = "a\tbcdefghij.txt";
        let display = crate::display_name::truncate_preserving_extension(name, 8);
        assert_eq!(display, "a\u{2409}b\u{2026}.txt");
        // "b" (after the tab) keeps its own column instead of the ellipsis.
        assert_eq!(
            map_ranges_to_display(name, &display, &[(2, 3)]),
            vec![(2, 3)]
        );
    }
    #[test]
    fn icon_grid_keeps_keyboard_focus_and_reveals_distant_cursor() {
        let temp = crate::testutil::TempDir::new();
        for i in 0..150 {
            temp.file(&format!("file-{i:03}.txt"), "text");
        }
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.set_browser_mode(crate::panel::BrowserMode::Icons);
        panel.set_cursor(130);
        panel.set_scroll_to_cursor(true);
        let ctx = egui::Context::default();
        render_list_frame(&ctx, &mut panel, Vec::new());
        render_list_frame(&ctx, &mut panel, Vec::new());
        assert!(panel.scroll_anchor() <= 129);
        assert!(ctx.memory(|memory| memory.focused().is_some()));
        assert!(!panel.scroll_to_cursor());
        let columns = ctx
            .data(|data| data.get_temp::<usize>(egui::Id::new(("icon_columns", "left"))))
            .unwrap();
        assert!(columns > 1);
        panel.set_cursor(131);
        render_list_frame(&ctx, &mut panel, vec![key_press(egui::Key::Space)]);
        assert_eq!(panel.cursor(), 131);
    }
}
