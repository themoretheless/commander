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

/// Render `entries` (Name / Size / Modified) in the chosen format. Text is
/// tab-separated with the human size string; CSV is RFC-4180-escaped with a
/// Kind column, raw byte sizes (empty for folders) and ISO 8601 UTC modified
/// times (empty when unknown), so spreadsheets can parse them; Markdown is a pipe table with `|` escaped inside cells. Text and
/// Markdown also escape embedded control characters (newline, tab, carriage
/// return) so one entry can never span more than one line or cell.
pub fn format(entries: &[&FileEntry], fmt: ListingFormat) -> String {
    match fmt {
        ListingFormat::Text => entries
            .iter()
            .map(|e| {
                format!(
                    "{}\t{}\t{}",
                    escape_controls(&e.name),
                    escape_controls(&e.size_str),
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
                    md_escape(&e.size_str),
                    md_escape(&e.modified_str)
                ));
            }
            out
        }
    }
}

/// `time` as an ISO 8601 / RFC 3339 UTC timestamp to the second.
fn iso_8601_utc(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Quote a CSV field when it contains a comma, quote, or newline, doubling any
/// embedded quotes (RFC 4180). A field starting with `=`, `+`, `-`, `@`, tab or
/// carriage return is first prefixed with `'`, so a spreadsheet opening the
/// export shows a hostile file name such as `=HYPERLINK(...)` as text instead
/// of evaluating it as a formula.
fn csv_escape(s: &str) -> String {
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

/// Escape `|` so a cell can't break the Markdown table. Borrows unchanged when
/// there is nothing to escape, so a large listing of plain names allocates
/// nothing here.
fn md_escape(s: &str) -> Cow<'_, str> {
    let escaped = escape_controls(s);
    if !escaped.contains('|') {
        return escaped;
    }
    Cow::Owned(escaped.replace('|', "\\|"))
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

    fn entry(name: &str, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
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
    fn empty_text_listing_is_empty() {
        assert_eq!(format(&[], ListingFormat::Text), "");
    }
}
