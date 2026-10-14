//! Confirmation dialog for pending copy/move/delete operations.

use super::*;

pub mod method_tabs;
use crate::scan::{FlatFileEntry, FlatList};
use crate::transfer::OverwritePolicy;
use std::collections::HashSet;
use std::sync::Arc;

/// What the confirm dialog derives from the pending operation, kept across
/// frames: the flow animation repaints continuously, and re-cloning the scan,
/// re-reading conflict metadata or re-probing the volume each frame is waste.
#[derive(Default)]
pub struct ConfirmCache {
    /// Finished scan, its file total, and the list it was read from (kept
    /// alive so `Arc::ptr_eq` cannot match a reused allocation).
    flat: Option<(FlatList, Arc<Vec<FlatFileEntry>>, u64)>,
    /// Conflict detail, the staged transfer it was read for, and when.
    conflicts: Option<(
        ConflictKey,
        std::time::Instant,
        Arc<Vec<crate::conflict::Conflict>>,
    )>,
    profile: Option<(PathBuf, Arc<crate::volume_profile::VolumeProfile>)>,
}

/// How long conflict detail (size/mtime on both sides) is shown before it is
/// re-read, so destination files changed while the dialog is open show up.
const CONFLICT_DETAIL_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// Identity of the staged transfer that conflict detail was read for. The
/// `FlatList` is created per staged transfer, so it tells a replaced or
/// restaged operation apart even when target, names and counts match; it is
/// kept alive so `Arc::ptr_eq` cannot match a reused allocation.
struct ConflictKey {
    flat: FlatList,
    target: PathBuf,
    names: Vec<String>,
    entries: usize,
}

impl ConflictKey {
    fn matches(
        &self,
        flat: &FlatList,
        target: &std::path::Path,
        names: &[String],
        entries: usize,
    ) -> bool {
        Arc::ptr_eq(&self.flat, flat)
            && self.target == target
            && self.names == names
            && self.entries == entries
    }
}

impl ConfirmCache {
    /// The finished scan behind `source` and its file total, or `None` while
    /// it is still running. A scan publishes exactly once per `FlatList`, so
    /// it is copied out of the lock only on the first frame it is seen.
    fn flat(&mut self, source: &FlatList) -> Option<(Arc<Vec<FlatFileEntry>>, u64)> {
        if let Some((cached_source, flat, total)) = &self.flat
            && Arc::ptr_eq(cached_source, source)
        {
            return Some((flat.clone(), *total));
        }
        let flat = Arc::new(crate::lock_util::recover(source).clone()?);
        let total = flat_total(&flat);
        self.flat = Some((source.clone(), flat.clone(), total));
        Some((flat, total))
    }

    /// Per-conflict detail, re-read from disk when the staged transfer
    /// changes (a resolve, restage or replacement) or after
    /// `CONFLICT_DETAIL_TTL`.
    fn conflicts(&mut self, ws: &Workspace) -> Arc<Vec<crate::conflict::Conflict>> {
        let Some(PendingOp::Transfer(tr)) = &ws.pending_op else {
            return Arc::default();
        };
        self.conflicts_for(
            &tr.flat,
            &tr.target,
            &tr.conflicts,
            tr.entries.len(),
            std::time::Instant::now(),
            || ws.pending_conflicts(),
        )
    }

    fn conflicts_for(
        &mut self,
        flat: &FlatList,
        target: &std::path::Path,
        names: &[String],
        entries: usize,
        now: std::time::Instant,
        load: impl FnOnce() -> Vec<crate::conflict::Conflict>,
    ) -> Arc<Vec<crate::conflict::Conflict>> {
        if let Some((key, read_at, cached)) = &self.conflicts
            && key.matches(flat, target, names, entries)
            && now
                .checked_duration_since(*read_at)
                .is_some_and(|age| age < CONFLICT_DETAIL_TTL)
        {
            return cached.clone();
        }
        let detail = Arc::new(load());
        let key = ConflictKey {
            flat: flat.clone(),
            target: target.to_path_buf(),
            names: names.to_vec(),
            entries,
        };
        self.conflicts = Some((key, now, detail.clone()));
        detail
    }

    /// Volume profile of `target`, probed once per dialog rather than per
    /// frame (each probe stats the path, which can stall on network mounts).
    fn profile(&mut self, target: &std::path::Path) -> Arc<crate::volume_profile::VolumeProfile> {
        if let Some((cached_target, profile)) = &self.profile
            && cached_target == target
        {
            return profile.clone();
        }
        let profile = Arc::new(crate::volume_profile::profile(target));
        self.profile = Some((target.to_path_buf(), profile.clone()));
        profile
    }
}

/// Total logical size of the files in a preview. Saturates: sparse files can
/// report sizes whose sum exceeds `u64`.
fn flat_total(flat: &[FlatFileEntry]) -> u64 {
    flat.iter()
        .filter(|f| !f.is_dir)
        .fold(0, |total, f| total.saturating_add(f.size))
}

/// The copy/move flow animation moves one file every 80 ms but always
/// finishes within `FLOW_MAX_SECS`, so long lists stop repainting quickly.
const FLOW_SECS_PER_FILE: f64 = 0.08;
const FLOW_MAX_SECS: f64 = 1.5;

/// Rows already shown as transferred `elapsed` seconds into the animation.
/// With reduced motion every row is shown at once and nothing animates.
fn flow_rows(elapsed: f64, len: usize, reduced_motion: bool) -> usize {
    if reduced_motion {
        len
    } else {
        flow_transferred(elapsed, len)
    }
}

fn flow_transferred(elapsed: f64, len: usize) -> usize {
    let per_file = (elapsed / FLOW_SECS_PER_FILE) as usize;
    let capped = (len as f64 * (elapsed / FLOW_MAX_SECS)) as usize;
    per_file.max(capped).min(len)
}

fn unavailable_space_message(failure: Option<&crate::ports::NativeFailure>) -> String {
    match failure {
        Some(failure) => format!(
            "Available space could not be verified: {}. You can continue.",
            failure.message
        ),
        None => "Available space could not be verified. You can continue.".to_string(),
    }
}

/// Per-dialog egui temp state; it must not outlive the pending operation it
/// was created for, however that operation ends (confirm, auto-start,
/// dismiss), or the next dialog inherits a stale animation start / dry run.
const PENDING_FLOW_START: &str = "pending_flow_start";
const CONFIRM_DRY_RUN: &str = "confirm_dry_run";

fn clear_pending_dialog_state(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove::<f64>(egui::Id::new(PENDING_FLOW_START));
        d.remove::<bool>(egui::Id::new(CONFIRM_DRY_RUN));
        d.remove::<usize>(egui::Id::new(CONFLICT_DROPPED));
        d.remove::<bool>(egui::Id::new(CONFIRM_FOCUSED));
    });
}

/// True when keyboard focus sits on a widget drawn in `layer`. egui turns
/// Enter on a focused widget into a click (toggling a checkbox, opening a
/// combo), so the dialog-wide Enter shortcut must not fire on top of it.
fn focus_within_layer(ctx: &egui::Context, layer: egui::LayerId) -> bool {
    ctx.memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
        .is_some_and(|response| response.layer_id == layer)
}

/// The dialog-wide Enter must yield to whatever Enter already acted on: a
/// focused widget in the dialog itself, or a combo/menu popup (its items live
/// in their own foreground layer, and Enter there picks the item).
fn dialog_enter_suppressed(ctx: &egui::Context, layer: egui::LayerId) -> bool {
    let focus_in_popup = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
        .is_some_and(|response| {
            response.layer_id != layer && response.layer_id.order == egui::Order::Foreground
        });
    focus_within_layer(ctx, layer) || focus_in_popup || egui::Popup::is_any_open(ctx)
}

/// Set once the primary button took keyboard focus for this dialog, so the
/// sheet is announced on open but later focus moves are left alone.
const CONFIRM_FOCUSED: &str = "confirm_primary_focused";

/// Focus the primary button the first frame it can take focus, unless the
/// user already moved focus into the dialog (e.g. while the scan ran).
const fn should_focus_primary(already_focused: bool, enabled: bool, focus_in_dialog: bool) -> bool {
    !already_focused && enabled && !focus_in_dialog
}

/// Whether a click on an auto-focused confirm button should run. egui
/// clicks a focused button on Space as well as Enter, and Space is a
/// habitual file-manager key (select, Quick Look), so a Space press on the
/// focused button is ignored; Enter and the pointer still confirm.
pub(super) const fn honours_click(clicked: bool, space_pressed: bool, has_focus: bool) -> bool {
    clicked && !(space_pressed && has_focus)
}

/// [`honours_click`] for a live button response.
pub(super) fn confirm_clicked(ui: &egui::Ui, response: &egui::Response) -> bool {
    honours_click(
        response.clicked(),
        ui.input(|i| i.key_pressed(egui::Key::Space)),
        response.has_focus(),
    )
}

