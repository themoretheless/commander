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
/// A value is computed only when its token occurs in `template` (`None`
/// otherwise): the run-command bar previews every frame, and quoting a large
/// selection for a `{paths}` the line never uses would be wasted work.
fn substitutions(template: &str, ctx: &SelectionCtx) -> [(&'static str, Option<String>); 4] {
    fn quote_join(items: impl Iterator<Item = String>) -> String {
        items
            .map(|s| crate::clipboard::shell_quote(&s))
            .collect::<Vec<_>>()
            .join(" ")
    }
    let wanted = |tok: &str| template.contains(tok);
    [
        (
            "{paths}",
            wanted("{paths}")
                .then(|| quote_join(ctx.paths.iter().map(|p| p.to_string_lossy().to_string()))),
        ),
        (
            "{names}",
            wanted("{names}").then(|| quote_join(ctx.names().into_iter().map(dash_safe))),
        ),
        (
            "{dir_other}",
            wanted("{dir_other}")
                .then(|| crate::clipboard::shell_quote(&ctx.dir_other.to_string_lossy())),
        ),
        (
            "{dir}",
            wanted("{dir}").then(|| crate::clipboard::shell_quote(&ctx.dir.to_string_lossy())),
        ),
    ]
}

/// The first path a placeholder used by `template` would expand that is not
/// valid UTF-8. Such a path is shown with U+FFFD replacement characters, so
/// the command would target a different (or nonexistent) file; the bar
/// refuses to run while this returns `Some`.
pub fn non_utf8_path<'a>(template: &str, ctx: &'a SelectionCtx) -> Option<&'a std::path::Path> {
    let wanted = |tok: &str| template.contains(tok);
    let selection = (wanted("{paths}") || wanted("{names}")).then_some(ctx.paths.iter());
    let dirs = [
        wanted("{dir}").then_some(&ctx.dir),
        wanted("{dir_other}").then_some(&ctx.dir_other),
    ];
    selection
        .into_iter()
        .flatten()
        .chain(dirs.into_iter().flatten())
        .map(PathBuf::as_path)
        .find(|path| path.to_str().is_none())
}

/// A bare file name that starts with `-` would be parsed as an option by the
/// command it is passed to (`rm {names}` on a file named `-rf`); quoting does
/// not help, so such names are anchored to the current directory with `./`.
fn dash_safe(name: String) -> String {
    if name.starts_with('-') {
        format!("./{name}")
    } else {
        name
    }
}

/// Split the expanded command into literal vs substituted spans (single
/// left-to-right pass), so the run-command bar can tint placeholders.
///
/// Supported placeholders: `{paths}`, `{names}`, `{dir}`, `{dir_other}`. An
/// empty selection expands `{paths}`/`{names}` to nothing; a `{names}` entry
/// starting with `-` is written as `./-name` so it is not read as an option;
/// any unrecognised
/// `{token}` is left verbatim as literal text. Because the pass consumes the
/// template once and emits each substituted value opaquely, a substituted value
/// that happens to contain a placeholder (e.g. a file literally named
/// `{dir}.txt`) is never re-substituted.
pub fn preview_segments(template: &str, ctx: &SelectionCtx) -> Vec<Segment> {
    let subs = substitutions(template, ctx);
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
            .filter_map(|(tok, val)| {
                let val = val.as_ref()?;
                rest.find(tok).map(|pos| (pos, *tok, val))
            })
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
    /// the filter is ignored, and a multi-part filter such as `tar.gz` matches
    /// the whole suffix rather than only the last extension.
    pub fn matches(&self, entries: &[FileEntry]) -> bool {
        if self.exts.is_empty() {
            return true;
        }
        let suffixes: Vec<String> = self
            .exts
            .iter()
            .map(|e| format!(".{}", e.trim_start_matches('.').to_lowercase()))
            .collect();
        entries.iter().any(|e| {
            suffixes.iter().any(|suffix| {
                e.name_lower.len() > suffix.len() && e.name_lower.ends_with(suffix.as_str())
            })
        })
    }
}

/// An ordered set of saved command templates, persisted under the config dir
/// (mirroring [`crate::bookmarks`] / [`crate::smart_folder`]).
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Templates {
    pub items: Vec<Template>,
}

