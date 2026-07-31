//! Confirmation dialog for pending copy/move/delete operations.

use super::*;
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
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(6))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
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
                    Self::render_flat_list_virtual(ui, &flat, &[], &t, list_h, "pending_op_files");
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
                    let anim_id = egui::Id::new("pending_flow_start");
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
                        Self::render_flat_list_animated(
                            &mut cols[0],
                            &flat,
                            &conflicts,
                            &t,
                            list_h,
                            "pending_src",
                            transferred,
                            true,
                        );

                        // Right: only transferred files shown, highlighted
                        let list_h = (cols[1].available_height() - 80.0).max(60.0);
                        Self::render_flat_list_animated(
                            &mut cols[1],
                            &flat,
                            &conflicts,
                            &t,
                            list_h,
                            "pending_dst",
                            transferred,
                            false,
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
                                            "{}    src {} {}    \u{2192} dst {} {}",
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

                    use crate::conflict::RelationPolicy;
                    let mut chosen: Option<RelationPolicy> = None;
                    ui.horizontal_wrapped(|ui| {
                        let mut btn = |ui: &mut egui::Ui, label: &str, fill, fg, policy| {
                            if ui
                                .add(
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
                        if self.ws.resolve_pending_conflicts(policy) {
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

                // Action buttons
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Cancel")
                                    .size(13.0)
                                    .color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        self.dismiss_pending_op(ctx);
                    }
                    if !has_conflicts {
                        ui.add_space(8.0);
                        if ui
                            .add_enabled(
                                resource_ready
                                    && !overflow
                                    && !preflight_blocked
                                    && !mutations_blocked,
                                egui::Button::new(
                                    egui::RichText::new(action_label)
                                        .size(13.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(action_color)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            self.confirm_pending_op(ctx);
                        }
                    }
                });

                if escape_requested {
                    self.dismiss_pending_op(ctx);
                }
                if !has_conflicts
                    && resource_ready
                    && !overflow
                    && !preflight_blocked
                    && !mutations_blocked
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    self.confirm_pending_op(ctx);
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

    /// Close the dialog and reset its per-dialog egui state.
    fn dismiss_pending_op(&mut self, ctx: &egui::Context) {
        self.ws.dismiss_pending_op();
        ctx.data_mut(|d| {
            d.remove::<f64>(egui::Id::new("pending_flow_start"));
        });
    }

    /// Title on the left, Native/Buffered method tabs on the right.
    fn method_tabs_row(&mut self, ui: &mut egui::Ui, t: &ThemeColors, title: &str, count: usize) {
        let cur_method = match &self.ws.pending_op {
            Some(PendingOp::Transfer(tr)) => tr.method,
            _ => CopyMethod::Native,
        };

        let row_h = 28.0;
        let full_w = ui.available_width();
        let (row_rect, _) = ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::hover());
        let p = ui.painter();

        // Bottom line across full width
        p.line_segment(
            [
                egui::pos2(row_rect.left(), row_rect.bottom()),
                egui::pos2(row_rect.right(), row_rect.bottom()),
            ],
            Stroke::new(1.0_f32, t.border),
        );

        // Title on the left
        p.text(
            egui::pos2(row_rect.left() + 4.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{} — {} item(s)", title, count),
            egui::FontId::proportional(13.0),
            t.text_primary,
        );

        // Tabs on the right
        let tabs: &[(&str, CopyMethod)] = &[
            ("Native", CopyMethod::Native),
            ("Buffered", CopyMethod::Buffered),
        ];
        let tab_w = 90.0;
        let tabs_total_w = tab_w * tabs.len() as f32;
        let tabs_left = row_rect.right() - tabs_total_w;

        let mut clicked_method: Option<CopyMethod> = None;

        for (i, &(label, method)) in tabs.iter().enumerate() {
            let active = cur_method == method;
            let tab_rect = egui::Rect::from_min_size(
                egui::pos2(tabs_left + i as f32 * tab_w, row_rect.top()),
                Vec2::new(tab_w, row_h),
            );

            if active {
                p.rect_filled(tab_rect, CornerRadius::ZERO, t.bg_panel);
                // Left border
                p.line_segment(
                    [
                        egui::pos2(tab_rect.left(), tab_rect.bottom()),
                        egui::pos2(tab_rect.left(), tab_rect.top()),
                    ],
                    Stroke::new(1.0_f32, t.border),
                );
                // Top border
                p.line_segment(
                    [
                        egui::pos2(tab_rect.left(), tab_rect.top()),
                        egui::pos2(tab_rect.right(), tab_rect.top()),
                    ],
                    Stroke::new(1.0_f32, t.border),
                );
                // Right border
                p.line_segment(
                    [
                        egui::pos2(tab_rect.right(), tab_rect.top()),
                        egui::pos2(tab_rect.right(), tab_rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, t.border),
                );
                // Cover bottom line
                p.line_segment(
                    [
                        egui::pos2(tab_rect.left() + 1.0, tab_rect.bottom()),
                        egui::pos2(tab_rect.right() - 1.0, tab_rect.bottom()),
                    ],
                    Stroke::new(2.0_f32, t.bg_panel),
                );
            }

            let fg = if active { t.text_primary } else { t.text_muted };
            p.text(
                tab_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                fg,
            );

            // Click detection
            let tab_resp =
                ui.interact(tab_rect, ui.id().with(format!("tab_{}", i)), Sense::click());
            if tab_resp.clicked() {
                clicked_method = Some(method);
            }
            if tab_resp.hovered() && !active {
                p.rect_filled(
                    tab_rect,
                    CornerRadius::ZERO,
                    t.bg_hover.linear_multiply(0.2),
                );
            }
        }

        if let Some(method) = clicked_method
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
        let mut collision = transfer.name_policy.collision;
        let mut symlinks = transfer.symlink_policy;

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
            egui::ComboBox::from_id_salt("collision_policy")
                .selected_text(collision.label())
                .show_ui(ui, |ui| {
                    for policy in [
                        crate::filesystem_policy::CollisionPolicy::Ask,
                        crate::filesystem_policy::CollisionPolicy::KeepBoth,
                        crate::filesystem_policy::CollisionPolicy::Skip,
                    ] {
                        ui.selectable_value(&mut collision, policy, policy.label());
                    }
                });
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
        if collision != transfer.name_policy.collision {
            transfer.name_policy.collision = collision;
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
            self.toasts.push(crate::toasts::Toast::new(
                error,
                crate::toasts::ToastKind::Error,
                false,
                ui.ctx().input(|input| input.time),
            ));
        }
    }

    /// Draw a virtualized flat file list (only visible rows rendered).
    fn render_flat_list_virtual(
        ui: &mut egui::Ui,
        flat: &[FlatFileEntry],
        conflicts: &[String],
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
    ) {
        let row_h = 20.0;

        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show(ui, |ui| {
                        // Total count label
                        ui.label(
                            egui::RichText::new(format!("{} items", flat.len()))
                                .size(10.0)
                                .color(t.text_muted),
                        );

                        let scroll_offset = ui.clip_rect().top() - ui.min_rect().top();
                        let viewport_h = max_height;
                        let first = ((scroll_offset / row_h).floor() as usize).min(flat.len());
                        let visible_count = ((viewport_h / row_h).ceil() as usize + 2)
                            .min(flat.len().saturating_sub(first));

                        // Spacer before visible rows
                        if first > 0 {
                            ui.allocate_space(Vec2::new(
                                ui.available_width(),
                                first as f32 * row_h,
                            ));
                        }

                        // Render only visible rows
                        for (vi, fe) in flat[first..first + visible_count].iter().enumerate() {
                            let row_idx = first + vi;
                            let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), row_h),
                                Sense::hover(),
                            );
                            let p = ui.painter();

                            // Zebra stripe
                            if row_idx % 2 == 1 {
                                p.rect_filled(
                                    rect,
                                    CornerRadius::ZERO,
                                    t.bg_card.linear_multiply(0.15),
                                );
                            }

                            let indent = fe.depth as f32 * 14.0;
                            let icon = if fe.is_dir { "📁" } else { "📄" };
                            let color = if is_conflict {
                                t.accent_warning
                            } else if fe.depth > 0 {
                                t.text_secondary
                            } else {
                                t.text_primary
                            };

                            // Icon
                            p.text(
                                egui::pos2(rect.left() + indent, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                icon,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Name
                            p.text(
                                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &fe.name,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Size
                            if !fe.is_dir && fe.size > 0 {
                                p.text(
                                    egui::pos2(rect.right() - 4.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    format_size(fe.size),
                                    egui::FontId::proportional(10.0),
                                    t.text_muted,
                                );
                            }
                        }

                        // Spacer after visible rows
                        let after = flat.len().saturating_sub(first + visible_count);
                        if after > 0 {
                            ui.allocate_space(Vec2::new(
                                ui.available_width(),
                                after as f32 * row_h,
                            ));
                        }
                    });
            });
    }

    /// Animated file list for copy/move dialog.
    /// `transferred`: how many files have "moved" so far
    /// `is_source`: true = left side (files leaving), false = right side (files arriving)
    // Pure drawing helper: every argument is an independent rendering
    // input, so a parameter struct would only add a layer of indirection.
    #[allow(clippy::too_many_arguments)]
    fn render_flat_list_animated(
        ui: &mut egui::Ui,
        flat: &[FlatFileEntry],
        conflicts: &[String],
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
        transferred: usize,
        is_source: bool,
    ) {
        let row_h = 20.0;

        // On destination side, only show transferred files
        let visible_flat: &[FlatFileEntry] = if is_source {
            flat
        } else {
            &flat[..transferred]
        };

        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                let header = if is_source {
                    format!("Source ({} items)", flat.len())
                } else {
                    format!("Destination ({}/{})", transferred, flat.len())
                };
                ui.label(egui::RichText::new(header).size(10.0).color(t.text_muted));

                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show(ui, |ui| {
                        let scroll_offset = ui.clip_rect().top() - ui.min_rect().top();
                        let viewport_h = max_height;
                        let total = visible_flat.len();
                        let first = ((scroll_offset / row_h).floor() as usize).min(total);
                        let visible_count = ((viewport_h / row_h).ceil() as usize + 2)
                            .min(total.saturating_sub(first));

                        if first > 0 {
                            ui.allocate_space(Vec2::new(
                                ui.available_width(),
                                first as f32 * row_h,
                            ));
                        }

                        for (vi, fe) in visible_flat[first..first + visible_count]
                            .iter()
                            .enumerate()
                        {
                            let row_idx = first + vi;
                            let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
                            let is_transferred = row_idx < transferred;
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), row_h),
                                Sense::hover(),
                            );
                            let p = ui.painter();

                            // Background
                            if is_source && is_transferred {
                                // Transferred on source side — faded red tint
                                p.rect_filled(
                                    rect,
                                    CornerRadius::ZERO,
                                    t.accent_red.linear_multiply(0.08),
                                );
                            } else if !is_source {
                                // Destination side — green/accent tint
                                let alpha = if row_idx + 1 == transferred { 0.2 } else { 0.1 };
                                p.rect_filled(
                                    rect,
                                    CornerRadius::ZERO,
                                    t.accent.linear_multiply(alpha),
                                );
                            } else if row_idx % 2 == 1 {
                                p.rect_filled(
                                    rect,
                                    CornerRadius::ZERO,
                                    t.bg_card.linear_multiply(0.15),
                                );
                            }

                            let indent = fe.depth as f32 * 14.0;
                            let icon = if fe.is_dir { "📁" } else { "📄" };

                            let color = if is_conflict {
                                t.accent_warning
                            } else if is_source && is_transferred {
                                t.text_muted.linear_multiply(0.4) // very dim
                            } else if !is_source {
                                t.accent
                            } else {
                                t.text_primary
                            };

                            // Icon
                            p.text(
                                egui::pos2(rect.left() + indent, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                icon,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Name
                            p.text(
                                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &fe.name,
                                egui::FontId::proportional(11.0),
                                color,
                            );

                            // Strikethrough on transferred source items
                            if is_source && is_transferred {
                                let text_start = rect.left() + indent + 18.0;
                                let text_end = text_start + fe.name.len() as f32 * 6.5;
                                p.line_segment(
                                    [
                                        egui::pos2(text_start, rect.center().y),
                                        egui::pos2(
                                            text_end.min(rect.right() - 4.0),
                                            rect.center().y,
                                        ),
                                    ],
                                    Stroke::new(1.0_f32, t.text_muted.linear_multiply(0.3)),
                                );
                            }

                            // Size
                            if !fe.is_dir && fe.size > 0 {
                                let size_color = if is_source && is_transferred {
                                    t.text_muted.linear_multiply(0.3)
                                } else {
                                    t.text_muted
                                };
                                p.text(
                                    egui::pos2(rect.right() - 4.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    format_size(fe.size),
                                    egui::FontId::proportional(10.0),
                                    size_color,
                                );
                            }
                        }

                        let after = total.saturating_sub(first + visible_count);
                        if after > 0 {
                            ui.allocate_space(Vec2::new(
                                ui.available_width(),
                                after as f32 * row_h,
                            ));
                        }
                    });
            });
    }
}

fn bandwidth_label(limit: Option<u64>) -> String {
    limit.map_or_else(
        || "Unlimited".to_string(),
        |bytes| format!("{}/s", format_size(bytes)),
    )
}

fn quiet_hours_label(hours: Option<crate::transfer_tuning::QuietHours>) -> &'static str {
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
    use super::{unavailable_size_message, unavailable_space_message};

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
