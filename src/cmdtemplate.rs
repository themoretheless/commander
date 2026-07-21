//! Command templates for a future open-with / run-command bar: a user-defined
//! command line with placeholders that expand against the current selection,
//! each value shell-quoted so paths with spaces or quotes are safe to paste.
//!
//! Pure: this module only builds the command string and decides which templates
//! apply to a selection; spawning the process is the OS edge (the run-command
//! bar in the app layer). [`preview_segments`] additionally tags each span as
//! literal or substituted so the bar can tint placeholders, and [`expand`] is
//! defined in terms of it so the preview and the run can never diverge.

use crate::panel::FileEntry;
use serde::{Deserialize, Serialize};
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

/// Whether a preview span is fixed command text or an expanded placeholder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SegmentKind {
    Literal,
    Substituted,
}

/// One span of an expanded command line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Segment {
    pub text: String,
    pub kind: SegmentKind,
}

/// The recognised placeholders and their expanded values, in replacement
/// order. `{dir_other}` precedes `{dir}` so the longer token is matched first.
fn substitutions(ctx: &SelectionCtx) -> [(&'static str, String); 4] {
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
    [
        ("{paths}", quote_join(&paths)),
        ("{names}", quote_join(&ctx.names())),
        (
            "{dir_other}",
            crate::clipboard::shell_quote(&ctx.dir_other.to_string_lossy()),
        ),
        (
            "{dir}",
            crate::clipboard::shell_quote(&ctx.dir.to_string_lossy()),
        ),
    ]
}

/// Split the expanded command into literal vs substituted spans (single
/// left-to-right pass), so the run-command bar can tint placeholders.
///
/// Supported placeholders: `{paths}`, `{names}`, `{dir}`, `{dir_other}`. An
/// empty selection expands `{paths}`/`{names}` to nothing; any unrecognised
/// `{token}` is left verbatim as literal text. Because the pass consumes the
/// template once and emits each substituted value opaquely, a substituted value
/// that happens to contain a placeholder (e.g. a file literally named
/// `{dir}.txt`) is never re-substituted.
pub fn preview_segments(template: &str, ctx: &SelectionCtx) -> Vec<Segment> {
    let subs = substitutions(ctx);
    let mut out: Vec<Segment> = Vec::new();
    let push_literal = |out: &mut Vec<Segment>, s: &str| {
        if s.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(seg) if seg.kind == SegmentKind::Literal => seg.text.push_str(s),
            _ => out.push(Segment {
                text: s.to_string(),
                kind: SegmentKind::Literal,
            }),
        }
    };

    let mut rest = template;
    while !rest.is_empty() {
        // The earliest-positioned known placeholder in what remains.
        let next = subs
            .iter()
            .filter_map(|(tok, val)| rest.find(tok).map(|pos| (pos, *tok, val)))
            .min_by_key(|(pos, _, _)| *pos);
        match next {
            Some((pos, tok, val)) => {
                push_literal(&mut out, &rest[..pos]);
                if !val.is_empty() {
                    out.push(Segment {
                        text: val.clone(),
                        kind: SegmentKind::Substituted,
                    });
                }
                rest = &rest[pos + tok.len()..];
            }
            None => {
                push_literal(&mut out, rest);
                break;
            }
        }
    }
    out
}

/// Expand the supported placeholders in `template` against `ctx`, shell-quoting
/// every substituted path. Defined as the concatenation of
/// [`preview_segments`], so a preview and the command actually run are always
/// identical. See [`preview_segments`] for the placeholder list and rules.
pub fn expand(template: &str, ctx: &SelectionCtx) -> String {
    preview_segments(template, ctx)
        .into_iter()
        .map(|s| s.text)
        .collect()
}

/// A named command template plus the file extensions it applies to.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Template {
    /// Display label, e.g. "Open in Preview".
    pub name: String,
    /// The command line with placeholders, e.g. `open -a Preview {paths}`.
    pub raw: String,
    /// Extensions this template applies to (case-insensitive, dot optional).
    /// Empty means it applies to any selection.
    #[serde(default)]
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
}

/// An ordered set of saved command templates, persisted under the config dir
/// (mirroring [`crate::bookmarks`] / [`crate::smart_folder`]).
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Templates {
    pub items: Vec<Template>,
}

impl Templates {
    /// Append a template, unless one with the same command line already exists
    /// (de-duped by `raw`). Returns whether it was added.
    pub fn add(&mut self, template: Template) -> bool {
        if self.items.iter().any(|t| t.raw == template.raw) {
            return false;
        }
        self.items.push(template);
        true
    }

