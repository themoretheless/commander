//! Checksum-manifest UI; the workload hash lane owns file reads.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

pub struct Dialog {
    path: PathBuf,
    algorithm: Option<crate::checksum::Algorithm>,
    task: Option<ListTask>,
    report: Option<crate::checksum_list::Report>,
    error: Option<String>,
    view: crate::checksum_list_view::View,
}
struct ListTask {
    receiver: Receiver<crate::checksum_list::Report>,
    handle: crate::workload::TaskHandle,
    completed: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    stopping: bool,
}
impl Drop for ListTask {
    fn drop(&mut self) {
        self.handle.cancel();
    }
}
impl App {
    pub(crate) fn open_checksum_list(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.checksum_task.is_some() {
            self.toast(
                ctx,
                crate::toasts::ToastKind::Info,
                "Wait for the current checksum calculation",
                false,
            );
            return;
        }
        self.checksum_report = None;
        self.checksum_list_dialog = Some(Dialog {
            path,
            algorithm: None,
            task: None,
            report: None,
            error: None,
            view: Default::default(),
        });
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_list_report(&self) -> Option<&crate::checksum_list::Report> {
        self.checksum_list_dialog
            .as_ref()
            .filter(|state| state.task.is_none())
            .and_then(|state| state.report.as_ref())
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_list_view_is(
        &self,
        filter: crate::checksum_list_view::Filter,
        query: &str,
        names: &[&str],
    ) -> bool {
        self.checksum_list_dialog.as_ref().is_some_and(|state| {
            state.task.is_none()
                && state.view.is_current()
                && state.view.options.filter == filter
                && state.view.options.query == query
                && state.report.as_ref().is_some_and(|report| {
                    state
                        .view
                        .indices
                        .iter()
                        .filter_map(|&index| report.rows.get(index).map(|row| row.name.as_str()))
                        .eq(names.iter().copied())
                })
        })
    }

    pub(crate) fn show_checksum_list_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = self.checksum_list_dialog.as_mut() else {
            return;
        };
        if let Some(task) = &state.task {
            match task.receiver.try_recv() {
                Ok(report) => {
                    state.report = Some(report);
                    state.view.invalidate();
                    state.task = None;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100))
                }
                Err(TryRecvError::Disconnected) => {
                    state.error = Some(
                        if task.stopping {
                            "Verification cancelled before returning records"
                        } else {
                            "Verification stopped before returning a result"
                        }
                        .into(),
                    );
                    state.task = None;
                }
            }
        }
        let mut open = true;
        let mut run = false;
        let mut copy = None;
        egui::Window::new("Verify checksum list").default_width(650.0)
            .default_pos(egui::pos2((ctx.input(|input| input.viewport_rect().right()) - 690.0).max(16.0), 96.0)).open(&mut open).collapsible(false).show(ctx, |ui| {
            ui.add(egui::Label::new(state.path.display().to_string()).truncate()).on_hover_text(state.path.display().to_string());
            ui.small("Paths are relative to the list folder; outside paths and symlinks are refused. UTF-8, up to 1 MiB / 10000 records.");
            ui.small("Auto uses recognized filenames or unambiguous tagged/GNU/SFV content. Untagged 64-digit hashes need SHA-256/BLAKE3 selection. One algorithm per list.");
            ui.add_enabled_ui(state.task.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("List algorithm");
                    let auto = ui.selectable_value(&mut state.algorithm, None, "Auto");
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("checksum-list-algorithm", "Auto")), auto.rect));
                    if auto.changed() { state.report = None; state.error = None; }
                    for algorithm in crate::checksum::Algorithm::ALL {
                        let response = ui.selectable_value(&mut state.algorithm, Some(algorithm), if algorithm == crate::checksum::Algorithm::Crc32 { "CRC32 / SFV" } else { algorithm.label() });
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("checksum-list-algorithm", algorithm.label())), response.rect));
                        if response.changed() { state.report = None; state.error = None; }
                    }
                });
                if state.algorithm==Some(crate::checksum::Algorithm::Sha1) { ui.small("SHA-1 is provided for legacy checksum compatibility."); }
                let response = ui.button("Verify list");
                #[cfg(feature = "visual-qa")]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-list-run"), response.rect));
                run = response.clicked();
            });
            if let Some(task) = &mut state.task {
                ui.label(format!("Verifying {} of {} records…", task.completed.load(Ordering::Acquire), task.total.load(Ordering::Acquire)));
                if ui.add_enabled(!task.stopping, egui::Button::new(if task.stopping { "Stopping…" } else { "Cancel verification" })).clicked() { task.stopping = true; task.handle.cancel(); }
            }
            if let Some(error) = &state.error { ui.colored_label(ui.visuals().error_fg_color, error); }
            if let Some(report) = &state.report {
                if report.error.is_none() && let Some(algorithm) = report.algorithm { ui.label(format!("Verified using {}", algorithm.label())); }
                ui.label(report.summary());
                ui.horizontal_wrapped(|ui| {
                    for filter in crate::checksum_list_view::Filter::ALL {
                        let response = ui.selectable_value(&mut state.view.options.filter, filter, filter.label());
                        #[cfg(any(test, feature = "visual-qa"))]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("checksum-list-filter", filter.label())), response.rect));
                        #[cfg(not(any(test, feature = "visual-qa")))]
                        let _ = response;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut state.view.options.query).char_limit(256).desired_width(240.0).hint_text("Find filename (ignores case)"));
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, response.enabled(), "Find checksum result filename"));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("checksum-list-filter", "query")), response.rect));
                    if ui.add_enabled(!state.view.options.query.is_empty(), egui::Button::new("Clear search")).clicked() {
                        state.view.options.query.clear(); response.request_focus();
                    }
                });
                let changed = state.view.refresh(report);
                ui.small(format!("{} of {} completed records shown; summary covers the full verification.", state.view.indices.len(), report.rows.len()));
                if state.view.indices.is_empty() && !report.rows.is_empty() { ui.label("No matching completed records. Change the status or filename filter."); }
                let row_height = 4.0 * (ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y);
                let mut scroll = egui::ScrollArea::both().max_height(350.0);
                if changed { scroll = scroll.vertical_scroll_offset(0.0); }
                scroll.show_rows(ui, row_height, state.view.indices.len(), |ui, range| {
                    for visible in range {
                        let row = &report.rows[state.view.indices[visible]];
                        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), row_height), egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.set_min_height(row_height);
                            let color = if row.status == crate::checksum_list::Status::Match { ui.visuals().text_color() } else { ui.visuals().error_fg_color };
                            ui.horizontal(|ui| { ui.colored_label(color, row.status.label()); ui.add(egui::Label::new(row.name.escape_debug().to_string()).truncate()).on_hover_text(&row.name); });
                            for text in [format!("Expected {}", row.expected), format!("Actual {}", row.actual.as_deref().unwrap_or("-"))] {
                                ui.add(egui::Label::new(&text).truncate()).on_hover_text(text);
                            }
                            if let crate::checksum_list::Status::Error(error) = &row.status { ui.add(egui::Label::new(error).truncate()).on_hover_text(error); }
                        });
                    }
                });
                let response = ui.button("Copy verification report");
                #[cfg(feature = "visual-qa")]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-list-copy"), response.rect));
                if response.clicked() { copy = Some(Ok(report.text())); }
                let response = ui.add_enabled(!state.view.indices.is_empty(), egui::Button::new(format!("Copy visible report ({})", state.view.indices.len())));
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-list-copy-visible"), response.rect));
                if response.clicked() { copy = Some(crate::checksum_list_view::visible_text(report, &state.view)); }
            }
        });
        if !open {
            self.checksum_list_dialog = None;
            return;
        }
        if run {
            let path = state.path.clone();
            let algorithm = state.algorithm;
            let completed = Arc::new(AtomicUsize::new(0));
            let total = Arc::new(AtomicUsize::new(0));
            let worker_completed = completed.clone();
            let worker_total = total.clone();
            let repaint = ctx.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            self.checksum_generation = self.checksum_generation.wrapping_add(1).max(1);
            let root = path
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .to_path_buf();
            match self.workload.submit(
                crate::workload::TaskSpec::new(
                    crate::workload::TaskKind::Hash,
                    root,
                    self.checksum_generation,
                )
                .priority(crate::workload::Priority::Interactive),
                move |cancelled| {
                    let report = crate::checksum_list::verify(
                        &path,
                        algorithm,
                        &|| cancelled.is_cancelled(),
                        &worker_completed,
                        &worker_total,
                    );
                    let _ = sender.send(report);
                    repaint.request_repaint();
                },
            ) {
                Ok(handle) => {
                    state.report = None;
                    state.error = None;
                    state.task = Some(ListTask {
                        receiver,
                        handle,
                        completed,
                        total,
                        stopping: false,
                    });
                }
                Err(error) => state.error = Some(format!("Could not start verification: {error}")),
            }
        }
        if let Some(result) = copy {
            match result {
                Ok(text) => self.write_clipboard(&text, "checksum-list verification report", ctx),
                Err(error) => self.toast(ctx, crate::toasts::ToastKind::Error, error, false),
            }
        }
    }
}
