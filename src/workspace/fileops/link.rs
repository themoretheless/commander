//! Create symlinks / hard links in the inactive panel for the active selection.

use super::super::Workspace;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkReport {
    pub created: usize,
    pub errors: Vec<String>,
}

pub fn create_symlinks(workspace: &mut Workspace) -> LinkReport {
    create_links(workspace, LinkKind::Symbolic)
}

pub fn create_relative_symlinks(workspace: &mut Workspace) -> LinkReport {
    create_links(workspace, LinkKind::Relative)
}

pub fn create_hardlinks(workspace: &mut Workspace) -> LinkReport {
    create_links(workspace, LinkKind::Hard)
}

#[derive(Clone, Copy)]
enum LinkKind {
    Symbolic,
    Relative,
    Hard,
}

fn create_links(workspace: &mut Workspace, kind: LinkKind) -> LinkReport {
    let mut report = LinkReport::default();
    if let Some(reason) = workspace.mutation_block_reason("creating links") {
        report.errors.push(reason);
        return report;
    }
    let Ok(entries) = workspace.active_panel_ref().picked_entries() else {
        return report;
    };
    if entries.is_empty() {
        return report;
    }
    let dest_dir = workspace.inactive_panel().current_path.clone();
    for entry in entries {
        // The on-disk name, not the lossy display name, so non-UTF-8 names
        // are linked under their real name.
        let target_name = entry
            .path
            .file_name()
            .map_or_else(|| OsString::from(&entry.name), OsStr::to_os_string);
        let link_path = crate::fs_util::first_available(|i| {
            if i == 0 {
                dest_dir.join(&target_name)
            } else {
                dest_dir.join(numbered_link_name(&target_name, i + 1))
            }
        });
        let result = match kind {
            LinkKind::Symbolic => create_symlink(&entry.path, &link_path),
            LinkKind::Relative => relative_target(&entry.path, &link_path)
                .and_then(|target| create_symlink(&target, &link_path)),
            LinkKind::Hard => {
                if entry.is_dir {
                    Err(format!(
                        "Could not hard-link {}: hard links require a regular file",
                        entry.name
                    ))
                } else {
                    std::fs::hard_link(&entry.path, &link_path)
                        .map_err(|error| format!("Could not hard-link {}: {error}", entry.name))
                }
            }
        };
        match result {
            Ok(()) => report.created += 1,
            Err(error) => report.errors.push(error),
        }
    }
    workspace.left.refresh();
    workspace.right.refresh();
    report
}

/// "photo link 2.jpg": the counter goes before the extension so the link
/// keeps its type. Names without an extension (and dotfiles) get it at the end.
fn numbered_link_name(name: &OsStr, number: usize) -> OsString {
    let path = Path::new(name);
    let (stem, extension) = match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) => (stem, Some(extension)),
        _ => (name, None),
    };
    let mut numbered = stem.to_os_string();
    numbered.push(format!(" link {number}"));
    if let Some(extension) = extension {
        numbered.push(".");
        numbered.push(extension);
    }
    numbered
}

/// Resolve containing directories physically: `..` must not traverse lexical
/// symlink components. Preserve the source's last component, including dangling
/// symlinks, instead of silently linking to its final referent.
fn relative_target(source: &Path, link: &Path) -> Result<PathBuf, String> {
    let resolve = || -> std::io::Result<PathBuf> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "link paths need containing directories and a source name",
            )
        };
        let from = std::fs::canonicalize(link.parent().ok_or_else(invalid)?)?;
        let to = std::fs::canonicalize(source.parent().ok_or_else(invalid)?)?
            .join(source.file_name().ok_or_else(invalid)?);
        let a: Vec<_> = from.components().collect();
        let b: Vec<_> = to.components().collect();
        if a.first() != b.first() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "relative links require a shared filesystem root",
            ));
        }
        let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let mut target = PathBuf::new();
        for _ in common..a.len() {
            target.push("..");
        }
        for part in &b[common..] {
            target.push(part.as_os_str());
        }
        if target.as_os_str().is_empty() {
            target.push(".");
        }
        Ok(target)
    };
    resolve().map_err(|e| {
        format!(
            "Could not create relative symlink {} -> {}: {e}",
            link.display(),
            source.display()
        )
    })
}

fn create_symlink(source: &Path, link_path: &PathBuf) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::numbered_link_name;
    use std::ffi::OsStr;

    #[test]
    fn numbered_link_name_keeps_the_extension_last() {
        assert_eq!(
            numbered_link_name(OsStr::new("photo.jpg"), 2),
            "photo link 2.jpg"
        );
        assert_eq!(
            numbered_link_name(OsStr::new("archive.tar.gz"), 3),
            "archive.tar link 3.gz"
        );
        assert_eq!(numbered_link_name(OsStr::new("notes"), 2), "notes link 2");
        assert_eq!(
            numbered_link_name(OsStr::new(".bashrc"), 2),
            ".bashrc link 2"
        );
    }
}