/// Why the primary button is disabled, for its hover text; the first
/// failing gate wins. `None` when it is enabled.
fn primary_blocker(
    flat_ready: bool,
    resource_ready: bool,
    overflow: bool,
    preflight_blocked: bool,
    mutation_block: Option<String>,
    dry_run: bool,
) -> Option<String> {
    if !flat_ready {
        Some("Still scanning the selected items".to_string())
    } else if !resource_ready {
        Some("Waiting for the size and free-space check".to_string())
    } else if overflow {
        Some("Not enough free space at the destination".to_string())
    } else if preflight_blocked {
        Some("The destination does not allow writing".to_string())
    } else if dry_run {
        None
    } else {
        mutation_block
    }
}

/// Entries a conflict button already dropped from the pending transfer
/// (Skip Existing, Keep Newer, ...). Returning to Ask cannot bring them back.
const CONFLICT_DROPPED: &str = "confirm_conflict_dropped";

/// Note shown beside the collision combo once a conflict policy removed
/// items; `None` while Ask can still restore every choice.
fn dropped_items_note(dropped: usize) -> Option<String> {
    match dropped {
        0 => None,
        1 => Some("1 conflicting item was already skipped".to_string()),
        n => Some(format!("{n} conflicting items were already skipped")),
    }
}

/// Dry run is offered for Delete/Move, but never while conflict-policy
/// buttons are shown: those buttons start the real transfer.
fn dry_run_supported(is_delete: bool, title: &str, has_conflicts: bool) -> bool {
    (is_delete || title == "Move") && !has_conflicts
}

/// Conflict-policy buttons confirm the transfer, so they share the primary
/// button's mutation gates (destination Write capability, read-only mode).
fn conflict_policy_enabled(preflight_blocked: bool, mutations_blocked: bool) -> bool {
    !preflight_blocked && !mutations_blocked
}

/// Collision choice that matches the transfer's effective overwrite policy.
/// `None` means the policy (Overwrite All) has no collision-combo entry.
fn collision_for_policy(
    policy: OverwritePolicy,
) -> Option<crate::filesystem_policy::CollisionPolicy> {
    use crate::filesystem_policy::CollisionPolicy;
    match policy {
        OverwritePolicy::Ask => Some(CollisionPolicy::Ask),
        OverwritePolicy::KeepBoth => Some(CollisionPolicy::KeepBoth),
        OverwritePolicy::SkipAll => Some(CollisionPolicy::Skip),
        OverwritePolicy::OverwriteAll => None,
    }
}

/// Toast for a conflict policy that left nothing to transfer (Skip Existing
/// when every item already exists), so the dialog does not just vanish.
fn nothing_left_message(title: &str, dropped: usize) -> String {
    let verb = title.to_lowercase();
    match dropped {
        1 => format!("Nothing to {verb}: the item already exists at the destination"),
        n => format!("Nothing to {verb}: all {n} items already exist at the destination"),
    }
}

/// Why Move to Trash did not start although the dialog allowed it: a
/// transfer or another mutation began while the sheet was open.
fn delete_not_started_message(block_reason: Option<String>) -> String {
    block_reason.unwrap_or_else(|| {
        "Wait for the transfer queue to finish before moving items to Trash".to_string()
    })
}

fn unavailable_size_message(failure: &crate::ports::NativeFailure) -> String {
    format!(
        "Transfer size could not be verified: {}. Confirmation is unavailable; check that \
         the files are readable, then cancel and start the operation again.",
        failure.message
    )
}

/// "1 item" / "3 items".
fn items_label(count: usize) -> String {
    if count == 1 {
        "1 item".to_string()
    } else {
        format!("{count} items")
    }
}

/// Dialog heading. Delete says what actually happens: items go to the
/// Trash and can be restored.
fn dialog_heading(title: &str, is_delete: bool, count: usize) -> String {
    let action = if is_delete { "Move to Trash" } else { title };
    format!("{action} — {}", items_label(count))
}

