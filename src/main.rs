#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod command;
mod crumbs;
mod dedup;
mod density;
mod fs_util;
mod fuzzy;
mod image_cache;
mod native_copy;
mod native_menu;
mod panel;
mod rename;
mod scan;
mod selection_summary;
mod session;
mod shelf;
mod sync;
mod theme;
mod transfer;
mod undo;
mod workspace;

#[cfg(test)]
mod testutil;

use eframe::NativeOptions;
use egui::ViewportBuilder;

fn main() -> eframe::Result<()> {
    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Commander")
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([900.0, 500.0])
            .with_titlebar_shown(false)
            .with_fullsize_content_view(true),
        vsync: true,
        hardware_acceleration: eframe::HardwareAcceleration::Required,
        ..Default::default()
    };

    eframe::run_native(
        "Commander",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc)))
        }),
    )
}
