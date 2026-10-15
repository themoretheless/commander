//! Compact file-name presentation that keeps the identifying suffix visible.

use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

const COMPOUND_SUFFIXES: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".d.ts", ".user.js", ".min.js", ".min.css",
];

fn suffix(name: &str) -> Option<&str> {
    if let Some(compound) = COMPOUND_SUFFIXES.iter().find(|compound| {
        name.get(name.len().saturating_sub(compound.len())..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(compound))
    }) {
        return name.get(name.len().saturating_sub(compound.len())..);
    }

    let extension = std::path::Path::new(name).extension()?.to_str()?;
    let suffix_len = extension.len().saturating_add(1);
    name.get(name.len().checked_sub(suffix_len)?..)
}

/// User-perceived characters (extended grapheme clusters) in `value`, so a
/// decomposed accent or a ZWJ emoji counts once and is never split.
fn grapheme_count(value: &str) -> usize {
    value.graphemes(true).count()
}

fn take_start(value: &str, count: usize) -> String {
    value.graphemes(true).take(count).collect()
}

fn take_end(value: &str, count: usize) -> String {
    let mut tail: Vec<&str> = value.graphemes(true).rev().take(count).collect();
    tail.reverse();
    tail.concat()
}

/// Replace C0 controls and DEL with their visible Control Pictures (U+2400
/// block), one char for one char, so a name containing a newline or tab shows
/// in full on one row instead of only up to the break.
fn visible_controls(name: &str) -> Cow<'_, str> {
    if !name.chars().any(|c| c.is_ascii_control()) {
        return Cow::Borrowed(name);
    }
    Cow::Owned(
        name.chars()
            .map(|c| match c {
                '\x7f' => '\u{2421}',
                c if c.is_ascii_control() => char::from_u32(0x2400 + c as u32).unwrap_or(c),
                c => c,
            })
            .collect(),
    )
}

/// Truncate to at most `max_chars` user-perceived characters (grapheme
/// clusters). For regular files, reserve space for the extension before
/// shortening the stem. Dotfiles and extensionless names retain both their
/// beginning and ending. Control characters are shown as visible symbols, so
/// such a name never comes back borrowed and the caller's full-name hover
/// applies.
pub fn truncate_preserving_extension(name: &str, max_chars: usize) -> Cow<'_, str> {
    match visible_controls(name) {
        Cow::Borrowed(name) => truncate_visible(name, max_chars),
        Cow::Owned(visible) => Cow::Owned(truncate_visible(&visible, max_chars).into_owned()),
    }
}

fn truncate_visible(name: &str, max_chars: usize) -> Cow<'_, str> {
    let name_chars = grapheme_count(name);
    if name_chars <= max_chars {
        return Cow::Borrowed(name);
    }
    if max_chars == 0 {
        return Cow::Borrowed("");
    }
    if max_chars == 1 {
        return Cow::Borrowed("\u{2026}");
    }

    if let Some(suffix) = suffix(name) {
        let suffix_chars = grapheme_count(suffix);
        if suffix_chars.saturating_add(2) <= max_chars {
            let stem = &name[..name.len() - suffix.len()];
            let stem_budget = max_chars - suffix_chars - 1;
            return Cow::Owned(format!("{}\u{2026}{suffix}", take_start(stem, stem_budget)));
        }
    }

    let content = max_chars - 1;
    let start = content.div_ceil(2);
    let end = content / 2;
    Cow::Owned(format!(
        "{}\u{2026}{}",
        take_start(name, start),
        take_end(name, end)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_short_names_borrowed() {
        assert!(matches!(
            truncate_preserving_extension("notes.txt", 20),
            Cow::Borrowed("notes.txt")
        ));
    }

    #[test]
    fn preserves_regular_and_compound_extensions() {
        assert_eq!(
            truncate_preserving_extension("annual-report-final.pdf", 14),
            "annual-re\u{2026}.pdf"
        );
        assert_eq!(
            truncate_preserving_extension("backup-production-2026.tar.gz", 16),
            "backup-p\u{2026}.tar.gz"
        );
        assert_eq!(
            truncate_preserving_extension("component.generated.d.ts", 12),
            "compon\u{2026}.d.ts"
        );
    }

    #[test]
    fn dotfiles_and_unicode_keep_both_ends() {
        assert_eq!(
            truncate_preserving_extension(".gitignore", 7),
            ".gi\u{2026}ore"
        );
        let shortened = truncate_preserving_extension("отчёт-подробный.txt", 12);
        assert_eq!(shortened.chars().count(), 12);
        assert!(shortened.ends_with(".txt"));
    }

    #[test]
    fn truncation_never_splits_a_grapheme() {
        // "e" + combining acute (NFD, as macOS often stores names).
        let shortened = truncate_preserving_extension("abcde\u{301}", 4);
        assert_eq!(shortened, "ab\u{2026}e\u{301}");
        assert!(!shortened.contains("\u{2026}\u{301}"));
        // A ZWJ family emoji is one character of the budget.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let name = format!("{family}{family}{family}.png");
        assert_eq!(
            truncate_preserving_extension(&name, 6),
            format!("{family}\u{2026}.png")
        );
    }

    #[test]
    fn control_characters_are_shown_as_symbols() {
        assert_eq!(
            truncate_preserving_extension("two\nlines\t.txt", 40),
            "two\u{240A}lines\u{2409}.txt"
        );
        assert_eq!(truncate_preserving_extension("a\x7f", 40), "a\u{2421}");
        assert_eq!(
            truncate_preserving_extension("first\nsecond-line.txt", 11),
            "first\u{240A}\u{2026}.txt"
        );
    }

    #[test]
    fn tiny_budgets_are_stable() {
        assert_eq!(truncate_preserving_extension("long.txt", 0), "");
        assert_eq!(truncate_preserving_extension("long.txt", 1), "\u{2026}");
        assert_eq!(truncate_preserving_extension("long.txt", 2), "l\u{2026}");
    }
}
