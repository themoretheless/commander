//! Preview content types and factories (image/text/info "Get Info").
//! Consolidated here for DRY (was duplicated/stubbed in panel.rs and this file).
//! SRP: separate from the listing/filter/state in panel/*. Small file for easy reading.
//! Reexported from panel so all existing call sites (TogglePreview, ToggleInfo, render) are unchanged.

use crate::panel::FileEntry;
use std::fs;
use std::path::PathBuf;

/// What the preview area (or info card) should show for a cursor entry.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Image(PathBuf),
    Text { path: PathBuf, content: String },
    Info(InfoCard),
}

/// Precomputed metadata card for the Get-Info inspector (display-only).
#[derive(Debug, Clone, PartialEq)]
pub struct InfoCard {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: String,
    pub children: Option<usize>,
    pub modified: String,
    pub permissions: String,
}

/// Format the low 9 bits of a unix mode as "rwxr-xr-x".
pub fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let triplet = (mode >> shift) & 0o7;
        s.push(if triplet & 0o4 != 0 { 'r' } else { '-' });
        s.push(if triplet & 0o2 != 0 { 'w' } else { '-' });
        s.push(if triplet & 0o1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Build a Get-Info card for `entry`. `dir_size`/`children` come from the
/// panel's already-computed maps (None while still measuring).
pub fn make_info(entry: &FileEntry, dir_size: Option<u64>, children: Option<usize>) -> InfoCard {
    let kind = if entry.is_dir {
        "Folder".to_string()
    } else if entry.extension.is_empty() {
        "Document".to_string()
    } else {
        format!("{} file", entry.extension.to_uppercase())
    };
    let size = if entry.is_dir {
        dir_size.map_or_else(|| "\u{2026}".to_string(), crate::panel::format_size)
    } else {
        crate::panel::format_size(entry.size)
    };
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&entry.path)
            .map(|m| format_mode(m.permissions().mode()))
            .unwrap_or_else(|_| "---------".to_string())
    };
    InfoCard {
        name: entry.name.clone(),
        path: entry.path.display().to_string(),
        kind,
        size,
        children: if entry.is_dir { children } else { None },
        modified: entry.modified_str.clone(),
        permissions,
    }
}

/// Create preview content for a file entry (image marker or text body).
pub fn make_preview(entry: &FileEntry) -> Option<PreviewContent> {
    if entry.is_dir {
        return None;
    }
    if entry.is_image() {
        Some(PreviewContent::Image(entry.path.clone()))
    } else {
        // Try to read as text (limit to 1MB)
        let Ok(meta) = fs::metadata(&entry.path) else {
            return None;
        };
        if meta.len() > 1024 * 1024 {
            return None; // Too large
        }
        let Ok(content) = fs::read_to_string(&entry.path) else {
            return None;
        };
        Some(PreviewContent::Text {
            path: entry.path.clone(),
            content,
        })
    }
}
