use super::*;
impl App {
    pub(super) fn show_workspace_profiles(&mut self, ctx: &egui::Context) {
        if self.ui.modals.workspace_profiles.is_none() {
            return;
        }
        let opened = Self::take_modal_opened(ctx, UiModal::WorkspaceProfiles);
        let mut close = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::WorkspaceProfiles,
        ));
        let mut apply = None;
        let mut copy = None;
        let state = self
            .ui
            .modals
            .workspace_profiles
            .as_mut()
            .expect("profile editor");
        let response = super::sheet_window("Workspace profiles", self.colors, 14).default_width(560.).default_height(600.).show(ctx, |ui| {
            ui.set_width(540.0_f32.min(ui.available_width()));
            if state.exchange_open{
                ui.label("Profile JSON: review before adding. Apply separately from the profile list.");
                ui.label(format!("Selected saved profile: {}",state.selected.as_deref().unwrap_or("None")));
                let button=ui.add_enabled(state.selected.is_some(),egui::Button::new("Copy selected profile JSON"));
                #[cfg(feature="visual-qa")]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("profile-json-copy"),button.rect));
                if button.clicked(){match state.export_selected(){Ok(text)=>copy=Some(text),Err(error)=>state.error=Some(error)}}
                egui::ScrollArea::vertical().id_salt("profile-json-text").max_height(150.).show(ui,|ui|{
                    let edit=ui.add(egui::TextEdit::multiline(&mut state.exchange_text).desired_rows(6).desired_width(f32::INFINITY).char_limit(crate::workspace_profile::MAX_EXCHANGE_BYTES));
                    if edit.changed(){state.import_preview=None;}
                    #[cfg(feature="visual-qa")]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("profile-json-input"),edit.rect.intersect(ui.clip_rect())));
                });
                let review=ui.button("Review import JSON");
                #[cfg(feature="visual-qa")]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("profile-json-review"),review.rect));
                if review.clicked() && let Err(error)=state.review_import(){state.error=Some(error);}
                if let Some(profile)=&state.import_preview{
                    egui::ScrollArea::vertical().id_salt("profile-json-review").max_height(120.).show(ui,|ui|{
                        ui.label(format!("New profile: {}",profile.name));
                        for path in [&profile.left_root,&profile.right_root]{ui.add(egui::Label::new(path.to_string_lossy()).truncate()).on_hover_text(path.to_string_lossy());}
                        ui.label(format!("Filters: {:?} / {:?}",profile.left_filter,profile.right_filter));
                        ui.label(format!("Policies: {:?}; {:?}; {:?}",profile.durability,profile.name_policy,profile.symlink_policy));
                        if let Ok(Some(layout))=profile.layout(){for (side,tabs) in [("Left",&layout.left),("Right",&layout.right)]{for (index,tab) in tabs.entries.iter().enumerate(){ui.add(egui::Label::new(format!("{side} {}{}: {}",index+1,if index==tabs.active{" *"}else{""},tab.path.display())).truncate()).on_hover_text(tab.path.to_string_lossy());}}}
                    });
                }
                ui.horizontal_wrapped(|ui|{
                    let add=ui.add_enabled(state.import_preview.is_some(),egui::Button::new("Add reviewed as new profile"));
                    #[cfg(feature="visual-qa")]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("profile-json-add"),add.rect));
                    if add.clicked() && let Err(error)=state.add_reviewed_import(){state.error=Some(error);}
                    if ui.button("Back to profiles").clicked(){state.exchange_open=false;}
                    close|=ui.button("Close").clicked();
                });
                if let Some(error)=&state.error{ui.colored_label(ui.visuals().error_fg_color,error);}
                return;
            }

            egui::ScrollArea::vertical().id_salt("workspace-profiles-body").max_height((ctx.content_rect().height()-140.).max(160.)).show(ui,|ui|{
            ui.label(
                "Named tab sets, active-panel filters and operation policies. Apply replaces open tabs.",
            );
            egui::ScrollArea::vertical()
                .id_salt("workspace-profile-list")
                .max_height(160.)
                .show(ui, |ui| {
                    for profile in &state.book.profiles {
                        if ui
                            .selectable_label(
                                state.selected.as_deref() == Some(&profile.name),
                                &profile.name,
                            )
                            .clicked()
                        {
                            state.selected = Some(profile.name.clone());
                        }
                    }
                });
            if let Some(profile) = state.selected.as_deref().and_then(|n| state.book.get(n)) {
                ui.add(
                    egui::Label::new(format!("Left: {}", profile.left_root.display())).truncate(),
                )
                .on_hover_text(profile.left_root.to_string_lossy());
                ui.add(egui::Label::new(format!("Filter: {}", profile.left_filter)).truncate())
                    .on_hover_text(&profile.left_filter);
                ui.add(
                    egui::Label::new(format!("Right: {}", profile.right_root.display())).truncate(),
                )
                .on_hover_text(profile.right_root.to_string_lossy());
                ui.add(egui::Label::new(format!("Filter: {}", profile.right_filter)).truncate())
                    .on_hover_text(&profile.right_filter);
                match profile.layout(){
                    Ok(Some(layout))=>{ui.label(format!("Tabs: {} left / {} right; active: {}",layout.left.entries.len(),layout.right.entries.len(),if layout.active==0{"Left"}else{"Right"}));
                        for (side,tabs) in [("Left",&layout.left),("Right",&layout.right)]{for (index,tab) in tabs.entries.iter().enumerate(){ui.add(egui::Label::new(format!("{side} {}{}: {}",index+1,if index==tabs.active{" *"}else{""},tab.path.display())).truncate()).on_hover_text(tab.path.to_string_lossy());}}
                    }
                    Ok(None)=>{ui.small("Legacy profile: current folders only; background tabs stay.");}
                    Err(error)=>{ui.colored_label(ui.visuals().error_fg_color,error);}
                }
                ui.label(format!(
                    "Durability: {:?}; names: {:?}; links: {:?}",
                    profile.durability, profile.name_policy, profile.symlink_policy
                ));
            }
            ui.horizontal(|ui| {
                ui.label("New or renamed profile name");
                let edit = ui.add(egui::TextEdit::singleline(&mut state.name).char_limit(80));
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("profile-name-input"), edit.rect));
                #[cfg(not(feature = "visual-qa"))]
                let _ = edit;
            });
            ui.horizontal_wrapped(|ui| {
                let save = ui.button("Save current as new");
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("profile-save"), save.rect));
                if save.clicked() {
                    let mut next = state.book.clone();
                    let result = self.ws.capture_named_profile(state.name.clone())
                        .and_then(|profile|next.add_named(profile))
                        .and_then(|()| state.commit(next));
                    match result {
                        Ok(()) => state.selected = Some(state.name.clone()),
                        Err(e) => state.error = Some(e),
                    }
                }
                let button = ui.add_enabled(
                    state.selected.is_some(),
                    egui::Button::new("Replace selected from current panels"),
                );
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("profile-replace"), button.rect));
                if button.clicked() {
                    let mut next = state.book.clone();
                    let profile = self.ws.capture_named_profile(state.selected.clone().expect("selection"));
                    if let Err(e) = profile.and_then(|profile|next.replace_named(profile))
                        .and_then(|()| state.commit(next))
                    {
                        state.error = Some(e);
                    }
                }
            });
            ui.horizontal_wrapped(|ui|{
                let rename=ui.add_enabled(state.selected.is_some(),egui::Button::new("Rename selected"));
                #[cfg(feature="visual-qa")]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("profile-rename"),rename.rect));
                if rename.clicked() && let Err(error)=state.rename_selected(state.name.clone()){state.error=Some(error);}
                let duplicate=ui.add_enabled(state.selected.is_some(),egui::Button::new("Duplicate selected"));
                #[cfg(feature="visual-qa")]ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("profile-duplicate"),duplicate.rect));
                if duplicate.clicked() && let Err(error)=state.duplicate_selected(state.name.clone()){state.error=Some(error);}
            });
            });
            ui.horizontal_wrapped(|ui| {
                let button = ui.add_enabled(
                    state.selected.is_some(),
                    egui::Button::new("Apply selected"),
                );
                #[cfg(feature = "visual-qa")]
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("profile-apply"), button.rect));
                if button.clicked() {
                    apply = state
                        .selected
                        .as_deref()
                        .and_then(|n| state.book.get(n))
                        .cloned();
                }
                if ui.button("Import / export JSON…").clicked(){state.exchange_open=true;}
                if ui.button("Reload profiles").clicked() {
                    state.reload();
                }
                let button = ui.button("Close");
                if opened {
                    button.request_focus();
                }
                close |= button.clicked();
            });
            if let Some(error) = &state.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
        #[cfg(not(feature = "visual-qa"))]
        let _ = response;
        #[cfg(feature = "visual-qa")]
        if let Some(response) = response {
            crate::visual_qa::record_response(
                ctx,
                crate::visual_qa::ProbeId::Confirmation,
                &response.response,
            );
        }
        if let Some(text) = copy {
            self.write_clipboard(&text, "saved workspace profile JSON", ctx);
        }
        if let Some(profile) = apply {
            let state = self.ui.modals.workspace_profiles.take();
            if !self.ws.apply_named_profile(&profile) {
                self.ui.modals.workspace_profiles = state;
                if let Some(state) = self.ui.modals.workspace_profiles.as_mut() {
                    state.error=Some("Profile was not applied; finish pending work and unprotect current tabs, then retry".into());
                }
            }
        } else if close {
            self.ui.modals.workspace_profiles = None;
        }
    }
}
