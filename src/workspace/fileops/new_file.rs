//! Create an empty document without following or replacing an existing entry.
use super::super::Workspace;
use crate::ui_request::UiRequest;

pub fn create_empty_file(workspace: &mut Workspace) {
    let refusal = workspace
        .mutation_block_reason("creating a file")
        .or_else(|| {
            (workspace.pending_op.is_some() || workspace.has_any_pending_ui_modal())
                .then(|| "Finish the pending dialog before creating a file".into())
        })
        .or_else(|| {
            let (left, right) = workspace.pane_read_only();
            (if workspace.active == super::super::ActivePanel::Left {
                left
            } else {
                right
            })
            .then(|| "The destination volume is read-only; could not create a file".into())
        });
    if let Some(message) = refusal {
        workspace.emit_ui_request(UiRequest::Notice {
            message,
            error: true,
        });
        return;
    }
    let base = workspace.active_panel_ref().current_path.clone();
    let result = (|| {
        for index in 0..1024 {
            let path = base.join(if index == 0 {
                "New File.txt".into()
            } else {
                format!("New File {index}.txt")
            });
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    drop(file);
                    return Ok(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("Could not create file {}: {error}", path.display()));
                }
            }
        }
        Err("Could not find an unused file name after 1024 attempts".into())
    })();
    let path = match result {
        Ok(path) => path,
        Err(message) => {
            workspace.emit_ui_request(UiRequest::Notice {
                message,
                error: true,
            });
            return;
        }
    };
    if workspace.left.current_path == base {
        workspace.left.refresh();
    }
    if workspace.right.current_path == base {
        workspace.right.refresh();
    }
    workspace
        .active_panel()
        .focus_child(path.file_name().unwrap().to_string_lossy().into_owned());
    workspace.emit_ui_request(UiRequest::Rename(path));
}
