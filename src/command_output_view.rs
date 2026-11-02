//! Searchable retained command output. The modal owns and drops background find jobs.
use crate::{app::CommandOutput, preview_find::PreviewFind, theme::ThemeColors};
use egui::{Color32, Galley, Id};
use std::{path::Path, sync::Arc};

#[derive(PartialEq)]
struct Key {
    stdout: String,
    stderr: String,
    failed: bool,
    width: f32,
    scale: f32,
    colors: [Color32; 3],
    text_percent: u16,
}
pub struct View {
    find: PreviewFind,
    lines: crate::preview_lines::PreviewLines,
    numbers: bool,
    wrap: bool,
    text_percent: u16,
    key: Option<Key>,
    galley: Option<Arc<Galley>>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            find: Default::default(),
            lines: Default::default(),
            numbers: false,
            wrap: true,
            text_percent: 100,
            key: None,
            galley: None,
        }
    }
}
impl View {
    #[cfg(any(test, feature = "visual-qa"))]
    pub fn search_state(&self) -> (usize, Option<usize>) {
        (self.find.match_count(), self.find.current_match())
    }
    #[cfg(feature = "visual-qa")]
    pub fn regex_ready(&self) -> bool {
        self.find.regex_ready()
    }
    pub fn owns_input(&self, ctx: &egui::Context, id: Id) -> bool {
        ctx.memory(|memory| {
            memory.has_focus(id.with("query")) || memory.has_focus(id.with("lines").with("input"))
        })
    }
    #[cfg(any(test, feature = "visual-qa"))]
    pub fn line_state(&self) -> (bool, usize, Option<usize>, bool, bool) {
        let (count, revealed, error, visible) = self.lines.navigation_state();
        (self.numbers, count, revealed, error, visible)
    }
    #[cfg(any(test, feature = "visual-qa"))]
    pub fn text_percent(&self) -> u16 {
        self.text_percent
    }
    fn layout(
        &mut self,
        ui: &egui::Ui,
        output: &CommandOutput,
        colors: ThemeColors,
        gutter: f32,
    ) -> Arc<Galley> {
        let width = if self.wrap {
            (ui.available_width() - gutter).max(24.0)
        } else {
            f32::INFINITY
        };
        let palette = [
            colors.text_primary,
            colors.accent_warning,
            colors.text_muted,
        ];
        if self.key.as_ref().is_none_or(|key| {
            key.stdout != output.stdout
                || key.stderr != output.stderr
                || key.failed != output.error.is_some()
                || key.width != width
                || key.scale != ui.ctx().pixels_per_point()
                || key.colors != palette
                || key.text_percent != self.text_percent
        }) {
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = width;
            let append = |job: &mut egui::text::LayoutJob, text: &str, color| {
                job.append(
                    text,
                    0.0,
                    egui::TextFormat::simple(
                        egui::FontId::monospace(11.0 * f32::from(self.text_percent) / 100.0),
                        color,
                    ),
                )
            };
            append(&mut job, &output.stdout, palette[0]);
            if !output.stderr.is_empty() {
                if !output.stdout.is_empty() && !output.stdout.ends_with('\n') {
                    append(&mut job, "\n", palette[0]);
                }
                append(&mut job, "stderr\n", palette[2]);
                append(&mut job, &output.stderr, palette[1]);
            }
            if output.stdout.is_empty() && output.stderr.is_empty() && output.error.is_none() {
                append(&mut job, "(no output)", palette[2]);
            }
            if self.key.as_ref().is_none_or(|key| {
                key.stdout != output.stdout
                    || key.stderr != output.stderr
                    || key.failed != output.error.is_some()
            }) {
                self.lines.bind_content(&Arc::from(job.text.as_str()));
            }
            self.galley = Some(ui.fonts_mut(|fonts| fonts.layout_job(job)));
            self.key = Some(Key {
                stdout: output.stdout.clone(),
                stderr: output.stderr.clone(),
                failed: output.error.is_some(),
                width,
                scale: ui.ctx().pixels_per_point(),
                colors: palette,
                text_percent: self.text_percent,
            });
        }
        self.galley.as_ref().expect("layout initialized").clone()
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: Id,
        path: &Path,
        output: &CommandOutput,
        colors: ThemeColors,
    ) {
        let text_scale = crate::preview_typography::toolbar_owned(
            ui,
            id.with("typography"),
            &mut self.text_percent,
        );
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.wrap, "Wrap output");
            let numbers = ui.checkbox(&mut self.numbers, "Line numbers");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id.with("line-numbers"), numbers.rect));
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = numbers;
        });
        // Bind content before measuring the count-dependent gutter.
        if self.key.as_ref().is_none_or(|key| {
            key.stdout != output.stdout
                || key.stderr != output.stderr
                || key.failed != output.error.is_some()
        }) {
            let _ = self.layout(ui, output, colors, 0.0);
        }
        let gutter = if self.numbers {
            self.lines.gutter_width_for_font(ui, 11.0 * text_scale)
        } else {
            0.0
        };
        let galley = self.layout(ui, output, colors, gutter);
        self.lines.update_layout(&galley);
        self.lines.toolbar(ui, id.with("lines"));
        self.find = PreviewFind::toolbar_owned(
            ui,
            id,
            path,
            &galley,
            std::mem::take(&mut self.find),
            "Find in output",
        );
        let scroll = egui::ScrollArea::both()
            .id_salt(id.with("scroll"))
            .max_height(220.0)
            .show(ui, |ui| {
                ui.style_mut().interaction.selectable_labels = true;
                let response = ui
                    .horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if gutter > 0.0 {
                            ui.allocate_space(egui::vec2(gutter, 0.0));
                        }
                        ui.add(egui::Label::new(galley))
                    })
                    .inner;
                self.find.decorate(ui, response.rect.min, colors.accent);
                self.lines.paint_and_reveal_for_font(
                    ui,
                    response.rect.min,
                    gutter,
                    colors.text_muted,
                    11.0 * text_scale,
                );
            });
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| {
            data.insert_temp(Id::new("run-command-output-rect"), scroll.inner_rect);
            data.insert_temp(id.with("scroll-offset"), scroll.state.offset);
        });
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn output() -> CommandOutput {
        CommandOutput {
            cmdline: "printf".into(),
            stdout: "Café\nline\twith spaces ".into(),
            stderr: "CAFÉ\n".into(),
            exit_code: Some(0),
            error: None,
            batch: None,
            context: None,
        }
    }
    #[test]
    fn layout_preserves_streams_and_cache_invalidates_on_same_length_edits_and_wrap() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut output = output();
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let colors = crate::theme::ThemeColors::dark();
            let first = view.layout(ui, &output, colors, 0.0);
            assert_eq!(first.text(), "Café\nline\twith spaces \nstderr\nCAFÉ\n");
            assert!(Arc::ptr_eq(&first, &view.layout(ui, &output, colors, 0.0)));
            output.stderr = "CAFè\n".into();
            let edited = view.layout(ui, &output, colors, 0.0);
            assert!(!Arc::ptr_eq(&first, &edited));
            assert!(edited.text().ends_with("CAFè\n"));
            view.wrap = false;
            let _ = view.layout(ui, &output, colors, 0.0);
            assert_eq!(view.key.as_ref().unwrap().width, f32::INFINITY);
        }));
    }
    #[test]
    fn magnification_reflows_text_and_gutter_without_changing_search_lines_or_report() {
        let ctx = egui::Context::default();
        let id = Id::new("output-scale-test");
        let output = output();
        let report =
            crate::command_report::export(&output, crate::command_report::Format::Json).unwrap();
        let mut view = View {
            find: PreviewFind::with_query("café"),
            numbers: true,
            ..Default::default()
        };
        let frame = |view: &mut View| {
            crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
                view.show(ui, id, Path::new("/"), &output, ThemeColors::dark())
            }))
        };
        frame(&mut view);
        let original = view.galley.as_ref().unwrap().clone();
        let lines = view.line_state().1;
        assert_eq!(view.search_state(), (2, Some(0)));
        view.text_percent = 200;
        frame(&mut view);
        let enlarged = view.galley.as_ref().unwrap().clone();
        assert!(!Arc::ptr_eq(&original, &enlarged));
        assert_eq!(original.text(), enlarged.text());
        assert!(enlarged.rows[0].rect().height() > original.rows[0].rect().height() * 1.8);
        assert_eq!(view.line_state().1, lines);
        assert_eq!(view.search_state(), (2, Some(0)));
        assert_eq!(view.text_percent(), 200);
        view.text_percent = 100;
        frame(&mut view);
        assert_eq!(view.galley.as_ref().unwrap().text(), original.text());
        assert_eq!(
            report,
            crate::command_report::export(&output, crate::command_report::Format::Json).unwrap()
        );
        assert_eq!(View::default().text_percent(), 100);
        assert!(
            ctx.data(|data| data.get_temp::<u16>(id.with("typography").with("text-percent")))
                .is_none()
        );
    }

    #[test]
    fn output_line_jump_reveals_distant_logical_line_and_invalid_enter_keeps_position_and_report() {
        let ctx = egui::Context::default();
        let id = Id::new("output-line-test");
        let mut output = output();
        output.stdout = "日本語 long output line\r\n".repeat(250);
        output.stderr.clear();
        let report =
            crate::command_report::export(&output, crate::command_report::Format::Json).unwrap();
        let mut view = View {
            numbers: true,
            ..Default::default()
        };
        let frame = |view: &mut View, events: Vec<egui::Event>| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| view.show(ui, id, Path::new("/"), &output, ThemeColors::dark()),
            ));
        };
        frame(&mut view, vec![]);
        assert_eq!(view.line_state(), (true, 251, None, false, false));
        ctx.memory_mut(|memory| memory.request_focus(id.with("lines").with("input")));
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(
            &mut view,
            vec![egui::Event::Text("200".into()), enter(true)],
        );
        assert!(!ctx.input(|input| input.key_pressed(egui::Key::Enter)));
        frame(&mut view, vec![enter(false)]);
        frame(&mut view, vec![]);
        let offset = ctx
            .data(|data| data.get_temp::<egui::Vec2>(id.with("scroll-offset")))
            .unwrap();
        assert!(offset.y > 2000.0);
        assert_eq!(view.line_state(), (true, 251, Some(200), false, true));
        // Select the existing field through its ordinary keyboard shortcut.
        let select = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command: true,
                ctrl: true,
                mac_cmd: true,
                ..Default::default()
            },
        };
        frame(
            &mut view,
            vec![select, egui::Event::Text("0".into()), enter(true)],
        );
        frame(&mut view, vec![enter(false)]);
        assert_eq!(view.line_state().2, Some(200));
        assert!(view.line_state().3);
        let after = ctx
            .data(|data| data.get_temp::<egui::Vec2>(id.with("scroll-offset")))
            .unwrap();
        assert!((offset.y - after.y).abs() < 1.0);
        assert_eq!(
            report,
            crate::command_report::export(&output, crate::command_report::Format::Json).unwrap()
        );
    }

    #[test]
    fn owned_find_navigates_both_streams_consumes_enter_and_leaves_report_unchanged() {
        let ctx = egui::Context::default();
        let id = Id::new("output-find-test");
        let output = output();
        let before =
            crate::command_report::export(&output, crate::command_report::Format::Json).unwrap();
        let mut view = View {
            find: PreviewFind::with_query("café"),
            ..Default::default()
        };
        let frame = |view: &mut View, events: Vec<egui::Event>| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    view.show(
                        ui,
                        id,
                        Path::new("/"),
                        &output,
                        crate::theme::ThemeColors::dark(),
                    )
                },
            ));
        };
        frame(&mut view, vec![]);
        assert_eq!(view.search_state(), (2, Some(0)));
        ctx.memory_mut(|memory| memory.request_focus(id.with("query")));
        let key = |pressed, modifiers| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        frame(&mut view, vec![key(true, Default::default())]);
        assert_eq!(view.search_state(), (2, Some(1)));
        assert!(!ctx.input(|input| input.key_pressed(egui::Key::Enter)));
        frame(&mut view, vec![key(false, Default::default())]);
        frame(&mut view, vec![key(true, egui::Modifiers::SHIFT)]);
        assert_eq!(view.search_state(), (2, Some(0)));
        assert!(ctx.data(|data| data.get_temp::<PreviewFind>(id)).is_none());
        assert_eq!(
            before,
            crate::command_report::export(&output, crate::command_report::Format::Json).unwrap()
        );
    }
}
