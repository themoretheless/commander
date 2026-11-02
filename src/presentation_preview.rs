//! Ordered slide text. No presentation execution, rendering or resource loading.
use crate::image_cache::PreviewError;
use crate::office_preview::{append, open_package, parse_xml, read_member, resolve_part_target};
use roxmltree::Node;
use std::path::Path;

const PRESENTATION: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const PRESENTATION_STRICT: &str = "http://purl.oclc.org/ooxml/presentationml/main";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const DRAWING_STRICT: &str = "http://purl.oclc.org/ooxml/drawingml/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
const TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const ODP_PRESENTATION: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";

fn invalid() -> PreviewError {
    PreviewError::InvalidDocument
}
fn ppt(node: Node<'_, '_>, name: &str) -> bool {
    matches!(
        node.tag_name().namespace(),
        Some(PRESENTATION | PRESENTATION_STRICT)
    ) && node.tag_name().name() == name
}
fn drawing(node: Node<'_, '_>, name: &str) -> bool {
    matches!(node.tag_name().namespace(), Some(DRAWING | DRAWING_STRICT))
        && node.tag_name().name() == name
}
fn heading(out: &mut String, index: usize, name: Option<&str>) -> Result<(), PreviewError> {
    append(
        out,
        &format!(
            "[Slide {}{}]\n",
            index + 1,
            name.filter(|s| !s.is_empty())
                .map(|s| format!(": {s}"))
                .unwrap_or_default()
        ),
    )
}

pub fn extract(
    path: &Path,
    bytes: Vec<u8>,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, PreviewError> {
    let mut zip = open_package(bytes)?;
    let mut remaining = 4 * 1024 * 1024;
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("odp"))
    {
        let xml =
            read_member(&mut zip, "content.xml", &mut remaining, cancelled)?.ok_or_else(invalid)?;
        let doc = parse_xml(&xml)?;
        let body = doc
            .descendants()
            .find(|n| n.has_tag_name((OFFICE, "presentation")))
            .ok_or_else(invalid)?;
        let pages = body
            .children()
            .filter(|n| n.has_tag_name((DRAW, "page")))
            .collect::<Vec<_>>();
        if pages.len() > 512 {
            return Err(PreviewError::TooLarge);
        }
        let mut out = String::new();
        for (index, page) in pages.into_iter().enumerate() {
            heading(&mut out, index, page.attribute((DRAW, "name")))?;
            slide_text(page, true, &mut out, cancelled)?;
        }
        return Ok(out);
    }
    let presentation = read_member(&mut zip, "ppt/presentation.xml", &mut remaining, cancelled)?
        .ok_or_else(invalid)?;
    let rels = read_member(
        &mut zip,
        "ppt/_rels/presentation.xml.rels",
        &mut remaining,
        cancelled,
    )?
    .ok_or_else(invalid)?;
    let presentation = parse_xml(&presentation)?;
    let rels = parse_xml(&rels)?;
    if !ppt(presentation.root_element(), "presentation") {
        return Err(invalid());
    }
    let mut mappings = std::collections::HashMap::new();
    for rel in rels
        .descendants()
        .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
    {
        if mappings
            .insert(rel.attribute("Id").ok_or_else(invalid)?, rel)
            .is_some()
        {
            return Err(invalid());
        }
    }
    let list = presentation
        .root_element()
        .children()
        .find(|n| ppt(*n, "sldIdLst"));
    let slides = list
        .into_iter()
        .flat_map(|n| n.children())
        .filter(|n| ppt(*n, "sldId"))
        .collect::<Vec<_>>();
    if slides.len() > 512 {
        return Err(PreviewError::TooLarge);
    }
    let mut out = String::new();
    for (index, slide) in slides.into_iter().enumerate() {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let id = slide
            .attribute((REL, "id"))
            .or_else(|| slide.attribute((REL_STRICT, "id")))
            .ok_or_else(invalid)?;
        let rel = *mappings.get(id).ok_or_else(invalid)?;
        if rel.attribute("TargetMode").is_some_and(|s| s != "Internal")
            || !rel.attribute("Type").is_some_and(|s| s.ends_with("/slide"))
        {
            return Err(invalid());
        }
        let part = resolve_part_target("ppt", rel.attribute("Target").ok_or_else(invalid)?)?;
        let xml = read_member(&mut zip, &part, &mut remaining, cancelled)?.ok_or_else(invalid)?;
        let doc = parse_xml(&xml)?;
        if !ppt(doc.root_element(), "sld") {
            return Err(invalid());
        }
        let content = doc
            .root_element()
            .children()
            .find(|n| ppt(*n, "cSld"))
            .ok_or_else(invalid)?;
        heading(&mut out, index, content.attribute("name"))?;
        slide_text(content, false, &mut out, cancelled)?;
    }
    Ok(out)
}

