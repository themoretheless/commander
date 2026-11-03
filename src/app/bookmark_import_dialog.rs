//! Review GTK or XBEL bookmark text before appending to the existing Favorites store.
use super::*;
#[derive(Default)]
pub struct Dialog {
    source: String,
    preview: Option<crate::bookmark_exchange::Preview>,
    error: Option<String>,
    message: Option<String>,
}
fn source_editor(ui: &mut egui::Ui, source: &mut String) -> egui::Response {
    egui::ScrollArea::vertical()
        .id_salt("bookmark-import-source")
        .max_height(120.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(source)
                    .char_limit(crate::bookmark_exchange::MAX_BYTES + 1)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY)
                    .hint_text("file:///home/user/Documents Work documents"),
            )
        })
        .inner
}

impl App {
    pub(crate) fn open_bookmark_import(&mut self) {
        self.bookmark_import = Some(Dialog::default());
    }
    #[cfg(feature = "visual-qa")]
    pub(crate) fn bookmark_import_preview(&self) -> Option<&crate::bookmark_exchange::Preview> {
        self.bookmark_import.as_ref()?.preview.as_ref()
    }
    pub(crate) fn show_bookmark_import(&mut self, ctx: &egui::Context) {
        let Some(state) = self.bookmark_import.as_mut() else {
            return;
        };
        let mut open = true;
        let mut parse = false;
        let mut apply = false;
        let mut copy = false;
        let mut copy_xbel = false;
        egui::Window::new("Import Favorites — GTK / XBEL").default_width(680.0)
            .default_pos(egui::pos2((ctx.input(|input| input.viewport_rect().right())-720.0).max(16.0),80.0))
            .open(&mut open).collapsible(false).show(ctx,|ui| {
                ui.small("Paste GTK/Nautilus text or XBEL XML from KDE. XBEL folders become a flat list. Preview before adding. Existing labels and quick-jump slots are kept.");
                ui.small("Local paths only; no directory checks, mounts or folder creation. Missing paths can be retained. Up to 64 KiB; 1000 GTK lines or XBEL bookmarks.");
                let input=source_editor(ui,&mut state.source);
                input.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit,input.enabled(),"GTK or XBEL bookmark text to import"));
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("bookmark-import-input"),input.rect));
                if input.changed() {state.preview=None; state.error=None; state.message=None;}
                let response=ui.button("Preview bookmarks"); parse=response.clicked();
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("bookmark-import-preview"),response.rect));
                if let Some(error)=&state.error {ui.colored_label(ui.visuals().error_fg_color,error);}
                if let Some(message)=&state.message {ui.label(message);}
                if let Some(preview)=state.preview.as_mut() {
                    ui.small(format!("{} records reviewed · {} selected new favorites",preview.rows.len(),preview.selected()));
                    let row_height=2.0*(ui.text_style_height(&egui::TextStyle::Body)+ui.spacing().item_spacing.y)+4.0;
                    egui::ScrollArea::vertical().max_height(220.0).show_rows(ui,row_height,preview.rows.len(),|ui,range| {
                        for index in range {
                            let row=&mut preview.rows[index];
                            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(),row_height),egui::Layout::top_down(egui::Align::Min),|ui| {
                                ui.set_min_height(row_height);
                                ui.horizontal(|ui| {
                                    let response=ui.add_enabled(row.status==crate::bookmark_exchange::Status::Ready,egui::Checkbox::new(&mut row.selected,""));
                                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Checkbox,response.enabled(),format!("Import bookmark on line {}",row.line)));
                                    #[cfg(any(test,feature="visual-qa"))]
                                    ui.ctx().data_mut(|data| {data.insert_temp(egui::Id::new(("bookmark-import-row",row.line)),response.rect);data.insert_temp(egui::Id::new(("bookmark-import-record",index)),response.rect);});
                                    ui.label(format!("{} · {}",row.line,row.status.label()));
                                    if let Some(bookmark)=&row.bookmark {ui.add(egui::Label::new(bookmark.name.escape_debug().to_string()).truncate());}
                                });
                                let detail=match (&row.bookmark,&row.status) {
                                    (Some(bookmark),_)=>bookmark.path.to_string_lossy().escape_debug().to_string(),
                                    (_,crate::bookmark_exchange::Status::Invalid(error))=>error.clone(),
                                    _=>"URI is not a local file location; not imported".into(),
                                };
                                ui.add(egui::Label::new(&detail).truncate()).on_hover_text(detail);
                            });
                        }
                    });
                }
                let selected=state.preview.as_ref().map_or(0,crate::bookmark_exchange::Preview::selected);
                let response=ui.add_enabled(selected>0,egui::Button::new(format!("Add selected favorites ({selected})"))); apply=response.clicked();
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("bookmark-import-apply"),response.rect));
                let response=ui.button("Copy GTK bookmarks"); copy=response.clicked();
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("bookmark-import-export"),response.rect));
                let response=ui.button("Copy XBEL bookmarks");copy_xbel=response.clicked();
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("bookmark-import-export-xbel"),response.rect));
            });
        if !open {
            self.bookmark_import = None;
            return;
        }
        if parse {
            state.message = None;
            match crate::bookmark_exchange::parse(&state.source, &self.ws.bookmarks) {
                Ok(preview) => {
                    state.preview = Some(preview);
                    state.error = None;
                }
                Err(error) => {
                    state.preview = None;
                    state.error = Some(error);
                }
            }
        }
        if apply && let Some(preview) = state.preview.as_ref() {
            match self.ws.import_favorites(&state.source, preview) {
                Ok(added) => {
                    state.message = Some(format!(
                        "Added {added} favorites. Existing entries and slots were kept."
                    ));
                    state.error = None;
                    state.preview = preview.rebase(&self.ws.bookmarks).ok();
                }
                Err(error) => state.error = Some(error),
            }
        }
        if copy {
            self.ws.execute(crate::command::Command::ExportFavoritesGtk);
        }
        if copy_xbel {
            self.ws
                .execute(crate::command::Command::ExportFavoritesXbel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_thousand_source_lines_do_not_push_review_controls_offscreen() {
        let ctx = egui::Context::default();
        let mut source = "file:///missing/folder Work docs\n".repeat(1000);
        let original = source.clone();
        let mut button = None;
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 500.0),
                )),
                ..Default::default()
            },
            |ui| {
                source_editor(ui, &mut source);
                button = Some(ui.button("Preview bookmarks").rect);
            },
        ));
        assert!(
            button.unwrap().bottom() < 180.0,
            "source editor must remain bounded"
        );
        assert_eq!(source, original);
    }
}
