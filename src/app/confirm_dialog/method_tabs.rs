//! Copy-method tab strip for the transfer confirmation dialog.
//!
//! This component only renders the current value and returns a newly selected
//! value. Ownership of pending-operation state stays with the dialog shell.

use super::*;

/// Hover text explaining what each copy method trades off.
const fn method_tooltip(method: CopyMethod) -> &'static str {
    match method {
        CopyMethod::Native => {
            "System copy, cloning files where the volume supports it; fastest. Falls back \
             to Buffered when a bandwidth limit, resume or symlink policy needs it"
        }
        CopyMethod::Buffered => {
            "Streams file contents through the app with byte-level progress; slower, but \
             behaves the same on every volume"
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    colors: &ThemeColors,
    title: &str,
    count: usize,
    current: CopyMethod,
) -> Option<CopyMethod> {
    let row_height = 28.0;
    let (row_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::hover());
    let painter = ui.painter();

    painter.line_segment(
        [
            egui::pos2(row_rect.left(), row_rect.bottom()),
            egui::pos2(row_rect.right(), row_rect.bottom()),
        ],
        Stroke::new(1.0, colors.border),
    );
    painter.text(
        egui::pos2(row_rect.left() + 4.0, row_rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{title} — {count} item(s)"),
        egui::FontId::proportional(13.0),
        colors.text_primary,
    );

    const TABS: [(&str, CopyMethod); 2] = [
        ("Native", CopyMethod::Native),
        ("Buffered", CopyMethod::Buffered),
    ];
    let tab_width = 90.0;
    let tabs_left = row_rect.right() - tab_width * TABS.len() as f32;
    let mut selected = None;

    for (index, (label, method)) in TABS.into_iter().enumerate() {
        let active = current == method;
        let tab_rect = egui::Rect::from_min_size(
            egui::pos2(tabs_left + index as f32 * tab_width, row_rect.top()),
            Vec2::new(tab_width, row_height),
        );

        if active {
            painter.rect_filled(tab_rect, CornerRadius::ZERO, colors.bg_panel);
            for (from, to) in [
                (tab_rect.left_bottom(), tab_rect.left_top()),
                (tab_rect.left_top(), tab_rect.right_top()),
                (tab_rect.right_top(), tab_rect.right_bottom()),
            ] {
                painter.line_segment([from, to], Stroke::new(1.0, colors.border));
            }
            painter.line_segment(
                [
                    egui::pos2(tab_rect.left() + 1.0, tab_rect.bottom()),
                    egui::pos2(tab_rect.right() - 1.0, tab_rect.bottom()),
                ],
                Stroke::new(2.0, colors.bg_panel),
            );
        }

        painter.text(
            tab_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            if active {
                colors.text_primary
            } else {
                colors.text_muted
            },
        );

        let response = ui.interact(
            tab_rect,
            ui.id().with(("copy_method_tab", index)),
            Sense::click(),
        );
        // Painter-only tab: expose it as a selectable item and show focus.
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                active,
                label,
            )
        });
        if response.hovered() && !active {
            painter.rect_filled(
                tab_rect,
                CornerRadius::ZERO,
                colors.bg_hover.linear_multiply(0.2),
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                tab_rect.shrink(1.0),
                CornerRadius::ZERO,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let response = response.on_hover_text(method_tooltip(method));
        if response.clicked() {
            selected = Some(method);
        }
    }

    selected
}

#[cfg(test)]
mod tests {
    use super::{CopyMethod, method_tooltip};

    #[test]
    fn each_method_explains_its_tradeoff() {
        assert!(method_tooltip(CopyMethod::Native).contains("fastest"));
        assert!(method_tooltip(CopyMethod::Buffered).contains("progress"));
    }
}
