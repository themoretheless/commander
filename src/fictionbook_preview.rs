//! Bounded FB2 body text, without binary payloads or external resources.
use crate::image_cache::PreviewError;
use crate::office_preview::{append, parse_xml};
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};
use roxmltree::Node;
use std::{io::Read, path::Path};

const FB2: &str = "http://www.gribuser.ru/xml/fictionbook/2.0";
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub fn compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().ends_with(".fb2.zip"))
}
pub fn supported(path: &Path) -> bool {
    if compressed(path) {
        return true;
    }
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("fb2"))
}
/// Decode a single captured file snapshot; ZIP members are never written to disk.
pub fn extract_file(
    path: &Path,
    bytes: Vec<u8>,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, PreviewError> {
    if !compressed(path) {
        return extract(&bytes, cancelled);
    }
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    let mut archive = crate::office_preview::open_package(bytes)?;
    let mut chosen = None;
    for index in 0..archive.len() {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let file = archive.by_index(index).map_err(|_| invalid())?;
        let name = file.name().map_err(|_| invalid())?;
        if file.is_dir() || !name.to_ascii_lowercase().ends_with(".fb2") {
            continue;
        }
        if name.contains(['\\', '\0'])
            || name.split('/').any(|part| matches!(part, "" | "." | ".."))
            || file
                .unix_mode()
                .is_some_and(|mode| matches!(mode & 0o170000, 0o120000 | 0o040000))
            || chosen.replace(index).is_some()
        {
            return Err(invalid());
        }
    }
    let mut file = archive
        .by_index(chosen.ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    if file.size() > MAX_INPUT_BYTES as u64 {
        return Err(PreviewError::TooLarge);
    }
    let mut xml = Vec::new();
    let mut chunk = [0; 32 * 1024];
    loop {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let n = file.read(&mut chunk).map_err(|_| invalid())?;
        if n == 0 {
            break;
        }
        if xml.len() + n > MAX_INPUT_BYTES {
            return Err(PreviewError::TooLarge);
        }
        xml.extend_from_slice(&chunk[..n]);
    }
    extract(&xml, cancelled)
}

fn invalid() -> PreviewError {
    PreviewError::InvalidDocument
}

fn declaration(text: &str) -> Option<String> {
    let text = text.strip_prefix("<?xml")?;
    let end = text.find("?>")?;
    static ENCODING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ENCODING
        .get_or_init(|| regex::Regex::new(r#"\bencoding\s*=\s*["']([^"']+)["']"#).unwrap())
        .captures(&text[..end])
        .map(|c| c[1].to_ascii_lowercase())
}
fn encoding(label: &str) -> Result<&'static Encoding, PreviewError> {
    match label {
        "utf-8" | "utf8" => Ok(UTF_8),
        "utf-16le" => Ok(UTF_16LE),
        "utf-16be" => Ok(UTF_16BE),
        "windows-1251" | "cp1251" => Ok(encoding_rs::WINDOWS_1251),
        "koi8-r" => Ok(encoding_rs::KOI8_R),
        "koi8-u" => Ok(encoding_rs::KOI8_U),
        "iso-8859-5" => Ok(encoding_rs::ISO_8859_5),
        "windows-1252" | "cp1252" => Ok(encoding_rs::WINDOWS_1252),
        _ => Err(invalid()),
    }
}
fn decode(bytes: &[u8]) -> Result<std::borrow::Cow<'_, str>, PreviewError> {
    let (fixed, bytes) = if let Some(bytes) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        (Some(UTF_8), bytes)
    } else if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
        (Some(UTF_16LE), bytes)
    } else if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
        (Some(UTF_16BE), bytes)
    } else if bytes.starts_with(b"<\0?\0") {
        (Some(UTF_16LE), bytes)
    } else if bytes.starts_with(b"\0<\0?") {
        (Some(UTF_16BE), bytes)
    } else {
        (None, bytes)
    };
    let chosen = if let Some(encoding) = fixed {
        encoding
    } else {
        let header = if bytes.starts_with(b"<?xml") {
            let end = bytes
                .windows(2)
                .position(|s| s == b"?>")
                .map_or(bytes.len(), |i| i + 2);
            String::from_utf8_lossy(&bytes[..end])
        } else {
            std::borrow::Cow::Borrowed("")
        };
        declaration(&header)
            .map(|label| encoding(&label))
            .transpose()?
            .unwrap_or(UTF_8)
    };
    let text = chosen
        .decode_without_bom_handling_and_without_replacement(bytes)
        .ok_or_else(invalid)?;
    if text.len() > MAX_INPUT_BYTES {
        return Err(PreviewError::TooLarge);
    }
    if let Some(label) = declaration(&text) {
        let compatible = if label == "utf-16" {
            chosen == UTF_16LE || chosen == UTF_16BE
        } else {
            encoding(&label)? == chosen
        };
        if !compatible {
            return Err(invalid());
        }
    }
    Ok(text)
}

