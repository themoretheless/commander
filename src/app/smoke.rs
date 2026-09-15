//! Headless UI smoke harness for `app/`.
//!
//! # `egui_kittest` scaffolding (future)
//!
//! A full [`egui_kittest`](https://docs.rs/egui_kittest) harness is deliberately
//! deferred. Pulling it as a default or Linux CI dependency risks breaking
//! `cargo check` on hosts without the native egui backends Commander already
//! stubs around. When adopting it:
//!
//! 1. Add `egui_kittest` under `[dev-dependencies]` pinned to the same egui
//!    minor as `Cargo.toml` (currently 0.36).
//! 2. Prefer `Harness::new_ui` against pure dialog widgets such as
//!    [`crate::app::confirm_dialog::method_tabs`], not the full `App`.
//! 3. Keep production binaries free of the harness crate.
//!
//! Until then, smoke coverage uses `egui::Context::run_ui` plus
//! [`crate::testutil::discard_egui_output`], matching `ui_contract` tests.

#[cfg(test)]
mod tests {
    use crate::app::confirm_dialog::method_tabs;
    use crate::theme::ThemeColors;
    use crate::transfer::CopyMethod;

    #[test]
    fn method_tabs_smoke_renders_without_selection_change() {
        let ctx = egui::Context::default();
        let mut selected = None;
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            selected = method_tabs::show(ui, &ThemeColors::light(), "Copy", 3, CopyMethod::Native);
        }));
        assert_eq!(selected, None);
    }
}
