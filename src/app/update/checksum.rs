//! Checksum calculation, verification state and its dialog.

use super::*;

impl App {
    pub(crate) fn start_checksum(&mut self, paths: Vec<std::path::PathBuf>, ctx: &egui::Context) {
        if paths.is_empty() || self.checksum_task.is_some() {
            return;
        }
        let mode = self.checksum_calculation;
        let algorithms = match mode {
            super::ChecksumCalculation::All => crate::checksum::AlgorithmSet::all(),
            super::ChecksumCalculation::Selected => {
                crate::checksum::AlgorithmSet::one(self.checksum_algorithm)
            }
            super::ChecksumCalculation::Checked => self.checksum_chosen_algorithms,
        };
        if algorithms.is_empty() {
            self.toast(
                ctx,
                crate::toasts::ToastKind::Error,
                "Select at least one checksum algorithm",
                false,
            );
            return;
        }
        self.checksum_list_dialog = None;
        self.checksum_generation = self.checksum_generation.wrapping_add(1).max(1);
        let generation = self.checksum_generation;
        let total = paths.len();
        let captured_paths = paths.clone();
        let root = paths
            .first()
            .and_then(|path| path.parent())
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let completed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_completed = std::sync::Arc::clone(&completed);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let repaint = ctx.clone();
        match self.workload.submit(
            crate::workload::TaskSpec::new(crate::workload::TaskKind::Hash, root, generation)
                .priority(crate::workload::Priority::Interactive),
            move |cancelled| {
                let report = crate::checksum::verify_paths_cancellable(
                    paths,
                    &cancelled,
                    worker_completed.as_ref(),
                    algorithms,
                );
                let _ = sender.send(report);
                repaint.request_repaint();
            },
        ) {
            Ok(task) => {
                self.checksum_report = None;
                self.checksum_expected.clear();
                self.checksum_encoding = crate::checksum::DigestEncoding::default();
                self.checksum_algorithm = if mode == super::ChecksumCalculation::All {
                    crate::checksum::Algorithm::default()
                } else if algorithms.contains(self.checksum_algorithm) {
                    self.checksum_algorithm
                } else {
                    algorithms.iter().next().expect("nonempty requested set")
                };
                self.checksum_calculated_algorithms = algorithms;
                self.checksum_paths = captured_paths;
                self.checksum_relative_paths = false;
                self.checksum_relative_base = None;
                self.checksum_task = Some(super::ChecksumTask {
                    receiver,
                    task,
                    completed,
                    total,
                    cancel_requested: false,
                });
            }
            Err(error) => self.toast(
                ctx,
                crate::toasts::ToastKind::Error,
                format!("Could not start checksum verification: {error}"),
                false,
            ),
        }
    }

