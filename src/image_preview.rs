//! Pan, zoom, rotate and mirror the bounded preview texture without modifying files.
use egui::{Id, TextureHandle, Ui, Vec2};
use std::path::{Path, PathBuf};

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.0;

#[derive(Clone)]
struct View {
    path: PathBuf,
    zoom: f32,
    enlarge_small: bool,
    centered: bool,
    orientation: Orientation,
    background: Background,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Background {
    #[default]
    Checkerboard,
    White,
    Black,
}

fn checker_texture(ctx: &egui::Context) -> TextureHandle {
    let key = Id::new("image-preview-checker-texture");
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(key)) {
        return texture;
    }
    let pixels = [192, 128, 128, 192].map(egui::Color32::from_gray).to_vec();
    let texture = ctx.load_texture(
        "image-preview-checker",
        egui::ColorImage::new([2, 2], pixels),
        egui::TextureOptions::NEAREST_REPEAT,
    );
    ctx.data_mut(|data| data.insert_temp(key, texture.clone()));
    texture
}

fn paint_background(ui: &Ui, rect: egui::Rect, background: Background) {
    match background {
        Background::White => {
            ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
        }
        Background::Black => {
            ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        }
        Background::Checkerboard => {
            let texture = checker_texture(ui.ctx());
            let uv = egui::Rect::from_min_size(egui::Pos2::ZERO, rect.size() / 24.0);
            let mut mesh = egui::epaint::Mesh::with_texture(texture.id());
            mesh.add_rect_with_uv(rect, uv, egui::Color32::WHITE);
            ui.painter().add(egui::Shape::mesh(mesh));
        }
    }
}

/// Source corners for display corners TL, TR, BR, BL. Permutations compose
/// rotations and mirrors in display space, without transforming source pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Orientation {
    transform: u8,
}

impl Orientation {
    fn corners(self) -> [usize; 4] {
        let turns = self.transform & 3;
        let mirrored = self.transform & 4 != 0;
        let mut result = [0, 1, 2, 3];
        for _ in 0..turns {
            result = [result[3], result[0], result[1], result[2]];
        }
        if mirrored {
            result = [result[1], result[0], result[3], result[2]];
        }
        result
    }
    fn compose(&mut self, order: [usize; 4]) {
        let old = self.corners();
        let target = order.map(|index| old[index]);
        self.transform = (0..8)
            .find(|value| Self { transform: *value }.corners() == target)
            .expect("dihedral composition");
    }
    fn rotate_right(&mut self) {
        self.compose([3, 0, 1, 2]);
    }
    fn rotate_half(&mut self) {
        self.compose([2, 3, 0, 1]);
    }
    fn rotate_left(&mut self) {
        self.compose([1, 2, 3, 0]);
    }
    fn mirror_horizontal(&mut self) {
        self.compose([1, 0, 3, 2]);
    }
    fn mirror_vertical(&mut self) {
        self.compose([3, 2, 1, 0]);
    }
    fn extent(self, size: Vec2) -> Vec2 {
        if self.transform & 1 != 0 {
            egui::vec2(size.y, size.x)
        } else {
            size
        }
    }
    fn mesh(self, texture: egui::TextureId, rect: egui::Rect) -> egui::epaint::Mesh {
        let positions = [
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ];
        let uv = [
            egui::pos2(0.0, 0.0),
            egui::pos2(1.0, 0.0),
            egui::pos2(1.0, 1.0),
            egui::pos2(0.0, 1.0),
        ];
        let mut mesh = egui::epaint::Mesh::with_texture(texture);
        for (pos, corner) in positions.into_iter().zip(self.corners()) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: uv[corner],
                color: egui::Color32::WHITE,
            });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        mesh
    }
}

fn id(side: &str) -> Id {
    Id::new(("image_preview_view", side))
}

fn change_zoom(zoom: f32, multiplier: f32) -> f32 {
    if !multiplier.is_finite() || multiplier <= 0.0 {
        return zoom;
    }
    (zoom * multiplier).clamp(MIN_ZOOM, MAX_ZOOM)
}

#[cfg(test)]
fn display_size(intrinsic: Vec2, viewport: Vec2, zoom: f32) -> Vec2 {
    display_size_options(intrinsic, viewport, zoom, false)
}

