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
pub fn style_label(style: PathStyle) -> &'static str {
    match style {
        PathStyle::FullPath => "path",
        PathStyle::NameOnly => "name",
        PathStyle::ParentPath => "parent path",
        PathStyle::FileUrl => "file URL",
        PathStyle::ShellEscaped => "shell-escaped path",
        PathStyle::RelativeToOther => "relative path",
    }
}

/// Format `paths` for the clipboard under `style`, one per line.
pub fn format(paths: &[PathBuf], style: PathStyle, other_root: Option<&Path>) -> String {
    paths
        .iter()
        .map(|p| format_one(p, style, other_root))
        .collect::<Vec<_>>()
        .join("\n")
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
        PathStyle::FileUrl => format!("file://{}", percent_encode_path(&p.to_string_lossy())),
        PathStyle::ShellEscaped => shell_quote(&p.to_string_lossy()),
        PathStyle::RelativeToOther => match other_root {
            Some(root) => p
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_else(|_| full()),
            None => full(),
        },
    }
}

/// Percent-encode a path, keeping `/` and the RFC 3986 unreserved set.
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
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
fn shell_quote(s: &str) -> String {
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
    fn multiple_paths_join_with_newlines() {
        let paths = [p("/a/x.txt"), p("/a/y.txt")];
        assert_eq!(
            format(&paths, PathStyle::FullPath, None),
            "/a/x.txt\n/a/y.txt"
        );
    }
}
