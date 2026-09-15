#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accessibility;
mod app;
mod archive;
mod assistive_timeline;
mod trust;
mod launch;
mod checksum;
pub mod benchmark_fixture;
mod bookmarks;
pub mod capability_diagnostic;
mod clipboard;
mod change_provenance;
mod cmdtemplate;
mod collections;
mod color_manage;
mod command;
mod compare;
mod conflict;
mod conflict_rules;
mod content_index;
mod crumbs;
mod dedup;
mod delta_copy;
mod density;
mod decoder_breaker;
mod display_name;
pub mod feature_flags;
mod file_color;
mod filesystem_policy;
mod focus_mode;
mod fs_at;
mod fs_util;
mod fuzzy;
mod image_cache;
mod io_budget;
mod jumplist;
pub mod klm;
mod listing_export;
mod machine_pressure;
mod lock_util;
mod logging;
pub mod measurement;
mod mount_guard;
mod native_copy;
mod native_effect;
mod native_menu;
mod native_release_qa;
mod operation;
mod operation_journal;
mod operation_view;
mod opqueue;
mod panel;
mod path_identity;
mod path_probe;
mod pathname;
mod persistence;
pub mod ports;
pub mod provider_runtime;
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
pub mod support_bundle;
mod encrypted_bundle;
mod support_encrypt;
mod sync;
mod sync_guard;
mod textdiff;
mod theme;
mod toasts;
mod transfer;
mod transfer_tuning;
mod tree_overview;
mod treemap;
mod ui_request;
mod undo;
mod verified_hash;
mod version_dedup;
mod version_store;
#[cfg(feature = "visual-qa")]
mod visual_qa;
mod volume_profile;
mod watcher_health;
mod watcher_policy;
pub mod workload;
mod workspace_profile;
mod workspace;

#[cfg(test)]
mod operation_verification;
#[cfg(test)]
mod testutil;

use eframe::NativeOptions;
use egui::ViewportBuilder;

fn main() -> eframe::Result<()> {
    logging::init();

    let launch = launch::sanitize_launch_paths(launch::parse_launch_args(std::env::args()));

    #[cfg(feature = "visual-qa")]
    if let Some(result) = visual_qa::maybe_run() {
        return result;
    }

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
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let context_menu = native_menu::MacOsContextMenu::new().map_err(|error| {
                std::io::Error::other(format!(
                    "could not construct the main-thread AppKit adapter: {error:?}"
                ))
            })?;
            let clipboard = native_effect::MacOsClipboard::new().map_err(|error| {
                std::io::Error::other(format!(
                    "could not construct the main-thread clipboard adapter: {error:?}"
                ))
            })?;
            let opener = native_effect::MacOsOpener::new().map_err(|error| {
                std::io::Error::other(format!(
                    "could not construct the main-thread opener adapter: {error:?}"
                ))
            })?;
            Ok(Box::new(app::App::new(
                cc,
                app::AppServices {
                    context_menu: std::rc::Rc::new(context_menu),
                    clipboard: std::rc::Rc::new(clipboard),
                    opener: std::rc::Rc::new(opener),
                    trash: std::sync::Arc::new(native_effect::NativeTrash),
                    free_space: std::sync::Arc::new(native_effect::NativeFreeSpace),
                    persistence: persistence::fs_persist(),
                    workload: workload::global_handle(),
                    directory_probe: std::sync::Arc::new(pathname::FsDirectoryProbe),
                },
                launch.clone(),
            )))
        }),
    )
}
