//! Bounded text-only Office packages. No external resources are opened.
use crate::image_cache::PreviewError;
use std::io::{Cursor, Read};
use std::path::Path;

pub const MAX_PACKAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_XML_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const WORD_STRICT: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
const OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";

pub fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        ["docx", "odt", "xlsx", "ods", "pptx", "odp", "epub"]
            .iter()
            .any(|extension| s.eq_ignore_ascii_case(extension))
    })
}

pub fn extract(
    path: &Path,
    bytes: Vec<u8>,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, PreviewError> {
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(PreviewError::TooLarge);
    }
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    if crate::epub_preview::supported(path) {
        return crate::epub_preview::extract(bytes, cancelled);
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("pptx") || s.eq_ignore_ascii_case("odp"))
    {
        return crate::presentation_preview::extract(path, bytes, cancelled);
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("xlsx") || s.eq_ignore_ascii_case("ods"))
    {
        return crate::spreadsheet_preview::extract(path, bytes, cancelled);
    }
    let docx = path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("docx"));
    let member = if docx {
        "word/document.xml"
    } else {
        "content.xml"
    };
    let mut zip = open_package(bytes)?;
    let mut remaining = MAX_XML_BYTES;
    let xml = read_member(&mut zip, member, &mut remaining, cancelled)?
        .ok_or(PreviewError::InvalidDocument)?;
    let doc = parse_xml(&xml)?;
    let body = doc
        .descendants()
        .find(|n| {
            let tag = n.tag_name();
            if docx {
                matches!(tag.namespace(), Some(WORD | WORD_STRICT)) && tag.name() == "body"
            } else {
                tag.namespace() == Some(OFFICE) && tag.name() == "text"
            }
        })
        .ok_or(PreviewError::InvalidDocument)?;
    let mut out = String::new();
    walk(body, docx, false, 0, &mut out, cancelled)?;
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    Ok(out)
}

pub type Package = zip::ZipArchive<Cursor<Vec<u8>>>;
pub fn open_package(bytes: Vec<u8>) -> Result<Package, PreviewError> {
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(PreviewError::TooLarge);
    }
    let zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| PreviewError::InvalidDocument)?;
    if zip.len() > 4096 {
        return Err(PreviewError::TooLarge);
    }
    Ok(zip)
}
pub fn read_member(
    zip: &mut Package,
    member: &str,
    remaining: &mut usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<String>, PreviewError> {
    let mut index = None;
    for i in 0..zip.len() {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let file = zip.by_index(i).map_err(|_| PreviewError::InvalidDocument)?;
        if file.name().map_err(|_| PreviewError::InvalidDocument)? == member
            && index.replace(i).is_some()
        {
            return Err(PreviewError::InvalidDocument);
        }
    }
    let Some(index) = index else {
        return Ok(None);
    };
    let mut file = zip
        .by_index(index)
        .map_err(|_| PreviewError::InvalidDocument)?;
    if file.size() > *remaining as u64 {
        return Err(PreviewError::TooLarge);
    }
    let mut xml = Vec::new();
    let mut chunk = [0; 32 * 1024];
    loop {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let count = file
            .read(&mut chunk)
            .map_err(|_| PreviewError::InvalidDocument)?;
        if count == 0 {
            break;
        }
        if xml.len() + count > *remaining {
            return Err(PreviewError::TooLarge);
        }
        xml.extend_from_slice(&chunk[..count]);
    }
    *remaining -= xml.len();
    Ok(Some(
        String::from_utf8(xml).map_err(|_| PreviewError::InvalidDocument)?,
    ))
}

pub fn append(out: &mut String, text: &str) -> Result<(), PreviewError> {
    if out.len().saturating_add(text.len()) > MAX_TEXT_BYTES {
        return Err(PreviewError::TooLarge);
    }
    out.push_str(text);
    Ok(())
}

pub fn walk(
    node: roxmltree::Node<'_, '_>,
    docx: bool,
    in_text: bool,
    depth: usize,
    out: &mut String,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PreviewError> {
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    if depth > 128 {
        return Err(PreviewError::TooLarge);
    }
    if node.is_text() {
        return if in_text {
            append(out, node.text().unwrap_or_default())
        } else {
            Ok(())
        };
    }
    let tag = node.tag_name();
    let word = matches!(tag.namespace(), Some(WORD | WORD_STRICT));
    let text = tag.namespace() == Some(TEXT);
    if (docx && word && matches!(tag.name(), "del" | "instrText"))
        || (!docx
            && ((tag.namespace() == Some(OFFICE)
                && matches!(tag.name(), "annotation" | "binary-data"))
                || (text && tag.name() == "tracked-changes")))
    {
        return Ok(());
    }
    let is_paragraph = if docx {
        word && tag.name() == "p"
    } else {
        text && matches!(tag.name(), "p" | "h")
    };
    let content = if docx {
        word && tag.name() == "t"
    } else {
        in_text || is_paragraph
    };
    if (docx && word && matches!(tag.name(), "br" | "cr"))
        || (!docx && text && tag.name() == "line-break")
    {
        append(out, "\n")?;
    }
    if (docx && word && tag.name() == "tab") || (!docx && text && tag.name() == "tab") {
        append(out, "\t")?;
    }
    if !docx && text && tag.name() == "s" {
        let count = node
            .attribute((TEXT, "c"))
            .unwrap_or("1")
            .parse::<usize>()
            .map_err(|_| PreviewError::InvalidDocument)?;
        if count > MAX_TEXT_BYTES.saturating_sub(out.len()) {
            return Err(PreviewError::TooLarge);
        }
        out.extend(std::iter::repeat_n(' ', count));
    }
    for child in node.children() {
        walk(child, docx, content, depth + 1, out, cancelled)?;
    }
    if is_paragraph {
        append(out, "\n")?;
    }
    let cell = if docx {
        word && tag.name() == "tc"
    } else {
        tag.namespace() == Some(TABLE) && tag.name() == "table-cell"
    };
    let row = if docx {
        word && tag.name() == "tr"
    } else {
        tag.namespace() == Some(TABLE) && tag.name() == "table-row"
    };
    if cell {
        if out.ends_with('\n') {
            out.pop();
        }
        append(out, "\t")?;
    }
    if row {
        if out.ends_with('\t') {
            out.pop();
        }
        append(out, "\n")?;
    }
    Ok(())
}

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture(docx: bool) -> Vec<u8> {
    package(
        if docx {
            "word/document.xml"
        } else {
            "content.xml"
        },
        if docx {
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Office preview</w:t></w:r></w:p><w:p><w:r><w:t>message: café / ёж 🌍</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#
        } else {
            r#"<o:document-content xmlns:o="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:t="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:b="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><o:body><o:text><t:h>Office preview</t:h><t:p>message: café / ёж 🌍</t:p><b:table><b:table-row><b:table-cell><t:p>A</t:p></b:table-cell><b:table-cell><t:p>B</t:p></b:table-cell></b:table-row></b:table></o:text></o:body></o:document-content>"#
        },
    )
}

#[cfg(any(test, feature = "visual-qa"))]
fn package(member: &str, xml: &str) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(member, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}

pub fn parse_xml(xml: &str) -> Result<roxmltree::Document<'_>, PreviewError> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
        },
    )
    .map_err(|_| PreviewError::InvalidDocument)?;
    if doc
        .descendants()
        .any(|node| node.ancestors().take(130).count() == 130)
    {
        return Err(PreviewError::TooLarge);
    }
    Ok(doc)
}

