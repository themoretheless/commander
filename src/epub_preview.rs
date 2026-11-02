//! Static EPUB spine text; no converters, URLs, scripts or embedded media.
use crate::image_cache::PreviewError;
use crate::office_preview::{append, open_package, parse_xml, read_member, resolve_part_target};
use std::path::Path;

const CONTAINER: &str = "urn:oasis:names:tc:opendocument:xmlns:container";
const OPF: &str = "http://www.idpf.org/2007/opf";
const ENCRYPTION: &str = "http://www.w3.org/2001/04/xmlenc#";

fn invalid() -> PreviewError {
    PreviewError::InvalidDocument
}
pub fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("epub"))
}

fn part(base: &str, href: &str) -> Result<String, PreviewError> {
    let mut bytes = Vec::new();
    let mut source = href.bytes();
    while let Some(byte) = source.next() {
        if byte == b'%' {
            let nibble = |byte: u8| {
                char::from(byte)
                    .to_digit(16)
                    .map(|v| v as u8)
                    .ok_or_else(invalid)
            };
            let value = nibble(source.next().ok_or_else(invalid)?)? * 16
                + nibble(source.next().ok_or_else(invalid)?)?;
            if value == 0 {
                return Err(invalid());
            }
            bytes.push(value);
        } else {
            bytes.push(byte);
        }
    }
    resolve_part_target(base, &String::from_utf8(bytes).map_err(|_| invalid())?)
}

pub fn extract(bytes: Vec<u8>, cancelled: &dyn Fn() -> bool) -> Result<String, PreviewError> {
    let mut zip = open_package(bytes)?;
    let mut remaining = 4 * 1024 * 1024;
    let container = read_member(
        &mut zip,
        "META-INF/container.xml",
        &mut remaining,
        cancelled,
    )?
    .ok_or_else(invalid)?;
    let container = parse_xml(&container)?;
    let rootfile = container
        .descendants()
        .find(|n| {
            n.has_tag_name((CONTAINER, "rootfile"))
                && n.attribute("media-type") == Some("application/oebps-package+xml")
        })
        .ok_or_else(invalid)?;
    let package = part("", rootfile.attribute("full-path").ok_or_else(invalid)?)?;
    let base = package.rsplit_once('/').map_or("", |(base, _)| base);
    let xml = read_member(&mut zip, &package, &mut remaining, cancelled)?.ok_or_else(invalid)?;
    let doc = parse_xml(&xml)?;
    if !doc.root_element().has_tag_name((OPF, "package")) {
        return Err(invalid());
    }
    let manifest = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name((OPF, "manifest")))
        .ok_or_else(invalid)?;
    let spine = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name((OPF, "spine")))
        .ok_or_else(invalid)?;
    let mut items = std::collections::HashMap::new();
    for item in manifest
        .children()
        .filter(|n| n.has_tag_name((OPF, "item")))
    {
        if items
            .insert(item.attribute("id").ok_or_else(invalid)?, item)
            .is_some()
        {
            return Err(invalid());
        }
    }
    let mut encrypted = std::collections::HashSet::new();
    if let Some(xml) = read_member(
        &mut zip,
        "META-INF/encryption.xml",
        &mut remaining,
        cancelled,
    )? {
        let encryption = parse_xml(&xml)?;
        for resource in encryption
            .descendants()
            .filter(|n| n.has_tag_name((ENCRYPTION, "CipherReference")))
        {
            encrypted.insert(part("", resource.attribute("URI").ok_or_else(invalid)?)?);
        }
    }
    let chapters = spine
        .children()
        .filter(|n| n.has_tag_name((OPF, "itemref")))
        .collect::<Vec<_>>();
    if chapters.len() > 512 {
        return Err(PreviewError::TooLarge);
    }
    let mut out = String::new();
    for (index, chapter) in chapters.into_iter().enumerate() {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let item = *items
            .get(chapter.attribute("idref").ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        if !matches!(
            item.attribute("media-type"),
            Some("application/xhtml+xml" | "text/html")
        ) {
            return Err(invalid());
        }
        let chapter = part(base, item.attribute("href").ok_or_else(invalid)?)?;
        if encrypted.contains(&chapter) {
            return Err(invalid());
        }
        let html =
            read_member(&mut zip, &chapter, &mut remaining, cancelled)?.ok_or_else(invalid)?;
        append(&mut out, &format!("[Chapter {}]\n", index + 1))?;
        chapter_text(&html, &mut out, cancelled)?;
    }
    Ok(out)
}

fn newline(out: &mut String) -> Result<(), PreviewError> {
    while out.ends_with([' ', '\t']) {
        out.pop();
    }
    if !out.ends_with('\n') {
        append(out, "\n")?;
    }
    Ok(())
}