    pub(super) fn poll_checksum(&mut self, ctx: &egui::Context) {
        let Some(task) = self.checksum_task.as_ref() else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(report) => {
                let summary = report.summary_line();
                let failed = report.rows.iter().any(|row| row.error.is_some());
                self.checksum_task = None;
                self.checksum_report = Some(report);
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
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let cancelled = task.cancel_requested;
                self.checksum_task = None;
                if cancelled {
                    self.checksum_report = Some(crate::checksum::ChecksumReport {
                        cancelled: true,
                        ..Default::default()
                    });
                }
                self.toast(
                    ctx,
                    if cancelled {
                        crate::toasts::ToastKind::Info
                    } else {
                        crate::toasts::ToastKind::Error
                    },
                    if cancelled {
                        "Checksum calculation cancelled"
                    } else {
                        "Checksum calculation stopped before returning a result"
                    },
                    false,
                );
            }
        }
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_relative_base_is(&self, base: &std::path::Path) -> bool {
        self.checksum_relative_paths
            && self
                .checksum_relative_base
                .as_ref()
                .and_then(|base| base.as_ref().ok())
                .is_some_and(|value| value == base)
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_relative_mode_is(&self, enabled: bool) -> bool {
        self.checksum_relative_paths == enabled
            && (!enabled
                || self
                    .checksum_relative_base
                    .as_ref()
                    .is_some_and(Result::is_ok))
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_comparison_is(
        &self,
        algorithm: crate::checksum::Algorithm,
        expected: crate::checksum::ExpectedMatch,
    ) -> bool {
        self.checksum_task.is_none()
            && self.checksum_algorithm == algorithm
            && self.checksum_report.as_ref().is_some_and(|report| {
                report.compare_expected_encoded(
                    &self.checksum_expected,
                    algorithm,
                    self.checksum_encoding,
                ) == expected
            })
    }

    /// Exercise the same session snapshot/store/restore path without touching
    /// the user's store: native QA injects `EphemeralPersist` into the App.
    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_reopen_preferences_for_qa(
        &mut self,
        ctx: &egui::Context,
    ) -> Result<serde_json::Value, String> {
        if self.checksum_task.is_some() {
            return Err("Checksum task is still running".into());
        }
        let session = self.to_session();
        crate::session::save_with(self.persistence.as_ref(), &session, &mut self.session_gate)
            .map_err(|e| format!("{e:?}"))?;
        let loaded = crate::session::load_with(self.persistence.as_ref())
            .value
            .ok_or("Missing saved session")?;
        let paths = self.checksum_paths.clone();
        self.restore_checksum_preferences(&serde_json::Value::Null);
        self.checksum_report = None;
        self.checksum_paths.clear();
        self.restore_checksum_preferences(&loaded.checksum_preferences);
        let restored = self.checksum_preferences().saved();
        if restored != session.checksum_preferences {
            return Err("Checksum choices changed on reopen".into());
        }
        self.start_checksum(paths, ctx);
        Ok(serde_json::json!({"saved":session.checksum_preferences,"restored":restored}))
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn checksum_calculation_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"selected":self.checksum_algorithm.label(),"comparable":self.checksum_report.as_ref().is_some_and(|report|report.single_digest(self.checksum_algorithm).is_some()),"summary":self.checksum_report.as_ref().map(|report|report.summary_line()),"busy":self.checksum_task.is_some(),"only":self.checksum_calculated_algorithms.single().map(|a|a.label()),"requested":self.checksum_calculated_algorithms.iter().map(|a|a.label()).collect::<Vec<_>>(),"checked":self.checksum_chosen_algorithms.iter().map(|a|a.label()).collect::<Vec<_>>(),"paths":self.checksum_paths,"rows":self.checksum_report.as_ref().map(|report|report.rows.iter().map(|row|serde_json::json!({"path":row.path,"algorithms":crate::checksum::Algorithm::ALL.into_iter().filter(|a|a.digest(row).is_some()).map(|a|a.label()).collect::<Vec<_>>(),"error":row.error,"skipped":row.skipped,"content":row.content_hash})).collect::<Vec<_>>())})
    }

