#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accessibility;
mod app;
mod archive;
mod bookmarks;
mod clipboard;
mod cmdtemplate;
mod collections;
mod command;
mod compare;
mod conflict;
mod content_index;
mod crumbs;
mod dedup;
mod delta_copy;
mod density;
mod file_color;
mod filesystem_policy;
mod focus_mode;
mod fs_util;
mod fuzzy;
mod image_cache;
mod io_budget;
mod jumplist;
mod listing_export;
mod lock_util;
mod mount_guard;
mod native_copy;
mod native_menu;
mod operation;
mod operation_journal;
mod operation_view;
mod opqueue;
mod panel;
mod path_identity;
mod query;
mod quick_actions;
mod receipts;
mod reldate;
mod rename;
mod rename_order;
mod scan;
mod search;
mod selection_summary;
mod selset;
mod session;
mod shelf;
mod smart_folder;
mod sync;
mod sync_guard;
mod textdiff;
mod theme;
mod toasts;
mod transfer;
mod transfer_tuning;
mod tree_overview;
mod treemap;
mod undo;
mod verified_hash;
mod version_store;
mod volume_profile;
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
        // eframe 0.35 moved vsync/hardware-acceleration into the
        // backend-specific `wgpu_options`/`glow_options`; the wgpu
        // defaults (AutoVsync, hardware-accelerated Metal adapter on
        // macOS) already match what this app wants.
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
