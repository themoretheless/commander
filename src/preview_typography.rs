//! Per-pane, session-only text preview magnification; independent of app zoom.
use egui::{Id, Ui};

pub fn toolbar(ui: &mut Ui, id: Id) -> f32 {
    let key = id.with("text-percent");
    let mut percent = ui
        .ctx()
        .data(|data| data.get_temp::<u16>(key).unwrap_or(100))
        .clamp(50, 200);
    ui.horizontal_wrapped(|ui| {
        ui.label("Text size");
        for (label, target) in [
            ("A−", percent.saturating_sub(25).max(50)),
            ("100%", 100),
            ("A+", (percent + 25).min(200)),
        ] {
            let response = ui.add_enabled(target != percent, egui::Button::new(label));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    response.enabled(),
                    match label {
                        "A−" => "Decrease preview text size",
                        "A+" => "Increase preview text size",
                        _ => "Reset preview text size",
                    },
                )
            });
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id.with(label), response.rect));
            if response.clicked() {
                percent = target;
            }
        }
        ui.label(format!("{percent}%"));
    });
    ui.ctx().data_mut(|data| data.insert_temp(key, percent));
    f32::from(percent) / 100.0
}

pub fn scale_job(job: &mut egui::text::LayoutJob, scale: f32) {
    for section in &mut job.sections {
        section.format.font_id.size *= scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, id: Id, events: Vec<egui::Event>) -> f32 {
        let mut scale = 0.0;
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                scale = toolbar(ui, id);
            },
        ));
        scale
    }

    fn click(ctx: &egui::Context, id: Id, label: &str) -> f32 {
        frame(ctx, id, vec![]);
        let rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(id.with(label)))
            .unwrap();
        let event = |pressed| egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            ctx,
            id,
            vec![egui::Event::PointerMoved(rect.center()), event(true)],
        );
        frame(ctx, id, vec![event(false)])
    }

    #[test]
    fn text_size_controls_clamp_reset_and_keep_panes_independent() {
        let ctx = egui::Context::default();
        let left = Id::new(("text_preview", "left"));
        let right = Id::new(("text_preview", "right"));
        assert_eq!(frame(&ctx, left, vec![]), 1.0);
        assert_eq!(click(&ctx, left, "A+"), 1.25);
        assert_eq!(frame(&ctx, right, vec![]), 1.0);
        assert_eq!(frame(&ctx, left, vec![]), 1.25);
        for _ in 0..8 {
            click(&ctx, left, "A+");
        }
        assert_eq!(frame(&ctx, left, vec![]), 2.0);
        assert_eq!(click(&ctx, left, "100%"), 1.0);
        for _ in 0..8 {
            click(&ctx, left, "A−");
        }
        assert_eq!(frame(&ctx, left, vec![]), 0.5);
        ctx.data_mut(|data| data.insert_temp(left.with("text-percent"), u16::MAX));
        assert_eq!(frame(&ctx, left, vec![]), 2.0);
    }

    #[test]
    fn font_scaling_preserves_document_text_ranges_and_heading_ratio() {
        let mut job = crate::document_preview::layout(
            &std::sync::Arc::from("# Heading\n\nplain **bold**"),
            crate::document_preview::DocumentKind::Markdown,
            egui::Color32::WHITE,
            300.0,
        );
        let original = job.clone();
        scale_job(&mut job, 1.5);
        assert_eq!(job.text, original.text);
        for (scaled, old) in job.sections.iter().zip(&original.sections) {
            assert_eq!(scaled.byte_range, old.byte_range);
            assert_eq!(scaled.format.font_id.size, old.format.font_id.size * 1.5);
            assert_eq!(scaled.format.font_id.family, old.format.font_id.family);
            assert_eq!(scaled.format.color, old.format.color);
        }
    }
}
