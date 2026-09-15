//! Create symlinks / hard links in the inactive panel for the active selection.

use super::super::Workspace;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkReport {
    pub created: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

pub(crate) fn create_symlinks(workspace: &mut Workspace) -> LinkReport {
    create_links(workspace, LinkKind::Symbolic)
}

pub(crate) fn create_hardlinks(workspace: &mut Workspace) -> LinkReport {
    create_links(workspace, LinkKind::Hard)
}

#[derive(Clone, Copy)]
enum LinkKind {
    Symbolic,
    Hard,
}

fn create_links(workspace: &mut Workspace, kind: LinkKind) -> LinkReport {
    let mut report = LinkReport::default();
    if workspace.mutation_commits_blocked() {
        report
            .errors
            .push("A file operation is already in progress".to_string());
        return report;
    }
    let Ok(entries) = workspace.active_panel_ref().selected_or_cursor() else {
        return report;
    };
    if entries.is_empty() {
        return report;
    }
    let dest_dir = workspace.inactive_panel().current_path.clone();
    for entry in entries {
        let target_name = entry.name.clone();
        let link_path = crate::fs_util::first_available(|i| {
            if i == 0 {
                dest_dir.join(&target_name)
            } else {
                dest_dir.join(format!("{target_name} link {}", i + 1))
            }
        });
        let result = match kind {
            LinkKind::Symbolic => create_symlink(&entry.path, &link_path),
            LinkKind::Hard => {
                if entry.is_dir {
                    Err("Hard links require a regular file".to_string())
                } else {
                    std::fs::hard_link(&entry.path, &link_path)
                        .map_err(|error| format!("Could not hard-link {}: {error}", entry.name))
                }
            }
        };
        match result {
            Ok(()) => report.created += 1,
            Err(error) => {
                report.skipped += 1;
                report.errors.push(error);
            }
        }
    }
    workspace.left.refresh();
    workspace.right.refresh();
    report
}

fn create_symlink(source: &std::path::Path, link_path: &PathBuf) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, link_path).map_err(|error| {
            format!(
                "Could not symlink {} -> {}: {error}",
                link_path.display(),
                source.display()
            )
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (source, link_path);
        Err("Symlinks are only supported on Unix hosts".to_string())
    }
}