fn display_size_options(intrinsic: Vec2, viewport: Vec2, zoom: f32, enlarge_small: bool) -> Vec2 {
    let intrinsic = intrinsic.max(Vec2::splat(1.0));
    let viewport = viewport.max(Vec2::splat(1.0));
    let fit = (viewport.x / intrinsic.x).min(viewport.y / intrinsic.y);
    let fit = if enlarge_small { fit } else { fit.min(1.0) };
    intrinsic * fit * zoom
}

fn view(ui: &Ui, side: &str, path: &Path) -> (View, bool) {
    let cached = ui.ctx().data(|data| data.get_temp::<View>(id(side)));
    match cached {
        Some(view) if view.path == path => (view, false),
        _ => (
            View {
                path: path.into(),
                zoom: 1.0,
                enlarge_small: false,
                centered: true,
                orientation: Orientation::default(),
                background: Background::default(),
            },
            true,
        ),
    }
}

/// Returns the desired logical decode extent and current scroll offset.
/// The cache applies its physical
/// 4096-pixel target and memory limits; zoom may exceed available source detail.
pub fn render(ui: &mut Ui, texture: &TextureHandle, path: &Path, side: &str) -> (Vec2, Vec2) {
    let (mut view, mut reset_scroll) = view(ui, side, path);
    let old_zoom = view.zoom;
    let old_enlarge = view.enlarge_small;
    let old_centered = view.centered;
    reset_scroll |= ui.ctx().data_mut(|data| {
        data.remove_temp::<bool>(id(side).with("reset"))
            .unwrap_or(false)
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if ui
            .button("Fit")
            .on_hover_text("Reset image zoom and position")
            .clicked()
        {
            view.zoom = 1.0;
            reset_scroll = true;
        }
        let minus = ui.add_enabled(view.zoom > MIN_ZOOM, egui::Button::new("-"));
        minus.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, minus.enabled(), "Zoom image out")
        });
        if minus.clicked() {
            view.zoom = change_zoom(view.zoom, 0.5);
        }
        let plus = ui.add_enabled(view.zoom < MAX_ZOOM, egui::Button::new("+"));
        plus.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, plus.enabled(), "Zoom image in")
        });
        if plus.clicked() {
            view.zoom = change_zoom(view.zoom, 2.0);
        }
        ui.label(format!("{:.0}% of fit", view.zoom * 100.0));
        let enlarge = ui.checkbox(&mut view.enlarge_small, "Enlarge small images")
            .on_hover_text("Allow Fit to fill the preview even for small images. Display only; enlargement may look blurry.");
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("image-preview-control", side, "Enlarge small images")), enlarge.rect));
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = enlarge;
    });
    ui.horizontal_wrapped(|ui| {
        let center = ui.checkbox(&mut view.centered, "Center image")
            .on_hover_text("Center images smaller than the preview. Disable to align the image to the upper left; source file is unchanged.");
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("image-preview-control", side, "Center image")), center.rect));
        #[cfg(not(any(test, feature = "visual-qa")))]
        let _ = center;
    });
    reset_scroll |= old_centered != view.centered;
    reset_scroll |= old_enlarge != view.enlarge_small;
    let old_orientation = view.orientation;
    ui.horizontal_wrapped(|ui| {
        for (label, action) in [
            ("Rotate left", 0),
            ("Rotate right", 1),
            ("Rotate 180°", 5),
            ("Mirror H", 2),
            ("Mirror V", 3),
            ("Reset orientation", 4),
        ] {
            let response = ui.button(label).on_hover_text(match action {
                0 => "Rotate preview 90 degrees counterclockwise",
                1 => "Rotate preview 90 degrees clockwise",
                2 => "Mirror preview horizontally",
                3 => "Mirror preview vertically",
                5 => "Rotate preview 180 degrees; source file is unchanged",
                _ => "Restore source orientation; source file is unchanged",
            });
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id(side).with(label), response.rect));
            if response.clicked() {
                match action {
                    0 => view.orientation.rotate_left(),
                    1 => view.orientation.rotate_right(),
                    2 => view.orientation.mirror_horizontal(),
                    3 => view.orientation.mirror_vertical(),
                    5 => view.orientation.rotate_half(),
                    _ => view.orientation = Orientation::default(),
                }
            }
        }
    });
    reset_scroll |= old_orientation != view.orientation;
    ui.horizontal_wrapped(|ui| {
        ui.label("Background");
        for (label, background) in [
            ("Checkerboard", Background::Checkerboard),
            ("White", Background::White),
            ("Black", Background::Black),
        ] {
            let response = ui
                .selectable_value(&mut view.background, background, label)
                .on_hover_text("Preview background for transparent pixels; the file is unchanged");
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id(side).with(label), response.rect));
            #[cfg(not(feature = "visual-qa"))]
            let _ = response;
        }
    });
    ui.small("Drag or scroll to pan; double-click to Fit. Preview only.");
    let viewport = ui.available_size().max(Vec2::splat(1.0));
    let intrinsic =
        view.orientation.extent(texture.size_vec2()) / ui.ctx().pixels_per_point().max(1.0);
    let size = display_size_options(intrinsic, viewport, view.zoom, view.enlarge_small);
    let mut scroll = egui::ScrollArea::both()
        .id_salt(id(side).with("scroll"))
        .auto_shrink([false; 2])
        .max_height(viewport.y)
        .scroll_source(egui::scroll_area::ScrollSource {
            drag: egui::scroll_area::DragScroll::Never,
            ..Default::default()
        });
    if reset_scroll {
        scroll = scroll.scroll_offset(Vec2::ZERO);
    }
    let output = scroll.show(ui, |ui| {
        let canvas_size = viewport.max(size);
        let (rect, response) = ui.allocate_exact_size(canvas_size, egui::Sense::click_and_drag());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Image,
                response.enabled(),
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy(),
            )
        });
        if response.dragged_by(egui::PointerButton::Primary) {
            ui.scroll_with_delta_animation(
                response.drag_delta(),
                egui::style::ScrollAnimation::none(),
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let image_rect = if view.centered {
            egui::Rect::from_center_size(rect.center(), size)
        } else {
            egui::Rect::from_min_size(rect.min, size)
        };
        paint_background(ui, image_rect, view.background);
        ui.painter().add(egui::Shape::mesh(
            view.orientation.mesh(texture.id(), image_rect),
        ));
        if response.hovered() {
            if response.double_clicked() {
                view.zoom = 1.0;
                // Reset on the next frame, after this scroll area's state is stored.
                ui.ctx()
                    .data_mut(|data| data.insert_temp(id(side).with("reset"), true));
                ui.ctx().request_repaint();
            } else {
                let multiplier = ui.input(|input| input.zoom_delta());
                if multiplier != 1.0 {
                    view.zoom = change_zoom(view.zoom, multiplier);
                    ui.ctx().request_repaint();
                }
            }
        }
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(id(side), view.clone()));
    if view.zoom != old_zoom || view.enlarge_small != old_enlarge || view.centered != old_centered {
        ui.ctx().request_repaint();
    }
    (
        view.orientation.extent(viewport * view.zoom),
        output.state.offset,
    )
}

