//! Pure aggregation of a selection: counts, total bytes, and a kind histogram.
//! No egui types, so the bucketing is unit-tested directly. Reuses the
//! image/video predicates on [`FileEntry`] and extension idioms.

use crate::panel::FileEntry;
use serde::{Deserialize, Serialize};

/// Coarse kind of an entry, for the selection breakdown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Document,
    Code,
    Archive,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "folders",
            Kind::Image => "images",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Document => "docs",
            Kind::Code => "code",
            Kind::Archive => "archives",
            Kind::Other => "other",
        }
    }
}

/// Classify an entry into a coarse [`Kind`] by type/extension.
pub fn kind_of(entry: &FileEntry) -> Kind {
    if entry.is_dir {
        return Kind::Folder;
    }
    if entry.is_static_image() {
        return Kind::Image;
    }
    if entry.is_video() {
        return Kind::Video;
    }
    match entry.extension.as_str() {
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "aiff" | "alac" => Kind::Audio,
        "pdf" | "txt" | "md" | "rtf" | "doc" | "docx" | "pages" | "odt" | "tex" | "key" | "ppt"
        | "pptx" | "xls" | "xlsx" | "csv" => Kind::Document,
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz" | "zst" | "dmg" => Kind::Archive,
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "c" | "cpp" | "cc" | "h" | "hpp"
        | "java" | "rb" | "swift" | "kt" | "sh" | "toml" | "json" | "yaml" | "yml" | "html"
        | "css" | "sql" | "lua" | "php" => Kind::Code,
        _ => Kind::Other,
    }
}

/// The stem of a file name (everything before the last interior dot), so a
/// common-prefix guess ignores the extension. A leading dot (dotfile) stays.
fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// Byte length of the shared leading run of `a` and `b`, on a char boundary.
fn common_prefix_len(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .take_while(|((_, ca), cb)| ca == cb)
        .map(|((i, ca), _)| i + ca.len_utf8())
        .last()
        .unwrap_or(0)
}

/// A shared filename prefix across `entries`, trimmed of trailing separators,
/// digits and spaces, if it is at least two characters. A single entry yields
/// its own (trimmed) stem.
fn common_stem_prefix(entries: &[FileEntry]) -> Option<String> {
    let stems: Vec<&str> = entries.iter().map(|e| stem(&e.name)).collect();
    let first = *stems.first()?;
    let mut len = first.len();
    for s in &stems[1..] {
        len = len.min(common_prefix_len(first, s));
    }
    let prefix = first[..len]
        .trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, '_' | '-' | ' ' | '.'));
    (prefix.chars().count() >= 2).then(|| prefix.to_string())
}

/// A folder-friendly label for a dominant kind, or `None` for kinds that make
/// no useful folder name (folders themselves, or the catch-all "other").
fn kind_folder_label(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Image => "Images",
        Kind::Video => "Videos",
        Kind::Audio => "Audio",
        Kind::Document => "Documents",
        Kind::Code => "Code",
        Kind::Archive => "Archives",
        Kind::Folder | Kind::Other => return None,
    })
}

/// Suggest a folder name for gathering `entries` into a new subfolder: a shared
/// filename prefix if there is one, else the dominant kind's label when one
/// kind is a strict majority, else "New Folder". Pure.
pub fn suggest_folder_name(entries: &[FileEntry]) -> String {
    if entries.is_empty() {
        return "New Folder".to_string();
    }
    if let Some(prefix) = common_stem_prefix(entries) {
        return prefix;
    }
    // Dominant kind: a strict majority (> half) that maps to a useful label.
    let summary = summarize(entries);
    if let Some(&(kind, n)) = summary.kinds.first()
        && n * 2 > entries.len()
        && let Some(label) = kind_folder_label(kind)
    {
        return label.to_string();
    }
    "New Folder".to_string()
}

/// What a selection contains: total count, how many are folders, the summed
/// byte size (folders report 0 here), and a per-kind histogram sorted by count.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SelectionSummary {
    pub count: usize,
    pub dir_count: usize,
    pub total_bytes: u64,
    /// (kind, count) sorted by count descending; ties keep first-seen order.
    pub kinds: Vec<(Kind, usize)>,
}

