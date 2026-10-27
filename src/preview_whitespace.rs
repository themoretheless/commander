//! Read-only whitespace markers painted over the original text galley.
use egui::{Color32, Galley, Id, Pos2, Rect, Stroke, Ui, Vec2};

pub fn toolbar(ui: &mut Ui, id: Id) -> bool {
    let mut enabled = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(id).unwrap_or(false));
    let response = ui
        .checkbox(&mut enabled, "Whitespace")
        .on_hover_text("Show spaces as dots and tabs as arrows; copied text stays unchanged");
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(id.with("control"), response.rect));
    #[cfg(not(any(test, feature = "visual-qa")))]
    let _ = response;
    ui.ctx().data_mut(|data| data.insert_temp(id, enabled));
    enabled
}

fn visit(galley: &Galley, origin: Pos2, clip: Rect, mut marker: impl FnMut(char, Rect)) {
    let first = galley
        .rows
        .partition_point(|row| origin.y + row.rect().bottom() < clip.top());
    for row in galley.rows.iter().skip(first) {
        if origin.y + row.rect().top() > clip.bottom() {
            break;
        }
        for glyph in &row.glyphs {
            if !matches!(glyph.chr, ' ' | '\t') || glyph.advance_width <= 0.0 {
                continue;
            }
            let rect = glyph
                .logical_rect()
                .translate(origin.to_vec2() + row.pos.to_vec2());
            if clip.intersects(rect) {
                marker(glyph.chr, rect);
            }
        }
    }
}

/// Counts space and tab markers respectively; the galley and its copy/search text are untouched.
pub fn paint(ui: &Ui, galley: &Galley, origin: Pos2, color: Color32) -> [usize; 2] {
    let mut counts = [0, 0];
    visit(galley, origin, ui.clip_rect(), |character, rect| {
        if character == ' ' {
            counts[0] += 1;
            ui.painter().circle_filled(
                rect.center(),
                (rect.height() * 0.07).clamp(0.6, 1.3),
                color,
            );
        } else {
            counts[1] += 1;
            let margin = (rect.width() * 0.1).min(2.0);
            let left = Pos2::new(rect.left() + margin, rect.center().y);
            let right = Pos2::new(rect.right() - margin, rect.center().y);
            let head = (rect.height() * 0.2).min(rect.width() * 0.25);
            let stroke = Stroke::new(1.0, color);
            ui.painter().line_segment([left, right], stroke);
            ui.painter()
                .line_segment([right - Vec2::new(head, head), right], stroke);
            ui.painter()
                .line_segment([right - Vec2::new(head, -head), right], stroke);
        }
    });
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markers_follow_actual_glyphs_and_clipping_without_rewriting_text() {
        let ctx = egui::Context::default();
        let source = " a\tb  \n\tёж x\n";
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                source.into(),
                egui::FontId::monospace(12.0),
                Color32::WHITE,
            );
            assert_eq!(paint(ui, &galley, ui.min_rect().min, Color32::GRAY), [4, 2]);
            assert_eq!(galley.text(), source);
            let mut bounds = Vec::new();
            visit(&galley, Pos2::ZERO, Rect::EVERYTHING, |character, rect| {
                bounds.push((character, rect))
            });
            assert_eq!(bounds.len(), 6);
            assert!(
                bounds
                    .iter()
                    .all(|(_, rect)| rect.is_finite() && rect.width() > 0.0)
            );
            let mut clipped = 0;
            visit(
                &galley,
                Pos2::ZERO,
                Rect::from_min_size(Pos2::new(10000.0, 10000.0), Vec2::splat(1.0)),
                |_, _| clipped += 1,
            );
            assert_eq!(clipped, 0);
        }));
    }

    #[test]
    fn whitespace_toggle_defaults_off_and_is_pane_local() {
        let ctx = egui::Context::default();
        let left = Id::new(("whitespace", "left"));
        let right = Id::new(("whitespace", "right"));
        for frame in 0..3 {
            let mut input = egui::RawInput::default();
            if frame > 0 {
                let rect = ctx
                    .data(|data| data.get_temp::<Rect>(left.with("control")))
                    .unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                assert_eq!(toolbar(ui, left), frame == 2);
                assert!(!toolbar(ui, right));
            }));
        }
    }
}
