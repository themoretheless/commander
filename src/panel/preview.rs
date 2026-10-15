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

/// Format the permission bits of a unix mode as "rwxr-xr-x". Like `ls -l`,
/// setuid/setgid show as `s` (`S` without execute) in the owner/group
/// execute slot and the sticky bit as `t` (`T`) in the other slot.
pub fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for (shift, special, mark) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let triplet = (mode >> shift) & 0o7;
        s.push(if triplet & 0o4 != 0 { 'r' } else { '-' });
        s.push(if triplet & 0o2 != 0 { 'w' } else { '-' });
        let execute = triplet & 0o1 != 0;
        s.push(match (mode & special != 0, execute) {
            (true, true) => mark,
            (true, false) => mark.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

/// The Get-Info permissions row. A failed stat says so (with its reason)
/// rather than showing a mode string a real `000` file would also show.
fn permissions_text(metadata: std::io::Result<fs::Metadata>) -> String {
    match metadata {
        #[cfg(unix)]
        Ok(metadata) => {
            use std::os::unix::fs::PermissionsExt;
            format_mode(metadata.permissions().mode())
        }
        #[cfg(not(unix))]
        Ok(metadata) => {
            let text = if metadata.permissions().readonly() {
                "read-only"
            } else {
                "read/write"
            };
            text.to_string()
        }
        Err(error) => format!("unavailable ({error})"),
    }
}

/// Toast text for a preview request on an image the preview provider
/// declined, which would otherwise open nothing without saying why.
pub fn image_preview_unavailable_message(entry: &FileEntry) -> String {
    format!(
        "No preview for {}: image preview is turned off or not rolled out here (Developer panel \u{2192} Runtime controls)",
        entry.name
    )
}

/// Build a Get-Info card for `entry`. `dir_size`/`children` come from the
/// panel's already-computed maps (None while still measuring).
pub fn make_info(entry: &FileEntry, dir_size: Option<u64>, children: Option<usize>) -> InfoCard {
    let target_kind = if entry.is_dir {
        "Folder".to_string()
    } else if entry.extension.is_empty() {
        "Document".to_string()
    } else {
        format!("{} file", entry.extension.to_uppercase())
    };
    // A link shows its target's kind and permissions (they are what opening
    // it uses), but must not pass for the target itself.
    let link_target = entry
        .is_symlink()
        .then(|| fs::read_link(&entry.path).ok())
        .flatten();
    let kind = info_kind(
        target_kind,
        entry.is_symlink(),
        entry.is_symlink() && fs::metadata(&entry.path).is_err(),
    );
    let size = if entry.is_dir {
        dir_size.map_or_else(|| "\u{2026}".to_string(), format_size)
    } else {
        format_size(entry.size)
    };
    let permissions = permissions_text(fs::metadata(&entry.path));
    let path = match link_target {
        Some(target) => format!("{} \u{2192} {}", entry.path.display(), target.display()),
        None => entry.path.display().to_string(),
    };
    InfoCard {
        name: entry.name.clone(),
        path,
        kind,
        size,
        children: if entry.is_dir { children } else { None },
        modified: entry.modified_str.clone(),
        permissions,
    }
}

/// The Get-Info "Kind" row: a symlink names itself (and a dangling one
/// says so) before the kind of what it points at.
fn info_kind(target_kind: String, is_symlink: bool, dangling: bool) -> String {
    match (is_symlink, dangling) {
        (true, true) => "Broken symlink".to_string(),
        (true, false) => format!("Symlink to {target_kind}"),
        (false, _) => target_kind,
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