pub fn resolve_part_target(base_dir: &str, value: &str) -> Result<String, PreviewError> {
    if value.contains(['\\', ':', '?', '#']) {
        return Err(PreviewError::InvalidDocument);
    }
    let joined = if value.starts_with('/') {
        value.trim_start_matches('/').to_string()
    } else {
        format!("{base_dir}/{value}")
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or(PreviewError::InvalidDocument)?;
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(PreviewError::InvalidDocument);
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_packages_preserve_unicode_paragraphs_and_table_text() {
        for (docx, path) in [(true, "a.DOCX"), (false, "a.odt")] {
            assert_eq!(
                extract(Path::new(path), fixture(docx), &|| false).unwrap(),
                "Office preview\nmessage: café / ёж 🌍\nA\tB\n"
            );
        }
    }

    #[test]
    fn namespace_prefixes_entities_breaks_and_deleted_text_are_handled() {
        let xml = format!(
            r#"<d:document xmlns:d="{WORD_STRICT}"><d:body><d:p><d:r><d:t> A &amp; B </d:t><d:tab/><d:t>я</d:t><d:br/><d:t>終</d:t></d:r><d:del><d:r><d:t>deleted</d:t></d:r></d:del><d:r><d:instrText>field code</d:instrText></d:r></d:p></d:body></d:document>"#
        );
        assert_eq!(
            extract(
                Path::new("a.docx"),
                package("word/document.xml", &xml),
                &|| false
            )
            .unwrap(),
            " A & B \tя\n終\n"
        );
        let xml = format!(
            r#"<o:document-content xmlns:o="{OFFICE}" xmlns:t="{TEXT}"><o:body><o:text><t:p>A<t:s t:c="3"/><t:span>B</t:span><t:tab/><t:line-break/>C<o:annotation><t:p>comment</t:p></o:annotation></t:p></o:text></o:body></o:document-content>"#
        );
        assert_eq!(
            extract(Path::new("a.odt"), package("content.xml", &xml), &|| false).unwrap(),
            "A   B\t\nC\n"
        );
    }

    #[test]
    fn invalid_packages_entities_and_excessive_expansion_never_publish_partial_text() {
        for bytes in [
            b"not a zip".to_vec(),
            package("wrong.xml", "<a/>"),
            package("word/document.xml", "<bad>"),
            package(
                "word/document.xml",
                "<!DOCTYPE a [<!ENTITY x 'expand'>]><a>&x;</a>",
            ),
        ] {
            assert_eq!(
                extract(Path::new("a.docx"), bytes, &|| false),
                Err(PreviewError::InvalidDocument)
            );
        }
        let xml = format!(
            r#"<o:text xmlns:o="{OFFICE}" xmlns:t="{TEXT}"><t:p>prefix<t:s t:c="999999999"/></t:p></o:text>"#
        );
        assert_eq!(
            extract(Path::new("a.odt"), package("content.xml", &xml), &|| false),
            Err(PreviewError::TooLarge)
        );
        assert_eq!(
            extract(Path::new("a.docx"), fixture(true), &|| true),
            Err(PreviewError::Cancelled)
        );
        let bytes = package("word/document.xml", &" ".repeat(MAX_XML_BYTES + 1));
        assert_eq!(
            extract(Path::new("a.docx"), bytes, &|| false),
            Err(PreviewError::TooLarge)
        );
        let xml = format!(
            r#"<w:body xmlns:w="{WORD}"><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body>"#,
            "a".repeat(MAX_TEXT_BYTES + 1)
        );
        assert_eq!(
            extract(
                Path::new("a.docx"),
                package("word/document.xml", &xml),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
    }
}
