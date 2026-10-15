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
    /// The enclosing directory.
    ParentPath,
    /// A `file://` URL with the path percent-encoded.
    FileUrl,
    /// Single-quoted for safe pasting into a shell.
    ShellEscaped,
    /// Relative to the other pane's folder (falls back to the full path).
    RelativeToOther,
}

/// Short label for the "Copied ..." toast.
pub const fn style_label(style: PathStyle) -> &'static str {
    match style {
        PathStyle::FullPath => "path",
        PathStyle::NameOnly => "name",
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
    for p in paths {
        let line = format_one(p, style, other_root);
        let line = match style {
            // Quoted and percent-encoded forms already carry controls safely.
            PathStyle::ShellEscaped | PathStyle::FileUrl => line,
            _ => one_line(line),
        };
        if style == PathStyle::ParentPath && lines.contains(&line) {
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
}