/// Source column header: the shared parent folder, or a folder count when
/// the picked items come from several folders (selected in another tab),
/// with the full list for the hover text.
fn source_header<'a>(
    parents: impl IntoIterator<Item = Option<&'a std::path::Path>>,
) -> (String, Option<String>) {
    let mut folders: Vec<&std::path::Path> = Vec::new();
    for parent in parents.into_iter().flatten() {
        if !folders.contains(&parent) {
            folders.push(parent);
        }
    }
    match folders.as_slice() {
        [] => (String::new(), None),
        [only] => (only.display().to_string(), None),
        many => (
            format!("{} folders", many.len()),
            Some(
                many.iter()
                    .map(|folder| folder.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ),
    }
}

fn not_enough_space_message(need: u64, free: u64) -> String {
    format!(
        "Not enough space: needs {}, only {} free ({} short)",
        format_size(need),
        format_size(free),
        format_size(need.saturating_sub(free))
    )
}

fn conflict_banner(count: usize) -> String {
    if count == 1 {
        "1 item already exists at the destination".to_string()
    } else {
        format!("{count} items already exist at the destination")
    }
}

/// One conflict row: the incoming (source) item against the existing
/// destination item, marking whichever is newer.
fn conflict_row_text(conflict: &crate::conflict::Conflict) -> String {
    let newer = |is_newer: bool| if is_newer { " (newer)" } else { "" };
    format!(
        "{}    Incoming {}{}  \u{2192}  Existing {}{}",
        conflict.name,
        format_size(conflict.src_size),
        newer(conflict.src_newer),
        format_size(conflict.dst_size),
        newer(conflict.dst_newer),
    )
}

/// Normalization warning: how many names the chosen form rewrites and how
/// many of the results collide.
fn normalization_note(changes: usize, collisions: usize) -> Option<String> {
    let mut parts = Vec::new();
    match changes {
        0 => {}
        1 => parts.push("1 name will be normalized".to_string()),
        n => parts.push(format!("{n} names will be normalized")),
    }
    match collisions {
        0 => {}
        1 => parts.push("1 collision".to_string()),
        n => parts.push(format!("{n} collisions")),
    }
    (!parts.is_empty()).then(|| parts.join("  \u{00b7}  "))
}

impl App {
    pub(crate) fn show_confirm_dialog(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let escape_requested =
            self.take_modal_escape(crate::accessibility::ModalSurface::Confirmation);
        let mutations_blocked = self.ws.mutations_blocked();
        let mutation_block = self.ws.mutation_block_reason("confirming this operation");
        let reduced_motion = self.accessibility_preferences.reduced_motion;
        let Some(op) = &self.ws.pending_op else {
            clear_pending_dialog_state(ctx);
            self.confirm_cache = ConfirmCache::default();
            return;
        };
        let _latency =
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::OperationDialog);

        // Snapshot display data so `self` stays free for the button handlers.
        let (title, action_label, action_color, count, target, source_dir, conflicts, flat_arc) =
            match op {
                PendingOp::Transfer(tr) => {
                    let (title, color) = match tr.kind {
                        TransferKind::Copy => ("Copy", t.accent),
                        TransferKind::Move => ("Move", t.accent_warning),
                    };
                    let source_dir = source_header(tr.entries.iter().map(|e| e.path.parent()));
                    (
                        title,
                        title,
                        color,
                        tr.entries.len(),
                        Some(tr.target.clone()),
                        source_dir,
                        tr.conflicts.clone(),
                        tr.flat.clone(),
                    )
                }
                PendingOp::Delete { entries, flat, .. } => (
                    "Delete",
                    "Move to Trash",
                    t.accent_red,
                    entries.len(),
                    None,
                    (String::new(), None),
                    vec![],
                    flat.clone(),
                ),
            };

        // One generation-bound resource snapshot. Pending disables confirmation;
        // Unknown is non-blocking only after the worker has published the full
        // logical size and is always presented as an explicit warning.
        let fit = match &self.ws.pending_op {
            Some(PendingOp::Transfer(tr)) => Some((
                tr.space_ready(),
                tr.overflows(),
                tr.need_bytes(),
                tr.free_space().cloned(),
                tr.needs_no_space(),
                tr.space_failure().cloned(),
            )),
            _ => None,
        };
        let resource_ready = fit.as_ref().is_none_or(|fit| fit.0);
        let overflow = fit.as_ref().is_some_and(|fit| fit.1);
        let preflight_blocked = matches!(
            &self.ws.pending_op,
            Some(PendingOp::Transfer(transfer))
                if transfer.filesystem.capabilities.state(crate::filesystem_policy::Capability::Write)
                    == crate::filesystem_policy::CapabilityState::Unavailable
        );

        let flat_opt = self.confirm_cache.flat(&flat_arc);
        let flat_ready = flat_opt.is_some();
        let (flat, total) = flat_opt.unwrap_or_default();

        let has_conflicts = !conflicts.is_empty()
            && matches!(
                &self.ws.pending_op,
                Some(PendingOp::Transfer(transfer)) if transfer.policy == OverwritePolicy::Ask
            );
        // Rich per-conflict detail (size/mtime each side) for the resolver.
        let rich_conflicts = if has_conflicts {
            self.confirm_cache.conflicts(&self.ws)
        } else {
            Arc::default()
        };
        let conflict_names: HashSet<&str> = conflicts.iter().map(String::as_str).collect();
        let is_delete = target.is_none();
        let win_title = dialog_heading(title, is_delete, count);
        let screen = ctx.input(|i| i.viewport_rect());
        let pad = 120.0;
        let avail_w = (screen.width() - pad * 2.0).max(300.0);
        let avail_h = (screen.height() - pad * 2.0).max(200.0);
        let win_w = 1000.0f32.min(avail_w);
        let win_h = 800.0f32.min(avail_h);

        let dialog_response = egui::Window::new(win_title.as_str())
            .collapsible(false)
            .resizable(false)
            .fixed_size(Vec2::new(win_w, win_h))
            .title_bar(false)
            .frame(super::dialog_frame(t, 6))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // Title + tabs on one line
                if is_delete {
                    ui.label(
                        egui::RichText::new(&win_title)
                            .size(14.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.add_space(6.0);
                } else {
                    self.method_tabs_row(ui, &t, title, count);
                }
                self.durability_row(ui, &t);
                self.filesystem_policy_row(ui, &t);
                self.resource_policy_row(ui, &t);
                ui.add_space(8.0);

                if !flat_ready {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Scanning files...")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                    });
                    ctx.request_repaint();
                } else if is_delete {
                    // ── Delete: single list (virtualized) ──
                    let list_h = (ui.available_height() - 80.0).max(60.0);
                    Self::render_flat_list(
                        ui,
                        &flat,
                        &HashSet::new(),
                        &t,
                        list_h,
                        "pending_op_files",
                        FlatListMode::Preview,
                    );
                } else {
                    // ── Copy/Move: two columns (source → destination) ──
                    let target_path = target.as_ref().unwrap();

                    // Headers
                    ui.columns(2, |cols| {
                        let (source_text, source_folders) = &source_dir;
                        let source = cols[0].label(
                            egui::RichText::new(format!("Source: {source_text}"))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        if let Some(folders) = source_folders {
                            source.on_hover_text(folders);
                        }
                        cols[1].label(
                            egui::RichText::new(format!("Destination: {}", target_path.display()))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                    ui.add_space(4.0);

                    // Animated flow: files gradually transfer from left to right
                    let anim_id = egui::Id::new(PENDING_FLOW_START);
                    let start_time: f64 = ctx.data_mut(|d| {
                        *d.get_temp_mut_or_insert_with(anim_id, || ctx.input(|i| i.time))
                    });
                    let elapsed = ctx.input(|i| i.time) - start_time;
                    let transferred = flow_rows(elapsed, flat.len(), reduced_motion);

                    // Request repaint while animation is running
                    if transferred < flat.len() {
                        ctx.request_repaint();
                    }

                    ui.columns(2, |cols| {
                        let list_h = (cols[0].available_height() - 80.0).max(60.0);

                        // Left: source tree — transferred files dimmed+strikethrough, rest normal
                        Self::render_flat_list(
                            &mut cols[0],
                            &flat,
                            &conflict_names,
                            &t,
                            list_h,
                            "pending_src",
                            FlatListMode::Source { transferred },
                        );

                        // Right: only transferred files shown, highlighted
                        let list_h = (cols[1].available_height() - 80.0).max(60.0);
                        Self::render_flat_list(
                            &mut cols[1],
                            &flat,
                            &conflict_names,
                            &t,
                            list_h,
                            "pending_dst",
                            FlatListMode::Destination { transferred },
                        );
                    });
                }

                // Total size (from flat list, files only)
                if total > 0 {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("Total: {}", format_size(total)))
                            .size(11.0)
                            .color(t.text_muted),
                    );
                }

                // Will-it-fit guard.
                if let Some((ready, over, need, free, no_extra_space, size_failure)) = &fit {
                    ui.add_space(2.0);
                    let (msg, color) = if let Some(failure) = size_failure {
                        (unavailable_size_message(failure), t.accent_red)
                    } else if !ready {
                        (
                            "Calculating size and available space...".to_string(),
                            t.text_muted,
                        )
                    } else if *no_extra_space {
                        ("No extra space needed (same volume)".to_string(), t.accent)
                    } else if let (
                        Some(need),
                        Some(crate::ports::SpaceProbeOutcome::Known {
                            bytes: free,
                            precision,
                        }),
                    ) = (need, free)
                    {
                        let approximate =
                            matches!(precision, crate::ports::SpacePrecision::SaturatedLowerBound);
                        if *over {
                            (not_enough_space_message(*need, *free), t.accent_red)
                        } else {
                            let message = if approximate {
                                format!(
                                    "Fits: {} into at least {} free",
                                    format_size(*need),
                                    format_size(*free)
                                )
                            } else {
                                format!(
                                    "Fits: {} into {} free",
                                    format_size(*need),
                                    format_size(*free)
                                )
                            };
                            (message, t.accent)
                        }
                    } else if let Some(crate::ports::SpaceProbeOutcome::Unknown(failure)) = free {
                        (unavailable_space_message(Some(failure)), t.accent_warning)
                    } else {
                        (unavailable_space_message(None), t.accent_warning)
                    };
                    ui.label(egui::RichText::new(msg).size(11.0).color(color));
                }

                // Conflict resolution: per-collision detail + relation policies.
                if has_conflicts {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("  {}", conflict_banner(conflicts.len())))
                            .size(12.0)
                            .color(t.accent_warning),
                    );
                    ui.add_space(4.0);

                    if !rich_conflicts.is_empty() {
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .id_salt("conflict_detail")
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for c in rich_conflicts.iter() {
                                    ui.label(
                                        egui::RichText::new(conflict_row_text(c))
                                            .size(11.0)
                                            .color(t.text_secondary),
                                    );
                                }
                            });
                        ui.add_space(4.0);
                    }

                    {
                        let left_root = self.ws.left.current_path.clone();
                        let right_root = self.ws.right.current_path.clone();
                        let names: Vec<String> =
                            rich_conflicts.iter().map(|c| c.name.clone()).collect();
                        let kind = names
                            .first()
                            .map(|name| {
                                crate::conflict_rules::ConflictRuleBook::kind_for_name(name)
                            })
                            .unwrap_or_else(|| "other".into());
                        if let Some(preview) = crate::conflict_rules::preview_for(
                            &left_root,
                            &right_root,
                            &kind,
                            &names,
                            5,
                        ) {
                            ui.label(egui::RichText::new(preview).size(11.0).color(t.text_muted));
                            ui.add_space(4.0);
                        } else if !names.is_empty() {
                            let sample = crate::conflict_rules::ConflictRuleBook::sample_preview(
                                crate::conflict_rules::StoredRelationPolicy::KeepBoth,
                                &names,
                                5,
                            );
                            ui.label(
                                egui::RichText::new(format!("Sample before apply: {sample}"))
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                            ui.add_space(4.0);
                        }
                    }

                    use crate::conflict::RelationPolicy;
                    let mut chosen: Option<RelationPolicy> = None;
                    let policy_enabled =
                        conflict_policy_enabled(preflight_blocked, mutations_blocked);
                    ui.horizontal_wrapped(|ui| {
                        let mut btn = |ui: &mut egui::Ui, label: &str, fill, fg, policy| {
                            if ui
                                .add_enabled(
                                    policy_enabled,
                                    egui::Button::new(
                                        egui::RichText::new(label).size(12.0).color(fg),
                                    )
                                    .fill(fill)
                                    .corner_radius(CornerRadius::ZERO),
                                )
                                .clicked()
                            {
                                chosen = Some(policy);
                            }
                            ui.add_space(4.0);
                        };
                        btn(
                            ui,
                            "Keep Both",
                            t.accent,
                            Color32::WHITE,
                            RelationPolicy::KeepBoth,
                        );
                        btn(
                            ui,
                            "Keep Newer",
                            t.bg_card,
                            t.text_primary,
                            RelationPolicy::KeepNewer,
                        );
                        btn(
                            ui,
                            "Keep Larger",
                            t.bg_card,
                            t.text_primary,
                            RelationPolicy::KeepLarger,
                        );
                        btn(
                            ui,
                            "Skip Existing",
                            t.bg_card,
                            t.text_primary,
                            RelationPolicy::SkipAll,
                        );
                        btn(
                            ui,
                            "Overwrite All",
                            t.accent_warning,
                            Color32::WHITE,
                            RelationPolicy::ReplaceAll,
                        );
                    });
                    if let Some(policy) = chosen {
                        {
                            let left_root = self.ws.left.current_path.clone();
                            let right_root = self.ws.right.current_path.clone();
                            let names: Vec<String> =
                                rich_conflicts.iter().map(|c| c.name.clone()).collect();
                            let kind = names
                                .first()
                                .map(|name| {
                                    crate::conflict_rules::ConflictRuleBook::kind_for_name(name)
                                })
                                .unwrap_or_else(|| "other".into());
                            let _ = crate::conflict_rules::ConflictRuleBook::sample_preview(
                                policy.into(),
                                &names,
                                5,
                            );
                            crate::conflict_rules::upsert(crate::conflict_rules::ConflictRule {
                                left_root,
                                right_root,
                                file_kind: kind,
                                policy: policy.into(),
                            });
                        }
                        let pending_len = |ws: &crate::workspace::Workspace| match &ws.pending_op {
                            Some(PendingOp::Transfer(tr)) => tr.entries.len(),
                            _ => 0,
                        };
                        let before = pending_len(&self.ws);
                        if self.ws.resolve_pending_conflicts(policy) {
                            let dropped = before.saturating_sub(pending_len(&self.ws));
                            if dropped > 0 {
                                ctx.data_mut(|d| {
                                    *d.get_temp_mut_or_default::<usize>(egui::Id::new(
                                        CONFLICT_DROPPED,
                                    )) += dropped;
                                });
                            }
                            let still_overflows = matches!(
                                &self.ws.pending_op,
                                Some(PendingOp::Transfer(tr)) if tr.overflows()
                            );
                            if !still_overflows {
                                self.confirm_or_explain(ctx);
                            }
                        } else {
                            // Nothing left to transfer (everything skipped).
                            self.toast(
                                ctx,
                                crate::toasts::ToastKind::Info,
                                nothing_left_message(title, before),
                                false,
                            );
                            self.dismiss_pending_op(ctx);
                        }
                    }
                }

                ui.add_space(12.0);

                // Dry-run preview for Delete/Move: toast summary without mutating.
                let supports_dry_run = dry_run_supported(is_delete, title, has_conflicts);
                let dry_run_id = egui::Id::new(CONFIRM_DRY_RUN);
                let mut dry_run = if supports_dry_run {
                    ui.ctx()
                        .data_mut(|d| *d.get_temp_mut_or_insert_with(dry_run_id, || false))
                } else {
                    false
                };
                if supports_dry_run {
                    ui.checkbox(&mut dry_run, "Dry run (preview only — do not change files)");
                    ui.ctx().data_mut(|d| d.insert_temp(dry_run_id, dry_run));
                    ui.add_space(6.0);
                }

                let can_preview = flat_ready && resource_ready && !overflow && !preflight_blocked;
                let can_apply = can_preview && !mutations_blocked;
                let primary_label = if dry_run && supports_dry_run {
                    "Preview"
                } else {
                    action_label
                };
                let primary_enabled = if dry_run && supports_dry_run {
                    can_preview
                } else {
                    can_apply
                };

                // Action buttons
                ui.horizontal(|ui| {
                    if ui
                        .add(super::dialog_button(
                            t,
                            super::ButtonRole::Secondary,
                            egui::RichText::new("Cancel").size(13.0),
                        ))
                        .clicked()
                    {
                        self.dismiss_pending_op(ctx);
                    }
                    if !has_conflicts {
                        ui.add_space(8.0);
                        let primary = ui.add_enabled(
                            primary_enabled,
                            egui::Button::new(
                                egui::RichText::new(primary_label)
                                    .size(13.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(action_color)
                            .corner_radius(CornerRadius::ZERO),
                        );
                        let primary = match primary_blocker(
                            flat_ready,
                            resource_ready,
                            overflow,
                            preflight_blocked,
                            mutation_block.clone(),
                            dry_run && supports_dry_run,
                        ) {
                            Some(reason) => primary.on_disabled_hover_text(reason),
                            None => primary,
                        };
                        // Focus the primary button once so the sheet is
                        // announced; Enter on it confirms exactly as before,
                        // while Space on it is ignored (see `honours_click`).
                        let focused_id = egui::Id::new(CONFIRM_FOCUSED);
                        let already = ctx.data(|d| d.get_temp::<bool>(focused_id).is_some());
                        if should_focus_primary(
                            already,
                            primary_enabled,
                            focus_within_layer(ctx, ui.layer_id()),
                        ) {
                            primary.request_focus();
                            ctx.data_mut(|d| d.insert_temp(focused_id, true));
                        }
                        if confirm_clicked(ui, &primary) {
                            if dry_run && supports_dry_run {
                                self.dry_run_pending_op(ctx, title, count, total);
                            } else {
                                self.confirm_or_explain(ctx);
                            }
                        }
                    }
                });

                if escape_requested {
                    self.dismiss_pending_op(ctx);
                }
                // Enter on a focused dialog widget already acted on that widget
                // (e.g. toggled Dry run); it must not also confirm.
                if !has_conflicts
                    && primary_enabled
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && !dialog_enter_suppressed(ctx, ui.layer_id())
                {
                    // Enter must honor dry-run: preview toast only, never mutate.
                    if dry_run && supports_dry_run {
                        self.dry_run_pending_op(ctx, title, count, total);
                    } else {
                        self.confirm_or_explain(ctx);
                    }
                }
            });
        #[cfg(not(feature = "visual-qa"))]
        let _ = dialog_response;
        #[cfg(feature = "visual-qa")]
        if let Some(response) = dialog_response {
            crate::visual_qa::record_response(
                ctx,
                crate::visual_qa::ProbeId::Confirmation,
                &response.response,
            );
        }
    }

    /// Toast a Delete/Move dry-run summary and close without mutating.
    fn dry_run_pending_op(
        &mut self,
        ctx: &egui::Context,
        title: &str,
        count: usize,
        total_bytes: u64,
    ) {
        let size = if total_bytes > 0 {
            format!(" ({})", format_size(total_bytes))
        } else {
            String::new()
        };
        self.toast(
            ctx,
            crate::toasts::ToastKind::Info,
            format!("Dry run: {title} {count} item(s){size} — nothing changed"),
            false,
        );
        self.dismiss_pending_op(ctx);
    }

    /// Confirm the pending operation; when a Delete could not start (a
    /// transfer or another mutation began while the sheet was open), say why
    /// instead of leaving the button doing nothing.
    fn confirm_or_explain(&mut self, ctx: &egui::Context) {
        self.confirm_pending_op(ctx);
        if matches!(self.ws.pending_op, Some(PendingOp::Delete { .. })) {
            let message =
                delete_not_started_message(self.ws.mutation_block_reason("moving items to Trash"));
            self.toast(ctx, crate::toasts::ToastKind::Info, message, false);
        }
    }

    /// Close the dialog and reset its per-dialog egui state.
    fn dismiss_pending_op(&mut self, ctx: &egui::Context) {
        self.ws.dismiss_pending_op();
        clear_pending_dialog_state(ctx);
    }

    /// Title on the left, Native/Buffered method tabs on the right.
    fn method_tabs_row(&mut self, ui: &mut egui::Ui, t: &ThemeColors, title: &str, count: usize) {
        let current = match &self.ws.pending_op {
            Some(PendingOp::Transfer(tr)) => tr.method,
            _ => CopyMethod::Native,
        };
        if let Some(method) = method_tabs::show(ui, t, title, count, current)
            && let Some(PendingOp::Transfer(tr)) = &mut self.ws.pending_op
        {
            tr.method = method;
        }
    }

    fn durability_row(&mut self, ui: &mut egui::Ui, t: &ThemeColors) {
        let is_delete = matches!(&self.ws.pending_op, Some(PendingOp::Delete { .. }));
        let current = match &self.ws.pending_op {
            Some(PendingOp::Transfer(transfer)) => transfer.durability,
            _ => self.ws.durability_profile,
        };
        let current_retention = match &self.ws.pending_op {
            Some(PendingOp::Transfer(transfer)) => transfer.version_retention,
            _ => self.ws.version_retention,
        };
        let mut selected = current;
        let mut retention = current_retention;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Durability")
                    .size(10.0)
                    .color(t.text_muted),
            );
            let (show_profiles, show_retention) = durability_controls(is_delete, selected);
            if !show_profiles {
                ui.label(
                    egui::RichText::new("Versioned (always)")
                        .size(10.0)
                        .color(t.text_secondary),
                )
                .on_hover_text("Moving to Trash always keeps a restorable version");
            }
            for profile in crate::operation::DurabilityProfile::ALL
                .into_iter()
                .filter(|_| show_profiles)
            {
                let tooltip = match profile {
                    crate::operation::DurabilityProfile::Fast => {
                        "Copy without content verification"
                    }
                    crate::operation::DurabilityProfile::Verified => {
                        "Verify staged content before final placement"
                    }
                    crate::operation::DurabilityProfile::Versioned => {
                        "Verify and preserve replaced or deleted data"
                    }
                };
                ui.selectable_value(&mut selected, profile, profile.label())
                    .on_hover_text(tooltip);
            }
            if show_retention {
                ui.add_space(8.0);
                let label = ui.label(
                    egui::RichText::new("Retention")
                        .size(10.0)
                        .color(t.text_muted),
                );
                egui::ComboBox::from_id_salt("version_retention")
                    .selected_text(retention.label())
                    .show_ui(ui, |ui| {
                        for policy in crate::operation::VersionRetentionPolicy::ALL {
                            ui.selectable_value(&mut retention, policy, policy.label())
                                .on_hover_text(policy.consequence());
                        }
                    })
                    .response
                    .labelled_by(label.id);
                ui.label(
                    egui::RichText::new(retention.consequence())
                        .size(10.0)
                        .color(t.text_secondary),
                );
            }
        });
        if selected != current {
            self.ws.durability_profile = selected;
            if let Some(PendingOp::Transfer(transfer)) = &mut self.ws.pending_op {
                transfer.durability = selected;
            }
        }
        if retention != current_retention {
            self.ws.version_retention = retention;
            if let Some(PendingOp::Transfer(transfer)) = &mut self.ws.pending_op {
                transfer.version_retention = retention;
            }
        }
    }

    fn filesystem_policy_row(&mut self, ui: &mut egui::Ui, t: &ThemeColors) {
        let Some(PendingOp::Transfer(transfer)) = &mut self.ws.pending_op else {
            return;
        };
        let previous_form = transfer.name_policy.normalization;
        let previous_symlinks = transfer.symlink_policy;
        // Show the effective policy: a conflict button (Keep Newer, Overwrite
        // All, ...) changes `transfer.policy` without touching the combo's
        // stored value, and Ask must stay selectable to return to the choices.
        let effective_collision = collision_for_policy(transfer.policy);
        let mut picked_collision = None;
        let mut symlinks = transfer.symlink_policy;
        // Ask only restores the conflict buttons while every entry is still
        // in the transfer; skipped items cannot be brought back from here.
        let dropped_note = dropped_items_note(
            ui.ctx()
                .data(|d| d.get_temp::<usize>(egui::Id::new(CONFLICT_DROPPED)))
                .unwrap_or(0),
        );

        let portability_details = (!transfer.filesystem.portability.is_empty()).then(|| {
            transfer
                .filesystem
                .portability
                .iter()
                .take(8)
                .map(|issue| {
                    let targets = issue
                        .targets
                        .iter()
                        .map(|target| target.label())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{} ({targets}): {}", issue.name, issue.reason)
                })
                .collect::<Vec<_>>()
                .join("\n")
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Filesystem")
                    .size(10.0)
                    .color(t.text_muted),
            );
            egui::ComboBox::from_id_salt("normalization_policy")
                .selected_text(transfer.name_policy.normalization.label())
                .show_ui(ui, |ui| {
                    for form in [
                        crate::filesystem_policy::NormalizationForm::Preserve,
                        crate::filesystem_policy::NormalizationForm::Nfc,
                        crate::filesystem_policy::NormalizationForm::Nfd,
                    ] {
                        ui.selectable_value(
                            &mut transfer.name_policy.normalization,
                            form,
                            form.label(),
                        );
                    }
                });
            let collision_text = match effective_collision {
                Some(policy) => policy.label(),
                None => "Overwrite on collision",
            };
            egui::ComboBox::from_id_salt("collision_policy")
                .selected_text(collision_text)
                .show_ui(ui, |ui| {
                    for policy in [
                        crate::filesystem_policy::CollisionPolicy::Ask,
                        crate::filesystem_policy::CollisionPolicy::KeepBoth,
                        crate::filesystem_policy::CollisionPolicy::Skip,
                    ] {
                        let enabled = policy != crate::filesystem_policy::CollisionPolicy::Ask
                            || dropped_note.is_none();
                        let response = ui.add_enabled(
                            enabled,
                            egui::Button::selectable(
                                effective_collision == Some(policy),
                                policy.label(),
                            ),
                        );
                        let response = match &dropped_note {
                            Some(note) if !enabled => response.on_disabled_hover_text(format!(
                                "{note}; cancel and start the transfer again to choose differently"
                            )),
                            _ => response,
                        };
                        if response.clicked() {
                            picked_collision = Some(policy);
                        }
                    }
                });
            if let Some(note) = &dropped_note {
                ui.label(egui::RichText::new(note).size(10.0).color(t.accent_warning));
            }
            egui::ComboBox::from_id_salt("symlink_policy")
                .selected_text(symlinks.label())
                .show_ui(ui, |ui| {
                    for policy in [
                        crate::filesystem_policy::SymlinkPolicy::Preserve,
                        crate::filesystem_policy::SymlinkPolicy::Follow,
                        crate::filesystem_policy::SymlinkPolicy::Skip,
                    ] {
                        ui.selectable_value(&mut symlinks, policy, policy.label())
                            .on_hover_text(policy.consequence());
                    }
                });

            if let Some(note) = normalization_note(
                transfer.filesystem.normalization.changes.len(),
                transfer.filesystem.normalization.collisions.len(),
            ) {
                ui.label(egui::RichText::new(note).size(10.0).color(t.accent_warning));
            }
            if let Some(details) = &portability_details {
                ui.label(
                    egui::RichText::new(format!(
                        "{} portability warning{}",
                        transfer.filesystem.portability.len(),
                        if transfer.filesystem.portability.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ))
                    .size(10.0)
                    .color(t.accent_warning),
                )
                .on_hover_text(details);
            }
            let unavailable = transfer.filesystem.capabilities.unavailable_labels();
            if !unavailable.is_empty() {
                ui.label(
                    egui::RichText::new(format!("Unavailable: {}", unavailable.join(", ")))
                        .size(10.0)
                        .color(t.text_muted),
                );
            }
        });
        // Hover text is unreachable by keyboard and screen readers; the same
        // details are also available in a collapsible section.
        if let Some(details) = portability_details {
            egui::CollapsingHeader::new(
                egui::RichText::new("Portability details")
                    .size(10.0)
                    .color(t.text_muted),
            )
            .id_salt("portability_details")
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(details)
                        .size(10.0)
                        .color(t.text_secondary),
                );
            });
        }

        if transfer.name_policy.normalization != previous_form {
            let names = transfer
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>();
            transfer.filesystem.normalization = crate::filesystem_policy::preview_normalization(
                names,
                transfer.name_policy.normalization,
                transfer
                    .filesystem
                    .capabilities
                    .case_sensitive
                    .unwrap_or(false),
                transfer.filesystem.capabilities.normalization_insensitive,
            );
        }
        if let Some(collision) = picked_collision
            && Some(collision) != effective_collision
        {
            transfer.name_policy.collision = collision;
            // Returning to Ask hands the decision back to the conflict
            // buttons; a choice made earlier must not auto-start the transfer.
            if collision == crate::filesystem_policy::CollisionPolicy::Ask {
                transfer.start_when_ready = false;
            }
            transfer.policy = match collision {
                crate::filesystem_policy::CollisionPolicy::Ask => OverwritePolicy::Ask,
                crate::filesystem_policy::CollisionPolicy::KeepBoth => OverwritePolicy::KeepBoth,
                crate::filesystem_policy::CollisionPolicy::Skip => OverwritePolicy::SkipAll,
            };
        }
        if symlinks != previous_symlinks {
            let repaint = ui.ctx().clone();
            self.ws
                .set_pending_symlink_policy(symlinks, move || repaint.request_repaint());
        }
    }

    fn resource_policy_row(&mut self, ui: &mut egui::Ui, t: &ThemeColors) {
        let Some(target) = self
            .ws
            .pending_op
            .as_ref()
            .and_then(|operation| match operation {
                PendingOp::Transfer(transfer) => Some(transfer.target.clone()),
                PendingOp::Delete { .. } => None,
            })
        else {
            return;
        };
        let profile = self.confirm_cache.profile(&target);
        let tuning = crate::transfer_tuning::snapshot(&profile);
        let current = crate::transfer_tuning::rule_for(&profile);
        let mut selected = current;

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(profile.backend.label())
                    .size(10.0)
                    .color(t.text_muted),
            )
            .on_hover_text(&profile.reason);
            ui.label(
                egui::RichText::new(format!(
                    "p95 {:.0} ms  \u{00b7}  {} worker{}",
                    tuning.p95_latency_ms,
                    tuning.concurrency,
                    if tuning.concurrency == 1 { "" } else { "s" }
                ))
                .size(10.0)
                .color(t.text_secondary),
            )
            .on_hover_text(
                "Measured latency of this volume (95th percentile) and how many files \
                 are copied in parallel",
            );
            if profile.capabilities.delta {
                let delta_enabled = current.max_bytes_per_second.is_none();
                ui.label(
                    egui::RichText::new(if delta_enabled {
                        "Delta auto"
                    } else {
                        "Delta off"
                    })
                    .size(10.0)
                    .color(if delta_enabled { t.accent } else { t.text_muted }),
                )
                .on_hover_text(
                    "Large similar replacements use fixed blocks; measured high-latency jobs may use FastCDC",
                );
            }

            let bandwidth = ui.label(
                egui::RichText::new("Bandwidth")
                    .size(10.0)
                    .color(t.text_muted),
            );
            egui::ComboBox::from_id_salt(("volume_bandwidth", profile.volume_id))
                .selected_text(bandwidth_label(selected.max_bytes_per_second))
                .show_ui(ui, |ui| {
                    for limit in crate::transfer_tuning::BANDWIDTH_CHOICES {
                        ui.selectable_value(
                            &mut selected.max_bytes_per_second,
                            limit,
                            bandwidth_label(limit),
                        );
                    }
                })
                .response
                .labelled_by(bandwidth.id);

            let quiet = ui.label(
                egui::RichText::new("Quiet hours")
                    .size(10.0)
                    .color(t.text_muted),
            );
            egui::ComboBox::from_id_salt(("volume_quiet_hours", profile.volume_id))
                .selected_text(quiet_hours_label(selected.quiet_hours))
                .show_ui(ui, |ui| {
                    for (hours, label) in [
                        (None, "No quiet hours"),
                        (
                            Some(crate::transfer_tuning::QuietHours {
                                start_hour: 22,
                                end_hour: 7,
                            }),
                            "Quiet 22:00-07:00",
                        ),
                        (
                            Some(crate::transfer_tuning::QuietHours {
                                start_hour: 0,
                                end_hour: 6,
                            }),
                            "Quiet 00:00-06:00",
                        ),
                    ] {
                        ui.selectable_value(&mut selected.quiet_hours, hours, label);
                    }
                })
                .response
                .labelled_by(quiet.id);
        });

        if selected != current
            && let Err(error) = crate::transfer_tuning::set_rule(&profile, selected)
        {
            let ctx = ui.ctx().clone();
            self.toast(&ctx, crate::toasts::ToastKind::Error, error, false);
        }
    }

    /// Draw a virtualized flat file list: only visible rows are laid out.
    fn render_flat_list(
        ui: &mut egui::Ui,
        flat: &[FlatFileEntry],
        conflicts: &HashSet<&str>,
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
        mode: FlatListMode,
    ) {
        const ROW_H: f32 = 20.0;
        let rows = match mode {
            FlatListMode::Destination { transferred } => &flat[..transferred.min(flat.len())],
            _ => flat,
        };
        let header = flat_list_header(mode, flat.len());
        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(header).size(10.0).color(t.text_muted));
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show_rows(ui, ROW_H, rows.len(), |ui, range| {
                        for row_idx in range {
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), ROW_H),
                                Sense::hover(),
                            );
                            // Painter-only rows: name them for screen readers.
                            let fe = &rows[row_idx];
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Label,
                                    ui.is_enabled(),
                                    flat_row_accessible_label(fe, is_conflict_row(fe, conflicts)),
                                )
                            });
                            paint_flat_row(
                                ui.painter(),
                                rect,
                                &rows[row_idx],
                                row_idx,
                                conflicts,
                                t,
                                mode,
                            );
                        }
                    });
            });
    }
}

