use crate::panel::SortOrder;
use egui::{
    Color32, CornerRadius, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo, WidgetType,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PaintedGlyph {
    BreadcrumbChevron,
    SortTriangle,
    ParentUp,
    FileDocument,
    ToolbarCompare,
    NavigationTriangle,
}

impl PaintedGlyph {
    pub(crate) const REQUIRED_CAPTURE: [Self; 6] = [
        Self::BreadcrumbChevron,
        Self::SortTriangle,
        Self::ParentUp,
        Self::FileDocument,
        Self::ToolbarCompare,
        Self::NavigationTriangle,
    ];

    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::BreadcrumbChevron => "breadcrumb_chevron",
            Self::SortTriangle => "sort_triangle",
            Self::ParentUp => "parent_up",
            Self::FileDocument => "file_document",
            Self::ToolbarCompare => "toolbar_compare",
            Self::NavigationTriangle => "navigation_triangle",
        }
    }
}

fn record(ui: &Ui, glyph: PaintedGlyph, rect: Rect) {
    #[cfg(feature = "visual-qa")]
    crate::visual_qa::record_painted_glyph(ui.ctx(), glyph, rect);
    #[cfg(not(feature = "visual-qa"))]
    let _ = (ui, glyph, rect);
}

pub(crate) fn navigation_triangle(ui: &Ui, rect: Rect, points_right: bool, color: Color32) {
    let icon = Rect::from_center_size(rect.center(), Vec2::new(9.0, 11.0));
    let points = if points_right {
        vec![icon.left_top(), icon.right_center(), icon.left_bottom()]
    } else {
        vec![icon.right_top(), icon.left_center(), icon.right_bottom()]
    };
    ui.painter()
        .add(Shape::convex_polygon(points, color, Stroke::NONE));
    record(ui, PaintedGlyph::NavigationTriangle, icon);
}

pub(crate) fn breadcrumb_chevron(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(12.0, 18.0), Sense::hover());
    let center = rect.center();
    let stroke = Stroke::new(1.4, color);
    ui.painter().line_segment(
        [
            egui::pos2(center.x - 2.0, center.y - 4.0),
            egui::pos2(center.x + 2.0, center.y),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(center.x + 2.0, center.y),
            egui::pos2(center.x - 2.0, center.y + 4.0),
        ],
        stroke,
    );
    record(
        ui,
        PaintedGlyph::BreadcrumbChevron,
        Rect::from_center_size(center, Vec2::new(7.0, 11.0)),
    );
    response
}

pub(crate) fn sort_accessible_label(label: &str, order: Option<SortOrder>) -> String {
    match order {
        Some(SortOrder::Asc) => {
            format!("{label}, sorted ascending; activate to sort descending")
        }
        Some(SortOrder::Desc) => {
            format!("{label}, sorted descending; activate to sort ascending")
        }
        None => format!("{label}, not sorted; activate to sort"),
    }
}

pub(crate) fn sort_header(
    ui: &mut Ui,
    label: &str,
    order: Option<SortOrder>,
    color: Color32,
) -> Response {
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let label_response =
                ui.label(egui::RichText::new(label).size(11.0).strong().color(color));
            let Some(order) = order else {
                return label_response;
            };
            let (rect, indicator_response) =
                ui.allocate_exact_size(Vec2::new(9.0, 9.0), Sense::hover());
            let icon = rect.shrink(1.0);
            let points = match order {
                SortOrder::Asc => vec![icon.center_top(), icon.right_bottom(), icon.left_bottom()],
                SortOrder::Desc => vec![icon.left_top(), icon.right_top(), icon.center_bottom()],
            };
            ui.painter()
                .add(Shape::convex_polygon(points, color, Stroke::NONE));
            record(ui, PaintedGlyph::SortTriangle, icon);
            label_response.union(indicator_response)
        })
        .inner
        .interact(Sense::click());
    let accessible = sort_accessible_label(label, order);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &accessible));
    response
}

pub(crate) fn parent_up(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
    let center = rect.center();
    let stroke = Stroke::new(1.5, color);
    ui.painter().line_segment(
        [
            egui::pos2(center.x, center.y + 5.0),
            egui::pos2(center.x, center.y - 3.0),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(center.x - 4.0, center.y),
            egui::pos2(center.x, center.y - 4.0),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(center.x, center.y - 4.0),
            egui::pos2(center.x + 4.0, center.y),
        ],
        stroke,
    );
    record(ui, PaintedGlyph::ParentUp, rect.shrink(1.0));
    response
}