fn chapter_text(
    html: &str,
    out: &mut String,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PreviewError> {
    use html5gum::{DefaultEmitter, Token, Tokenizer};
    let mut emitter = DefaultEmitter::default();
    emitter.naively_switch_states(true);
    let mut body = false;
    let mut finished = false;
    let mut hidden = Vec::new();
    let mut depth = 0_usize;
    let mut pre = 0_usize;
    for (index, token) in Tokenizer::new_with_emitter(html, emitter).enumerate() {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        if index >= 100_000 {
            return Err(PreviewError::TooLarge);
        }
        match token {
            Ok(Token::StartTag(tag)) => {
                let name = String::from_utf8_lossy(&tag.name)
                    .rsplit(':')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if name == "body" {
                    if body || finished {
                        return Err(invalid());
                    }
                    body = true;
                    continue;
                }
                if !body {
                    continue;
                }
                if matches!(
                    name.as_str(),
                    "script" | "style" | "head" | "template" | "iframe" | "object" | "svg" | "math"
                ) {
                    if !tag.self_closing {
                        hidden.push(name);
                    }
                    if hidden.len() > 128 {
                        return Err(PreviewError::TooLarge);
                    }
                    continue;
                }
                if !hidden.is_empty() {
                    continue;
                }
                if !matches!(
                    name.as_str(),
                    "br" | "img"
                        | "hr"
                        | "meta"
                        | "link"
                        | "input"
                        | "wbr"
                        | "source"
                        | "area"
                        | "embed"
                        | "param"
                        | "track"
                        | "col"
                        | "base"
                ) && !tag.self_closing
                {
                    depth += 1;
                    if depth > 128 {
                        return Err(PreviewError::TooLarge);
                    }
                }
                match name.as_str() {
                    "p" | "div" | "section" | "article" | "h1" | "h2" | "h3" | "h4" | "h5"
                    | "h6" | "tr" | "blockquote" | "li" | "pre" => newline(out)?,
                    "br" | "hr" => {
                        append(out, "\n")?;
                    }
                    "img" => append(out, "[Image]")?,
                    _ => {}
                }
                if name == "pre" {
                    pre += 1;
                }
            }
            Ok(Token::EndTag(tag)) => {
                let name = String::from_utf8_lossy(&tag.name)
                    .rsplit(':')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if name == "body" {
                    if !body || !hidden.is_empty() {
                        return Err(invalid());
                    }
                    body = false;
                    finished = true;
                    continue;
                }
                if !body {
                    continue;
                }
                if !hidden.is_empty() {
                    if hidden.last() == Some(&name) {
                        hidden.pop();
                    }
                    continue;
                }
                depth = depth.saturating_sub(1);
                if name == "pre" {
                    pre = pre.saturating_sub(1);
                    if !out.ends_with('\n') {
                        append(out, "\n")?;
                    }
                    continue;
                }
                match name.as_str() {
                    "p" | "div" | "section" | "article" | "h1" | "h2" | "h3" | "h4" | "h5"
                    | "h6" | "tr" | "blockquote" | "li" | "pre" => newline(out)?,
                    "td" | "th" => append(out, "\t")?,
                    _ => {}
                }
            }
            Ok(Token::String(text)) if body && hidden.is_empty() => {
                let text = String::from_utf8_lossy(&text.value);
                if pre > 0 {
                    append(out, &text)?;
                } else {
                    for ch in text.chars() {
                        if ch.is_ascii_whitespace() {
                            if !out.ends_with([' ', '\n', '\t']) {
                                append(out, " ")?;
                            }
                        } else {
                            append(out, ch.encode_utf8(&mut [0; 4]))?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !finished || body {
        return Err(invalid());
    }
    newline(out)
}

#[cfg(any(test, feature = "visual-qa"))]
pub const FIXTURE_TEXT: &str =
    "[Chapter 1]\nChapter Two\nmessage: café / ёж 🌍\n[Chapter 2]\nChapter One\nSecond chapter\n";

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture() -> Vec<u8> {
    book(
        "second",
        "application/xhtml+xml",
        "second%20chapter.xhtml",
        None,
    )
}

#[cfg(any(test, feature = "visual-qa"))]
fn book(first: &str, mime: &str, href: &str, encryption: Option<&str>) -> Vec<u8> {
    let container = format!(
        r#"<container xmlns="{CONTAINER}"><rootfiles><rootfile full-path="OEBPS/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#
    );
    let package_xml = format!(
        r#"<package xmlns="{OPF}" version="3.0"><manifest><item id="first" href="first.xhtml" media-type="application/xhtml+xml"/><item id="second" href="{href}" media-type="{mime}"/></manifest><spine><itemref idref="{first}"/><itemref idref="first"/></spine></package>"#
    );
    let mut parts = vec![
        ("META-INF/container.xml", container.as_str()),
        ("OEBPS/book.opf", package_xml.as_str()),
        (
            "OEBPS/first.xhtml",
            "<!DOCTYPE html><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>Hidden title</title></head><body><h1>Chapter One</h1><p>Second chapter</p></body></html>",
        ),
        (
            "OEBPS/second chapter.xhtml",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><style>style secret</style></head><body><h1>Chapter Two</h1><script>script secret</script><p>message: café / ёж 🌍</p></body></html>",
        ),
    ];
    if let Some(encryption) = encryption {
        parts.push(("META-INF/encryption.xml", encryption));
    }
    package(&parts)
}

#[cfg(any(test, feature = "visual-qa"))]
fn package(parts: &[(&str, &str)]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in parts {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spine_order_encoded_paths_unicode_and_static_body_text_are_preserved() {
        assert_eq!(
            crate::office_preview::extract(Path::new("book.EPUB"), fixture(), &|| false).unwrap(),
            FIXTURE_TEXT
        );
    }

    #[test]
    fn html_entities_inline_word_boundaries_tables_and_preformatted_text_are_retained() {
        let mut out = String::new();
        chapter_text("<!DOCTYPE html><html><head><title>ignore</title></head><body><p>A<b>B</b> C&nbsp;&amp; D</p><table><tr><td>я</td><td>🌍</td></tr></table><svg/><pre>A  \n B\t </pre><img src='https://example.com/private.png'/><template>hide me</template><iframe>also hidden</iframe></body></html>", &mut out, &|| false).unwrap();
        assert_eq!(out, "\nAB C\u{a0}& D\nя\t🌍\nA  \n B\t \n[Image]\n");
    }

    #[test]
    fn escaping_external_missing_and_invalid_spine_resources_fail() {
        for href in [
            "https://example.com/chapter.xhtml",
            "../../outside.xhtml",
            "%2e%2e/%2e%2e/outside.xhtml",
            "missing.xhtml",
            "bad%ZZ.xhtml",
            "%00.xhtml",
        ] {
            assert_eq!(
                extract(book("second", "application/xhtml+xml", href, None), &|| {
                    false
                }),
                Err(invalid())
            );
        }
        assert_eq!(
            extract(
                book(
                    "unknown",
                    "application/xhtml+xml",
                    "second%20chapter.xhtml",
                    None
                ),
                &|| false
            ),
            Err(invalid())
        );
        assert_eq!(
            extract(
                book("second", "image/png", "second%20chapter.xhtml", None),
                &|| false
            ),
            Err(invalid())
        );
    }

    #[test]
    fn encrypted_chapters_are_rejected_but_unread_obfuscated_fonts_do_not_block_text() {
        let encryption = |uri| {
            format!(
                r#"<encryption xmlns:e="{ENCRYPTION}"><e:EncryptedData><e:CipherData><e:CipherReference URI="{uri}"/></e:CipherData></e:EncryptedData></encryption>"#
            )
        };
        assert_eq!(
            extract(
                book(
                    "second",
                    "application/xhtml+xml",
                    "second%20chapter.xhtml",
                    Some(&encryption("OEBPS/second%20chapter.xhtml"))
                ),
                &|| false
            ),
            Err(invalid())
        );
        assert_eq!(
            extract(
                book(
                    "second",
                    "application/xhtml+xml",
                    "second%20chapter.xhtml",
                    Some(&encryption("OEBPS/fonts/a.ttf"))
                ),
                &|| false
            )
            .unwrap(),
            FIXTURE_TEXT
        );
    }

    #[test]
    fn malformed_containers_missing_bodies_cancellation_and_text_depth_budgets_fail() {
        assert_eq!(
            extract(package(&[("META-INF/container.xml", "<bad>")]), &|| false),
            Err(invalid())
        );
        assert_eq!(extract(fixture(), &|| true), Err(PreviewError::Cancelled));
        assert_eq!(
            chapter_text("<html><p>no body</p></html>", &mut String::new(), &|| false),
            Err(invalid())
        );
        let html = "<body>".to_string() + &"<div>".repeat(129) + &"</div>".repeat(129) + "</body>";
        assert_eq!(
            chapter_text(&html, &mut String::new(), &|| false),
            Err(PreviewError::TooLarge)
        );
        let html = format!("<body><p>{}</p></body>", "a".repeat(256 * 1024 + 1));
        assert_eq!(
            chapter_text(&html, &mut String::new(), &|| false),
            Err(PreviewError::TooLarge)
        );
    }
}
