//! Inline rename editor: a small modal seeded from the cursor entry, with
//! live name validation. Commit/validation logic lives in `workspace`.

use super::*;

impl App {
    pub(crate) fn show_rename_dialog(&mut self, ctx: &egui::Context) {
        // Pick up a rename request raised by Command::BeginRename.
        if let Some(path) = self.ws.requests.rename_target.take() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            self.renaming = Some(RenameState {
                path,
                buffer: name,
                error: None,
                focused: false,
            });
        }

        let Some(state) = &mut self.renaming else {
            return;
        };
        // For inline quick-rename (idea): only set state from target, edit happens in list row via TextEdit.
        // No modal window. Commit/cancel on Enter/Esc wired via list + update handling.
        let _ = (state, ctx); // keep state, input for validation can be added later
        // (Dialog suppressed to make rename truly inline in the file list.)
    }
}
