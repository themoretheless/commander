//! Logical source lines, independent of visual wrapping and syntax colors.
use egui::{Color32, Galley, Id, Pos2, Rect, Ui, Vec2};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct PreviewLines {
    content: Option<std::sync::Arc<str>>,
    galley: Option<Arc<Galley>>,
    rows: std::sync::Arc<Vec<Rect>>,
    count: usize,
    input: String,
    error: bool,
    reveal: Option<usize>,
}

impl PreviewLines {
    pub fn load(ui: &Ui, id: Id, content: &std::sync::Arc<str>) -> Self {
        let mut state = ui
            .ctx()
            .data(|data| data.get_temp::<Self>(id).unwrap_or_default());
        if state
            .content
            .as_ref()
            .is_none_or(|old| !std::sync::Arc::ptr_eq(old, content))
        {
            state.count = content.bytes().filter(|&byte| byte == b'\n').count() + 1;
            state.content = Some(content.clone());
            state.input.clear();
            state.error = false;
            state.reveal = None;
            state.galley = None;
            state.rows = Default::default();
        }
        state
    }

    pub fn gutter_width(&self, ui: &Ui) -> f32 {
        let digits = self.count.max(1).ilog10() as usize + 1;
        ui.fonts_mut(|fonts| fonts.glyph_width(&egui::FontId::monospace(12.0), '0')) * digits as f32
            + 16.0
    }

    pub fn toolbar(&mut self, ui: &mut Ui, id: Id) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} line{}",
                self.count,
                if self.count == 1 { "" } else { "s" }
            ));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .id(id.with("input"))
                    .desired_width(65.0)
                    .char_limit(10)
                    .hint_text("Line"),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    ui.is_enabled(),
                    "Go to line number",
                )
            });
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("Go to line").clicked() || enter {
                match self.input.trim().parse::<usize>() {
                    Ok(line) if (1..=self.count).contains(&line) => {
                        self.reveal = Some(line - 1);
                        self.error = false;
                    }
                    _ => self.error = true,
                }
            }
            if self.error {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Enter 1–{}", self.count),
                );
            }
        });
    }

    pub fn update_layout(&mut self, galley: &Arc<Galley>) {
        if self
            .galley
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, galley))
        {
            return;
        }
        let mut rows = Vec::with_capacity(self.count);
        let mut first = true;
        for row in &galley.rows {
            if first {
                rows.push(row.rect());
            }
            first = row.ends_with_newline;
        }
        self.rows = std::sync::Arc::new(rows);
        self.galley = Some(galley.clone());
    }

    pub fn paint_and_reveal(&mut self, ui: &mut Ui, origin: Pos2, width: f32, color: Color32) {
        if let Some(line) = self.reveal.take()
            && let Some(row) = self.rows.get(line)
        {
            ui.scroll_to_rect_animation(
                Rect::from_min_size(
                    origin + Vec2::new(0.0, row.top()),
                    Vec2::new(1.0, row.height()),
                ),
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
        }
        if width <= 0.0 {
            return;
        }
        let clip = ui.clip_rect();
        let top = clip.top() - origin.y;
        let bottom = clip.bottom() - origin.y;
        let start = self.rows.partition_point(|row| row.bottom() < top);
        let end = self.rows.partition_point(|row| row.top() <= bottom);
        // Only visible logical-line starts are painted. Wrapped continuation rows
        // have no extra numbers; the original text remains the selectable label.
        for (index, row) in self.rows[start..end].iter().enumerate() {
            ui.painter().text(
                origin + Vec2::new(-8.0, row.top()),
                egui::Align2::RIGHT_TOP,
                (start + index + 1).to_string(),
                egui::FontId::monospace(12.0),
                color,
            );
        }
    }

    pub fn save(self, ui: &Ui, id: Id) {
        ui.ctx().data_mut(|data| data.insert_temp(id, self));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enter_jumps_to_a_distant_line_and_invalid_input_does_not_scroll() {
        let ctx = egui::Context::default();
        let id = Id::new("jump-test");
        let text: std::sync::Arc<str> = "long line
"
        .repeat(250)
        .into();
        let mut state = PreviewLines::default();
        let mut greatest_offset = 0.0_f32;
        for frame in 0..4 {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 400.0))),
                ..Default::default()
            };
            if frame == 1 {
                input.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                });
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                if frame == 0 {
                    state = PreviewLines::load(ui, id, &text);
                    state.input = "200".into();
                    ui.memory_mut(|memory| memory.request_focus(id.with("input")));
                }
                state.toolbar(ui, id);
                let galley = ui.fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        text.to_string(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                        300.0,
                    ))
                });
                state.update_layout(&galley);
                let output = egui::ScrollArea::vertical()
                    .id_salt("line-scroll")
                    .max_height(100.0)
                    .show(ui, |ui| {
                        let response = ui.add(egui::Label::new(galley));
                        state.paint_and_reveal(ui, response.rect.min, 0.0, Color32::WHITE);
                    });
                greatest_offset = greatest_offset.max(output.state.offset.y);
            }));
        }
        assert!(
            greatest_offset > 2000.0,
            "line 200 must become visible: {greatest_offset}"
        );
        assert!(!state.error);
        assert!(state.reveal.is_none());
        state.input = "0".into();
        ctx.memory_mut(|memory| memory.request_focus(id.with("input")));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| state.toolbar(ui, id)));
        assert!(state.error);
        assert!(state.reveal.is_none());
    }

    #[test]
    fn logical_lines_survive_unicode_crlf_empty_lines_and_wrapping() {
        let ctx = egui::Context::default();
        let text: std::sync::Arc<str> = "日本語 long wrapped source line\r\n\nlast\n".into();
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let mut lines = PreviewLines::load(ui, Id::new("test"), &text);
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout_job(egui::text::LayoutJob::simple(
                    text.to_string(),
                    egui::FontId::monospace(12.0),
                    Color32::WHITE,
                    50.0,
                ))
            });
            assert_eq!(lines.count, 4);
            assert!(galley.rows.len() > lines.count);
            lines.update_layout(&galley);
            assert_eq!(lines.rows.len(), 4);
            assert_eq!(lines.rows[0], galley.rows[0].rect());
            assert!(lines.rows[1].top() > galley.rows[1].rect().top());
            let cached = lines.rows.clone();
            lines.update_layout(&galley);
            assert!(std::sync::Arc::ptr_eq(&cached, &lines.rows));
            lines.input = "3".into();
            lines.save(ui, Id::new("test"));
            let reused = PreviewLines::load(ui, Id::new("test"), &text);
            assert_eq!(reused.input, "3");
            assert!(std::sync::Arc::ptr_eq(&cached, &reused.rows));
            let changed = PreviewLines::load(ui, Id::new("test"), &std::sync::Arc::from("new"));
            assert_eq!(changed.count, 1);
            assert!(changed.input.is_empty());
        }));
    }
}
