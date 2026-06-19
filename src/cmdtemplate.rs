//! Command templates for a future open-with / run-command bar: a user-defined
//! command line with placeholders that expand against the current selection,
//! each value shell-quoted so paths with spaces or quotes are safe to paste.
//!
//! Pure: this module only builds the command string and decides which templates
//! apply to a selection; actually spawning the process is the OS edge, wired in
//! a later iteration. The whole surface is exercised by the unit tests below
//! until then.
#![allow(dead_code)] // remove once the open-with / run-command bar is wired

use crate::panel::FileEntry;
use std::path::PathBuf;

/// The selection a template expands against. File names are derived from
/// `paths` (their final component), so the two never drift apart.
pub struct SelectionCtx {
    /// Absolute paths of the selected entries.
    pub paths: Vec<PathBuf>,
    /// The active pane's directory.
    pub dir: PathBuf,
    /// The other pane's directory (for "move/copy to the other side" commands).
    pub dir_other: PathBuf,
}

impl SelectionCtx {
    /// The selected entries' file names (final path component each).
    fn names(&self) -> Vec<String> {
        self.paths
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// Expand the supported placeholders in `template` against `ctx`:
///
/// - `{paths}`  -> every selected path, each shell-quoted, space-joined
/// - `{names}`  -> every selected file name, each shell-quoted, space-joined
/// - `{dir}`        -> the active directory, shell-quoted
/// - `{dir_other}`  -> the other pane's directory, shell-quoted
///
/// An empty selection expands `{paths}`/`{names}` to the empty string. Any
/// unrecognised `{token}` is left in place verbatim, so a literal brace in a
/// command survives untouched.
pub fn expand(template: &str, ctx: &SelectionCtx) -> String {
    let quote_join = |items: &[String]| -> String {
        items
            .iter()
            .map(|s| crate::clipboard::shell_quote(s))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let paths: Vec<String> = ctx
        .paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let paths_expanded = quote_join(&paths);
    let names_expanded = quote_join(&ctx.names());
    let dir = crate::clipboard::shell_quote(&ctx.dir.to_string_lossy());
    let dir_other = crate::clipboard::shell_quote(&ctx.dir_other.to_string_lossy());

    // `{dir_other}` is replaced before `{dir}`; the tokens do not overlap as
    // substrings ("{dir_other}" never contains "{dir}"), but ordering it first
    // keeps the intent obvious.
    template
        .replace("{paths}", &paths_expanded)
        .replace("{names}", &names_expanded)
        .replace("{dir_other}", &dir_other)
        .replace("{dir}", &dir)
}

/// A named command template plus the file extensions it applies to.
pub struct Template {
    /// The command line with placeholders, e.g. `open -a Preview {paths}`.
    pub raw: String,
    /// Extensions this template applies to (case-insensitive, dot optional).
    /// Empty means it applies to any selection.
    pub exts: Vec<String>,
}

impl Template {
    /// Whether this template should be offered for `entries`: true when its
    /// extension filter is empty (applies to everything) or any selected entry
    /// has a matching extension. Matching is case-insensitive; a leading dot in
    /// the filter is ignored.
    pub fn matches(&self, entries: &[FileEntry]) -> bool {
        if self.exts.is_empty() {
            return true;
        }
        let wanted: Vec<String> = self
            .exts
            .iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .collect();
        // `FileEntry.extension` is already lowercased at construction.
        entries.iter().any(|e| wanted.contains(&e.extension))
    }

    /// Expand this template's command line against `ctx`.
    pub fn expand(&self, ctx: &SelectionCtx) -> String {
        expand(&self.raw, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn ctx(paths: &[&str], dir: &str, dir_other: &str) -> SelectionCtx {
        SelectionCtx {
            paths: paths.iter().map(PathBuf::from).collect(),
            dir: PathBuf::from(dir),
            dir_other: PathBuf::from(dir_other),
        }
    }

    fn entry(tmp: &TempDir, name: &str) -> FileEntry {
        let p = tmp.file(name, "");
        let meta = std::fs::metadata(&p).unwrap();
        FileEntry::from_meta(p, &meta).unwrap()
    }

    #[test]
    fn expands_paths_quoted_and_space_joined() {
        let c = ctx(&["/a/b.txt", "/a/c d.txt"], "/a", "/other");
        assert_eq!(expand("open {paths}", &c), "open '/a/b.txt' '/a/c d.txt'");
    }

    #[test]
    fn expands_names_from_final_component() {
        let c = ctx(&["/a/b.txt", "/a/c d.txt"], "/a", "/other");
        assert_eq!(expand("zip out {names}", &c), "zip out 'b.txt' 'c d.txt'");
    }

    #[test]
    fn expands_both_directories() {
        let c = ctx(&["/a/b.txt"], "/a", "/other side");
        assert_eq!(
            expand("cp {paths} {dir_other}", &c),
            "cp '/a/b.txt' '/other side'"
        );
        assert_eq!(expand("cd {dir}", &c), "cd '/a'");
    }

    #[test]
    fn quotes_embedded_single_quotes() {
        let c = ctx(&["/a/it's mine.txt"], "/a", "/o");
        // Single quote is closed, escaped as '\'' , and reopened.
        assert_eq!(expand("cat {paths}", &c), "cat '/a/it'\\''s mine.txt'");
        assert_eq!(expand("echo {names}", &c), "echo 'it'\\''s mine.txt'");
    }

    #[test]
    fn empty_selection_expands_path_and_name_lists_to_nothing() {
        let c = ctx(&[], "/a", "/o");
        assert_eq!(expand("ls {paths}", &c), "ls ");
        assert_eq!(expand("ls {names}", &c), "ls ");
        // Directories still expand.
        assert_eq!(expand("ls {dir}", &c), "ls '/a'");
    }

    #[test]
    fn unknown_placeholder_is_left_verbatim() {
        let c = ctx(&["/a/b.txt"], "/a", "/o");
        assert_eq!(expand("{frobnicate} {dir}", &c), "{frobnicate} '/a'");
    }

    #[test]
    fn dir_token_does_not_clobber_dir_other() {
        // Replacing {dir} must not corrupt an adjacent {dir_other}.
        let c = ctx(&[], "/here", "/there");
        assert_eq!(expand("{dir} {dir_other}", &c), "'/here' '/there'");
    }

    #[test]
    fn matches_empty_filter_applies_to_any_selection() {
        let tmp = TempDir::new();
        let t = Template {
            raw: "open {paths}".into(),
            exts: vec![],
        };
        assert!(t.matches(&[entry(&tmp, "anything.bin")]));
        assert!(t.matches(&[])); // even an empty selection
    }

    #[test]
    fn matches_when_any_entry_has_a_listed_extension() {
        let tmp = TempDir::new();
        let t = Template {
            raw: "code {paths}".into(),
            exts: vec!["rs".into(), "toml".into()],
        };
        let rs = entry(&tmp, "main.rs");
        let png = entry(&tmp, "logo.png");
        assert!(t.matches(&[png.clone(), rs.clone()])); // one matches
        assert!(!t.matches(&[png])); // none match
    }

    #[test]
    fn matches_is_case_insensitive_and_dot_optional() {
        let tmp = TempDir::new();
        // Filter given with mixed case and a leading dot.
        let t = Template {
            raw: "preview {paths}".into(),
            exts: vec![".JPG".into()],
        };
        // FileEntry lowercases the extension at construction.
        let img = entry(&tmp, "Photo.JPG");
        assert_eq!(img.extension, "jpg");
        assert!(t.matches(&[img]));
    }
}
