//! Pure formatting of a directory listing for the clipboard: plain text, CSV,
//! or a Markdown table. No I/O, so the formatting is unit-tested directly. The
//! UI builds the entry slice and copies the returned string.

use crate::panel::FileEntry;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListingFormat {
    Text,
    Csv,
    Markdown,
}

impl ListingFormat {
    pub fn label(self) -> &'static str {
        match self {
            ListingFormat::Text => "text",
            ListingFormat::Csv => "CSV",
            ListingFormat::Markdown => "Markdown",
        }
    }
}

/// Render `entries` (Name / Size / Modified) in the chosen format. Text is
/// tab-separated with the human size string; CSV is RFC-4180-escaped with raw
/// byte sizes; Markdown is a pipe table with `|` escaped inside cells.
pub fn format(entries: &[&FileEntry], fmt: ListingFormat) -> String {
    match fmt {
        ListingFormat::Text => entries
            .iter()
            .map(|e| format!("{}\t{}\t{}", e.name, e.size_str, e.modified_str))
            .collect::<Vec<_>>()
            .join("\n"),
        ListingFormat::Csv => {
            let mut out = String::from("Name,Size (bytes),Modified\n");
            for e in entries {
                out.push_str(&csv_escape(&e.name));
                out.push(',');
                out.push_str(&e.size.to_string());
                out.push(',');
                out.push_str(&csv_escape(&e.modified_str));
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

/// Quote a CSV field when it contains a comma, quote, or newline, doubling any
/// embedded quotes (RFC 4180).
fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Escape `|` so a cell can't break the Markdown table.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|")
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
        assert!(out.starts_with("Name,Size (bytes),Modified\n"));
        assert!(out.contains("\"a,b.txt\",5,2026-06-25"));
    }

    #[test]
    fn markdown_is_a_table_and_escapes_pipes() {
        let v = vec![entry("a|b.txt", 10)];
        let out = format(&refs(&v), ListingFormat::Markdown);
        assert!(out.contains("| Name | Size | Modified |"));
        assert!(out.contains("| a\\|b.txt |"));
    }

    #[test]
    fn empty_text_listing_is_empty() {
        assert_eq!(format(&[], ListingFormat::Text), "");
    }
}
