//! Pure formatting of a directory listing for the clipboard: plain text, CSV,
//! or a Markdown table. No I/O, so the formatting is unit-tested directly. The
//! UI builds the entry slice and copies the returned string.

use crate::panel::FileEntry;
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListingFormat {
    Text,
    Csv,
    Markdown,
}

impl ListingFormat {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenamePlanFormat {
    OriginalNames,
    ProposedNames,
    MappingCsv,
}

impl RenamePlanFormat {
    pub const fn label(self) -> &'static str {
        match self {
            Self::OriginalNames => "original names",
            Self::ProposedNames => "proposed names",
            Self::MappingCsv => "rename mapping",
        }
    }
}

pub fn can_copy_rename_plan(
    plans: &[crate::rename::RenamePlan],
    format: RenamePlanFormat,
    error: Option<&str>,
) -> bool {
    !plans.is_empty()
        && (format != RenamePlanFormat::ProposedNames
            || (error.is_none()
                && plans.iter().all(|plan| {
                    matches!(
                        plan.status,
                        crate::rename::PlanStatus::Ok | crate::rename::PlanStatus::Unchanged
                    )
                })))
}

/// Export the complete current plan, preserving order and duplicate text. Plain
/// display lists escape line controls; CSV retains raw fields and validation state.
pub fn rename_plan(
    plans: &[crate::rename::RenamePlan],
    format: RenamePlanFormat,
    error: Option<&str>,
) -> Option<String> {
    if !can_copy_rename_plan(plans, format, error) {
        return None;
    }
    if format != RenamePlanFormat::MappingCsv {
        return Some(
            plans
                .iter()
                .map(|plan| {
                    escape_controls(if format == RenamePlanFormat::OriginalNames {
                        &plan.from
                    } else {
                        &plan.to
                    })
                    .into_owned()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let mut text = String::from("Old name,New name,Status,Rule error\n");
    for plan in plans {
        let status = if error.is_some() {
            "invalid_rule"
        } else {
            match plan.status {
                crate::rename::PlanStatus::Ok => "ok",
                crate::rename::PlanStatus::Unchanged => "unchanged",
                crate::rename::PlanStatus::Invalid => "invalid",
                crate::rename::PlanStatus::DateUnavailable => "date_unavailable",
                crate::rename::PlanStatus::Collision => "collision",
                crate::rename::PlanStatus::ExpansionLimit => "expansion_limit",
            }
        };
        text.push_str(&csv_escape(&plan.from));
        text.push(',');
        text.push_str(&csv_escape(&plan.to));
        text.push(',');
        text.push_str(status);
        text.push(',');
        text.push_str(&csv_escape(error.unwrap_or("")));
        text.push('\n');
    }
    Some(text)
}

/// Render `entries` (Name / Size / Modified) in the chosen format. Text is
/// tab-separated with the human size string; CSV is RFC-4180-escaped with a
/// Kind column, raw byte sizes (empty for folders) and ISO 8601 UTC modified
/// times (empty when unknown), so spreadsheets can parse them; Markdown is a
/// pipe table with `|` escaped inside cells. Text and Markdown leave folder
/// sizes empty because the listing placeholder is not a measured size, and
/// escape embedded control characters (newline, tab, carriage return) so one
/// entry can never span more than one line or cell.
pub fn format(entries: &[&FileEntry], fmt: ListingFormat) -> String {
    match fmt {
        ListingFormat::Text => entries
            .iter()
            .map(|e| {
                format!(
                    "{}\t{}\t{}",
                    escape_controls(&e.name),
                    escape_controls(human_size_cell(e)),
                    escape_controls(&e.modified_str)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ListingFormat::Csv => {
            let mut out = String::from("Name,Kind,Size (bytes),Modified\n");
            for e in entries {
                out.push_str(&csv_escape(&e.name));
                out.push_str(if e.is_dir { ",folder," } else { ",file," });
                // A folder's listed size is not its content size; leave it blank.
                if !e.is_dir {
                    out.push_str(&e.size.to_string());
                }
                out.push(',');
                if let Some(modified) = e.modified {
                    out.push_str(&iso_8601_utc(modified));
                }
                out.push('\n');
            }
            out
        }
        ListingFormat::Markdown => {
            let mut out = String::from("| Name | Size | Modified |\n| --- | --- | --- |\n");
            for e in entries {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    md_escape(&e.name),
                    md_escape(human_size_cell(e)),
                    md_escape(&e.modified_str)
                ));
            }
            out
        }
    }
}

/// Find result metadata, retaining the full (possibly virtual archive) path.
pub fn search_csv(entries: &[&FileEntry]) -> String {
    let mut out = String::from("Path,Name,Kind,Size (bytes),Modified\n");
    for entry in entries {
        out.push_str(&csv_escape(&entry.path.to_string_lossy()));
        out.push(',');
        out.push_str(&csv_escape(&entry.name));
        out.push_str(if entry.is_dir { ",folder," } else { ",file," });
        if !entry.is_dir {
            out.push_str(&entry.size.to_string());
        }
        out.push(',');
        if let Some(modified) = entry.modified {
            out.push_str(&csv_escape(&iso_8601_utc_precision(
                modified,
                chrono::SecondsFormat::AutoSi,
            )));
        }
        out.push('\n');
    }
    out
}

fn human_size_cell(entry: &FileEntry) -> &str {
    if entry.is_dir { "" } else { &entry.size_str }
}

/// `time` as an ISO 8601 / RFC 3339 UTC timestamp to the second.
pub fn iso_8601_utc(time: std::time::SystemTime) -> String {
    iso_8601_utc_precision(time, chrono::SecondsFormat::Secs)
}

pub fn iso_8601_utc_precision(
    time: std::time::SystemTime,
    precision: chrono::SecondsFormat,
) -> String {
    let converted = || {
        let nanos = match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => i128::try_from(duration.as_nanos()).ok()?,
            Err(error) => -i128::try_from(error.duration().as_nanos()).ok()?,
        };
        let seconds = i64::try_from(nanos.div_euclid(1_000_000_000)).ok()?;
        let subsec = u32::try_from(nanos.rem_euclid(1_000_000_000)).ok()?;
        chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, subsec)
            .map(|date| date.to_rfc3339_opts(precision, true))
    };
    converted().unwrap_or_default()
}

/// Quote a CSV field when it contains a comma, quote, or newline, doubling any
/// embedded quotes (RFC 4180). A field starting with `=`, `+`, `-`, `@`, tab or
/// carriage return is first prefixed with `'`, so a spreadsheet opening the
/// export shows a hostile file name such as `=HYPERLINK(...)` as text instead
/// of evaluating it as a formula.
pub fn csv_escape(s: &str) -> String {
    let s: Cow<'_, str> = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        Cow::Owned(format!("'{s}"))
    } else {
        Cow::Borrowed(s)
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.into_owned()
    }
}

/// Escape Markdown syntax and controls so a file name is always literal cell
/// text, never emphasis, a link, code, HTML, or another table cell. Borrows
/// unchanged when possible so plain listings allocate nothing here.
fn md_escape(s: &str) -> Cow<'_, str> {
    if !s.contains([
        '\\', '\n', '\r', '\t', '|', '`', '*', '_', '[', ']', '<', '>',
    ]) {
        return Cow::Borrowed(s);
    }
    let mut escaped = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\\' | '|' | '`' | '*' | '_' | '[' | ']' | '<' | '>' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    Cow::Owned(escaped)
}

/// Replace control characters that would break a line- or cell-oriented layout
/// with visible escape sequences. The backslash goes first so the mapping stays
/// unambiguous. Returns the input unchanged (and unallocated) when it holds none
/// of those characters.
pub fn escape_controls(s: &str) -> Cow<'_, str> {
    if !s.contains(['\\', '\n', '\r', '\t']) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn rename_row(
        from: &str,
        to: &str,
        status: crate::rename::PlanStatus,
    ) -> crate::rename::RenamePlan {
        crate::rename::RenamePlan {
            from: from.into(),
            to: to.into(),
            status,
        }
    }

    #[test]
    fn rename_lists_preserve_order_duplicates_and_escape_controls() {
        use crate::rename::PlanStatus::*;
        use RenamePlanFormat::*;
        let rows = [
            rename_row("я\n\t\r", "二", Ok),
            rename_row("same", "二", Unchanged),
        ];
        assert_eq!(
            rename_plan(&rows, OriginalNames, None).unwrap(),
            "я\\n\\t\\r\nsame"
        );
        assert_eq!(rename_plan(&rows, ProposedNames, None).unwrap(), "二\n二");
        for format in [OriginalNames, ProposedNames, MappingCsv] {
            assert!(rename_plan(&[], format, None).is_none());
        }
    }

    #[test]
    fn rename_proposals_require_valid_rows_but_review_exports_remain_available() {
        use crate::rename::PlanStatus::*;
        use RenamePlanFormat::*;
        for status in [Invalid, Collision, ExpansionLimit] {
            let rows = [rename_row("old", "new", status)];
            assert!(rename_plan(&rows, ProposedNames, None).is_none());
            assert!(rename_plan(&rows, OriginalNames, None).is_some());
            assert!(rename_plan(&rows, MappingCsv, None).is_some());
        }
        let rows = [rename_row("same", "same", Unchanged)];
        assert_eq!(rename_plan(&rows, ProposedNames, None).unwrap(), "same");
        assert!(rename_plan(&rows, ProposedNames, Some("bad regex")).is_none());
    }

    #[test]
    fn rename_csv_quotes_raw_names_and_validation_errors() {
        use crate::rename::PlanStatus::*;
        use RenamePlanFormat::*;
        let rows = [
            rename_row("a,\"b", "c\nd", Collision),
            rename_row("x", "y", ExpansionLimit),
        ];
        assert_eq!(
            rename_plan(&rows, MappingCsv, None).unwrap(),
            "Old name,New name,Status,Rule error\n\"a,\"\"b\",\"c\nd\",collision,\nx,y,expansion_limit,\n"
        );
        assert_eq!(
            rename_plan(&rows[..1], MappingCsv, Some("bad,\"rule\"\n")).unwrap(),
            "Old name,New name,Status,Rule error\n\"a,\"\"b\",\"c\nd\",invalid_rule,\"bad,\"\"rule\"\"\n\"\n"
        );
    }

    fn entry(name: &str, size: u64) -> FileEntry {
        FileEntry {
            symlink: None,
            permissions: None,
            allocated_size: None,
            hard_links: None,
            owner_uid: None,
            owner_gid: None,
            accessed: None,
            created: None,
            accessed_str: "Unavailable".into(),
            created_str: "Unavailable".into(),
            changed: None,
            changed_str: "Unavailable".into(),
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
            path: PathBuf::from(format!("/x/{name}")),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size,
            extension: String::new(),
            modified: None,
            modified_str: "2026-06-25".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn refs(v: &[FileEntry]) -> Vec<&FileEntry> {
        v.iter().collect()
    }

    #[test]
    fn search_details_csv_preserves_paths_bytes_and_unknown_folder_cells() {
        let mut file = entry("=SUM(1,2)\n\"x\".txt", u64::MAX);
        file.modified = Some(std::time::UNIX_EPOCH - std::time::Duration::from_millis(500));
        let mut folder = entry("folder", u64::MAX);
        folder.is_dir = true;
        let text = search_csv(&[&file, &folder]);
        assert_eq!(
            text,
            "Path,Name,Kind,Size (bytes),Modified\n\"/x/=SUM(1,2)\n\"\"x\"\".txt\",\"'=SUM(1,2)\n\"\"x\"\".txt\",file,18446744073709551615,1969-12-31T23:59:59.500Z\n/x/folder,folder,folder,,\n"
        );
        if let Some(far) =
            std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(1 << 43))
        {
            file.modified = Some(far);
            assert!(search_csv(&[&file]).ends_with(",file,18446744073709551615,\n"));
            assert!(
                format(&[&file], ListingFormat::Csv).ends_with(",file,18446744073709551615,\n")
            );
        }
    }

    #[test]
    fn text_is_tab_separated() {
        let v = vec![entry("a.txt", 100)];
        assert_eq!(
            format(&refs(&v), ListingFormat::Text),
            "a.txt\t100 B\t2026-06-25"
        );
    }

    #[test]
    fn csv_has_header_and_escapes_commas() {
        let v = vec![entry("a,b.txt", 5)];
        let out = format(&refs(&v), ListingFormat::Csv);
        assert!(out.starts_with("Name,Kind,Size (bytes),Modified\n"));
        assert!(out.contains("\"a,b.txt\",file,5,\n"));
    }

    #[test]
    fn csv_marks_folders_and_writes_machine_readable_dates() {
        use std::time::{Duration, UNIX_EPOCH};
        let mut file = entry("a.txt", 5);
        file.modified = Some(UNIX_EPOCH + Duration::from_secs(1_782_380_400));
        let mut folder = entry("Photos", 4096);
        folder.is_dir = true;
        let out = format(&refs(&[file, folder]), ListingFormat::Csv);
        assert_eq!(
            out,
            "Name,Kind,Size (bytes),Modified\n\
             a.txt,file,5,2026-06-25T09:40:00Z\n\
             Photos,folder,,\n"
        );
    }

    #[test]
    fn text_and_markdown_leave_unmeasured_folder_sizes_empty() {
        let mut folder = entry("Photos", 4096);
        folder.is_dir = true;
        folder.size_str = "…".to_string();

        assert_eq!(
            format(&[&folder], ListingFormat::Text),
            "Photos\t\t2026-06-25"
        );
        assert!(format(&[&folder], ListingFormat::Markdown).contains("| Photos |  | 2026-06-25 |"));
    }

    #[test]
    fn csv_neutralises_formula_prefixes() {
        for (name, cell) in [
            ("=1+1", "'=1+1"),
            ("+x", "'+x"),
            ("-rf", "'-rf"),
            ("@SUM(A1)", "'@SUM(A1)"),
            ("\tx", "'\tx"),
            ("\rx", "\"'\rx\""),
            ("=a,b", "\"'=a,b\""),
            ("a=b", "a=b"),
        ] {
            let v = vec![entry(name, 5)];
            let out = format(&refs(&v), ListingFormat::Csv);
            assert!(
                out.contains(&format!("\n{cell},file,5,")),
                "{name:?} -> {out:?}"
            );
        }
    }

    #[test]
    fn markdown_is_a_table_and_escapes_pipes() {
        let v = vec![entry("a|b.txt", 10)];
        let out = format(&refs(&v), ListingFormat::Markdown);
        assert!(out.contains("| Name | Size | Modified |"));
        assert!(out.contains("| a\\|b.txt |"));
    }

    #[test]
    fn markdown_names_cannot_become_formatting_links_or_html() {
        let v = vec![
            entry("*draft*.md", 1),
            entry("[x](y).txt", 1),
            entry("<tag>_`code`_.txt", 1),
        ];
        let out = format(&refs(&v), ListingFormat::Markdown);

        assert!(out.contains("| \\*draft\\*.md |"), "{out}");
        assert!(out.contains("| \\[x\\](y).txt |"), "{out}");
        assert!(out.contains("| \\<tag\\>\\_\\`code\\`\\_.txt |"), "{out}");
    }

    #[test]
    fn text_escapes_embedded_newline_and_tab() {
        let v = vec![entry("a\nb\tc.txt", 1)];
        let out = format(&refs(&v), ListingFormat::Text);
        assert_eq!(out.lines().count(), 1);
        assert!(out.starts_with("a\\nb\\tc.txt\t"));
    }

    #[test]
    fn markdown_escapes_embedded_newline() {
        let v = vec![entry("a\nb.txt", 1)];
        let out = format(&refs(&v), ListingFormat::Markdown);
        assert_eq!(out.lines().count(), 3);
        assert!(out.contains("| a\\nb.txt |"));
    }

    #[test]
    fn csv_doubles_quotes_and_quotes_embedded_newlines() {
        let v = vec![entry("say \"hi\".txt", 5), entry("a\nb.txt", 6)];
        let out = format(&refs(&v), ListingFormat::Csv);
        assert!(
            out.contains("\n\"say \"\"hi\"\".txt\",file,5,\n"),
            "{out:?}"
        );
        assert!(out.contains("\n\"a\nb.txt\",file,6,\n"), "{out:?}");
    }

    #[test]
    fn literal_backslash_n_stays_distinct_from_a_real_newline() {
        let literal = entry("a\\nb", 1);
        let newline = entry("a\nb", 1);
        let text = |e: &FileEntry| format(&[e], ListingFormat::Text);
        assert!(text(&literal).starts_with("a\\\\nb\t"));
        assert!(text(&newline).starts_with("a\\nb\t"));
        assert_ne!(text(&literal), text(&newline));
        let markdown = format(&[&literal], ListingFormat::Markdown);
        assert!(markdown.contains("| a\\\\nb |"), "{markdown:?}");
    }

    #[test]
    fn empty_text_listing_is_empty() {
        assert_eq!(format(&[], ListingFormat::Text), "");
    }
}
