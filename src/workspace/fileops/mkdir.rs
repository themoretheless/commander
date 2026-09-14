//! Create a collision-free folder in the active directory.

use super::super::Workspace;

pub(crate) fn create_dir(workspace: &mut Workspace) {
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
    let _ = std::fs::create_dir(&path);
    workspace.left.refresh();
    workspace.right.refresh();
}
