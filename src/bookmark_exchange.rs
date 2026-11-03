//! GTK text and XBEL bookmark exchange. Pure lexical parsing; never probes imported paths.
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
fn bookmark_record(uri: &str, label: Option<&str>) -> Result<Bookmark, Status> {
    decode_uri(uri).and_then(|path| {
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
    })
}
/// Detect XML after BOM/leading whitespace; GTK keeps its existing line grammar.
pub fn parse(source: &str, current: &Bookmarks) -> Result<Preview, String> {
    if source.len() > MAX_BYTES {
        return Err("Bookmark text is limited to 64 KiB".into());
    }
    if source
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
    {
        parse_xbel(source, current)
    } else {
        parse_gtk(source, current)
    }
}
fn xbel_tag(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().namespace().is_none() && node.has_tag_name(name)
}
fn parse_xbel(source: &str, current: &Bookmarks) -> Result<Preview, String> {
    if source.contains("<!ENTITY") {
        return Err("XBEL entity declarations are not supported".into());
    }
    let doc = roxmltree::Document::parse_with_options(
        source.trim_start_matches('\u{feff}'),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: 8192,
        },
    )
    .map_err(|error| format!("Invalid XBEL: {error}"))?;
    let root = doc.root_element();
    if !xbel_tag(root, "xbel") {
        return Err("Expected an unnamespaced XBEL root element".into());
    }
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for node in root.descendants().filter(roxmltree::Node::is_element) {
        if node
            .ancestors()
            .filter(|parent| parent.is_element())
            .take(33)
            .count()
            > 32
        {
            return Err("XBEL nesting is limited to 32 elements".into());
        }
        if !xbel_tag(node, "bookmark")
            || !node
                .ancestors()
                .skip(1)
                .take_while(|p| *p != root)
                .all(|p| xbel_tag(p, "folder"))
        {
            continue;
        }
        if rows.len() == MAX_LINES {
            return Err("XBEL is limited to 1000 bookmarks".into());
        }
        let line = doc.text_pos_at(node.range().start).row as usize;
        let titles = node
            .children()
            .filter(|n| xbel_tag(*n, "title"))
            .collect::<Vec<_>>();
        let title = if titles.len() > 1
            || titles
                .first()
                .is_some_and(|title| title.children().any(|child| child.is_element()))
        {
            Err(Status::Invalid(
                "XBEL bookmark needs at most one plain-text title".into(),
            ))
        } else {
            Ok(titles.first().map(|title| {
                title
                    .children()
                    .filter(|child| child.is_text())
                    .filter_map(|child| child.text())
                    .collect::<String>()
            }))
        };
        let result = title.and_then(|title| {
            node.attribute("href")
                .ok_or_else(|| Status::Invalid("XBEL bookmark has no href".into()))
                .and_then(|uri| bookmark_record(uri, title.as_deref()))
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
            line,
            bookmark,
            status,
            selected,
        });
    }
    if rows.is_empty() {
        return Err("No XBEL bookmark records to review".into());
    }
    Ok(Preview {
        rows,
        source: source.into(),
    })
}
fn parse_gtk(source: &str, current: &Bookmarks) -> Result<Preview, String> {
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
        let result = bookmark_record(uri, label);
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

pub fn export_xbel(current: &Bookmarks) -> Result<String, String> {
    // Keep the existing path/name validation and URL encoding contract.
    let gtk = export(current)?;
    if current.items.len() > MAX_LINES {
        return Err("XBEL is limited to 1000 bookmarks".into());
    }
    let mut out = String::new();
    let append = |out: &mut String, text: &str| -> Result<(), String> {
        if text.len() > MAX_BYTES.saturating_sub(out.len()) {
            return Err("XBEL export exceeds 64 KiB; nothing copied".into());
        }
        out.push_str(text);
        Ok(())
    };
    let escape = |out: &mut String, text: &str| -> Result<(), String> {
        for ch in text.chars() {
            if !matches!(ch,'\u{9}'|'\u{a}'|'\u{d}'|' '..='\u{d7ff}'|'\u{e000}'..='\u{fffd}'|'\u{10000}'..='\u{10ffff}')
            {
                return Err("XBEL export needs XML 1.0 characters; nothing copied".into());
            }
            match ch {
                '&' => append(out, "&amp;")?,
                '<' => append(out, "&lt;")?,
                '>' => append(out, "&gt;")?,
                '"' => append(out, "&quot;")?,
                '\'' => append(out, "&apos;")?,
                _ => {
                    let mut bytes = [0; 4];
                    append(out, ch.encode_utf8(&mut bytes))?;
                }
            }
        }
        Ok(())
    };
    append(
        &mut out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\">\n",
    )?;
    for (line, bookmark) in gtk.lines().zip(&current.items) {
        let uri = line
            .split_once(' ')
            .map(|(uri, _)| uri)
            .ok_or("Missing bookmark URI")?;
        append(&mut out, "  <bookmark href=\"")?;
        escape(&mut out, uri)?;
        append(&mut out, "\"><title>")?;
        escape(&mut out, &bookmark.name)?;
        append(&mut out, "</title></bookmark>\n")?;
    }
    append(&mut out, "</xbel>\n")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xbel_nested_local_bookmarks_preserve_order_labels_status_and_review_choices() {
        let mut current = Bookmarks::default();
        current.add("Original", "/old");
        current.items[0].slot = Some(3);
        let source = r#"<?xml version="1.0"?><!DOCTYPE xbel><xbel version="1.0" xmlns:e="urn:extra">
<title>Top</title><bookmark href="file:///old"><title>Replacement</title></bookmark>
<folder><title>Folder</title><bookmark href="file:///missing/%D1%91%D0%B6%20x"><title>Проект &amp; XML<!--ignore--></title></bookmark>
<bookmark href="file://localhost/missing/%D1%91%D0%B6%20x"><title>Duplicate</title></bookmark>
<bookmark href="smb://server/share"><title>Remote</title></bookmark><bookmark href="file:///bad%GG"/>
<bookmark href="file:///empty"><title/></bookmark><bookmark href="file:///fallback"/>
<folder><bookmark href="file:///other"><title>Other</title></bookmark></folder></folder>
<info><bookmark href="file:///ignored-metadata"/></info><e:bookmark href="file:///ignored-extension"/>
<bookmark><title>No href</title></bookmark><bookmark href="file:///markup"><title><b>bad</b></title></bookmark>
</xbel>"#;
        let mut preview = parse(source, &current).unwrap();
        assert_eq!(preview.rows.len(), 10);
        assert_eq!(preview.rows[0].status, Status::Existing);
        assert_eq!(preview.rows[1].status, Status::Ready);
        assert_eq!(preview.rows[2].status, Status::Duplicate);
        assert_eq!(preview.rows[3].status, Status::Unsupported);
        assert!(matches!(preview.rows[4].status, Status::Invalid(_)));
        assert!(matches!(preview.rows[5].status, Status::Invalid(_)));
        assert_eq!(preview.rows[6].bookmark.as_ref().unwrap().name, "fallback");
        assert_eq!(preview.rows[7].status, Status::Ready);
        assert!(matches!(preview.rows[8].status, Status::Invalid(_)));
        assert!(matches!(preview.rows[9].status, Status::Invalid(_)));
        assert_eq!(
            preview.rows[1].bookmark.as_ref().unwrap().name,
            "Проект & XML"
        );
        assert_eq!(
            preview.rows[1].bookmark.as_ref().unwrap().path,
            Path::new("/missing/ёж x")
        );
        assert_eq!(preview.rows[1].line, 3);
        assert_eq!(preview.selected(), 3);
        preview.rows[6].selected = false;
        let (merged, added) = merge(source, &preview, &current).unwrap();
        assert_eq!(added, 2);
        assert_eq!(merged.items[0], current.items[0]);
        assert_eq!(merged.items[1].path, Path::new("/missing/ёж x"));
        assert_eq!(merged.items[2].path, Path::new("/other"));
        assert_eq!(preview.rebase(&merged).unwrap().selected(), 0);
        assert!(merge(&(source.to_owned() + " "), &preview, &current).is_err());
    }
    #[test]
    fn xbel_refuses_malformed_entities_depth_nodes_records_and_source_limits() {
        for source in [
            "<xbel><bookmark href='file:///ok'/>",
            "<other><bookmark href='file:///ok'/></other>",
            "<xbel xmlns='urn:foreign'><bookmark href='file:///ok'/></xbel>",
            "<!DOCTYPE xbel [<!ENTITY x 'file:///unsafe'>]><xbel><bookmark href='&x;'/></xbel>",
            "<!DOCTYPE xbel [<!ENTITY x SYSTEM 'file:///missing/private'>]><xbel><bookmark href='&x;'/></xbel>",
        ] {
            assert!(parse(source, &Bookmarks::default()).is_err(), "{source}");
        }
        let external = "<!DOCTYPE xbel SYSTEM 'https://example.invalid/xbel.dtd'><xbel><bookmark href='file:///ok'/></xbel>";
        assert_eq!(
            parse(external, &Bookmarks::default()).unwrap().selected(),
            1
        );
        let too_many = format!(
            "<xbel>{}</xbel>",
            "<bookmark href='file:///a'/>".repeat(1001)
        );
        assert!(
            parse(&too_many, &Bookmarks::default())
                .unwrap_err()
                .contains("1000")
        );
        let deep = format!(
            "<xbel>{}<bookmark href='file:///a'/>{}</xbel>",
            "<folder>".repeat(32),
            "</folder>".repeat(32)
        );
        assert!(
            parse(&deep, &Bookmarks::default())
                .unwrap_err()
                .contains("nesting")
        );
        let nodes = format!("<xbel>{}</xbel>", "<s/>".repeat(8192));
        assert!(parse(&nodes, &Bookmarks::default()).is_err());
        assert!(
            parse(
                &format!("<xbel>{}</xbel>", " ".repeat(MAX_BYTES)),
                &Bookmarks::default()
            )
            .unwrap_err()
            .contains("64 KiB")
        );
    }
    #[test]
    fn xbel_export_round_trips_exact_escaped_fields_and_refuses_partial_or_lossy_output() {
        let mut current = Bookmarks::default();
        current.add("Проект & <XML> \"'", "/missing/ёж x#?%&");
        current.items[0].slot = Some(5);
        current.add("Second", "/another/missing");
        let source = export_xbel(&current).unwrap();
        assert!(source.contains("&amp; &lt;XML&gt; &quot;&apos;"));
        let preview = parse(&("\u{feff}".to_owned() + &source), &Bookmarks::default()).unwrap();
        assert_eq!(
            preview
                .rows
                .iter()
                .map(|r| r.bookmark.as_ref().unwrap().path.clone())
                .collect::<Vec<_>>(),
            current
                .items
                .iter()
                .map(|b| b.path.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            preview.rows[0].bookmark.as_ref().unwrap().name,
            current.items[0].name
        );
        assert!(
            preview
                .rows
                .iter()
                .all(|r| r.bookmark.as_ref().unwrap().slot.is_none())
        );
        current.items[0].name = "invalid\u{fffe}".into();
        assert!(export_xbel(&current).is_err());
        current.items[0].name = "&".repeat(256);
        current.items = (0..100)
            .map(|n| Bookmark {
                name: current.items[0].name.clone(),
                path: format!("/missing/{n}").into(),
                slot: None,
            })
            .collect();
        assert!(export_xbel(&current).is_err());
        assert!(export_xbel(&Bookmarks::default()).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            current.items.truncate(1);
            current.items[0].path = std::ffi::OsString::from_vec(vec![b'/', 255]).into();
            assert!(export_xbel(&current).is_err());
        }
    }
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