fn file_badge(extension: &str) -> String {
    let badge = extension
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(3)
        .flat_map(char::to_uppercase)
        .collect::<String>();
    if badge.is_empty() {
        "DOC".to_string()
    } else {
        badge
    }
}

pub(crate) fn file_document(
    ui: &mut Ui,
    extension: &str,
    color: Color32,
    background: Color32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(24.0, 20.0), Sense::hover());
    let document = Rect::from_center_size(rect.center(), Vec2::new(17.0, 19.0));
    let fold = 5.0;
    let outline = vec![
        document.left_top(),
        egui::pos2(document.right() - fold, document.top()),
        egui::pos2(document.right(), document.top() + fold),
        document.right_bottom(),
        document.left_bottom(),
        document.left_top(),
    ];
    ui.painter()
        .add(Shape::line(outline, Stroke::new(1.1, color)));
    ui.painter().line_segment(
        [
            egui::pos2(document.right() - fold, document.top()),
            egui::pos2(document.right() - fold, document.top() + fold),
        ],
        Stroke::new(1.0, color),
    );
    ui.painter().line_segment(
        [
            egui::pos2(document.right() - fold, document.top() + fold),
            egui::pos2(document.right(), document.top() + fold),
        ],
        Stroke::new(1.0, color),
    );
    let badge = file_badge(extension);
    let badge_rect = Rect::from_center_size(
        egui::pos2(document.center().x, document.bottom() - 5.0),
        Vec2::new(document.width() - 3.0, 7.0),
    );
    ui.painter()
        .rect_filled(badge_rect, CornerRadius::same(1), color);
    ui.painter().text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        badge,
        egui::FontId::monospace(6.5),
        background,
    );
    record(ui, PaintedGlyph::FileDocument, document);
    response
}

pub(crate) fn toolbar_compare_button(ui: &mut Ui, fill: Color32, color: Color32) -> Response {
    let response = ui.add_sized(
        Vec2::new(34.0, 24.0),
        egui::Button::new("")
            .fill(fill)
            .corner_radius(crate::theme::ROUNDING_SM),
    );
    let icon = Rect::from_center_size(response.rect.center(), Vec2::new(16.0, 11.0));
    let stroke = Stroke::new(1.3, color);
    let upper_y = icon.top() + 2.0;
    let lower_y = icon.bottom() - 2.0;
    ui.painter().line_segment(
        [
            egui::pos2(icon.left(), upper_y),
            egui::pos2(icon.right(), upper_y),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(icon.right() - 3.0, upper_y - 2.5),
            egui::pos2(icon.right(), upper_y),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(icon.right(), upper_y),
            egui::pos2(icon.right() - 3.0, upper_y + 2.5),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(icon.right(), lower_y),
            egui::pos2(icon.left(), lower_y),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(icon.left() + 3.0, lower_y - 2.5),
            egui::pos2(icon.left(), lower_y),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(icon.left(), lower_y),
            egui::pos2(icon.left() + 3.0, lower_y + 2.5),
        ],
        stroke,
    );
    record(ui, PaintedGlyph::ToolbarCompare, icon);
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Compare panels"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_glyph_contract_is_painted_and_complete() {
        assert_eq!(
            PaintedGlyph::REQUIRED_CAPTURE.map(PaintedGlyph::key),
            [
                "breadcrumb_chevron",
                "sort_triangle",
                "parent_up",
                "file_document",
                "toolbar_compare",
                "navigation_triangle",
            ]
        );
    }

    #[test]
    fn semantic_fallback_text_is_ascii_and_describes_sort_direction() {
        for (order, expected) in [
            (None, "not sorted"),
            (Some(SortOrder::Asc), "sorted ascending"),
            (Some(SortOrder::Desc), "sorted descending"),
        ] {
            let label = sort_accessible_label("Name", order);
            assert!(label.is_ascii());
            assert!(label.contains(expected));
        }
        for extension in ["rs", "PDF", "", "файл"] {
            let badge = file_badge(extension);
            assert!(badge.is_ascii());
            assert!(!badge.is_empty());
            assert!(badge.len() <= 3);
        }
    }

    #[test]
    fn sort_header_publishes_direction_as_button_semantics() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            sort_header(ui, "Name", Some(SortOrder::Asc), egui::Color32::WHITE);
        });

        let update = output
            .platform_output
            .accesskit_update
            .expect("headless AccessKit output");
        let button = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.role() == egui::accesskit::Role::Button)
            .expect("sort button node");
        assert_eq!(
            button.label(),
            Some("Name, sorted ascending; activate to sort descending")
        );
    }
}