pub fn extract(bytes: &[u8], cancelled: &dyn Fn() -> bool) -> Result<String, PreviewError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(PreviewError::TooLarge);
    }
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    let text = decode(bytes)?;
    let doc = parse_xml(&text)?;
    if !doc.root_element().has_tag_name((FB2, "FictionBook")) {
        return Err(invalid());
    }
    let mut out = String::new();
    if let Some(title) = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name((FB2, "description")))
        .and_then(|n| n.children().find(|n| n.has_tag_name((FB2, "title-info"))))
        .and_then(|n| n.children().find(|n| n.has_tag_name((FB2, "book-title"))))
    {
        let mut title_text = String::new();
        walk(title, true, &mut title_text, cancelled)?;
        if !title_text.is_empty() {
            append(&mut out, &format!("[Book: {title_text}]\n"))?;
        }
    }
    let bodies = doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((FB2, "body")))
        .collect::<Vec<_>>();
    if bodies.is_empty() {
        return Err(invalid());
    }
    for body in bodies {
        if let Some(name) = body.attribute("name") {
            append(&mut out, &format!("[Body: {name}]\n"))?;
        }
        walk(body, false, &mut out, cancelled)?;
    }
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    Ok(out)
}

fn walk(
    node: Node<'_, '_>,
    in_text: bool,
    out: &mut String,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PreviewError> {
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    if node.is_text() {
        return if in_text {
            append(out, node.text().unwrap_or_default())
        } else {
            Ok(())
        };
    }
    if !node.is_element() {
        return Ok(());
    }
    let tag = node.tag_name();
    if tag.namespace() != Some(FB2) || tag.name() == "binary" {
        return Ok(());
    }
    let paragraph = matches!(tag.name(), "p" | "subtitle" | "v" | "text-author" | "date");
    let cell = matches!(tag.name(), "td" | "th");
    if tag.name() == "image" {
        return append(out, "[Image]");
    }
    if tag.name() == "empty-line" {
        return append(out, "\n");
    }
    for child in node.children() {
        walk(child, in_text || paragraph || cell, out, cancelled)?;
    }
    if paragraph || tag.name() == "stanza" {
        append(out, "\n")?;
    }
    if cell {
        if out.ends_with('\n') {
            out.pop();
        }
        append(out, "\t")?;
    }
    if tag.name() == "tr" {
        if out.ends_with('\t') {
            out.pop();
        }
        append(out, "\n")?;
    }
    Ok(())
}

#[cfg(any(test, feature = "visual-qa"))]
pub const FIXTURE_TEXT: &str = "[Book: Книга]\nГлава первая\nmessage: café / ёж 🌍\nСтрока 1\nСтрока 2\n\nA\tB\n[Body: notes]\nNote text\n";

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture() -> Vec<u8> {
    let xml = format!(
        r#"<?xml version="1.0" encoding="windows-1251"?><FictionBook xmlns="{FB2}"><description><title-info><book-title>Книга</book-title></title-info><document-info><p>metadata secret</p></document-info></description><body><section><title><p>Глава первая</p></title><p>message: caf&#233; / ёж &#x1F30D;</p><poem><stanza><v>Строка 1</v><v>Строка 2</v></stanza></poem><table><tr><td>A</td><td>B</td></tr></table></section></body><body name="notes"><section><p>Note text</p></section></body><binary id="cover">binary secret</binary></FictionBook>"#
    );
    let (bytes, _, failed) = encoding_rs::WINDOWS_1251.encode(&xml);
    assert!(!failed);
    bytes.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_files(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::{Cursor, Write};
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn zipped_book_preserves_encoding_and_refuses_ambiguous_or_unsafe_members() {
        let bytes = fixture();
        let path = Path::new("book.FB2.ZIP");
        assert!(supported(path) && compressed(path));
        assert!(!supported(Path::new("book.zip")));
        assert_eq!(
            extract_file(
                path,
                zip_files(&[("folder/BOOK.FB2", &bytes), ("cover.png", b"unused")]),
                &|| false
            )
            .unwrap(),
            FIXTURE_TEXT
        );
        for files in [
            vec![("one.fb2", bytes.as_slice()), ("two.fb2", bytes.as_slice())],
            vec![("../book.fb2", bytes.as_slice())],
            vec![("/book.fb2", bytes.as_slice())],
            vec![("cover.png", b"no book".as_slice())],
        ] {
            assert!(extract_file(path, zip_files(&files), &|| false).is_err());
        }
        assert!(extract_file(path, b"not a zip".to_vec(), &|| false).is_err());
        assert_eq!(
            extract_file(path, zip_files(&[("book.fb2", &bytes)]), &|| true),
            Err(PreviewError::Cancelled)
        );
    }

    #[test]
    fn zipped_book_bounds_expansion_and_checks_member_integrity() {
        let path = Path::new("book.fb2.zip");
        let large = vec![b' '; MAX_INPUT_BYTES + 1];
        assert_eq!(
            extract_file(path, zip_files(&[("book.fb2", &large)]), &|| false),
            Err(PreviewError::TooLarge)
        );
        let bytes = fixture();
        use std::io::{Cursor, Write};
        let mut stored = zip::ZipWriter::new(Cursor::new(Vec::new()));
        stored
            .start_file(
                "book.fb2",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        stored.write_all(&bytes).unwrap();
        let mut zip = stored.finish().unwrap().into_inner();
        let index = zip
            .windows(bytes.len())
            .position(|part| part == bytes)
            .unwrap();
        zip[index] ^= 1;
        assert!(extract_file(path, zip, &|| false).is_err());
        assert_eq!(
            extract_file(Path::new("raw.fb2"), bytes, &|| false).unwrap(),
            FIXTURE_TEXT
        );
    }

    #[test]
    fn windows_1251_book_preserves_title_bodies_poetry_and_tables_without_payloads() {
        assert_eq!(extract(&fixture(), &|| false).unwrap(), FIXTURE_TEXT);
        assert!(supported(Path::new("book.FB2")));
    }

    #[test]
    fn utf8_utf16_and_declared_cyrillic_codepages_decode_without_replacement() {
        for (label, codec) in [
            ("UTF-8", UTF_8),
            ("windows-1251", encoding_rs::WINDOWS_1251),
            ("KOI8-R", encoding_rs::KOI8_R),
            ("koi8-u", encoding_rs::KOI8_U),
            ("iso-8859-5", encoding_rs::ISO_8859_5),
        ] {
            let xml = format!(
                r#"<?xml version="1.0" encoding="{label}"?><FictionBook xmlns="{FB2}"><body><p>Привет ёж</p></body></FictionBook>"#
            );
            let (bytes, _, failed) = codec.encode(&xml);
            assert!(!failed);
            assert_eq!(extract(&bytes, &|| false).unwrap(), "Привет ёж\n");
            if codec == UTF_8 {
                let mut bom = vec![0xef, 0xbb, 0xbf];
                bom.extend_from_slice(&bytes);
                assert_eq!(extract(&bom, &|| false).unwrap(), "Привет ёж\n");
            }
        }
        let xml = format!(
            r#"<?xml version="1.0" encoding="windows-1252"?><FictionBook xmlns="{FB2}"><body><p>café</p></body></FictionBook>"#
        );
        let (bytes, _, failed) = encoding_rs::WINDOWS_1252.encode(&xml);
        assert!(!failed);
        assert_eq!(extract(&bytes, &|| false).unwrap(), "café\n");
        for little in [true, false] {
            for bom in [true, false] {
                let xml = format!(
                    r#"<?xml version="1.0" encoding="UTF-16"?><FictionBook xmlns="{FB2}"><body><p>ёж 🌍</p></body></FictionBook>"#
                );
                let mut bytes = if bom {
                    if little {
                        vec![0xff, 0xfe]
                    } else {
                        vec![0xfe, 0xff]
                    }
                } else {
                    Vec::new()
                };
                for unit in xml.encode_utf16() {
                    bytes.extend_from_slice(&if little {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    });
                }
                assert_eq!(extract(&bytes, &|| false).unwrap(), "ёж 🌍\n");
            }
        }
    }

    #[test]
    fn invalid_bytes_unknown_encodings_and_conflicting_boms_are_rejected() {
        for bytes in [
            vec![0xff],
            vec![0xff, 0xfe, 0, 0xd8],
            b"<?xml version='1.0' encoding='unknown'?><bad/>".to_vec(),
            b"\xef\xbb\xbf<?xml version='1.0' encoding='windows-1251'?><bad/>".to_vec(),
        ] {
            assert_eq!(extract(&bytes, &|| false), Err(invalid()));
        }
    }

    #[test]
    fn inline_links_images_empty_lines_and_namespace_aliases_stay_static() {
        let xml = format!(
            r##"<f:FictionBook xmlns:f="{FB2}" xmlns:l="http://www.w3.org/1999/xlink"><f:body><f:section><f:p>A<f:emphasis>B</f:emphasis><f:a l:href="https://example.com/"> C</f:a><f:image l:href="#image"/></f:p><f:empty-line/><f:subtitle>Subtitle</f:subtitle></f:section></f:body><f:binary>not shown</f:binary></f:FictionBook>"##
        );
        assert_eq!(
            extract(xml.as_bytes(), &|| false).unwrap(),
            "AB C[Image]\n\nSubtitle\n"
        );
    }

    #[test]
    fn malformed_roots_dtd_budgets_and_cancellation_do_not_publish_partial_text() {
        for xml in [
            "<FictionBook/>",
            "<bad>",
            "<!DOCTYPE a [<!ENTITY x 'expand'>]><a>&x;</a>",
        ] {
            assert_eq!(extract(xml.as_bytes(), &|| false), Err(invalid()));
        }
        assert_eq!(
            extract(&vec![0; MAX_INPUT_BYTES + 1], &|| false),
            Err(PreviewError::TooLarge)
        );
        let xml = format!(
            r#"<FictionBook xmlns="{FB2}"><body><p>{}</p></body></FictionBook>"#,
            "a".repeat(256 * 1024 + 1)
        );
        assert_eq!(
            extract(xml.as_bytes(), &|| false),
            Err(PreviewError::TooLarge)
        );
        let calls = std::cell::Cell::new(0);
        assert_eq!(
            extract(&fixture(), &|| {
                let n = calls.get();
                calls.set(n + 1);
                n >= 3
            }),
            Err(PreviewError::Cancelled)
        );
    }
}