    /// Remove the template at `index`, if valid. Returns whether one was removed.
    #[allow(dead_code)] // wired once the bar grows an edit/manage affordance
    pub fn remove(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            self.items.remove(index);
            true
        } else {
            false
        }
    }

    /// Move the template at `from` to `to` (clamped), preserving the order of
    /// the rest. Returns whether `from` was valid.
    #[allow(dead_code)] // wired once the bar grows reordering
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.items.len() {
            return false;
        }
        let to = to.min(self.items.len() - 1);
        if from != to {
            let t = self.items.remove(from);
            self.items.insert(to, t);
        }
        true
    }

    /// The templates that apply to `entries` (extension filter accepts them),
    /// in stored order.
    pub fn matching(&self, entries: &[FileEntry]) -> Vec<&Template> {
        self.items.iter().filter(|t| t.matches(entries)).collect()
    }
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("command_templates.json")
}

/// Load saved templates, or an empty set if absent/corrupt.
pub fn load() -> Templates {
    Templates {
        items: crate::persistence::load_item_store(&store_path(), "Command templates"),
    }
}

/// Save templates atomically (temp file + rename). Returns `false` if
/// serialization or the atomic write failed, so the caller can surface it.
pub fn save(store: &Templates) -> bool {
    match serde_json::to_string_pretty(store) {
        Ok(json) => crate::fs_util::write_atomic(&store_path(), &json),
        Err(_) => false,
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
            name: "Open".into(),
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
            name: "Edit in VS Code".into(),
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
            name: "Preview".into(),
            raw: "preview {paths}".into(),
            exts: vec![".JPG".into()],
        };
        // FileEntry lowercases the extension at construction.
        let img = entry(&tmp, "Photo.JPG");
        assert_eq!(img.extension, "jpg");
        assert!(t.matches(&[img]));
    }

    fn tmpl(name: &str, raw: &str, exts: &[&str]) -> Template {
        Template {
            name: name.into(),
            raw: raw.into(),
            exts: exts.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn preview_segments_tags_literals_and_substitutions() {
        let c = ctx(&["/a/b.txt", "/a/c d.txt"], "/a", "/o");
        let segs = preview_segments("open {paths} now", &c);
        let kinds: Vec<SegmentKind> = segs.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SegmentKind::Literal,
                SegmentKind::Substituted,
                SegmentKind::Literal
            ]
        );
        assert_eq!(segs[0].text, "open ");
        assert_eq!(segs[1].text, "'/a/b.txt' '/a/c d.txt'");
        assert_eq!(segs[2].text, " now");
        // Concatenation equals expand().
        let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, expand("open {paths} now", &c));
    }

    #[test]
    fn preview_no_placeholder_is_all_literal() {
        let c = ctx(&["/a/b.txt"], "/a", "/o");
        let segs = preview_segments("git status", &c);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].kind, SegmentKind::Literal);
        assert_eq!(segs[0].text, "git status");
    }

    #[test]
    fn preview_unknown_placeholder_stays_literal() {
        let c = ctx(&["/a/b.txt"], "/a", "/o");
        let segs = preview_segments("{nope} {dir}", &c);
        // "{nope} " literal, then the substituted dir.
        assert_eq!(segs[0].kind, SegmentKind::Literal);
        assert_eq!(segs[0].text, "{nope} ");
        assert_eq!(segs[1].kind, SegmentKind::Substituted);
        assert_eq!(segs[1].text, "'/a'");
    }

    #[test]
    fn expand_does_not_re_substitute_a_substituted_value() {
        // A file literally named "{dir}.txt" must not have its "{dir}" expanded.
        let c = ctx(&["/a/{dir}.txt"], "/a", "/o");
        assert_eq!(expand("cat {paths}", &c), "cat '/a/{dir}.txt'");
    }

    #[test]
    fn templates_store_add_dedupes_matching_and_round_trips() {
        let mut store = Templates::default();
        assert!(store.add(tmpl("Code", "code {paths}", &["rs"])));
        assert!(store.add(tmpl("Open", "open {paths}", &[])));
        // Same command line is rejected (dedupe by raw).
        assert!(!store.add(tmpl("Code again", "code {paths}", &["rs"])));
        assert_eq!(store.items.len(), 2);

        let tmp = TempDir::new();
        let rs = entry(&tmp, "main.rs");
        let png = entry(&tmp, "logo.png");
        // For an .rs selection both the rs-filtered and the catch-all match.
        let names: Vec<&str> = store
            .matching(&[rs])
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["Code", "Open"]);
        // For a .png selection only the catch-all matches.
        let names: Vec<&str> = store
            .matching(&[png])
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["Open"]);

        let json = serde_json::to_string(&store).unwrap();
        let back: Templates = serde_json::from_str(&json).unwrap();
        assert_eq!(store, back);
    }

    #[test]
    fn templates_reorder_and_remove() {
        let mut store = Templates::default();
        store.add(tmpl("a", "a", &[]));
        store.add(tmpl("b", "b", &[]));
        store.add(tmpl("c", "c", &[]));
        assert!(store.reorder(2, 0));
        let names: Vec<&str> = store.items.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["c", "a", "b"]);
        assert!(store.remove(0));
        assert_eq!(store.items[0].name, "a");
        assert!(!store.remove(9));
    }
}
