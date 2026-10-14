//! Review panel for integrity-uncertain operation failures. It sits at the
//! top of the Operations panel's Recovery tab; the Activity strip announces
//! it. Mutations stay blocked by the workspace until the review completes.

use super::*;

fn review_recovery_request(operation_id: &crate::operation::OperationId) -> UiRequest {
    UiRequest::ReviewRecovery(operation_id.clone())
}

/// Where "Show location" lands for an affected path.
#[derive(Debug, PartialEq, Eq)]
enum LocationTarget {
    /// Its folder still exists: open it with the cursor on the item.
    Reveal(PathBuf),
    /// Its folder is gone: open the nearest folder that still exists.
    Nearest(PathBuf),
    /// Nothing on the path exists any more.
    Gone,
}

fn location_target(
    path: &std::path::Path,
    is_dir: impl Fn(&std::path::Path) -> bool,
) -> LocationTarget {
    match path.parent() {
        Some(parent) if is_dir(parent) => LocationTarget::Reveal(path.to_path_buf()),
        Some(parent) => parent
            .ancestors()
            .skip(1)
            .find(|dir| !dir.as_os_str().is_empty() && is_dir(dir))
            .map_or(LocationTarget::Gone, |dir| {
                LocationTarget::Nearest(dir.to_path_buf())
            }),
        None => LocationTarget::Gone,
    }
}

impl App {
    pub(crate) fn show_safe_state_details(&mut self, ui: &mut egui::Ui) {
        let Some(state) = self.ws.safe_state.clone() else {
            return;
        };
        let t = self.colors;
        let mut acknowledge = false;
        let mut inspect = false;
        let mut recovery = false;

        Frame::NONE
            .fill(t.bg_card)
            .inner_margin(Margin::same(10))
            .stroke(Stroke::new(1.0, t.accent_warning))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("Operation paused for integrity review")
                        .size(13.0)
                        .strong()
                        .color(t.accent_warning),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(&state.reason)
                        .size(12.0)
                        .color(t.text_primary),
                );
                ui.label(
                    egui::RichText::new(&state.operation_id.0)
                        .size(10.0)
                        .monospace()
                        .color(t.text_muted),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for failure in &state.failures {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(failure.class.label())
                                        .size(10.0)
                                        .strong()
                                        .color(t.accent_warning),
                                );
                                ui.label(
                                    egui::RichText::new(&failure.message)
                                        .size(11.0)
                                        .color(t.text_secondary),
                                );
                            });
                            if let Some(path) = &failure.path {
                                ui.label(
                                    egui::RichText::new(path.display().to_string())
                                        .size(10.0)
                                        .monospace()
                                        .color(t.text_muted),
                                );
                            }
                            ui.add_space(4.0);
                        }
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(super::dialog_button(
                            t,
                            super::ButtonRole::Secondary,
                            egui::RichText::new("Recovery center").size(13.0),
                        ))
                        .clicked()
                    {
                        recovery = true;
                    }
                    if !state.paths.is_empty()
                        && ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Show location").size(13.0),
                            ))
                            .clicked()
                    {
                        inspect = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Primary,
                                egui::RichText::new("Review complete").size(13.0),
                            ))
                            .clicked()
                        {
                            acknowledge = true;
                        }
                    });
                });
            });

        if inspect && let Some(path) = state.paths.first() {
            let ctx = ui.ctx().clone();
            match location_target(path, std::path::Path::is_dir) {
                LocationTarget::Reveal(path) => self.ws.reveal(&path),
                LocationTarget::Nearest(dir) => {
                    self.toast(
                        &ctx,
                        crate::toasts::ToastKind::Info,
                        format!(
                            "{} no longer exists; showing {} instead",
                            path.parent().unwrap_or(path).display(),
                            dir.display()
                        ),
                        false,
                    );
                    self.ws.active_panel().navigate_to(dir);
                }
                LocationTarget::Gone => self.toast(
                    &ctx,
                    crate::toasts::ToastKind::Error,
                    format!("Location no longer exists: {}", path.display()),
                    false,
                ),
            }
        }
        if acknowledge {
            self.ws.acknowledge_safe_state();
        }
        if recovery {
            self.ws
                .emit_ui_request(review_recovery_request(&state.operation_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_state_review_preserves_the_operation_identity() {
        let operation_id = crate::operation::OperationId("safe-state-op".to_string());

        assert_eq!(
            review_recovery_request(&operation_id),
            UiRequest::ReviewRecovery(operation_id)
        );
    }

    #[test]
    fn show_location_reveals_the_item_or_falls_back_to_an_existing_ancestor() {
        use std::path::Path;
        let existing = |dirs: &'static [&'static str]| {
            move |dir: &Path| dirs.iter().any(|d| Path::new(d) == dir)
        };
        assert_eq!(
            location_target(Path::new("/a/b/file"), existing(&["/a/b", "/a", "/"])),
            LocationTarget::Reveal(PathBuf::from("/a/b/file"))
        );
        assert_eq!(
            location_target(Path::new("/a/b/file"), existing(&["/a", "/"])),
            LocationTarget::Nearest(PathBuf::from("/a"))
        );
        assert_eq!(
            location_target(Path::new("gone/file"), existing(&[])),
            LocationTarget::Gone
        );
    }
}
