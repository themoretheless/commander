//! Pure path-to-clipboard formatting. No I/O, so quoting, URL percent-encoding
//! and relative-path computation are unit-tested directly. The UI just copies
//! the returned string.

use std::path::{Path, PathBuf};

/// How a selection's paths are rendered for the clipboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathStyle {
    /// The full POSIX path.
    FullPath,
    /// Just the final component (file name).
    NameOnly,
    /// Final component with its last extension removed (a lexical operation).
    NameWithoutExtension,
    /// The enclosing directory.
    ParentPath,
    /// A `file://` URL with the path percent-encoded.
    FileUrl,
    /// Single-quoted for safe pasting into a shell.
    ShellEscaped,
    /// Relative to the other pane's folder (falls back to the full path).
    RelativeToOther,
}

/// Copy the captured Name/Size/Modified display cells as quoted TSV.
/// No metadata is re-read. Bound both row count and encoded clipboard bytes.
pub fn file_details<'a>(rows: impl IntoIterator<Item = [&'a str; 3]>) -> Result<String, String> {
    const MAX_BYTES: usize = 1024 * 1024;
    const MAX_ROWS: usize = 10_000;
    let encoded_len = |row: [&str; 3]| -> Option<usize> {
        row.iter().try_fold(3usize, |total, cell| {
            total
                .checked_add(cell.len())?
                .checked_add(2)?
                .checked_add(cell.bytes().filter(|&byte| byte == b'"').count())
        })
    };
    let header = ["Name", "Size", "Modified"];
    let mut size = encoded_len(header).ok_or("File details exceed 1 MiB")?;
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .quote_style(csv::QuoteStyle::Always)
        .from_writer(Vec::new());
    writer
        .write_record(header)
        .map_err(|error| error.to_string())?;
    let mut any = false;
    for (index, row) in rows.into_iter().enumerate() {
        if index == MAX_ROWS {
            return Err("File details are limited to 10000 entries".into());
        }
        size = size
            .checked_add(encoded_len(row).ok_or("File details exceed 1 MiB")?)
            .filter(|&size| size <= MAX_BYTES)
            .ok_or("File details exceed 1 MiB")?;
        writer
            .write_record(row)
            .map_err(|error| error.to_string())?;
        any = true;
    }
    if !any {
        return Err("No visible file details to copy".into());
    }
    let bytes = writer.into_inner().map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

/// Short label for the "Copied ..." toast.
pub const fn style_label(style: PathStyle) -> &'static str {
    match style {
        PathStyle::FullPath => "path",
        PathStyle::NameOnly => "name",
        PathStyle::NameWithoutExtension => "name without extension",
        PathStyle::ParentPath => "parent path",
        PathStyle::FileUrl => "file URL",
        PathStyle::ShellEscaped => "shell-escaped path",
        PathStyle::RelativeToOther => "relative path",
    }
}