fn slide_text(
    node: Node<'_, '_>,
    odp: bool,
    out: &mut String,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PreviewError> {
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    if odp
        && (node.has_tag_name((ODP_PRESENTATION, "notes"))
            || node.has_tag_name((OFFICE, "annotation"))
            || node.has_tag_name((OFFICE, "binary-data"))
            || node.has_tag_name((OFFICE, "scripts")))
    {
        return Ok(());
    }
    if odp && (node.has_tag_name((TEXT, "p")) || node.has_tag_name((TEXT, "h"))) {
        return crate::office_preview::walk(node, false, false, 0, out, cancelled);
    }
    if !odp && drawing(node, "t") {
        return append(out, node.text().unwrap_or_default());
    }
    if !odp && drawing(node, "br") {
        append(out, "\n")?;
    }
    for child in node.children().filter(Node::is_element) {
        slide_text(child, odp, out, cancelled)?;
    }
    if !odp && drawing(node, "p") {
        append(out, "\n")?;
    }
    let cell = if odp {
        node.has_tag_name((TABLE, "table-cell"))
    } else {
        drawing(node, "tc")
    };
    let row = if odp {
        node.has_tag_name((TABLE, "table-row"))
    } else {
        drawing(node, "tr")
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
pub const FIXTURE_TEXT: &str =
    "[Slide 1: Overview]\nmessage: café / ёж 🌍\n[Slide 2: Details]\nSecond slide\nA\tB\n";

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture(pptx: bool) -> Vec<u8> {
    if !pptx {
        return package(&[(
            "content.xml",
            &format!(
                r#"<o:document-content xmlns:o="{OFFICE}" xmlns:d="{DRAW}" xmlns:t="{TEXT}" xmlns:b="{TABLE}" xmlns:p="{ODP_PRESENTATION}"><o:body><o:presentation><d:page d:name="Overview"><p:notes><t:p>speaker secret</t:p></p:notes><d:frame><d:text-box><t:p>message: café / ёж 🌍</t:p></d:text-box></d:frame></d:page><d:page d:name="Details"><d:frame><d:text-box><t:p>Second slide</t:p></d:text-box></d:frame><b:table><b:table-row><b:table-cell><t:p>A</t:p></b:table-cell><b:table-cell><t:p>B</t:p></b:table-cell></b:table-row></b:table></d:page></o:presentation></o:body></o:document-content>"#
            ),
        )]);
    }
    let presentation = format!(
        r#"<p:presentation xmlns:p="{PRESENTATION}" xmlns:r="{REL}"><p:sldIdLst><p:sldId id="900" r:id="second-file"/><p:sldId id="100" r:id="first-file"/></p:sldIdLst></p:presentation>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="first-file" Type="{REL}/slide" Target="slides/slide1.xml"/><Relationship Id="second-file" Type="{REL}/slide" Target="slides/slide2.xml"/></Relationships>"#
    );
    let first = format!(
        r#"<p:sld xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld name="Details"><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Second slide</a:t></a:r></a:p></p:txBody></p:sp><p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>A</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>B</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#
    );
    let second = format!(
        r#"<p:sld xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld name="Overview"><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>message: café / ёж 🌍</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
    );
    package(&[
        ("ppt/presentation.xml", &presentation),
        ("ppt/_rels/presentation.xml.rels", &rels),
        ("ppt/slides/slide1.xml", &first),
        ("ppt/slides/slide2.xml", &second),
    ])
}

#[cfg(any(test, feature = "visual-qa"))]
fn package(parts: &[(&str, &str)]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, xml) in parts {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_slide(slide: &str, relationships: &str, strict: bool) -> Vec<u8> {
        let ns = if strict {
            PRESENTATION_STRICT
        } else {
            PRESENTATION
        };
        let rel = if strict { REL_STRICT } else { REL };
        package(&[
            (
                "ppt/presentation.xml",
                &format!(
                    r#"<s:presentation xmlns:s="{ns}" xmlns:r="{rel}"><s:sldIdLst><s:sldId r:id="s"/></s:sldIdLst></s:presentation>"#
                ),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                &format!(r#"<Relationships xmlns="{PACKAGE_REL}">{relationships}</Relationships>"#),
            ),
            ("ppt/slides/one.xml", slide),
        ])
    }
    fn internal() -> String {
        format!(r#"<Relationship Id="s" Type="{REL}/slide" Target="./slides/one.xml"/>"#)
    }

    #[test]
    fn both_formats_preserve_slide_order_unicode_and_tables_but_omit_notes() {
        for (pptx, path) in [(true, "deck.PPTX"), (false, "deck.ODP")] {
            assert_eq!(
                crate::office_preview::extract(Path::new(path), fixture(pptx), &|| false).unwrap(),
                FIXTURE_TEXT
            );
        }
    }

    #[test]
    fn strict_namespaces_runs_breaks_entities_and_unnamed_slides_are_supported() {
        let xml = format!(
            r#"<z:sld xmlns:z="{PRESENTATION_STRICT}" xmlns:g="{DRAWING_STRICT}"><z:cSld><z:spTree><z:sp><z:txBody><g:p><g:r><g:t>A &amp; </g:t></g:r><g:r><g:t>я</g:t></g:r><g:br/><g:r><g:t>🌍</g:t></g:r></g:p></z:txBody></z:sp></z:spTree></z:cSld></z:sld>"#
        );
        assert_eq!(
            extract(
                Path::new("deck.pptx"),
                single_slide(&xml, &internal(), true),
                &|| false
            )
            .unwrap(),
            "[Slide 1]\nA & я\n🌍\n"
        );
    }

    #[test]
    fn invalid_missing_duplicate_and_external_slide_references_are_rejected() {
        let xml = format!(r#"<p:sld xmlns:p="{PRESENTATION}"><p:cSld/></p:sld>"#);
        for relationship in [
            format!(
                r#"<Relationship Id="s" Type="{REL}/slide" Target="https://example.com/one.xml" TargetMode="External"/>"#
            ),
            format!(r#"<Relationship Id="s" Type="{REL}/slide" Target="../../escape.xml"/>"#),
            format!(r#"<Relationship Id="s" Type="{REL}/slide" Target="missing.xml"/>"#),
            format!(r#"<Relationship Id="s" Type="{REL}/slideMaster" Target="slides/one.xml"/>"#),
            internal().repeat(2),
        ] {
            assert_eq!(
                extract(
                    Path::new("deck.pptx"),
                    single_slide(&xml, &relationship, false),
                    &|| false
                ),
                Err(invalid())
            );
        }
        assert_eq!(
            extract(
                Path::new("deck.pptx"),
                single_slide("<wrong/>", &internal(), false),
                &|| false
            ),
            Err(invalid())
        );
    }

    #[test]
    fn odp_lists_explicit_spaces_and_breaks_skip_notes_and_comments() {
        let xml = format!(
            r#"<o:document-content xmlns:o="{OFFICE}" xmlns:d="{DRAW}" xmlns:t="{TEXT}" xmlns:p="{ODP_PRESENTATION}"><o:presentation><d:page><t:list><t:list-item><t:p>A<t:s t:c="2"/>B<t:tab/><t:line-break/>C</t:p></t:list-item></t:list><o:annotation><t:p>comment</t:p></o:annotation><p:notes><t:p>notes</t:p></p:notes></d:page></o:presentation></o:document-content>"#
        );
        assert_eq!(
            extract(
                Path::new("deck.odp"),
                package(&[("content.xml", &xml)]),
                &|| false
            )
            .unwrap(),
            "[Slide 1]\nA  B\t\nC\n"
        );
    }

    #[test]
    fn presentation_budgets_cancellation_and_malformed_xml_are_checked() {
        assert_eq!(
            extract(Path::new("deck.pptx"), fixture(true), &|| true),
            Err(PreviewError::Cancelled)
        );
        let xml = format!(
            r#"<o:presentation xmlns:o="{OFFICE}" xmlns:d="{DRAW}">{}</o:presentation>"#,
            "<d:page/>".repeat(513)
        );
        assert_eq!(
            extract(
                Path::new("deck.odp"),
                package(&[("content.xml", &xml)]),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
        let xml = format!(
            r#"<o:presentation xmlns:o="{OFFICE}" xmlns:d="{DRAW}" xmlns:t="{TEXT}"><d:page><t:p>{}</t:p></d:page></o:presentation>"#,
            "X".repeat(256 * 1024)
        );
        assert_eq!(
            extract(
                Path::new("deck.odp"),
                package(&[("content.xml", &xml)]),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
        assert_eq!(
            extract(
                Path::new("deck.odp"),
                package(&[("content.xml", "<bad>")]),
                &|| false
            ),
            Err(invalid())
        );
    }
}
