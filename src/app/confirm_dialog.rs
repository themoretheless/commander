//! Confirmation dialog for pending copy/move/delete operations.

use super::*;

pub mod method_tabs;
use crate::scan::FlatFileEntry;
use crate::transfer::OverwritePolicy;

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

fn unavailable_size_message(failure: &crate::ports::NativeFailure) -> String {
    format!(
        "Transfer size could not be verified: {}. Confirmation is unavailable.",
        failure.message
    )
}

impl App {
    pub(crate) fn show_confirm_dialog(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let escape_requested =
            self.take_modal_escape(crate::accessibility::ModalSurface::Confirmation);
        let mutations_blocked = self.ws.mutations_blocked();
        let Some(op) = &self.ws.pending_op else {
            clear_pending_dialog_state(ctx);
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
                    let source_dir = tr
                        .entries
                        .first()
                        .and_then(|e| e.path.parent())
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
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
                    String::new(),
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

        let flat_opt = crate::lock_util::recover(&flat_arc).clone();
        let flat_ready = flat_opt.is_some();
        let flat = flat_opt.unwrap_or_default();

        // Rich per-conflict detail (size/mtime each side) for the resolver.
        let rich_conflicts = self.ws.pending_conflicts();

        let has_conflicts = !conflicts.is_empty()
            && matches!(
                &self.ws.pending_op,
                Some(PendingOp::Transfer(transfer)) if transfer.policy == OverwritePolicy::Ask
            );
        let is_delete = target.is_none();
        let win_title = format!("{} — {} item(s)", title, count);
        let screen = ctx.input(|i| i.viewport_rect());
        let pad = 120.0;
        let avail_w = (screen.width() - pad * 2.0).max(300.0);
        let avail_h = (screen.height() - pad * 2.0).max(200.0);
        let win_w = 1000.0f32.min(avail_w);
        let win_h = 800.0f32.min(avail_h);

        let dialog_response = egui::Window::new(win_title)
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
                        egui::RichText::new(format!("Delete — {} item(s)", count))
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
                        &[],
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
                        cols[0].label(
                            egui::RichText::new(format!("Source: {}", source_dir))
                                .size(11.0)
                                .color(t.text_muted),
                        );
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
                    // Transfer one file every 80ms, all done in ~N*80ms
                    let transferred = ((elapsed / 0.08) as usize).min(flat.len());

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
                            &conflicts,
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
                            &conflicts,
                            &t,
                            list_h,
                            "pending_dst",
                            FlatListMode::Destination { transferred },
                        );
                    });
                }

                // Total size (from flat list, files only)
                let total: u64 = flat.iter().filter(|f| !f.is_dir).map(|f| f.size).sum();
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
                            (
                                format!(
                                    "Not enough space: needs {} more than {} free",
                                    format_size(need.saturating_sub(*free)),
                                    format_size(*free)
                                ),
                                t.accent_red,
                            )
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
                        egui::RichText::new(format!(
                            "  {} file(s) already exist at destination",
                            conflicts.len()
                        ))
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
                                for c in &rich_conflicts {
                                    let src = if c.src_newer { "src newer" } else { "" };
                                    let dst = if c.dst_newer { "dst newer" } else { "" };
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}    src {} {}    \u{00bb} dst {} {}",
                                            c.name,
                                            format_size(c.src_size),
                                            src,
                                            format_size(c.dst_size),
                                            dst,
                                        ))
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
                                self.confirm_pending_op(ctx);
                            }
                        } else {
                            // Nothing left to transfer (everything skipped).
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
                        if ui
                            .add_enabled(
                                primary_enabled,
                                egui::Button::new(
                                    egui::RichText::new(primary_label)
                                        .size(13.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(action_color)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            if dry_run && supports_dry_run {
                                self.dry_run_pending_op(ctx, title, count, total);
                            } else {
                                self.confirm_pending_op(ctx);
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
                        self.confirm_pending_op(ctx);
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
            for profile in crate::operation::DurabilityProfile::ALL {
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
            if selected == crate::operation::DurabilityProfile::Versioned {
                ui.add_space(8.0);
                ui.label(
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
                    });
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

            let change_count = transfer.filesystem.normalization.changes.len();
            let collision_count = transfer.filesystem.normalization.collisions.len();
            if change_count > 0 || collision_count > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "{change_count} rename preview  \u{00b7}  {collision_count} collision{}",
                        if collision_count == 1 { "" } else { "s" }
                    ))
                    .size(10.0)
                    .color(t.accent_warning),
                );
            }
            if !transfer.filesystem.portability.is_empty() {
                let details = transfer
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
                    .join("\n");
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
        let profile = crate::volume_profile::profile(&target);
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
                });

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
                });
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
        conflicts: &[String],
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
        let header = match mode {
            FlatListMode::Preview => format!("{} items", flat.len()),
            FlatListMode::Source { .. } => format!("Source ({} items)", flat.len()),
            FlatListMode::Destination { .. } => {
                format!("Destination ({}/{})", rows.len(), flat.len())
            }
        };
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
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), ROW_H),
                                Sense::hover(),
                            );
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
    conflicts: &[String],
    t: &ThemeColors,
    mode: FlatListMode,
) {
    let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
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
    p.text(
        egui::pos2(text_start, mid),
        egui::Align2::LEFT_CENTER,
        &fe.name,
        font,
        color,
    );
    if moved_away {
        let text_end = text_start + fe.name.len() as f32 * 6.5;
        p.line_segment(
            [
                egui::pos2(text_start, mid),
                egui::pos2(text_end.min(rect.right() - 4.0), mid),
            ],
            Stroke::new(1.0_f32, t.text_muted.linear_multiply(0.3)),
        );
    }
    if !fe.is_dir && fe.size > 0 {
        let size_color = if moved_away {
            t.text_muted.linear_multiply(0.3)
        } else {
            t.text_muted
        };
        p.text(
            egui::pos2(rect.right() - 4.0, mid),
            egui::Align2::RIGHT_CENTER,
            format_size(fe.size),
            egui::FontId::proportional(10.0),
            size_color,
        );
    }
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
        CONFIRM_DRY_RUN, CONFLICT_DROPPED, OverwritePolicy, PENDING_FLOW_START,
        clear_pending_dialog_state, collision_for_policy, conflict_policy_enabled,
        dialog_enter_suppressed, dropped_items_note, dry_run_supported, focus_within_layer,
        unavailable_size_message, unavailable_space_message,
    };
    use crate::filesystem_policy::CollisionPolicy;

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
        });
        clear_pending_dialog_state(&ctx);
        ctx.data(|d| {
            assert_eq!(d.get_temp::<usize>(egui::Id::new(CONFLICT_DROPPED)), None);
            assert_eq!(d.get_temp::<f64>(egui::Id::new(PENDING_FLOW_START)), None);
            assert_eq!(d.get_temp::<bool>(egui::Id::new(CONFIRM_DRY_RUN)), None);
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
}