#[cfg(feature = "visual-qa")]
pub fn seed(ctx: &egui::Context, side: &str, path: &Path, zoom: f32) {
    ctx.data_mut(|data| {
        data.insert_temp(
            id(side),
            View {
                path: path.into(),
                zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
                enlarge_small: false,
                centered: true,
                orientation: Orientation::default(),
                background: Background::default(),
            },
        )
    });
}

#[cfg(feature = "visual-qa")]
pub fn zoom(ctx: &egui::Context, side: &str) -> Option<f32> {
    ctx.data(|data| data.get_temp::<View>(id(side)).map(|view| view.zoom))
}

#[cfg(feature = "visual-qa")]
pub fn centered(ctx: &egui::Context, side: &str) -> Option<bool> {
    ctx.data(|data| data.get_temp::<View>(id(side)).map(|view| view.centered))
}

#[cfg(feature = "visual-qa")]
pub fn enlarges_small(ctx: &egui::Context, side: &str) -> bool {
    ctx.data(|data| {
        data.get_temp::<View>(id(side))
            .is_some_and(|view| view.enlarge_small)
    })
}

#[cfg(feature = "visual-qa")]
pub fn orientation(ctx: &egui::Context, side: &str) -> Option<[usize; 4]> {
    ctx.data(|data| {
        data.get_temp::<View>(id(side))
            .map(|view| view.orientation.corners())
    })
}

#[cfg(feature = "visual-qa")]
pub fn black_background_active(ctx: &egui::Context, side: &str) -> bool {
    ctx.data(|data| {
        data.get_temp::<View>(id(side))
            .is_some_and(|view| view.background == Background::Black)
    })
}