impl Templates {
    /// An explicit save updates metadata for the same command without moving its row.
    pub fn save_template(&mut self, template: Template) -> bool {
        if let Some(existing) = self.items.iter_mut().find(|item| item.raw == template.raw) {
            if *existing == template {
                return false;
            }
            *existing = template;
        } else {
            self.items.push(template);
        }
        true
    }

    /// Append a template, unless one with the same command line already exists
    /// (de-duped by `raw`). Returns whether it was added.
    #[cfg(test)]
    pub fn add(&mut self, template: Template) -> bool {
        if self.items.iter().any(|t| t.raw == template.raw) {
            return false;
        }
        self.items.push(template);
        true
    }

    /// Remove the template at `index`, if valid. Returns whether one was removed.
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
    #[cfg(test)]
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
    crate::persistence::save_item_store(&store_path(), "Command templates", store)
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
    fn updating_template_metadata_preserves_order_and_does_not_duplicate_command() {
        let mut store = Templates::default();
        store.save_template(tmpl("Before", "file {paths}", &[]));
        store.save_template(tmpl("After", "wc -l {paths}", &[]));
        assert!(store.save_template(tmpl("Pictures", "file {paths}", &["png"])));
        assert_eq!(store.items.len(), 2);
        assert_eq!(store.items[0].name, "Pictures");
        assert_eq!(store.items[0].exts, vec!["png"]);
        assert_eq!(store.items[1].name, "After");
        assert!(!store.save_template(tmpl("Pictures", "file {paths}", &["png"])));
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
    fn names_starting_with_dash_are_not_options() {
        // `rm {names}` on a file named "-rf" must not become `rm '-rf'`.
        let c = ctx(&["/a/-rf", "/a/a-b", "/a/-"], "/a", "/o");
        assert_eq!(expand("rm {names}", &c), "rm './-rf' 'a-b' './-'");
        // Absolute paths already start with a separator and are unchanged.
        assert_eq!(expand("rm {paths}", &c), "rm '/a/-rf' '/a/a-b' '/a/-'");
    }

    #[test]
    fn substitutions_skip_tokens_absent_from_the_template() {
        let c = ctx(&["/a/b.txt"], "/a", "/o");
        let subs = substitutions("cd {dir}", &c);
        let computed: Vec<&str> = subs
            .iter()
            .filter(|(_, v)| v.is_some())
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(computed, vec!["{dir}"]);
        // `{dir_other}` alone does not count as containing `{dir}`.
        let subs = substitutions("cp {paths} {dir_other}", &c);
        let computed: Vec<&str> = subs
            .iter()
            .filter(|(_, v)| v.is_some())
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(computed, vec!["{paths}", "{dir_other}"]);
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

    #[test]
    fn matches_multi_part_extension_filters() {
        let tmp = TempDir::new();
        let t = Template {
            name: "Untar".into(),
            raw: "tar xzf {paths}".into(),
            exts: vec!["tar.gz".into()],
        };
        assert!(t.matches(&[entry(&tmp, "backup.TAR.GZ")]));
        assert!(!t.matches(&[entry(&tmp, "notes.gz")]));
        assert!(!t.matches(&[entry(&tmp, "tar.gz.txt")]));
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

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_reported_only_for_used_placeholders() {
        use std::os::unix::ffi::OsStrExt;
        let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.txt"));
        let mut c = ctx(&["/tmp/ok.txt"], "/tmp", "/other");
        c.paths.push(bad.clone());
        assert_eq!(non_utf8_path("cat {paths}", &c), Some(bad.as_path()));
        assert_eq!(non_utf8_path("cat {names}", &c), Some(bad.as_path()));
        // The bad path is not expanded by these lines, so they may run.
        assert_eq!(non_utf8_path("ls {dir} {dir_other}", &c), None);
        assert_eq!(non_utf8_path("make", &c), None);

        let mut c = ctx(&["/tmp/ok.txt"], "/tmp", "/other");
        c.dir_other = bad.clone();
        assert_eq!(
            non_utf8_path("cp {paths} {dir_other}", &c),
            Some(bad.as_path())
        );
        assert_eq!(non_utf8_path("cp {paths} {dir}", &c), None);
    }
}