/// Aggregate `entries` into a [`SelectionSummary`].
pub fn summarize(entries: &[FileEntry]) -> SelectionSummary {
    let mut kinds: Vec<(Kind, usize)> = Vec::new();
    let mut dir_count = 0;
    let mut total_bytes = 0u64;
    for e in entries {
        if e.is_dir {
            dir_count += 1;
        }
        total_bytes += e.size;
        let k = kind_of(e);
        if let Some(slot) = kinds.iter_mut().find(|(kk, _)| *kk == k) {
            slot.1 += 1;
        } else {
            kinds.push((k, 1));
        }
    }
    // Stable sort by count descending; ties keep first-seen order.
    kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    SelectionSummary {
        count: entries.len(),
        dir_count,
        total_bytes,
        kinds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, is_dir: bool, size: u64) -> FileEntry {
        let ext = PathBuf::from(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/x/{name}")),
            is_dir,
            size,
            extension: ext,
            modified: None,
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    #[test]
    fn kind_of_buckets_by_extension_and_type() {
        assert_eq!(kind_of(&entry("Pics", true, 0)), Kind::Folder);
        assert_eq!(kind_of(&entry("a.png", false, 1)), Kind::Image);
        assert_eq!(kind_of(&entry("a.mp4", false, 1)), Kind::Video);
        assert_eq!(kind_of(&entry("a.mp3", false, 1)), Kind::Audio);
        assert_eq!(kind_of(&entry("a.pdf", false, 1)), Kind::Document);
        assert_eq!(kind_of(&entry("a.rs", false, 1)), Kind::Code);
        assert_eq!(kind_of(&entry("a.zip", false, 1)), Kind::Archive);
        assert_eq!(kind_of(&entry("a.xyz", false, 1)), Kind::Other);
        assert_eq!(kind_of(&entry("noext", false, 1)), Kind::Other);
    }

    #[test]
    fn summarize_counts_dirs_bytes_and_kinds() {
        let entries = vec![
            entry("a.png", false, 100),
            entry("b.png", false, 200),
            entry("c.rs", false, 50),
            entry("Folder", true, 0),
        ];
        let s = summarize(&entries);
        assert_eq!(s.count, 4);
        assert_eq!(s.dir_count, 1);
        assert_eq!(s.total_bytes, 350);
        // Images (2) lead, then code/folder (1 each) in first-seen order.
        assert_eq!(s.kinds[0], (Kind::Image, 2));
        assert!(s.kinds.contains(&(Kind::Code, 1)));
        assert!(s.kinds.contains(&(Kind::Folder, 1)));
    }

    #[test]
    fn empty_selection_is_zeroed() {
        let s = summarize(&[]);
        assert_eq!(s.count, 0);
        assert_eq!(s.dir_count, 0);
        assert_eq!(s.total_bytes, 0);
        assert!(s.kinds.is_empty());
    }

    #[test]
    fn suggest_folder_name_uses_common_prefix() {
        let e = vec![
            entry("IMG_001.jpg", false, 1),
            entry("IMG_002.jpg", false, 1),
        ];
        assert_eq!(suggest_folder_name(&e), "IMG");
        let e = vec![
            entry("report-jan.pdf", false, 1),
            entry("report-feb.pdf", false, 1),
        ];
        assert_eq!(suggest_folder_name(&e), "report");
    }

    #[test]
    fn suggest_folder_name_falls_back_to_dominant_kind() {
        // No shared prefix, but a clear image majority.
        let e = vec![
            entry("cat.jpg", false, 1),
            entry("dog.png", false, 1),
            entry("bird.jpeg", false, 1),
        ];
        assert_eq!(suggest_folder_name(&e), "Images");
    }

    #[test]
    fn suggest_folder_name_mixed_is_new_folder() {
        // No prefix and no majority kind (one each, plus a lone 'other').
        let e = vec![
            entry("a.jpg", false, 1),
            entry("b.pdf", false, 1),
            entry("c.rs", false, 1),
            entry("d.xyz", false, 1),
        ];
        assert_eq!(suggest_folder_name(&e), "New Folder");
    }

    #[test]
    fn suggest_folder_name_single_and_empty() {
        assert_eq!(suggest_folder_name(&[]), "New Folder");
        // A single file trims trailing digits off its stem.
        assert_eq!(
            suggest_folder_name(&[entry("Vacation2024.jpg", false, 1)]),
            "Vacation"
        );
    }

    #[test]
    fn kinds_sorted_by_count_descending() {
        let entries = vec![
            entry("a.rs", false, 1),
            entry("b.rs", false, 1),
            entry("c.rs", false, 1),
            entry("d.png", false, 1),
        ];
        let s = summarize(&entries);
        assert_eq!(s.kinds[0], (Kind::Code, 3));
        assert_eq!(s.kinds[1], (Kind::Image, 1));
    }
}