#[cfg(feature = "visual-qa")]
pub fn checkerboard_active(ctx: &egui::Context, side: &str) -> bool {
    ctx.data(|data| {
        data.get_temp::<View>(id(side))
            .is_some_and(|view| view.background == Background::Checkerboard)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enlargement_preserves_aspect_and_large_image_fit_and_zoom() {
        let viewport = Vec2::new(400.0, 300.0);
        let small = Vec2::new(40.0, 20.0);
        assert_eq!(display_size_options(small, viewport, 1.0, false), small);
        assert_eq!(
            display_size_options(small, viewport, 1.0, true),
            Vec2::new(400.0, 200.0)
        );
        assert_eq!(
            display_size_options(small, viewport, 2.0, true),
            Vec2::new(800.0, 400.0)
        );
        let tall = Vec2::new(20.0, 40.0);
        assert_eq!(
            display_size_options(tall, viewport, 1.0, true),
            Vec2::new(150.0, 300.0)
        );
        let large = Vec2::new(1000.0, 500.0);
        assert_eq!(
            display_size_options(large, viewport, 1.0, false),
            display_size_options(large, viewport, 1.0, true)
        );
    }

    #[test]
    fn checkbox_enlarges_actual_mesh_and_new_file_resets_only_its_pane() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "small-image",
            egui::ColorImage::filled([32, 16], egui::Color32::WHITE),
            Default::default(),
        );
        let path = Path::new("/small.png");
        let mut sizes = Vec::new();
        for frame in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(500.0, 400.0),
                )),
                ..Default::default()
            };
            if frame > 0 {
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new((
                            "image-preview-control",
                            "left",
                            "Enlarge small images",
                        )))
                    })
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
            let output = crate::testutil::take_egui_output(ctx.run_ui(input, |ui| {
                render(ui, &texture, path, "left");
            }));
            let mesh = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh),
                    _ => None,
                })
                .unwrap();
            let bounds = mesh
                .vertices
                .iter()
                .fold(egui::Rect::NOTHING, |mut bounds, vertex| {
                    bounds.extend_with(vertex.pos);
                    bounds
                });
            sizes.push(bounds.size());
        }
        assert_eq!(sizes[0], Vec2::new(32.0, 16.0));
        assert!(sizes[2].x > sizes[0].x * 4.0);
        assert!((sizes[2].x / sizes[2].y - 2.0).abs() < 0.001);
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            assert!(view(ui, "left", path).0.enlarge_small);
            assert!(!view(ui, "left", Path::new("/new.png")).0.enlarge_small);
            assert!(!view(ui, "right", path).0.enlarge_small);
        }));
    }

    #[test]
    fn center_control_moves_actual_mesh_without_resizing_or_changing_other_view_options() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "alignment",
            egui::ColorImage::filled([32, 16], egui::Color32::WHITE),
            Default::default(),
        );
        let path = Path::new("/small.png");
        let mut bounds = Vec::new();
        for frame in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(500.0, 400.0),
                )),
                ..Default::default()
            };
            if frame > 0 {
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new((
                            "image-preview-control",
                            "left",
                            "Center image",
                        )))
                    })
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
            let output = crate::testutil::take_egui_output(ctx.run_ui(input, |ui| {
                render(ui, &texture, path, "left");
            }));
            let mesh = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh),
                    _ => None,
                })
                .unwrap();
            bounds.push(
                mesh.vertices
                    .iter()
                    .fold(egui::Rect::NOTHING, |mut rect, vertex| {
                        rect.extend_with(vertex.pos);
                        rect
                    }),
            );
        }
        assert_eq!(bounds[0].size(), bounds[2].size());
        assert!(bounds[2].min.x < bounds[0].min.x - 100.0);
        assert!(bounds[2].min.y < bounds[0].min.y - 40.0);
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let shown = view(ui, "left", path).0;
            assert!(!shown.centered);
            assert_eq!(shown.zoom, 1.0);
            assert!(!shown.enlarge_small);
            assert_eq!(shown.orientation, Orientation::default());
            assert_eq!(shown.background, Background::Checkerboard);
            assert!(view(ui, "right", path).0.centered);
            assert!(view(ui, "left", Path::new("/another.png")).0.centered);
        }));
    }

    #[test]
    fn checkerboard_reuses_one_tiny_repeat_texture_and_constant_geometry() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint_background(
                ui,
                egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(100_000.0)),
                Background::Checkerboard,
            );
        });
        let uploads = std::mem::take(&mut output.textures_delta.set);
        output.textures_delta.clear();
        let texture = checker_texture(&ctx);
        assert_eq!(texture.size(), [2, 2]);
        let delta = &uploads.get(&texture.id()).unwrap()[0];
        assert_eq!(delta.options, egui::TextureOptions::NEAREST_REPEAT);
        let egui::ImageData::Color(image) = &delta.image;
        assert_eq!(
            image.pixels,
            [192, 128, 128, 192].map(egui::Color32::from_gray)
        );
        let mesh = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => Some(mesh),
                _ => None,
            })
            .unwrap();
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices.len(), 6);
        let next = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::splat(100.0));
            paint_background(ui, rect, Background::White);
            paint_background(ui, rect, Background::Black);
            paint_background(ui, rect, Background::Checkerboard);
        });
        assert_eq!(checker_texture(&ctx).id(), texture.id());
        let no_upload = !next.textures_delta.set.contains_key(&texture.id());
        crate::testutil::discard_egui_output(next);
        assert!(no_upload);
    }

    #[test]
    fn half_turn_matches_two_right_turns_in_every_mirrored_orientation() {
        for transform in 0..8 {
            let original = Orientation { transform };
            let mut half = original;
            half.rotate_half();
            let mut twice = original;
            twice.rotate_right();
            twice.rotate_right();
            assert_eq!(half, twice);
            assert_eq!(
                half.extent(egui::vec2(320.0, 200.0)),
                original.extent(egui::vec2(320.0, 200.0))
            );
            half.rotate_half();
            assert_eq!(half, original);
        }
    }

    #[test]
    fn all_orientations_compose_in_display_space_and_preserve_extent() {
        let extent = egui::vec2(900.0, 600.0);
        let mut unique = std::collections::HashSet::new();
        for transform in 0..8 {
            let original = Orientation { transform };
            unique.insert(original.corners());
            let mut view = original;
            for _ in 0..4 {
                view.rotate_right();
            }
            assert_eq!(view, original);
            view.rotate_left();
            view.rotate_right();
            assert_eq!(view, original);
            view.mirror_horizontal();
            view.mirror_horizontal();
            assert_eq!(view, original);
            view.mirror_vertical();
            view.mirror_vertical();
            assert_eq!(view, original);
            assert_eq!(original.extent(original.extent(extent)), extent);
            view.rotate_right();
            assert_ne!(view.extent(extent), original.extent(extent));
        }
        assert_eq!(unique.len(), 8);
        let mut view = Orientation::default();
        view.rotate_right();
        assert_eq!(view.corners(), [3, 0, 1, 2]);
        view.mirror_horizontal();
        assert_eq!(view.corners(), [0, 3, 2, 1]);
        view.rotate_left();
        assert_eq!(view.corners(), [3, 2, 1, 0]);
    }

    #[test]
    fn rotated_mesh_maps_source_corners_without_changing_screen_geometry() {
        let mut orientation = Orientation::default();
        orientation.rotate_right();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(60.0, 90.0));
        let mesh = orientation.mesh(egui::TextureId::Managed(9), rect);
        assert_eq!(mesh.texture_id, egui::TextureId::Managed(9));
        assert_eq!(
            mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>(),
            vec![
                rect.left_top(),
                rect.right_top(),
                rect.right_bottom(),
                rect.left_bottom()
            ]
        );
        assert_eq!(
            mesh.vertices.iter().map(|v| v.uv).collect::<Vec<_>>(),
            vec![
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0)
            ]
        );
        assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn switching_file_resets_orientation_without_changing_other_pane() {
        let ctx = egui::Context::default();
        let mut orientation = Orientation::default();
        orientation.mirror_horizontal();
        ctx.data_mut(|data| {
            data.insert_temp(
                id("left"),
                View {
                    path: "/old.png".into(),
                    zoom: 2.0,
                    enlarge_small: false,
                    centered: true,
                    orientation,
                    background: Background::Black,
                },
            )
        });
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(
                view(ui, "left", Path::new("/old.png")).0.background,
                Background::Black
            );
            assert_eq!(
                view(ui, "left", Path::new("/new.png")).0.background,
                Background::Checkerboard
            );
            assert_eq!(
                view(ui, "right", Path::new("/old.png")).0.background,
                Background::Checkerboard
            );
            assert_eq!(
                view(ui, "left", Path::new("/old.png")).0.orientation,
                orientation
            );
            assert_eq!(
                view(ui, "left", Path::new("/new.png")).0.orientation,
                Orientation::default()
            );
            assert_eq!(
                view(ui, "right", Path::new("/old.png")).0.orientation,
                Orientation::default()
            );
        }));
    }

    #[test]
    fn fit_is_aspect_preserving_and_zoom_is_explicitly_relative() {
        let original = Vec2::new(1200.0, 600.0);
        let viewport = Vec2::new(300.0, 200.0);
        assert_eq!(
            display_size(original, viewport, 1.0),
            Vec2::new(300.0, 150.0)
        );
        assert_eq!(
            display_size(original, viewport, 4.0),
            Vec2::new(1200.0, 600.0)
        );
        assert_eq!(
            display_size(Vec2::new(40.0, 20.0), viewport, 1.0),
            Vec2::new(40.0, 20.0)
        );
    }
    #[test]
    fn zoom_limits_and_invalid_gestures_do_not_poison_state() {
        assert_eq!(change_zoom(8.0, 2.0), MAX_ZOOM);
        assert_eq!(change_zoom(0.25, 0.5), MIN_ZOOM);
        for value in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            assert_eq!(change_zoom(2.0, value), 2.0);
        }
    }
    #[test]
    fn mouse_drag_pans_a_zoomed_image() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "pan-test",
            egui::ColorImage::filled([1200, 800], egui::Color32::WHITE),
            Default::default(),
        );
        let path = Path::new("/pan.png");
        ctx.data_mut(|data| {
            data.insert_temp(
                id("left"),
                View {
                    path: path.into(),
                    zoom: 4.0,
                    enlarge_small: false,
                    centered: true,
                    orientation: Orientation::default(),
                    background: Background::default(),
                },
            )
        });
        let mut greatest_offset = Vec2::ZERO;
        for (frame, position) in [
            egui::pos2(200.0, 170.0),
            egui::pos2(200.0, 170.0),
            egui::pos2(100.0, 120.0),
            egui::pos2(100.0, 120.0),
        ]
        .into_iter()
        .enumerate()
        {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 250.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(position));
            if frame == 1 || frame == 3 {
                input.events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: frame == 1,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                let (_, offset) = render(ui, &texture, path, "left");
                greatest_offset = greatest_offset.max(offset);
            }));
        }
        assert!(
            greatest_offset.x > 20.0 && greatest_offset.y > 10.0,
            "drag must pan the image: {greatest_offset:?}"
        );
    }

    #[test]
    fn double_click_returns_a_zoomed_image_to_fit() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "reset-test",
            egui::ColorImage::filled([1200, 800], egui::Color32::WHITE),
            Default::default(),
        );
        let path = Path::new("/reset.png");
        ctx.data_mut(|data| {
            data.insert_temp(
                id("left"),
                View {
                    path: path.into(),
                    zoom: 4.0,
                    enlarge_small: false,
                    centered: true,
                    orientation: Orientation::default(),
                    background: Background::default(),
                },
            )
        });
        for frame in 0..7 {
            let position = egui::pos2(150.0, 150.0);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 250.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(position));
            if (1..=4).contains(&frame) {
                input.events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: frame % 2 == 1,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                render(ui, &texture, path, "left");
            }));
        }
        assert_eq!(
            ctx.data(|data| data.get_temp::<View>(id("left")).unwrap().zoom),
            1.0
        );
    }

    #[test]
    fn image_changes_reset_view_and_panes_keep_independent_state() {
        let ctx = egui::Context::default();
        let path = Path::new("/one.png");
        ctx.data_mut(|data| {
            data.insert_temp(
                id("left"),
                View {
                    path: path.into(),
                    zoom: 4.0,
                    enlarge_small: false,
                    centered: true,
                    orientation: Orientation::default(),
                    background: Background::default(),
                },
            )
        });
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(view(ui, "left", path).0.zoom, 4.0);
            assert!(!view(ui, "left", path).1);
            assert_eq!(view(ui, "right", path).0.zoom, 1.0);
            assert!(view(ui, "left", Path::new("/two.png")).1);
        }));
    }
}
