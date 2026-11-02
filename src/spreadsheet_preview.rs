//! Text-only workbook values. Never evaluates formulas or opens external relationships.
use crate::image_cache::PreviewError;
use crate::office_preview::parse_xml as parse;
use crate::office_preview::{append, open_package, read_member};
use roxmltree::{Document, Node};
use std::path::Path;

const XML_LIMIT: usize = 4 * 1024 * 1024;
const TEXT_LIMIT: usize = 256 * 1024;
const SHEET: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const SHEET_STRICT: &str = "http://purl.oclc.org/ooxml/spreadsheetml/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL_STRICT: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
const TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
const TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";

fn invalid() -> PreviewError {
    PreviewError::InvalidDocument
}
fn sheet_tag(node: Node<'_, '_>, name: &str) -> bool {
    matches!(node.tag_name().namespace(), Some(SHEET | SHEET_STRICT))
        && node.tag_name().name() == name
}
fn tag(node: Node<'_, '_>, ns: &str, name: &str) -> bool {
    node.has_tag_name((ns, name))
}
fn repeated(out: &mut String, character: char, count: usize) -> Result<(), PreviewError> {
    if count > TEXT_LIMIT.saturating_sub(out.len()) {
        return Err(PreviewError::TooLarge);
    }
    out.extend(std::iter::repeat_n(character, count));
    Ok(())
}
fn rich_text(node: Node<'_, '_>) -> Result<String, PreviewError> {
    let mut text = String::new();
    for run in node
        .descendants()
        .filter(|n| sheet_tag(*n, "t") && !n.ancestors().any(|a| sheet_tag(a, "rPh")))
    {
        append(&mut text, run.text().unwrap_or_default())?;
    }
    Ok(text)
}
fn target(value: &str) -> Result<String, PreviewError> {
    crate::office_preview::resolve_part_target("xl", value)
}

pub fn extract(
    path: &Path,
    bytes: Vec<u8>,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, PreviewError> {
    let mut zip = open_package(bytes)?;
    let mut remaining = XML_LIMIT;
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("ods"))
    {
        let xml =
            read_member(&mut zip, "content.xml", &mut remaining, cancelled)?.ok_or_else(invalid)?;
        return ods(&parse(&xml)?, cancelled);
    }
    let book =
        read_member(&mut zip, "xl/workbook.xml", &mut remaining, cancelled)?.ok_or_else(invalid)?;
    let relationships = read_member(
        &mut zip,
        "xl/_rels/workbook.xml.rels",
        &mut remaining,
        cancelled,
    )?
    .ok_or_else(invalid)?;
    let book = parse(&book)?;
    let rels = parse(&relationships)?;
    let mut mappings = std::collections::HashMap::new();
    for rel in rels
        .descendants()
        .filter(|n| tag(*n, PACKAGE_REL, "Relationship"))
    {
        let id = rel.attribute("Id").ok_or_else(invalid)?;
        if mappings.insert(id, rel).is_some() {
            return Err(invalid());
        }
    }
    let resolve = |rel: Node<'_, '_>| -> Result<String, PreviewError> {
        if rel.attribute("TargetMode").is_some_and(|s| s != "Internal") {
            return Err(invalid());
        }
        target(rel.attribute("Target").ok_or_else(invalid)?)
    };
    let shared_rel = mappings
        .values()
        .filter(|r| {
            r.attribute("Type")
                .is_some_and(|s| s.ends_with("/sharedStrings"))
        })
        .copied()
        .collect::<Vec<_>>();
    if shared_rel.len() > 1 {
        return Err(invalid());
    }
    let mut strings = Vec::new();
    if let Some(rel) = shared_rel.first() {
        let xml = read_member(&mut zip, &resolve(*rel)?, &mut remaining, cancelled)?
            .ok_or_else(invalid)?;
        let doc = parse(&xml)?;
        for item in doc.descendants().filter(|n| sheet_tag(*n, "si")) {
            strings.push(rich_text(item)?);
        }
    }
    let sheets = book
        .descendants()
        .filter(|n| sheet_tag(*n, "sheet"))
        .collect::<Vec<_>>();
    if sheets.is_empty() {
        return Err(invalid());
    }
    if sheets.len() > 128 {
        return Err(PreviewError::TooLarge);
    }
    let mut out = String::new();
    for sheet in sheets {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        append(
            &mut out,
            &format!("[{}]\n", sheet.attribute("name").ok_or_else(invalid)?),
        )?;
        let id = sheet
            .attribute((REL, "id"))
            .or_else(|| sheet.attribute((REL_STRICT, "id")))
            .ok_or_else(invalid)?;
        let rel = *mappings.get(id).ok_or_else(invalid)?;
        if !rel
            .attribute("Type")
            .is_some_and(|s| s.ends_with("/worksheet"))
        {
            return Err(invalid());
        }
        let xml = read_member(&mut zip, &resolve(rel)?, &mut remaining, cancelled)?
            .ok_or_else(invalid)?;
        xlsx_sheet(&parse(&xml)?, &strings, &mut out, cancelled)?;
    }
    Ok(out)
}