/// How a flat list row reads: a plain preview, or one side of the
/// copy/move flow where the first `transferred` rows have "moved".
#[derive(Clone, Copy)]
enum FlatListMode {
    Preview,
    Source { transferred: usize },
    Destination { transferred: usize },
}

fn paint_flat_row(
    p: &egui::Painter,
    rect: egui::Rect,
    fe: &FlatFileEntry,
    row_idx: usize,
    conflicts: &HashSet<&str>,
    t: &ThemeColors,
    mode: FlatListMode,
) {
    let is_conflict = is_conflict_row(fe, conflicts);
    let moved_away = matches!(mode, FlatListMode::Source { transferred } if row_idx < transferred);
    match mode {
        FlatListMode::Destination { transferred } => {
            let alpha = if row_idx + 1 == transferred { 0.2 } else { 0.1 };
            p.rect_filled(rect, CornerRadius::ZERO, t.accent.linear_multiply(alpha));
        }
        _ if moved_away => {
            p.rect_filled(rect, CornerRadius::ZERO, t.accent_red.linear_multiply(0.08));
        }
        _ if row_idx % 2 == 1 => {
            p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.15));
        }
        _ => {}
    }
    let color = if is_conflict {
        t.accent_warning
    } else if moved_away {
        t.text_muted.linear_multiply(0.4)
    } else {
        match mode {
            FlatListMode::Destination { .. } => t.accent,
            FlatListMode::Preview if fe.depth > 0 => t.text_secondary,
            _ => t.text_primary,
        }
    };
    let indent = fe.depth as f32 * 14.0;
    let icon = if fe.is_dir { "📁" } else { "📄" };
    let font = egui::FontId::proportional(11.0);
    let mid = rect.center().y;
    p.text(
        egui::pos2(rect.left() + indent, mid),
        egui::Align2::LEFT_CENTER,
        icon,
        font.clone(),
        color,
    );
    let text_start = rect.left() + indent + 18.0;
    let size_color = if moved_away {
        t.text_muted.linear_multiply(0.3)
    } else {
        t.text_muted
    };
    // The size is right-aligned; the name is elided to end before it.
    let size_galley = (!fe.is_dir && fe.size > 0).then(|| {
        p.layout_no_wrap(
            format_size(fe.size),
            egui::FontId::proportional(10.0),
            size_color,
        )
    });
    let name_right = size_galley.as_ref().map_or(rect.right() - 4.0, |galley| {
        rect.right() - 4.0 - galley.size().x - 8.0
    });
    let name = elided_name(p, &fe.name, font, color, name_right - text_start);
    let name_rect = egui::Align2::LEFT_CENTER.anchor_size(egui::pos2(text_start, mid), name.size());
    p.galley(name_rect.min, name, color);
    if moved_away {
        p.line_segment(
            [
                egui::pos2(text_start, mid),
                egui::pos2(name_rect.right().min(name_right), mid),
            ],
            Stroke::new(1.0_f32, t.text_muted.linear_multiply(0.3)),
        );
    }
    if let Some(size) = size_galley {
        let size_rect = egui::Align2::RIGHT_CENTER
            .anchor_size(egui::pos2(rect.right() - 4.0, mid), size.size());
        p.galley(size_rect.min, size, size_color);
    }
}

