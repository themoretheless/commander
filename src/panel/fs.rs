//! Filesystem reading and directory classification helpers.
//! SRP: "how we read a directory listing and determine its status" in one small file.
//! Uses jwalk for traversal (respects hidden). Extracted to keep panel.rs focused on state.
//! Compare to mc/FAR directory models.

use std::fs;
use std::path::{Path, PathBuf};

use super::FileEntry;

/// Why a directory listing is the way it is, so an empty list can be told
/// apart from an unreadable or vanished directory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DirStatus {
    /// Read succeeded and there are entries.
    Listed,
    /// Read succeeded but the directory is empty.
    Empty,
    /// Permission denied.
    Denied,
    /// The directory no longer exists.
    Gone,
}

/// Classify a directory read for UI messaging. `is_empty` is whether the
/// listing came back with zero entries.
pub fn classify_dir(path: &Path, is_empty: bool) -> DirStatus {
    match fs::read_dir(path) {
        Ok(_) => {
            if is_empty {
                DirStatus::Empty
            } else {
                DirStatus::Listed
            }
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => DirStatus::Gone,
            _ => DirStatus::Denied,
        },
    }
}

pub(crate) fn read_dir(path: &Path, show_hidden: bool) -> Vec<FileEntry> {
    jwalk::WalkDir::new(path)
        .max_depth(1)
        .skip_hidden(!show_hidden)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.depth() == 1) // skip the root dir itself
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            FileEntry::from_meta(e.path(), &meta)
        })
        .collect()
}

/// List subdirectories of `path` (for tree view).
pub fn subdirs(path: &Path, show_hidden: bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            show_hidden
                || !p
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
        })
        .collect();
    dirs.sort_by(|a, b| {
        a.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .cmp(&b.file_name().map(|n| n.to_string_lossy().to_lowercase()))
    });
    dirs
}