/// Format `paths` for the clipboard under `style`, one per line. Parent
/// paths are deduplicated (first occurrence wins), so several files from one
/// folder yield that folder once.
pub fn format(paths: &[PathBuf], style: PathStyle, other_root: Option<&Path>) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(paths.len());
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for p in paths {
        let line = format_one(p, style, other_root);
        let line = match style {
            // Quoted and percent-encoded forms already carry controls safely.
            PathStyle::ShellEscaped | PathStyle::FileUrl => line,
            _ => one_line(line),
        };
        if style == PathStyle::ParentPath && !seen.insert(p.parent().unwrap_or(p).to_path_buf()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// Keep a path with a newline (legal in Unix names) on one line, escaping it
/// the way listing export does. Paths without line controls are untouched,
/// so Windows backslash separators are never doubled.
fn one_line(line: String) -> String {
    if line.contains(['\n', '\r', '\t']) {
        crate::listing_export::escape_controls(&line).into_owned()
    } else {
        line
    }
}

fn format_one(p: &Path, style: PathStyle, other_root: Option<&Path>) -> String {
    let full = || p.to_string_lossy().to_string();
    match style {
        PathStyle::FullPath => full(),
        PathStyle::NameOnly => p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(full),
        PathStyle::NameWithoutExtension => p
            .file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(full),
        PathStyle::ParentPath => p
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default(),
        PathStyle::FileUrl => format!("file://{}", percent_encode_path(&path_bytes(p))),
        PathStyle::ShellEscaped => shell_quote(&p.to_string_lossy()),
        PathStyle::RelativeToOther => match other_root {
            Some(root) => p
                .strip_prefix(root)
                // The other pane's own folder is "." rather than an empty line.
                .map(|r| {
                    if r.as_os_str().is_empty() {
                        ".".to_string()
                    } else {
                        r.to_string_lossy().to_string()
                    }
                })
                .unwrap_or_else(|_| full()),
            None => full(),
        },
    }
}

/// The raw bytes of `p` for URL encoding. On Unix these are the exact name
/// bytes, so a non-UTF-8 name is encoded faithfully instead of as U+FFFD.
#[cfg(unix)]
fn path_bytes(p: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    std::borrow::Cow::Borrowed(p.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_bytes(p: &Path) -> std::borrow::Cow<'_, [u8]> {
    match p.to_string_lossy() {
        std::borrow::Cow::Borrowed(s) => std::borrow::Cow::Borrowed(s.as_bytes()),
        std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s.into_bytes()),
    }
}

/// Percent-encode a path, keeping `/` and the RFC 3986 unreserved set.
fn percent_encode_path(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Wrap in single quotes for the shell, escaping embedded single quotes as
/// the standard `'\''` sequence.
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn file_details_tsv_round_trips_controls_and_enforces_encoded_limits() {
        let rows = [["a\tb\n\"c\"", "12 B", "today"], ["ёж", "…", ""]];
        let text = file_details(rows).unwrap();
        let parsed: Vec<Vec<String>> = csv::ReaderBuilder::new()
            .delimiter(b'\t')
            .has_headers(false)
            .from_reader(text.as_bytes())
            .records()
            .map(|row| row.unwrap().iter().map(str::to_owned).collect())
            .collect();
        assert_eq!(parsed[0], ["Name", "Size", "Modified"]);
        assert_eq!(parsed[1], rows[0]);
        assert_eq!(parsed[2], rows[1]);
        assert!(file_details([]).is_err());
        assert!(file_details(std::iter::repeat_n(["", "", ""], 10000)).is_ok());
        assert!(file_details(std::iter::repeat_n(["", "", ""], 10001)).is_err());
        let overhead = file_details([["", "", ""]]).unwrap().len();
        let exact = "a".repeat(1024 * 1024 - overhead);
        assert_eq!(file_details([[&exact, "", ""]]).unwrap().len(), 1024 * 1024);
        let over = exact + "a";
        assert!(file_details([[&over, "", ""]]).is_err());
        let quotes = "\"".repeat(1024 * 1024 / 2);
        assert!(file_details([[&quotes, "", ""]]).is_err());
    }

    #[test]
    fn parent_deduplication_uses_native_paths_before_control_escaping() {
        let paths = [
            PathBuf::from("/x/line\\nname/a"),
            PathBuf::from("/x/line\nname/b"),
        ];
        assert_eq!(
            format(&paths, PathStyle::ParentPath, None),
            "/x/line\\nname\n/x/line\\nname"
        );
    }

    #[test]
    fn names_without_extensions_preserve_dotfiles_unicode_and_other_dots() {
        for (source, expected) in [
            ("/tmp/Photo.JPG", "Photo"),
            ("/tmp/archive.tar.gz", "archive.tar"),
            ("/tmp/.env", ".env"),
            ("/tmp/.config.json", ".config"),
            ("/tmp/README", "README"),
            ("/tmp/trailing.", "trailing"),
            ("/tmp/ёж.文.txt", "ёж.文"),
            ("/tmp/folder.with.dots", "folder.with"),
            ("/", "/"),
        ] {
            assert_eq!(
                format(&[p(source)], PathStyle::NameWithoutExtension, None),
                expected
            );
        }
    }

    #[test]
    fn names_without_extensions_keep_selection_order_duplicates_and_control_escaping() {
        let paths = [
            p("/a/a.txt"),
            p("/b/a.jpg"),
            p("/a/line\nname.txt"),
            p("/a/tab\tname.txt"),
        ];
        assert_eq!(
            format(&paths, PathStyle::NameWithoutExtension, None),
            "a\na\nline\\nname\ntab\\tname"
        );
        assert_eq!(format(&[], PathStyle::NameWithoutExtension, None), "");
    }

    #[test]
    fn full_name_and_parent() {
        let path = [p("/Users/me/a b.txt")];
        assert_eq!(
            format(&path, PathStyle::FullPath, None),
            "/Users/me/a b.txt"
        );
        assert_eq!(format(&path, PathStyle::NameOnly, None), "a b.txt");
        assert_eq!(format(&path, PathStyle::ParentPath, None), "/Users/me");
    }

    #[test]
    fn name_only_of_root_falls_back() {
        assert_eq!(format(&[p("/")], PathStyle::NameOnly, None), "/");
    }

    #[test]
    fn file_url_percent_encodes_spaces() {
        assert_eq!(
            format(&[p("/Users/me/a b.txt")], PathStyle::FileUrl, None),
            "file:///Users/me/a%20b.txt"
        );
    }

    #[test]
    fn shell_escaping_handles_quotes_and_spaces() {
        assert_eq!(
            format(&[p("/tmp/it's a.txt")], PathStyle::ShellEscaped, None),
            "'/tmp/it'\\''s a.txt'"
        );
    }

    #[test]
    fn relative_to_other_under_and_not_under() {
        let root = p("/root");
        assert_eq!(
            format(
                &[p("/root/sub/f.txt")],
                PathStyle::RelativeToOther,
                Some(&root)
            ),
            "sub/f.txt"
        );
        // Not under the other root -> full path.
        assert_eq!(
            format(
                &[p("/elsewhere/f.txt")],
                PathStyle::RelativeToOther,
                Some(&root)
            ),
            "/elsewhere/f.txt"
        );
    }

    #[test]
    fn relative_to_other_of_the_other_root_itself_is_dot() {
        let root = p("/a/b");
        assert_eq!(
            format(&[p("/a/b")], PathStyle::RelativeToOther, Some(&root)),
            "."
        );
    }

    #[test]
    fn parent_path_lists_each_folder_once() {
        let paths = [p("/a/x.txt"), p("/b/z.txt"), p("/a/y.txt")];
        assert_eq!(format(&paths, PathStyle::ParentPath, None), "/a\n/b");
        // Other styles keep one line per path.
        assert_eq!(format(&paths, PathStyle::NameOnly, None).lines().count(), 3);
    }

    #[test]
    fn newline_in_a_name_stays_on_one_line() {
        let paths = [p("/a/two\nlines.txt"), p("/a/y.txt")];
        let text = format(&paths, PathStyle::FullPath, None);
        assert_eq!(text, "/a/two\\nlines.txt\n/a/y.txt");
        assert_eq!(text.lines().count(), 2);
        assert_eq!(
            format(&paths[..1], PathStyle::NameOnly, None),
            "two\\nlines.txt"
        );
        // Shell quoting keeps the raw newline; it is safe inside quotes.
        assert_eq!(
            format(&paths[..1], PathStyle::ShellEscaped, None),
            "'/a/two\nlines.txt'"
        );
        // Backslashes alone are left alone.
        assert_eq!(
            format(&[p("/a/b\\c")], PathStyle::FullPath, None),
            "/a/b\\c"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_url_encodes_non_utf8_bytes_exactly() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = PathBuf::from(OsStr::from_bytes(b"/tmp/caf\xe9.txt"));
        assert_eq!(
            format(&[path], PathStyle::FileUrl, None),
            "file:///tmp/caf%E9.txt"
        );
    }

    #[test]
    fn multiple_paths_join_with_newlines() {
        let paths = [p("/a/x.txt"), p("/a/y.txt")];
        assert_eq!(
            format(&paths, PathStyle::FullPath, None),
            "/a/x.txt\n/a/y.txt"
        );
    }

    #[test]
    fn file_url_escapes_url_syntax_and_multibyte_names() {
        // `%`, `#` and `?` would otherwise read as an escape, a fragment and
        // a query; `é` is encoded as its two UTF-8 bytes.
        assert_eq!(
            format(&[p("/a/é%#?.txt")], PathStyle::FileUrl, None),
            "file:///a/%C3%A9%25%23%3F.txt"
        );
        assert_eq!(
            percent_encode_path("/Az09-_.~/+&=:;@'!".as_bytes()),
            "/Az09-_.~/%2B%26%3D%3A%3B%40%27%21"
        );
    }
}
