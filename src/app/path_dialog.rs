//! Go-to-path input (Cmd+L) with debounced off-thread directory probing.

use super::*;

impl App {
    pub(crate) fn open_path(&mut self, ctx: &egui::Context) {
        let opening_panel = self.ws.active;
        let current = self
            .ws
            .active_panel_ref()
            .current_path
            .display()
            .to_string();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
        let dialog_id = self.issue_transient_nonce();
        let now = ctx.input(|input| input.time);
        self.ui.modals.path_input = Some(crate::path_probe::PathDialogState::new(
            dialog_id,
            opening_panel,
            current,
            home,
            now,
        ));
        Self::mark_modal_opened(ctx, UiModal::Path);
    }

    pub(crate) fn show_path_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::Path);
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Path);
        if self.ui.modals.path_input.is_none() {
            return;
        }

        let now = ctx.input(|input| input.time);
        let repaint = ctx.clone();
        let notify: std::sync::Arc<dyn Fn() + Send + Sync> =
            std::sync::Arc::new(move || repaint.request_repaint());
        if let Some(state) = self.ui.modals.path_input.as_mut() {
            state.probe.drive(
                now,
                &self.workload,
                std::sync::Arc::clone(&self.directory_probe),
                notify,
            );
        }

        let t = self.colors;
        let mut go: Option<(ActivePanel, std::path::PathBuf)> = None;
        let mut cancel = false;

        egui::Window::new("Go to path")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                let state = self
                    .ui
                    .modals
                    .path_input
                    .as_mut()
                    .expect("path dialog remains open while rendering");
                ui.set_width(440.0);
                ui.label(
                    egui::RichText::new("Go to folder")
                        .size(12.0)
                        .color(t.text_muted),
                );
                ui.add_space(6.0);
                let response = ui.add(
                    egui::TextEdit::singleline(state.probe.input_mut())
                        .desired_width(f32::INFINITY)
                        .hint_text("~/Documents")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_label("Folder path");
                });
                if just_opened {
                    response.request_focus();
                }
                if response.changed() {
                    state.probe.input_changed(now);
                }

                ui.add_space(4.0);
                let status = state.probe.status();
                let status_message = status.message();
                let status_color = match status {
                    crate::path_probe::ProbeStatus::Valid => t.accent,
                    crate::path_probe::ProbeStatus::Error(_)
                    | crate::path_probe::ProbeStatus::WorkerFailed(_) => t.accent_red,
                    crate::path_probe::ProbeStatus::Waiting
                    | crate::path_probe::ProbeStatus::Checking => t.text_muted,
                };
                let status_response = ui
                    .allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(status_message.clone())
                                        .size(11.0)
                                        .color(status_color),
                                )
                                .truncate(),
                            )
                        },
                    )
                    .inner
                    .on_hover_text(status_message);
                ui.ctx().accesskit_node_builder(status_response.id, |node| {
                    node.set_role(egui::accesskit::Role::Status);
                    node.set_live(egui::accesskit::Live::Polite);
                });

                let exact_input = state.probe.input().to_string();
                let resolved = state
                    .probe
                    .validated_path(&exact_input)
                    .map(std::path::Path::to_path_buf);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            resolved.is_some(),
                            egui::Button::new(
                                egui::RichText::new("Go").size(13.0).color(Color32::WHITE),
                            )
                            .fill(t.accent)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                        && let Some(path) = &resolved
                    {
                        go = Some((state.opening_panel, path.clone()));
                    }
                    ui.add_space(8.0);
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
                        cancel = true;
                    }
                    if ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && let Some(path) = &resolved
                    {
                        go = Some((state.opening_panel, path.clone()));
                    }
                    if escape_requested {
                        cancel = true;
                    }
                });

                if let Some(delay) = state.probe.repaint_after(now) {
                    ui.ctx().request_repaint_after(delay);
                }
            });

        if cancel {
            self.ui.modals.path_input = None;
            return;
        }
        if let Some((panel, path)) = go {
            self.ui.modals.path_input = None;
            match panel {
                ActivePanel::Left => self.ws.left.navigate_to(path),
                ActivePanel::Right => self.ws.right.navigate_to(path),
            }
        }
    }
}