    pub(super) fn show_checksum_dialog(&mut self, ctx: &egui::Context) {
        if self.checksum_task.is_none() && self.checksum_report.is_none() {
            return;
        }
        let mut open = true;
        let mut copy = None;
        let mut cancel = false;
        let mut export_error = None;
        let mut recalculate = None;
        egui::Window::new("Checksums").default_width(590.0)
            .default_pos(egui::pos2((ctx.input(|input| input.viewport_rect().right()) - 620.0).max(16.0), 96.0))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("checksum-sheet").max_height((ctx.input(|input| input.viewport_rect().height()) - 160.0).max(120.0)).show(ui, |ui| {
                #[cfg(feature = "visual-qa")]
                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-sheet-clip"), ui.clip_rect()));
                if let Some(task) = &self.checksum_task {
                    let completed = task.completed.load(std::sync::atomic::Ordering::Acquire);
                    ui.label(format!("Hashing {completed} of {} files…", task.total));
                    if ui.add_enabled(!task.cancel_requested, egui::Button::new(if task.cancel_requested { "Stopping…" } else { "Cancel" })).clicked() {
                        cancel = true;
                    }
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                if let Some(report) = &self.checksum_report {
                    ui.label(report.summary_line());
                    let calculated=self.checksum_calculated_algorithms;
                    if calculated!=crate::checksum::AlgorithmSet::all() {ui.small(format!("Calculated {}",calculated.iter().map(|a|a.label()).collect::<Vec<_>>().join(", ")));}
                    let digest_lines = (calculated.len()+usize::from(calculated==crate::checksum::AlgorithmSet::all())) as f32;
                    let row_height = ui.text_style_height(&egui::TextStyle::Body) + digest_lines * ui.text_style_height(&egui::TextStyle::Monospace) + 2.0 * ui.spacing().item_spacing.y + 8.0;
                    egui::ScrollArea::both().max_height(320.0).show_rows(ui, row_height, report.rows.len(), |ui, range| {
                        ui.set_min_width(550.0);
                        for row in &report.rows[range] {
                            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), row_height), egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.set_min_height(row_height);
                                let path = row.path.to_string_lossy().escape_debug().to_string();
                                ui.add(egui::Label::new(&path).truncate()).on_hover_text(&path);
                                if let Some(message) = row.error.as_ref().or(row.skipped.as_ref()) { ui.add(egui::Label::new(message).truncate()).on_hover_text(message); }
                                else { ui.add(egui::Label::new(egui::RichText::new(if calculated==crate::checksum::AlgorithmSet::all() {row.display_digests()}else{calculated.iter().map(|algorithm|format!("{} {}",algorithm.label(),algorithm.digest(row).unwrap_or("-"))).collect::<Vec<_>>().join("\n")}).monospace()).wrap_mode(egui::TextWrapMode::Extend)); }
                            });
                        }
                    });
                    let old_algorithm = self.checksum_algorithm;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Compare as");
                        for algorithm in crate::checksum::Algorithm::ALL {
                            let choice = ui.selectable_value(&mut self.checksum_algorithm, algorithm, algorithm.label());
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("checksum-algorithm", algorithm.label())), choice.rect));
                            #[cfg(not(feature = "visual-qa"))]
                            let _ = choice;
                        }
                    });
                    if self.checksum_algorithm == crate::checksum::Algorithm::Sha1 { ui.small("SHA-1: legacy checksum compatibility; use SHA-256 for new security-sensitive checks."); }
                    if matches!(self.checksum_algorithm,crate::checksum::Algorithm::Xxh3_64|crate::checksum::Algorithm::Xxh3_128) {ui.small("XXH3: non-cryptographic checksum; use SHA-256 for security-sensitive checks.");}
                    let details=egui::CollapsingHeader::new("Calculation algorithms").show(ui,|ui| {
                        egui::Grid::new("checksum-algorithm-grid").num_columns(4).spacing(egui::vec2(12.0,6.0)).show(ui,|ui| {
                            for (index,algorithm) in crate::checksum::Algorithm::ALL.into_iter().enumerate() {
                                let mut checked=self.checksum_chosen_algorithms.contains(algorithm);
                                let response=ui.push_id(("checksum-calculation-set",algorithm.label()),|ui|ui.checkbox(&mut checked,algorithm.label())).inner;
                                #[cfg(feature="visual-qa")]
                                ui.ctx().data_mut(|data|{data.insert_temp(egui::Id::new(("checksum-set-algorithm",algorithm.label())),response.rect);data.insert_temp(egui::Id::new(("checksum-set-visible",algorithm.label())),ui.clip_rect().contains_rect(response.rect));});
                                if response.changed(){self.checksum_chosen_algorithms.set(algorithm,checked);}
                                if index%4==3 {ui.end_row();}
                            }
                        });
                        ui.small("Editing this set does not change completed results. Apply with Recalculate checked set.");
                    });
                    #[cfg(feature="visual-qa")]
                    ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("checksum-calculation-set"),details.header_response.rect));
                    let _=details;
                    ui.horizontal_wrapped(|ui| {
                        for (mode,label) in [(super::ChecksumCalculation::Selected,"Recalculate selected only"),(super::ChecksumCalculation::All,"Recalculate all"),(super::ChecksumCalculation::Checked,"Recalculate checked set")] {
                            let button=ui.add_enabled(!self.checksum_paths.is_empty() && (mode!=super::ChecksumCalculation::Checked || !self.checksum_chosen_algorithms.is_empty()),egui::Button::new(label)).on_hover_text("Read the captured files afresh with the selected scope. This calculation mode and the checked set are remembered across restarts. Cancellation and changed-file checks remain enabled.");
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|data|{data.insert_temp(egui::Id::new(label),button.rect);data.insert_temp(egui::Id::new((label,"enabled")),button.enabled());});
                            if button.clicked(){recalculate=Some(mode);}
                        }
                    });
                    let algorithm = self.checksum_algorithm;
                    let comparable = report.single_digest(algorithm).is_some();
                    let expected_label = format!("Expected {}", algorithm.label());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&expected_label);
                        for encoding in [crate::checksum::DigestEncoding::Hex, crate::checksum::DigestEncoding::Base64] {
                            let option = ui.selectable_value(&mut self.checksum_encoding, encoding, encoding.label());
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("checksum-encoding", encoding.label())), option.rect));
                            #[cfg(not(feature = "visual-qa"))]
                            let _ = option;
                        }
                    });
                    let encoding_hint = match self.checksum_encoding { crate::checksum::DigestEncoding::Hex => format!("Paste {} hexadecimal characters", algorithm.hex_len()), crate::checksum::DigestEncoding::Base64 => "Paste standard padded Base64 digest".into() };
                    let expected = ui.add_enabled(comparable, egui::TextEdit::singleline(&mut self.checksum_expected).desired_width(f32::INFINITY).char_limit(256).hint_text(encoding_hint));
                    expected.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, expected.enabled(), &expected_label));
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-expected"), expected.rect));
                    use crate::checksum::ExpectedMatch;
                    let invalid_message = match self.checksum_encoding { crate::checksum::DigestEncoding::Hex => format!("Enter exactly {} hexadecimal characters", algorithm.hex_len()), crate::checksum::DigestEncoding::Base64 => format!("Enter standard padded Base64 for a {}-byte digest", algorithm.hex_len()/2) };
                    let (message, color) = match report.compare_expected_encoded(&self.checksum_expected, self.checksum_algorithm, self.checksum_encoding) {
                        ExpectedMatch::Empty if comparable => ("Paste an expected digest to compare this file", ui.visuals().text_color()),
                        ExpectedMatch::Empty | ExpectedMatch::Unavailable => ("Select one completed file to compare an expected digest", ui.visuals().text_color()),
                        ExpectedMatch::Invalid => (invalid_message.as_str(), ui.visuals().error_fg_color),
                        ExpectedMatch::Match => ("Digest matches the selected algorithm", self.colors.accent),
                        ExpectedMatch::Mismatch => ("Digest does not match the selected algorithm", ui.visuals().error_fg_color),
                    };
                    ui.colored_label(color, message);
                    let relative = ui.checkbox(&mut self.checksum_relative_paths, "Relative paths")
                        .on_hover_text("Portable lists rooted at the common directory of completed files. Save the list there and verify from that directory; filenames start with ./.");
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-relative-paths"), relative.rect));
                    if !self.checksum_relative_paths { self.checksum_relative_base = None; }
                    else if relative.changed() || old_algorithm != algorithm || self.checksum_relative_base.is_none() {
                        self.checksum_relative_base = Some(report.list_base(algorithm));
                    }
                    if self.checksum_relative_paths && let Some(base) = &self.checksum_relative_base {
                        match base {
                            Ok(base) => {
                                let text = format!("Save list in: {}", base.to_string_lossy().escape_debug());
                                ui.add(egui::Label::new(&text).truncate()).on_hover_text(text);
                            }
                            Err(error) => { ui.colored_label(ui.visuals().error_fg_color, error); }
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(!report.rows.is_empty(), egui::Button::new("Copy report")).clicked() { copy = Some(report.detailed_text()); }
                        let copy_digest = ui.add_enabled(comparable, egui::Button::new(format!("Copy {} digest", algorithm.label())));
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-copy-digest"), copy_digest.rect));
                        if copy_digest.clicked() {
                            copy = report.single_digest(algorithm).map(str::to_owned);
                        }
                        let copy_base64 = ui.add_enabled(comparable, egui::Button::new("Copy Base64"))
                            .on_hover_text(format!("Copy {} digest bytes as standard padded Base64. Choose Hex or Base64 for expected values; checksum lists use hexadecimal.", algorithm.label()));
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-copy-base64"), copy_base64.rect));
                        if copy_base64.clicked() { copy = report.single_digest_base64(algorithm); }
                        let list_algorithm = algorithm;
                        let list_label = if list_algorithm == crate::checksum::Algorithm::Crc32 { "SFV" } else { list_algorithm.label() };
                        let list_ready = !self.checksum_relative_paths || self.checksum_relative_base.as_ref().is_some_and(Result::is_ok);
                        let scope = if self.checksum_relative_paths { "relative " } else { "" };
                        let copy_list = ui.add_enabled(report.successful() > 0 && list_ready, egui::Button::new(format!("Copy {scope}{} list ({})", list_label, report.successful()))).on_hover_text("Computed files only; skipped, failed and unprocessed entries are omitted");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("checksum-copy-list"), copy_list.rect));
                        if copy_list.clicked() {
                            let base = if self.checksum_relative_paths { self.checksum_relative_base.as_ref().and_then(|base| base.as_ref().ok()).map(std::path::PathBuf::as_path) } else { None };
                            // A missing/invalid relative base never falls back to absolute export.
                            if list_ready {
                                let result = match base { Some(base) => report.checksum_list_at(list_algorithm, Some(base)), None => report.checksum_list(list_algorithm) };
                                match result { Ok(text) => copy = Some(text), Err(error) => export_error = Some(error) }
                            }
                        }
                    });
                }
                });
            });
        if let Some(text) = copy {
            self.write_clipboard(&text, "checksum report", ctx);
        }
        if let Some(error) = export_error {
            self.toast(ctx, crate::toasts::ToastKind::Error, error, false);
        }
        if cancel && let Some(task) = self.checksum_task.as_mut() {
            task.cancel_requested = true;
            task.task.cancel();
        }
        if let Some(mode) = recalculate {
            self.checksum_calculation = mode;
            self.start_checksum(self.checksum_paths.clone(), ctx);
        }
        if !open {
            self.checksum_paths.clear();
            if let Some(task) = self.checksum_task.take() {
                task.task.cancel();
            }
            self.checksum_report = None;
        }
    }
}
