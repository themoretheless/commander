//! FileEntry model and display helpers.
//! Small focused piece (SRP): the data row shown in lists, its metadata, icon, natural formatting.
//! Extracted from monolithic panel.rs for easier understanding in small chunks.
//! Pure functions here are easy to unit-test in isolation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// On-disk entry: mtime as seconds+nanos since UNIX epoch, and size. (kept for cache serde in sizes module later)
#[derive(Serialize, Deserialize)]
pub struct CacheEntry {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub name_lower: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub extension: String,
    pub modified: Option<std::time::SystemTime>,
    /// Pre-formatted date string (rendered every frame, formatted once).
    pub modified_str: String,
    /// Pre-formatted size string for files ("…" for dirs).
    pub size_str: String,
}

impl FileEntry {
    /// Build an entry from a path and its (already fetched) metadata.
    pub fn from_meta(path: PathBuf, meta: &fs::Metadata) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta.modified().ok();
        let modified_str = match modified {
            Some(time) => {
                let datetime: chrono::DateTime<chrono::Local> = time.into();
                datetime.format("%d %b %y  %H:%M").to_string()
            }
            None => "–".to_string(),
        };
        let size_str = if is_dir {
            "…".to_string()
        } else {
            format_size(size)
        };
        let name_lower = name.to_lowercase();
        Some(FileEntry {
            name,
            name_lower,
            path,
            is_dir,
            size,
            extension,
            modified,
            modified_str,
            size_str,
        })
    }

    pub fn is_image(&self) -> bool {
        self.is_static_image() || self.is_video()
    }

    pub fn is_static_image(&self) -> bool {
        matches!(
            self.extension.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "webp"
                | "svg"
                | "ico"
                | "heic"
                | "heif"
                | "tiff"
                | "tif"
                | "dng"
                | "cr2"
                | "cr3"
                | "nef"
                | "arw"
                | "orf"
                | "raf"
                | "rw2"
                | "pef"
                | "srw"
        )
    }

    pub fn is_video(&self) -> bool {
        matches!(
            self.extension.as_str(),
            "mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" | "wmv" | "flv"
        )
    }

    pub fn icon(&self) -> &str {
        if self.is_dir {
            return "📁";
        }
        match self.extension.as_str() {
            "rs" => "🦀",
            "py" => "🐍",
            "js" | "ts" | "jsx" | "tsx" => "🟨",
            "html" | "css" | "scss" => "🌐",
            "json" | "toml" | "yaml" | "yml" | "xml" => "⚙️",
            "md" | "txt" | "rtf" | "doc" | "docx" => "📄",
            "pdf" => "📕",
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => "🖼️",
            "mp4" | "mov" | "avi" | "mkv" | "webm" => "🎬",
            "mp3" | "wav" | "flac" | "aac" | "ogg" => "🎵",
            "zip" | "tar" | "gz" | "7z" | "rar" | "bz2" | "xz" => "📦",
            "sh" | "bash" | "zsh" | "fish" => "💻",
            "exe" | "dmg" | "app" | "msi" => "⚡",
            "swift" => "🐦",
            "go" => "🐹",
            "java" | "kt" => "☕",
            "c" | "cpp" | "h" | "hpp" => "🔧",
            "lock" => "🔒",
            _ => "📄",
        }
    }

    pub fn size_display(&self) -> &str {
        &self.size_str
    }

    pub fn size_display_with_dir_size(&self, dir_sizes: &HashMap<PathBuf, u64>) -> String {
        if self.is_dir
            && let Some(&size) = dir_sizes.get(&self.path)
        {
            return format_size(size);
        }
        self.size_str.clone()
    }

    pub fn modified_display(&self) -> &str {
        &self.modified_str
    }
}

/// Natural/human size formatting. Kept here because it's the display companion to FileEntry.size_str.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
