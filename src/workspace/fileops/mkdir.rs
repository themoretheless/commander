//! Create a collision-free folder in an explicitly chosen pane directory.

use super::super::{ActivePanel, Workspace};
use crate::ui_request::UiRequest;

pub fn create_dir(workspace: &mut Workspace) {
    create_dir_at(workspace, workspace.active);
}

pub fn create_dir_in_other(workspace: &mut Workspace) {
    create_dir_at(workspace, workspace.active.other());
}

fn create_dir_at(workspace: &mut Workspace, destination: ActivePanel) {
    if let Some(reason) = workspace.mutation_block_reason("creating a folder") {
        workspace.emit_ui_request(UiRequest::Notice {
            message: reason,
            error: true,
        });
        return;
    }
    let (left_read_only, right_read_only) = workspace.pane_read_only();
    if match destination {
        ActivePanel::Left => left_read_only,
        ActivePanel::Right => right_read_only,
    } {
        workspace.emit_ui_request(UiRequest::Notice {
            message: "The destination volume is read-only; could not create a folder".into(),
            error: true,
        });
        return;
    }
    let base = match destination {
        ActivePanel::Left => &workspace.left,
        ActivePanel::Right => &workspace.right,
    }
    .current_path
    .clone();
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
    // A different source directory keeps its live rows, cursor and selection.
    // When both panes show the same directory, both must see its new entry.
    if workspace.left.current_path == base {
        workspace.left.refresh();
    }
    if workspace.right.current_path == base {
        workspace.right.refresh();
    }
    // Land on the new folder and open rename, so it can be named at once.
    if let Some(name) = path.file_name() {
        let panel = match destination {
            ActivePanel::Left => &mut workspace.left,
            ActivePanel::Right => &mut workspace.right,
        };
        panel.focus_child(name.to_string_lossy().into_owned());
    }
    workspace.emit_ui_request(UiRequest::Rename(path));
}
