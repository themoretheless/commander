use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::{FileEntry, format_size};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewIdentity {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl PreviewIdentity {
    pub fn from_entry(entry: &FileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
        }
    }

    pub fn matches_entry(&self, entry: &FileEntry) -> bool {
        self.path == entry.path && self.size == entry.size && self.modified == entry.modified
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Image(PathBuf),
    Pending(PreviewIdentity),
    Text {
        identity: PreviewIdentity,
        content: Arc<str>,
    },
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
        dir_size.map_or_else(|| "\u{2026}".to_string(), format_size)
    } else {
        format_size(entry.size)
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

/// Create a preview marker without reading file contents. Text is resolved by
/// the app's cancellable background preview pipeline.
pub fn make_preview(entry: &FileEntry) -> Option<PreviewContent> {
    if entry.is_dir {
        return None;
    }
    if !entry.is_image() {
        return Some(PreviewContent::Pending(PreviewIdentity::from_entry(entry)));
    }

    let root = entry.path.parent().unwrap_or(Path::new("/"));
    if !crate::provider_runtime::activate_builtin(
        "native-preview",
        &crate::provider_runtime::ActivationRequest {
            capability: crate::provider_runtime::ProviderCapability::PreviewImage,
            root,
            extension: (!entry.extension.is_empty()).then_some(entry.extension.as_str()),
            bytes: Some(entry.size),
        },
    ) {
        return None;
    }
    Some(PreviewContent::Image(entry.path.clone()))
}