/// `name` laid out on one line, elided with "…" to fit `max_width`.
fn elided_name(
    p: &egui::Painter,
    name: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(name.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    p.layout_job(job)
}

/// What a screen reader announces for a preview row.
fn flat_row_accessible_label(fe: &FlatFileEntry, is_conflict: bool) -> String {
    let mut label = fe.name.clone();
    if fe.is_dir {
        label.push_str(", folder");
    } else if fe.size > 0 {
        label.push_str(", ");
        label.push_str(&format_size(fe.size));
    }
    if is_conflict {
        label.push_str(", already exists at the destination");
    }
    label
}

/// Header above a preview list. The scan counts every nested file and
/// folder, so it says "in total" to stay distinct from the title's count of
/// picked items; the destination side is only an illustration, so it shows
/// no counter that would read as transfer progress.
fn flat_list_header(mode: FlatListMode, len: usize) -> String {
    match mode {
        FlatListMode::Preview => format!("{} in total", items_label(len)),
        FlatListMode::Source { .. } => format!("Source ({} in total)", items_label(len)),
        FlatListMode::Destination { .. } => "Destination preview".to_string(),
    }
}

/// Which durability controls the sheet offers: `(profile buttons,
/// retention combo)`. Moving to Trash always runs Versioned, so a Delete
/// shows no profile choice (it would be ignored and silently change the
/// transfer default) but always offers retention, which it does honour.
fn durability_controls(
    is_delete: bool,
    selected: crate::operation::DurabilityProfile,
) -> (bool, bool) {
    let versioned = selected == crate::operation::DurabilityProfile::Versioned;
    (!is_delete, is_delete || versioned)
}

/// Only top-level rows can collide with a destination name.
fn is_conflict_row(fe: &FlatFileEntry, conflicts: &HashSet<&str>) -> bool {
    fe.depth == 0 && conflicts.contains(fe.name.as_str())
}

fn bandwidth_label(limit: Option<u64>) -> String {
    limit.map_or_else(
        || "Unlimited".to_string(),
        |bytes| format!("{}/s", format_size(bytes)),
    )
}

const fn quiet_hours_label(hours: Option<crate::transfer_tuning::QuietHours>) -> &'static str {
    match hours {
        None => "No quiet hours",
        Some(crate::transfer_tuning::QuietHours {
            start_hour: 22,
            end_hour: 7,
        }) => "Quiet 22:00-07:00",
        Some(crate::transfer_tuning::QuietHours {
            start_hour: 0,
            end_hour: 6,
        }) => "Quiet 00:00-06:00",
        Some(_) => "Custom quiet hours",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIRM_DRY_RUN, CONFIRM_FOCUSED, CONFLICT_DETAIL_TTL, CONFLICT_DROPPED, ConfirmCache,
        FLOW_MAX_SECS, FlatListMode, OverwritePolicy, PENDING_FLOW_START,
        clear_pending_dialog_state, collision_for_policy, conflict_banner, conflict_policy_enabled,
        conflict_row_text, delete_not_started_message, dialog_enter_suppressed, dialog_heading,
        dropped_items_note, dry_run_supported, durability_controls, elided_name, flat_list_header,
        flat_row_accessible_label, flat_total, flow_rows, flow_transferred, focus_within_layer,
        honours_click, is_conflict_row, normalization_note, not_enough_space_message,
        nothing_left_message, primary_blocker, should_focus_primary, source_header,
        unavailable_size_message, unavailable_space_message,
    };
    use crate::filesystem_policy::CollisionPolicy;
    use crate::scan::{FlatFileEntry, FlatList};
    use std::collections::HashSet;
    use std::sync::Arc;

    fn entry(name: &str, size: u64, is_dir: bool, depth: usize) -> FlatFileEntry {
        FlatFileEntry {
            name: name.to_string(),
            size,
            is_dir,
            depth,
        }
    }

    #[test]
    fn preview_total_saturates_instead_of_overflowing() {
        let flat = vec![
            entry("sparse-a", u64::MAX, false, 0),
            entry("sparse-b", 2, false, 0),
            entry("dir", 7, true, 0),
        ];
        assert_eq!(flat_total(&flat), u64::MAX);
        assert_eq!(
            flat_total(&[entry("a", 3, false, 0), entry("d", 9, true, 0)]),
            3
        );
        assert_eq!(flat_total(&[]), 0);
    }

    #[test]
    fn flow_animation_finishes_within_the_cap_for_long_lists() {
        // Short lists keep the 80 ms per file pace.
        assert_eq!(flow_transferred(0.0, 5), 0);
        assert_eq!(flow_transferred(0.17, 5), 2);
        assert_eq!(flow_transferred(10.0, 5), 5);
        // A 5000-file list is done at the cap, not after ~400 s.
        assert_eq!(flow_transferred(FLOW_MAX_SECS, 5000), 5000);
        assert!(flow_transferred(FLOW_MAX_SECS / 2.0, 5000) >= 2500);
        assert_eq!(flow_transferred(-1.0, 10), 0);
        assert_eq!(flow_transferred(f64::NAN, 10), 0);
    }

    #[test]
    fn scan_is_copied_once_and_follows_a_restaged_list() {
        let mut cache = ConfirmCache::default();
        let source = crate::scan::pending_flat_list();
        assert!(cache.flat(&source).is_none(), "scan still running");
        *crate::lock_util::recover(&source) = Some(vec![entry("a", 4, false, 0)]);
        let (first, total) = cache.flat(&source).expect("scan finished");
        assert_eq!(total, 4);
        let (second, _) = cache.flat(&source).expect("cached");
        assert!(Arc::ptr_eq(&first, &second), "no per-frame copy");

        let restaged = crate::scan::pending_flat_list();
        *crate::lock_util::recover(&restaged) = Some(vec![entry("b", 9, false, 0)]);
        let (third, total) = cache.flat(&restaged).expect("new scan");
        assert_eq!(third[0].name, "b");
        assert_eq!(total, 9);
    }

    #[test]
    fn conflict_detail_follows_the_staged_transfer_and_expires() {
        let mut cache = ConfirmCache::default();
        let target = std::path::Path::new("/dest");
        let names = vec!["a.txt".to_string()];
        let flat = crate::scan::pending_flat_list();
        let start = std::time::Instant::now();
        let loads = std::cell::Cell::new(0);
        let read = |cache: &mut ConfirmCache, flat: &FlatList, now| {
            cache.conflicts_for(flat, target, &names, 1, now, || {
                loads.set(loads.get() + 1);
                Vec::new()
            });
        };
        read(&mut cache, &flat, start);
        read(
            &mut cache,
            &flat,
            start + std::time::Duration::from_millis(500),
        );
        assert_eq!(loads.get(), 1, "same transfer within the TTL is cached");
        // A replacement with the same target, names and count but its own
        // scan list is a different transfer.
        let replaced = crate::scan::pending_flat_list();
        read(
            &mut cache,
            &replaced,
            start + std::time::Duration::from_millis(600),
        );
        assert_eq!(loads.get(), 2);
        read(&mut cache, &replaced, start + CONFLICT_DETAIL_TTL * 2);
        assert_eq!(loads.get(), 3, "detail is re-read after the TTL");
        // A clock that went backwards also re-reads.
        read(&mut cache, &replaced, start);
        assert_eq!(loads.get(), 4);
    }

    #[test]
    fn volume_profile_is_probed_once_per_target() {
        let temp = crate::testutil::TempDir::new();
        let other = temp.path().join("other");
        std::fs::create_dir_all(&other).expect("dir");
        let mut cache = ConfirmCache::default();
        let first = cache.profile(temp.path());
        let second = cache.profile(temp.path());
        assert!(Arc::ptr_eq(&first, &second), "same target is not re-probed");
        let third = cache.profile(&other);
        assert!(!Arc::ptr_eq(&first, &third), "a new target is probed");
    }

    #[test]
    fn only_top_level_rows_are_marked_as_conflicts() {
        let conflicts: HashSet<&str> = ["a.txt"].into_iter().collect();
        assert!(is_conflict_row(&entry("a.txt", 1, false, 0), &conflicts));
        assert!(!is_conflict_row(&entry("a.txt", 1, false, 1), &conflicts));
        assert!(!is_conflict_row(&entry("b.txt", 1, false, 0), &conflicts));
    }

    #[test]
    fn resolved_conflict_policy_is_reflected_in_the_collision_combo() {
        assert_eq!(
            collision_for_policy(OverwritePolicy::Ask),
            Some(CollisionPolicy::Ask)
        );
        assert_eq!(
            collision_for_policy(OverwritePolicy::KeepBoth),
            Some(CollisionPolicy::KeepBoth)
        );
        assert_eq!(
            collision_for_policy(OverwritePolicy::SkipAll),
            Some(CollisionPolicy::Skip)
        );
        // Overwrite All / Keep Newer must not read as "Ask", or picking Ask
        // would look like no change and the conflict buttons never return.
        assert_eq!(collision_for_policy(OverwritePolicy::OverwriteAll), None);
    }

    #[test]
    fn dry_run_is_hidden_while_conflict_buttons_would_run_the_transfer() {
        assert!(dry_run_supported(false, "Move", false));
        assert!(dry_run_supported(true, "Delete", false));
        assert!(!dry_run_supported(false, "Move", true));
        assert!(!dry_run_supported(false, "Copy", false));
    }

    #[test]
    fn conflict_policy_buttons_respect_mutation_gates() {
        assert!(conflict_policy_enabled(false, false));
        assert!(!conflict_policy_enabled(true, false));
        assert!(!conflict_policy_enabled(false, true));
    }

    #[test]
    fn clearing_dialog_state_drops_flow_start_and_dry_run() {
        let ctx = egui::Context::default();
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new(PENDING_FLOW_START), 1.0f64);
            d.insert_temp(egui::Id::new(CONFIRM_DRY_RUN), true);
            d.insert_temp(egui::Id::new(CONFLICT_DROPPED), 2usize);
            d.insert_temp(egui::Id::new(CONFIRM_FOCUSED), true);
        });
        clear_pending_dialog_state(&ctx);
        ctx.data(|d| {
            assert_eq!(d.get_temp::<usize>(egui::Id::new(CONFLICT_DROPPED)), None);
            assert_eq!(d.get_temp::<f64>(egui::Id::new(PENDING_FLOW_START)), None);
            assert_eq!(d.get_temp::<bool>(egui::Id::new(CONFIRM_DRY_RUN)), None);
            assert_eq!(d.get_temp::<bool>(egui::Id::new(CONFIRM_FOCUSED)), None);
        });
    }

    #[test]
    fn focused_dialog_checkbox_suppresses_dialog_enter() {
        let ctx = egui::Context::default();
        let mut layer = None;
        let mut flag = false;
        for _ in 0..3 {
            crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                let response = ui.checkbox(&mut flag, "Dry run");
                response.request_focus();
                layer = Some(ui.layer_id());
            }));
        }
        let layer = layer.expect("frame ran");
        assert!(focus_within_layer(&ctx, layer));
        let other = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("elsewhere"));
        assert!(!focus_within_layer(&ctx, other));
        let focused = ctx.memory(|m| m.focused()).expect("checkbox focused");
        ctx.memory_mut(|m| m.surrender_focus(focused));
        assert!(!focus_within_layer(&ctx, layer));
        assert!(!dialog_enter_suppressed(&ctx, layer));
    }

    #[test]
    fn open_combo_popup_suppresses_dialog_enter() {
        let ctx = egui::Context::default();
        let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("dialog"));
        assert!(!dialog_enter_suppressed(&ctx, layer));
        egui::Popup::open_id(&ctx, egui::Id::new("collision_policy_popup"));
        assert!(dialog_enter_suppressed(&ctx, layer));
    }

    #[test]
    fn dropped_items_block_returning_to_ask() {
        assert_eq!(dropped_items_note(0), None);
        assert!(dropped_items_note(1).is_some_and(|n| n.starts_with("1 conflicting item ")));
        assert!(dropped_items_note(3).is_some_and(|n| n.contains("3 conflicting items")));
    }

    #[test]
    fn unknown_space_is_presented_as_a_non_blocking_warning_not_fits() {
        let failure = crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Unsupported,
            message: "probe unavailable".to_string(),
        };
        let message = unavailable_space_message(Some(&failure));
        assert!(message.contains("could not be verified"));
        assert!(message.contains("You can continue"));
        assert!(!message.contains("Fits"));
    }

    #[test]
    fn unknown_transfer_size_is_blocking_and_explained() {
        let failure = crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Unknown,
            message: "directory changed while scanning".to_string(),
        };
        let message = unavailable_size_message(&failure);
        assert!(message.contains("could not be verified"));
        assert!(message.contains("Confirmation is unavailable"));
    }

    #[test]
    fn fully_skipped_transfer_says_nothing_was_copied() {
        assert_eq!(
            nothing_left_message("Copy", 3),
            "Nothing to copy: all 3 items already exist at the destination"
        );
        assert_eq!(
            nothing_left_message("Move", 1),
            "Nothing to move: the item already exists at the destination"
        );
    }

    #[test]
    fn blocked_trash_names_the_blocker() {
        assert!(delete_not_started_message(None).contains("transfer queue"));
        assert_eq!(
            delete_not_started_message(Some("Safe-state review is required".into())),
            "Safe-state review is required"
        );
    }

    #[test]
    fn delete_heading_matches_the_trash_button() {
        assert_eq!(dialog_heading("Delete", true, 1), "Move to Trash — 1 item");
        assert_eq!(dialog_heading("Copy", false, 3), "Copy — 3 items");
    }

    #[test]
    fn source_header_names_several_folders() {
        use std::path::Path;
        let same = [Some(Path::new("/a")), Some(Path::new("/a"))];
        assert_eq!(source_header(same), ("/a".to_string(), None));
        let mixed = [
            Some(Path::new("/a")),
            Some(Path::new("/b")),
            Some(Path::new("/a")),
        ];
        assert_eq!(
            source_header(mixed),
            ("2 folders".to_string(), Some("/a\n/b".to_string()))
        );
    }

    #[test]
    fn space_shortfall_states_need_free_and_gap() {
        let message = not_enough_space_message(4096, 1024);
        assert!(message.contains("needs 4"), "{message}");
        assert!(message.contains("only 1"), "{message}");
        assert!(message.contains("short"), "{message}");
    }

    #[test]
    fn conflict_text_is_pluralized_and_plain() {
        assert_eq!(
            conflict_banner(1),
            "1 item already exists at the destination"
        );
        assert_eq!(
            conflict_banner(2),
            "2 items already exist at the destination"
        );
        let conflict = crate::conflict::Conflict {
            name: "a.txt".into(),
            src_path: "/s/a.txt".into(),
            src_size: 0,
            dst_size: 0,
            src_is_dir: false,
            dst_is_dir: false,
            src_newer: true,
            dst_newer: false,
        };
        let row = conflict_row_text(&conflict);
        assert!(row.contains("Incoming") && row.contains("(newer)") && row.contains("Existing"));
        assert!(!row.contains("src") && !row.contains("dst"), "{row}");
    }

    #[test]
    fn normalization_note_says_names_will_change() {
        assert_eq!(normalization_note(0, 0), None);
        assert_eq!(
            normalization_note(3, 1).as_deref(),
            Some("3 names will be normalized  \u{00b7}  1 collision")
        );
        assert_eq!(
            normalization_note(1, 0).as_deref(),
            Some("1 name will be normalized")
        );
    }

    #[test]
    fn list_headers_count_consistently_and_destination_shows_no_counter() {
        assert_eq!(
            flat_list_header(FlatListMode::Preview, 1),
            "1 item in total"
        );
        assert_eq!(
            flat_list_header(FlatListMode::Source { transferred: 0 }, 57),
            "Source (57 items in total)"
        );
        let destination = flat_list_header(FlatListMode::Destination { transferred: 12 }, 57);
        assert!(!destination.contains("12"), "{destination}");
    }

    #[test]
    fn unverified_size_suggests_a_next_step() {
        let failure = crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Unknown,
            message: "x".to_string(),
        };
        assert!(unavailable_size_message(&failure).contains("start the operation again"));
    }

    #[test]
    fn reduced_motion_shows_every_row_without_animating() {
        assert_eq!(flow_rows(0.0, 40, true), 40);
        assert_eq!(flow_rows(0.0, 40, false), 0);
    }

    #[test]
    fn long_and_wide_names_are_elided_to_the_name_column() {
        let ctx = egui::Context::default();
        let mut widths = Vec::new();
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            for name in ["a".repeat(300), "Ж".repeat(300), "文".repeat(300)] {
                let galley = elided_name(
                    ui.painter(),
                    &name,
                    egui::FontId::proportional(11.0),
                    egui::Color32::WHITE,
                    120.0,
                );
                widths.push(galley.size().x);
            }
        }));
        for width in widths {
            assert!(
                width <= 121.0,
                "name painted {width} px into a 120 px column"
            );
        }
    }

    #[test]
    fn preview_rows_have_accessible_names() {
        assert_eq!(
            flat_row_accessible_label(&entry("docs", 0, true, 0), false),
            "docs, folder"
        );
        let label = flat_row_accessible_label(&entry("a.txt", 2048, false, 0), true);
        assert!(label.starts_with("a.txt, 2"), "{label}");
        assert!(
            label.ends_with("already exists at the destination"),
            "{label}"
        );
    }

    #[test]
    fn delete_offers_retention_but_no_ignored_durability_choice() {
        use crate::operation::DurabilityProfile;
        assert_eq!(
            durability_controls(true, DurabilityProfile::Fast),
            (false, true)
        );
        assert_eq!(
            durability_controls(false, DurabilityProfile::Fast),
            (true, false)
        );
        assert_eq!(
            durability_controls(false, DurabilityProfile::Versioned),
            (true, true)
        );
    }

    #[test]
    fn space_on_focused_confirm_button_does_not_confirm() {
        assert!(
            honours_click(true, false, true),
            "Enter or pointer confirms"
        );
        assert!(
            !honours_click(true, true, true),
            "Space on the focused button"
        );
        assert!(
            honours_click(true, true, false),
            "pointer click while Space held elsewhere"
        );
        assert!(!honours_click(false, false, true));
    }

    #[test]
    fn primary_button_takes_focus_once_when_it_can() {
        assert!(should_focus_primary(false, true, false));
        assert!(!should_focus_primary(true, true, false), "only once");
        assert!(
            !should_focus_primary(false, false, false),
            "disabled while scanning"
        );
        assert!(
            !should_focus_primary(false, true, true),
            "user already moved focus"
        );
    }

    #[test]
    fn disabled_primary_names_the_first_blocker() {
        let blocked = Some("Wait for the current Trash operation".to_string());
        assert_eq!(
            primary_blocker(false, false, true, true, blocked.clone(), false).as_deref(),
            Some("Still scanning the selected items")
        );
        assert_eq!(
            primary_blocker(true, true, true, false, None, false).as_deref(),
            Some("Not enough free space at the destination")
        );
        assert_eq!(
            primary_blocker(true, true, false, false, blocked.clone(), false),
            blocked
        );
        assert_eq!(
            primary_blocker(true, true, false, false, blocked, true),
            None
        );
    }
}
