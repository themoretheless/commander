//! GTK bookmark text exchange. Pure lexical parsing; never probes imported paths.
use crate::bookmarks::{Bookmark, Bookmarks};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};
pub const MAX_BYTES: usize = 64 * 1024;
const MAX_LINES: usize = 1000;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    Existing,
    Duplicate,
    Unsupported,
    Invalid(String),
}
impl Status {
    pub fn label(&self) -> &str {
        match self {
            Self::Ready => "Ready",
            Self::Existing => "Already bookmarked",
            Self::Duplicate => "Duplicate input",
            Self::Unsupported => "Unsupported remote URI",
            Self::Invalid(_) => "Invalid",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub bookmark: Option<Bookmark>,
    pub status: Status,
    pub selected: bool,
}
#[derive(Debug)]
pub struct Preview {
    pub rows: Vec<Row>,
    source: String,
}
impl Preview {
    pub fn selected(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.status == Status::Ready && row.selected)
            .count()
    }
    /// Reclassify current duplicates after a successful merge, keeping unchecked choices unchecked.
    pub fn rebase(&self, current: &Bookmarks) -> Result<Self, String> {
        let mut next = parse(&self.source, current)?;
        for (row, previous) in next.rows.iter_mut().zip(&self.rows) {
            row.selected = row.status == Status::Ready && previous.selected;
        }
        Ok(next)
    }
}
fn decode_uri(uri: &str) -> Result<PathBuf, Status> {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return Err(Status::Invalid("Expected a file:// URI".into()));
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Err(Status::Unsupported);
    }
    let Some(slash) = rest.find('/') else {
        return Err(Status::Invalid("File URI has no absolute path".into()));
    };
    let host = &rest[..slash];
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return Err(Status::Unsupported);
    }
    let raw = &rest[slash..];
    if raw.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(Status::Invalid(
            "Encode control characters in file URIs".into(),
        ));
    }
    if raw.contains(['?', '#']) {
        return Err(Status::Invalid(
            "Encode ? and # in file paths as %3F and %23".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(raw.len());
    let mut input = raw.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = |byte: u8| (byte as char).to_digit(16).map(|value| value as u8);
            let high = input.next().and_then(hex);
            let low = input.next().and_then(hex);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(Status::Invalid("Malformed percent escape".into()));
            };
            bytes.push((high << 4) | low);
        } else {
            bytes.push(byte);
        }
    }
    let decoded =
        String::from_utf8(bytes).map_err(|_| Status::Invalid("Path is not UTF-8".into()))?;
    let path = Path::new(&decoded);
    if decoded.contains('\0')
        || !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(Status::Invalid(
            "Require an absolute local path without NUL or parent traversal".into(),
        ));
    }
    Ok(path
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect())
}
pub fn parse(source: &str, current: &Bookmarks) -> Result<Preview, String> {
    if source.len() > MAX_BYTES {
        return Err("Bookmark text is limited to 64 KiB".into());
    }
    if source.lines().take(MAX_LINES + 1).count() > MAX_LINES {
        return Err("Bookmark text is limited to 1000 lines".into());
    }
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in source.trim_start_matches('\u{feff}').lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (uri, label) = line
            .split_once(' ')
            .map_or((line, None), |(uri, label)| (uri, Some(label)));
        let result = decode_uri(uri).and_then(|path| {
            let name = label.map(str::trim).map(str::to_owned).unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("/")
                    .into()
            });
            if !crate::bookmarks::valid_name(&name)
                || label.is_some_and(|label| label.chars().any(char::is_control))
            {
                return Err(Status::Invalid(
                    "Label must contain 1–256 characters without controls".into(),
                ));
            }
            Ok(Bookmark {
                name,
                path,
                slot: None,
            })
        });
        let (bookmark, status) = match result {
            Ok(bookmark) => {
                let status = if !seen.insert(bookmark.path.clone()) {
                    Status::Duplicate
                } else if current.contains(&bookmark.path) {
                    Status::Existing
                } else {
                    Status::Ready
                };
                (Some(bookmark), status)
            }
            Err(status) => (None, status),
        };
        let selected = status == Status::Ready;
        rows.push(Row {
            line: index + 1,
            bookmark,
            status,
            selected,
        });
    }
    if rows.is_empty() {
        return Err("No bookmark records to review".into());
    }
    Ok(Preview {
        rows,
        source: source.into(),
    })
}
/// Reparse at commit time and append only selected, still-new paths. Existing labels/slots win.
pub fn merge(
    source: &str,
    preview: &Preview,
    current: &Bookmarks,
) -> Result<(Bookmarks, usize), String> {
    if source != preview.source {
        return Err("Text changed; preview it again before adding".into());
    }
    let fresh = parse(source, current)?;
    if fresh.rows.len() != preview.rows.len()
        || fresh
            .rows
            .iter()
            .zip(&preview.rows)
            .any(|(fresh, old)| fresh.line != old.line || fresh.bookmark != old.bookmark)
    {
        return Err("Preview changed; review the text again".into());
    }
    let mut next = current.clone();
    let mut added = 0;
    for (row, old) in fresh.rows.iter().zip(&preview.rows) {
        if old.selected && old.status == Status::Ready && row.status == Status::Ready {
            let bookmark = row.bookmark.as_ref().ok_or("Missing bookmark candidate")?;
            added += usize::from(next.add(bookmark.name.clone(), bookmark.path.clone()));
        }
    }
    Ok((next, added))
}
pub fn export(current: &Bookmarks) -> Result<String, String> {
    let mut output = String::new();
    for bookmark in &current.items {
        if !bookmark.path.is_absolute()
            || bookmark.path.to_str().is_none()
            || bookmark
                .path
                .to_str()
                .is_some_and(|path| path.contains('\0'))
            || bookmark
                .path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
            || !crate::bookmarks::valid_name(&bookmark.name)
        {
            return Err("A bookmark has an invalid path or label; edit it before exporting".into());
        }
        let uri = crate::clipboard::format(
            std::slice::from_ref(&bookmark.path),
            crate::clipboard::PathStyle::FileUrl,
            None,
        );
        if uri
            .len()
            .saturating_add(bookmark.name.len())
            .saturating_add(2)
            > MAX_BYTES.saturating_sub(output.len())
        {
            return Err("Export exceeds 64 KiB; reduce the bookmark list".into());
        }
        output.push_str(&uri);
        output.push(' ');
        output.push_str(&bookmark.name);
        output.push('\n');
    }
    if output.is_empty() {
        return Err("No favorites to export".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gtk_text_preserves_local_unicode_paths_labels_and_order_without_probing() {
        let source = "\u{feff}# comment\r\nfile:///absent/%D0%BF%D0%B0%D0%BF%D0%BA%D0%B0%20x  Проект UI  \r\nfile://localhost/other/a%23%3F%25\r\nfile:///\n";
        let preview = parse(source, &Bookmarks::default()).unwrap();
        assert_eq!(preview.rows.len(), 3);
        let a = preview.rows[0].bookmark.as_ref().unwrap();
        assert_eq!(a.path, Path::new("/absent/папка x"));
        assert_eq!(a.name, "Проект UI");
        assert_eq!(preview.rows[1].bookmark.as_ref().unwrap().name, "a#?%");
        assert_eq!(preview.rows[2].bookmark.as_ref().unwrap().name, "/");
        assert_eq!(preview.selected(), 3);
        assert!(preview.rows.iter().all(|row| row.status == Status::Ready));
    }
    #[test]
    fn unsafe_remote_malformed_and_duplicate_records_are_reviewed_individually() {
        let source = "file:///a Keep\nfile:///a Other\nsmb://server/share Remote\nfile://server/share Remote\nfile:///bad%XZ Bad\nfile:///bad%00 Bad\nfile:///bad%FF Bad\nfile:///a/../b Bad\nfile:///query?x Bad\nrelative Bad\nfile:///blank   \n";
        let mut store = Bookmarks::default();
        store.add("Existing", "/a");
        store.assign_slot(Path::new("/a"), 3);
        let preview = parse(source, &store).unwrap();
        assert_eq!(preview.rows[0].status, Status::Existing);
        assert_eq!(preview.rows[1].status, Status::Duplicate);
        assert_eq!(preview.rows[2].status, Status::Unsupported);
        assert_eq!(preview.rows[3].status, Status::Unsupported);
        assert!(
            preview.rows[4..]
                .iter()
                .all(|row| matches!(row.status, Status::Invalid(_)))
        );
        assert_eq!(preview.selected(), 0);
        let (next, count) = merge(source, &preview, &store).unwrap();
        assert_eq!(count, 0);
        assert_eq!(next, store);
    }
    #[test]
    fn merge_rechecks_current_duplicates_preserves_metadata_and_rejects_dirty_or_corrupt_preview() {
        let source = "file:///a New A\nfile:///b New B\nfile:///c New C\n";
        let mut store = Bookmarks::default();
        let mut preview = parse(source, &store).unwrap();
        preview.rows[2].selected = false;
        store.add("Existing wins", "/a");
        store.assign_slot(Path::new("/a"), 2);
        let (next, count) = merge(source, &preview, &store).unwrap();
        assert_eq!(count, 1);
        assert_eq!(next.items[0], store.items[0]);
        assert_eq!(next.items[1].path, Path::new("/b"));
        assert!(!next.contains(Path::new("/c")));
        let rebased = preview.rebase(&next).unwrap();
        assert_eq!(rebased.selected(), 0);
        assert!(merge("file:///elsewhere", &preview, &store).is_err());
        preview.rows[1].bookmark.as_mut().unwrap().path = "/tampered".into();
        assert!(merge(source, &preview, &store).is_err());
    }
    #[test]
    fn gtk_export_round_trips_supported_names_without_quick_jump_slot_reassignment() {
        let mut store = Bookmarks::default();
        store.add("Проект UI", "/missing/a #?% ü");
        store.add("Root", "/");
        store.assign_slot(Path::new("/"), 9);
        let text = export(&store).unwrap();
        assert!(text.contains("a%20%23%3F%25%20%C3%BC"));
        let preview = parse(&text, &Bookmarks::default()).unwrap();
        let (next, count) = merge(&text, &preview, &Bookmarks::default()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(next.items[0].name, store.items[0].name);
        assert_eq!(next.items[0].path, store.items[0].path);
        assert!(next.items.iter().all(|bookmark| bookmark.slot.is_none()));
        store.items[0].name = "bad\nlabel".into();
        assert!(export(&store).is_err());
        assert!(export(&Bookmarks::default()).is_err());
    }
    #[test]
    fn bounded_import_and_export_fail_before_returning_partial_data() {
        assert!(parse(&"a".repeat(MAX_BYTES + 1), &Bookmarks::default()).is_err());
        assert!(parse(&"\n".repeat(MAX_LINES + 1), &Bookmarks::default()).is_err());
        assert!(parse("# only comment", &Bookmarks::default()).is_err());
        let mut store = Bookmarks::default();
        store.add("Large", format!("/{}", "x".repeat(MAX_BYTES)));
        assert!(export(&store).is_err());
    }
}
