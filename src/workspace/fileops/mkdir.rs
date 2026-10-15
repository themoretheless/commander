//! Create a collision-free folder in the active directory.

use super::super::Workspace;
use crate::ui_request::UiRequest;

pub fn create_dir(workspace: &mut Workspace) {
    if workspace.mutation_commits_blocked() {
        return;
    }
    let base = workspace.active_panel_ref().current_path.clone();
    let path = crate::fs_util::first_available(|i| {
        if i == 0 {
            base.join("New Folder")
        } else {
            base.join(format!("New Folder {}", i))
        }
    });
    if let Err(error) = std::fs::create_dir(&path) {
        workspace.emit_ui_request(UiRequest::Notice {
            message: format!("Could not create folder {}: {error}", path.display()),
            error: true,
        });
        return;
    }
    workspace.left.refresh();
    workspace.right.refresh();
    // Land on the new folder and open rename, so it can be named at once.
    if let Some(name) = path.file_name() {
        workspace
            .active_panel()
            .focus_child(name.to_string_lossy().into_owned());
    }
    workspace.emit_ui_request(UiRequest::Rename(path));
}