fn number(value: Option<&str>, default: usize, maximum: usize) -> Result<usize, PreviewError> {
    let number = match value {
        Some(value) => value.parse().map_err(|_| invalid())?,
        None => default,
    };
    if number == 0 || number > maximum {
        return Err(invalid());
    }
    Ok(number)
}
fn column(reference: &str, row: usize) -> Result<usize, PreviewError> {
    let letters = reference.bytes().take_while(u8::is_ascii_uppercase).count();
    if letters == 0 || letters == reference.len() {
        return Err(invalid());
    }
    let mut column = 0_usize;
    for byte in reference[..letters].bytes() {
        column = column
            .checked_mul(26)
            .and_then(|n| n.checked_add(usize::from(byte - b'A') + 1))
            .ok_or_else(invalid)?;
    }
    if column > 16384 || number(Some(&reference[letters..]), 1, 1048576)? != row {
        return Err(invalid());
    }
    Ok(column)
}
fn xlsx_sheet(
    doc: &Document<'_>,
    strings: &[String],
    out: &mut String,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PreviewError> {
    let data = doc
        .descendants()
        .find(|n| sheet_tag(*n, "sheetData"))
        .ok_or_else(invalid)?;
    let mut next_row = 1;
    for row in data.children().filter(|n| sheet_tag(*n, "row")) {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let index = number(row.attribute("r"), next_row, 1048576)?;
        if index < next_row {
            return Err(invalid());
        }
        repeated(out, '\n', index - next_row)?;
        let mut next_column = 1;
        for cell in row.children().filter(|n| sheet_tag(*n, "c")) {
            if cancelled() {
                return Err(PreviewError::Cancelled);
            }
            let index = cell
                .attribute("r")
                .map(|r| column(r, index))
                .transpose()?
                .unwrap_or(next_column);
            if index < next_column || index > 16384 {
                return Err(invalid());
            }
            repeated(
                out,
                '\t',
                index - next_column + usize::from(next_column > 1),
            )?;
            let value = cell
                .children()
                .find(|n| sheet_tag(*n, "v"))
                .and_then(|n| n.text());
            let text = match cell.attribute("t").unwrap_or("n") {
                "s" => strings
                    .get(
                        value
                            .ok_or_else(invalid)?
                            .parse::<usize>()
                            .map_err(|_| invalid())?,
                    )
                    .ok_or_else(invalid)?
                    .clone(),
                "inlineStr" => rich_text(cell)?,
                "b" => match value {
                    Some("1") => "TRUE",
                    Some("0") => "FALSE",
                    None => "",
                    _ => return Err(invalid()),
                }
                .to_string(),
                "n" | "str" | "e" | "d" => value.map(str::to_string).unwrap_or_else(|| {
                    cell.children()
                        .find(|n| sheet_tag(*n, "f"))
                        .and_then(|n| n.text())
                        .map(|f| format!("={f}"))
                        .unwrap_or_default()
                }),
                _ => return Err(invalid()),
            };
            append(out, &text)?;
            next_column = index + 1;
        }
        append(out, "\n")?;
        next_row = index + 1;
    }
    Ok(())
}

fn ods(doc: &Document<'_>, cancelled: &dyn Fn() -> bool) -> Result<String, PreviewError> {
    let body = doc
        .descendants()
        .find(|n| tag(*n, OFFICE, "spreadsheet"))
        .ok_or_else(invalid)?;
    let tables = body
        .children()
        .filter(|n| tag(*n, TABLE, "table"))
        .collect::<Vec<_>>();
    if tables.is_empty() {
        return Err(invalid());
    }
    if tables.len() > 128 {
        return Err(PreviewError::TooLarge);
    }
    let mut out = String::new();
    for table in tables {
        let mut pending_empty_rows = 0_usize;
        let mut row_count = 0_usize;
        append(
            &mut out,
            &format!(
                "[{}]\n",
                table.attribute((TABLE, "name")).ok_or_else(invalid)?
            ),
        )?;
        for row in table.descendants().filter(|n| {
            tag(*n, TABLE, "table-row")
                && n.ancestors().find(|a| tag(*a, TABLE, "table")) == Some(table)
        }) {
            if cancelled() {
                return Err(PreviewError::Cancelled);
            }
            let rows = number(row.attribute((TABLE, "number-rows-repeated")), 1, 1048576)?;
            row_count = row_count.checked_add(rows).ok_or(PreviewError::TooLarge)?;
            if row_count > 1048576 {
                return Err(PreviewError::TooLarge);
            }
            let mut text = String::new();
            let mut columns = 0;
            let mut written_columns = 0;
            for cell in row
                .children()
                .filter(|n| tag(*n, TABLE, "table-cell") || tag(*n, TABLE, "covered-table-cell"))
            {
                let count = number(cell.attribute((TABLE, "number-columns-repeated")), 1, 16384)?;
                if columns + count > 16384 {
                    return Err(PreviewError::TooLarge);
                }
                let mut value = String::new();
                for p in cell.children().filter(|n| tag(*n, TEXT, "p")) {
                    crate::office_preview::walk(p, false, false, 0, &mut value, cancelled)?;
                }
                if value.ends_with('\n') {
                    value.pop();
                }
                if value.is_empty() {
                    value = [
                        "string-value",
                        "value",
                        "boolean-value",
                        "date-value",
                        "time-value",
                    ]
                    .into_iter()
                    .find_map(|name| cell.attribute((OFFICE, name)))
                    .or_else(|| cell.attribute((TABLE, "formula")))
                    .unwrap_or_default()
                    .to_string();
                }
                if value.is_empty() {
                    columns += count;
                } else {
                    for _ in 0..count {
                        repeated(
                            &mut text,
                            '\t',
                            columns - written_columns + usize::from(written_columns > 0),
                        )?;
                        append(&mut text, &value)?;
                        columns += 1;
                        written_columns = columns;
                    }
                }
            }
            if text.is_empty() {
                pending_empty_rows += rows;
                continue;
            }
            repeated(&mut out, '\n', pending_empty_rows)?;
            pending_empty_rows = 0;
            append(&mut text, "\n")?;
            if text.len().saturating_mul(rows) > TEXT_LIMIT.saturating_sub(out.len()) {
                return Err(PreviewError::TooLarge);
            }
            for _ in 0..rows {
                if cancelled() {
                    return Err(PreviewError::Cancelled);
                }
                append(&mut out, &text)?;
            }
        }
    }
    Ok(out)
}

#[cfg(any(test, feature = "visual-qa"))]
pub const FIXTURE_TEXT: &str = "[Data]\nmessage: café / ёж 🌍\t42\tTRUE\n[Notes]\nSecond sheet\n";

#[cfg(any(test, feature = "visual-qa"))]
pub fn fixture(xlsx: bool) -> Vec<u8> {
    if !xlsx {
        return package(&[(
            "content.xml",
            &format!(
                r#"<o:document-content xmlns:o="{OFFICE}" xmlns:t="{TEXT}" xmlns:b="{TABLE}"><o:body><o:spreadsheet><b:table b:name="Data"><b:table-row><b:table-cell><t:p>message: café / ёж 🌍</t:p></b:table-cell><b:table-cell o:value="42"/><b:table-cell o:boolean-value="true"><t:p>TRUE</t:p></b:table-cell></b:table-row></b:table><b:table b:name="Notes"><b:table-row><b:table-cell><t:p>Second sheet</t:p></b:table-cell></b:table-row></b:table></o:spreadsheet></o:body></o:document-content>"#
            ),
        )]);
    }
    let book = format!(
        r#"<workbook xmlns="{SHEET}" xmlns:r="{REL}"><sheets><sheet name="Data" r:id="r1"/><sheet name="Notes" r:id="r2"/></sheets></workbook>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/worksheet" Target="worksheets/a.xml"/><Relationship Id="r2" Type="{REL}/worksheet" Target="worksheets/b.xml"/><Relationship Id="ss" Type="{REL}/sharedStrings" Target="strings.xml"/></Relationships>"#
    );
    package(&[
        ("xl/workbook.xml", &book),
        ("xl/_rels/workbook.xml.rels", &rels),
        (
            "xl/strings.xml",
            &format!(
                r#"<sst xmlns="{SHEET}"><si><r><t>message: </t></r><r><t>café / ёж 🌍</t></r></si></sst>"#
            ),
        ),
        (
            "xl/worksheets/a.xml",
            &format!(
                r#"<worksheet xmlns="{SHEET}"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><f>40+2</f><v>42</v></c><c r="C1" t="b"><v>1</v></c></row></sheetData></worksheet>"#
            ),
        ),
        (
            "xl/worksheets/b.xml",
            &format!(
                r#"<worksheet xmlns="{SHEET}"><sheetData><row><c t="inlineStr"><is><t>Second sheet</t></is></c></row></sheetData></worksheet>"#
            ),
        ),
    ])
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

    fn single_sheet(sheet: &str, relationship: &str) -> Vec<u8> {
        package(&[
            (
                "xl/workbook.xml",
                &format!(
                    r#"<workbook xmlns="{SHEET_STRICT}" xmlns:r="{REL_STRICT}"><sheets><sheet name="Values" r:id="s"/></sheets></workbook>"#
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                &format!(r#"<Relationships xmlns="{PACKAGE_REL}">{relationship}</Relationships>"#),
            ),
            ("xl/worksheets/value.xml", sheet),
        ])
    }
    fn internal() -> String {
        format!(r#"<Relationship Id="s" Type="{REL}/worksheet" Target="./worksheets/value.xml"/>"#)
    }

    #[test]
    fn workbook_values_sheet_order_rich_strings_and_cached_formulas_agree() {
        for (xlsx, path) in [(true, "book.XLSX"), (false, "book.ODS")] {
            assert_eq!(
                crate::office_preview::extract(Path::new(path), fixture(xlsx), &|| false).unwrap(),
                FIXTURE_TEXT
            );
        }
    }

    #[test]
    fn sparse_coordinates_preserve_gaps_precision_errors_dates_and_literal_formulas() {
        let xml = format!(
            r#"<worksheet xmlns="{SHEET_STRICT}"><sheetData><row r="2"><c r="B2"><v>9007199254740993123456789</v></c><c r="D2"><f>SUM(A1:A9)</f></c><c r="E2" t="e"><v>#DIV/0!</v></c><c r="F2" t="d"><v>2026-10-30</v></c></row></sheetData></worksheet>"#
        );
        assert_eq!(
            extract(
                Path::new("x.xlsx"),
                single_sheet(&xml, &internal()),
                &|| false
            )
            .unwrap(),
            "[Values]\n\n\t9007199254740993123456789\t\t=SUM(A1:A9)\t#DIV/0!\t2026-10-30\n"
        );
    }

    #[test]
    fn relationship_resolution_rejects_external_escape_missing_and_duplicate_targets() {
        let xml = format!(r#"<worksheet xmlns="{SHEET}"><sheetData/></worksheet>"#);
        for rel in [
            format!(
                r#"<Relationship Id="s" Type="{REL}/worksheet" Target="https://example.com/sheet.xml" TargetMode="External"/>"#
            ),
            format!(r#"<Relationship Id="s" Type="{REL}/worksheet" Target="../../outside.xml"/>"#),
            format!(r#"<Relationship Id="s" Type="{REL}/worksheet" Target="missing.xml"/>"#),
            internal().repeat(2),
        ] {
            assert_eq!(
                extract(Path::new("x.xlsx"), single_sheet(&xml, &rel), &|| false),
                Err(invalid())
            );
        }
    }

    #[test]
    fn invalid_cell_references_and_shared_indices_do_not_publish_partial_values() {
        for cell in [
            r#"<c r="XFE1"><v>1</v></c>"#,
            r#"<c r="A2"><v>1</v></c>"#,
            r#"<c t="s"><v>99</v></c>"#,
            r#"<c t="b"><v>2</v></c>"#,
            r#"<c r="B1"/><c r="A1"/>"#,
        ] {
            let xml = format!(
                r#"<worksheet xmlns="{SHEET}"><sheetData><row r="1">{cell}</row></sheetData></worksheet>"#
            );
            assert_eq!(
                extract(
                    Path::new("x.xlsx"),
                    single_sheet(&xml, &internal()),
                    &|| false
                ),
                Err(invalid())
            );
        }
    }

    #[test]
    fn ods_repeats_covered_cells_and_formula_fallback_are_bounded() {
        let xml = |count: &str| {
            format!(
                r#"<o:document-content xmlns:o="{OFFICE}" xmlns:t="{TEXT}" xmlns:b="{TABLE}"><o:spreadsheet><b:table b:name="Repeat"><b:table-row b:number-rows-repeated="{count}"><b:table-cell b:number-columns-repeated="2"><t:p>A</t:p></b:table-cell><b:covered-table-cell/><b:table-cell b:formula="of:=1+2"/></b:table-row></b:table></o:spreadsheet></o:document-content>"#
            )
        };
        assert_eq!(
            extract(
                Path::new("x.ods"),
                package(&[("content.xml", &xml("2"))]),
                &|| false
            )
            .unwrap(),
            "[Repeat]\nA\tA\t\tof:=1+2\nA\tA\t\tof:=1+2\n"
        );
        assert_eq!(
            extract(
                Path::new("x.ods"),
                package(&[("content.xml", &xml("262144"))]),
                &|| false
            ),
            Err(PreviewError::TooLarge)
        );
        assert_eq!(
            extract(Path::new("x.xlsx"), fixture(true), &|| true),
            Err(PreviewError::Cancelled)
        );
    }

    #[test]
    fn ods_empty_sheet_tail_is_not_expanded_but_internal_gaps_are_preserved() {
        let xml = format!(
            r#"<o:document-content xmlns:o="{OFFICE}" xmlns:t="{TEXT}" xmlns:b="{TABLE}"><o:spreadsheet><b:table b:name="Sparse"><b:table-row b:number-rows-repeated="2"><b:table-cell b:number-columns-repeated="16384"/></b:table-row><b:table-row><b:table-cell b:number-columns-repeated="2"/><b:table-cell><t:p>X</t:p></b:table-cell><b:table-cell b:number-columns-repeated="16381"/></b:table-row><b:table-row b:number-rows-repeated="1048573"><b:table-cell b:number-columns-repeated="16384"/></b:table-row></b:table></o:spreadsheet></o:document-content>"#
        );
        assert_eq!(
            extract(
                Path::new("x.ods"),
                package(&[("content.xml", &xml)]),
                &|| false
            )
            .unwrap(),
            "[Sparse]\n\n\n\t\tX\n"
        );
    }

    #[test]
    fn spreadsheet_xml_rejects_dtd_and_excessive_depth_before_value_traversal() {
        let deep = "<a>".repeat(129) + &"</a>".repeat(129);
        assert_eq!(parse(&deep).unwrap_err(), PreviewError::TooLarge);
        assert!(parse("<!DOCTYPE a [<!ENTITY x 'expand'>]><a>&x;</a>").is_err());
    }

    #[test]
    fn xml_budget_is_aggregate_across_package_parts() {
        let mut zip = open_package(package(&[("a.xml", "abc"), ("b.xml", "def")])).unwrap();
        let mut remaining = 5;
        assert_eq!(
            read_member(&mut zip, "a.xml", &mut remaining, &|| false)
                .unwrap()
                .as_deref(),
            Some("abc")
        );
        assert_eq!(
            read_member(&mut zip, "b.xml", &mut remaining, &|| false),
            Err(PreviewError::TooLarge)
        );
    }
}
